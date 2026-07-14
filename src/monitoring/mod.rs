use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::RwLock;
use tracing::{debug, info};

pub struct VramSentinel {
    min_free_vram_mb: usize,
    #[allow(dead_code)] // wired in a future loop when auto-unload lands
    auto_unload: bool,
    system: Arc<RwLock<System>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub total_vram_mb: usize,
    pub free_vram_mb: usize,
    pub used_vram_mb: usize,
    pub total_ram_mb: usize,
    pub cpu_cores: usize,
    pub os: String,
    /// One of `nvidia`, `amd`, `apple-silicon`, `apple-intel`, `cpu`, `unknown`.
    /// Lets the CLI explain *why* it picked the model it picked.
    pub gpu_kind: GpuKind,
    pub recommended_model: String,
    pub optimal_context: usize,
    pub optimal_gpu_layers: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuKind {
    Nvidia,
    Amd,
    AppleSilicon,
    AppleIntel,
    Cpu,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStatus {
    pub running: bool,
    pub loaded_models: Vec<String>,
    pub active_requests: usize,
}

impl VramSentinel {
    pub fn new(min_free_vram_mb: usize, auto_unload: bool) -> Self {
        Self {
            min_free_vram_mb,
            auto_unload,
            system: Arc::new(RwLock::new(System::new_all())),
        }
    }

    pub async fn detect_hardware(&self) -> HardwareProfile {
        let mut sys = self.system.write().await;
        sys.refresh_all();

        let total_ram = (sys.total_memory() / (1024 * 1024)) as usize;
        let cpu_cores = sys.cpus().len();
        let os = std::env::consts::OS.to_string();

        let (gpu_kind, total_vram, free_vram) = detect_vram(total_ram);

        let recommended_model = suggest_model(free_vram).to_string();
        let optimal_context = calculate_optimal_context(free_vram);
        let optimal_gpu_layers = calculate_gpu_layers(free_vram);

        HardwareProfile {
            total_vram_mb: total_vram,
            free_vram_mb: free_vram,
            used_vram_mb: total_vram.saturating_sub(free_vram),
            total_ram_mb: total_ram,
            cpu_cores,
            os,
            gpu_kind,
            recommended_model,
            optimal_context,
            optimal_gpu_layers,
        }
    }

    pub async fn check_health(&self, min_free_mb: Option<usize>) -> HealthStatus {
        let threshold = min_free_mb.unwrap_or(self.min_free_vram_mb);
        let profile = self.detect_hardware().await;

        let vram_status = if profile.free_vram_mb >= threshold {
            ResourceStatus::Healthy
        } else if profile.free_vram_mb >= threshold / 2 {
            ResourceStatus::Warning
        } else {
            ResourceStatus::Critical
        };

        let ram_status = if profile.total_ram_mb / 2
            < profile.total_ram_mb.saturating_sub(profile.used_vram_mb)
        {
            ResourceStatus::Healthy
        } else {
            ResourceStatus::Warning
        };

        HealthStatus {
            recommendations: generate_recommendations(&profile, vram_status),
            hardware_profile: profile,
            vram_status,
            ram_status,
        }
    }

    pub async fn auto_optimize(&self) -> Result<OptimizationPlan> {
        let profile = self.detect_hardware().await;

        let plan = OptimizationPlan {
            recommended_model: profile.recommended_model.clone(),
            optimal_context: profile.optimal_context,
            optimal_num_gpu: profile.optimal_gpu_layers,
            // keep_alive: 5min on tight VRAM (so models get evicted), 1h on big rigs.
            keep_alive_duration: if profile.free_vram_mb < 8_000 {
                300
            } else {
                3600
            },
            unload_threshold_mb: profile.free_vram_mb / 4,
            apply_onnx: profile.gpu_kind == GpuKind::Cpu,
        };

        info!("generated optimization plan: {plan:?}");
        Ok(plan)
    }
}

// =====================================================================
// Pure functions — these are unit-tested in `tests/monitoring_logic.rs`
// =====================================================================

/// Map free VRAM (in MB) to a sensible Ollama model tag.
///
/// Tiers were chosen against the [Ollama model library][library] for the
/// Q4_K_M quantization most users actually pull. Numbers err on the side of
/// "leave headroom for context and KV cache" — running a 7B at exactly its
/// minimum quoted VRAM will OOM the moment you ask it to read a long file.
///
/// [library]: https://ollama.com/library
pub fn suggest_model(free_vram_mb: usize) -> &'static str {
    match free_vram_mb {
        // 80B-A3B MoE — ~52 GB resident at Q4; the strongest local coder
        // for 64 GB+ unified-memory Macs and multi-GPU rigs.
        v if v >= 64_000 => "qwen3-coder-next",
        // 70B R1 distill — ~43 GB at Q4; reasoning + coding.
        v if v >= 48_000 => "deepseek-r1:70b",
        // Best open dense coder of the Qwen 3.6 generation — ~17 GB at Q4.
        v if v >= 24_000 => "qwen3.6:27b",
        // Gemma 4 12B — ~8 GB at Q4 with 256K context and image input.
        v if v >= 16_000 => "gemma4:12b",
        v if v >= 10_000 => "qwen3.5:9b",
        v if v >= 6_000 => "qwen3.5:4b",
        v if v >= 3_000 => "qwen3.5:2b",
        // No GPU detected (or tiny): the user is going to suffer either way,
        // but a sub-1B is at least usable on CPU. Don't lie about 9B being viable.
        _ => "qwen3.5:0.8b",
    }
}

pub fn calculate_optimal_context(free_vram_mb: usize) -> usize {
    match free_vram_mb {
        v if v >= 48_000 => 131_072,
        v if v >= 24_000 => 65_536,
        v if v >= 16_000 => 32_768,
        v if v >= 10_000 => 16_384,
        v if v >= 6_000 => 8_192,
        _ => 4_096,
    }
}

pub fn calculate_gpu_layers(free_vram_mb: usize) -> i32 {
    match free_vram_mb {
        v if v >= 24_000 => -1, // -1 = "all layers on GPU"
        v if v >= 16_000 => 75,
        v if v >= 10_000 => 50,
        v if v >= 6_000 => 33,
        v if v >= 3_000 => 16,
        _ => 0, // CPU only
    }
}

// =====================================================================
// VRAM detection — platform-specific, none of it `unwrap()`s.
// =====================================================================

/// Returns `(gpu_kind, total_mb, free_mb)`. Falls back to `(Cpu, 0, 0)` rather
/// than fabricating numbers — *consumers must handle the zero case*. The
/// `total_ram_mb` argument is used as the unified-memory budget on Apple
/// Silicon (where there is no separate VRAM).
fn detect_vram(total_ram_mb: usize) -> (GpuKind, usize, usize) {
    // NVIDIA first — most common config in r/LocalLLaMA, and `nvidia-smi`
    // exists on Linux *and* Windows when an NVIDIA driver is installed.
    if let Some((total, free)) = detect_nvidia_smi() {
        debug!("detected nvidia gpu: total={total} free={free}");
        return (GpuKind::Nvidia, total, free);
    }

    // AMD via rocm-smi (Linux). The output format is annoying so we keep it
    // best-effort.
    if let Some((total, free)) = detect_rocm_smi() {
        debug!("detected amd gpu: total={total} free={free}");
        return (GpuKind::Amd, total, free);
    }

    // Apple Silicon: unified memory. Metal can address most of total RAM
    // (Apple's own guideline is `recommended_max_working_set_size` which is
    // ~75% of physical RAM on M-series). We give it 70% to leave room for
    // the OS and other apps.
    #[cfg(target_os = "macos")]
    {
        if is_apple_silicon() {
            let budget = (total_ram_mb as f64 * 0.70) as usize;
            return (GpuKind::AppleSilicon, budget, budget);
        }
        // Intel Mac: discrete GPU detection via system_profiler.
        if let Some((total, free)) = detect_macos_intel_vram() {
            return (GpuKind::AppleIntel, total, free);
        }
    }
    let _ = total_ram_mb;

    // Nothing detected — be honest about CPU-only.
    (GpuKind::Cpu, 0, 0)
}

fn detect_nvidia_smi() -> Option<(usize, usize)> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    // Sum across all GPUs — for multi-GPU rigs the user gets the aggregate.
    // We surface only the first card's free for tighter "fits this model"
    // reasoning, since Ollama loads to one GPU at a time by default.
    let mut total_sum = 0usize;
    let mut first_free = None;
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() != 2 {
            continue;
        }
        let total: usize = parts[0].parse().ok()?;
        let free: usize = parts[1].parse().ok()?;
        total_sum += total;
        if first_free.is_none() {
            first_free = Some(free);
        }
    }
    Some((total_sum, first_free?))
}

fn detect_rocm_smi() -> Option<(usize, usize)> {
    // `rocm-smi --showmeminfo vram --json` is the most parser-friendly output.
    let output = Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    // Schema: { "card0": { "VRAM Total Memory (B)": "...", "VRAM Total Used Memory (B)": "..." } }
    let card = json.as_object()?.values().next()?;
    let total_b: u64 = card.get("VRAM Total Memory (B)")?.as_str()?.parse().ok()?;
    let used_b: u64 = card
        .get("VRAM Total Used Memory (B)")?
        .as_str()?
        .parse()
        .ok()?;
    let total_mb = (total_b / (1024 * 1024)) as usize;
    let used_mb = (used_b / (1024 * 1024)) as usize;
    Some((total_mb, total_mb.saturating_sub(used_mb)))
}

#[cfg(target_os = "macos")]
fn is_apple_silicon() -> bool {
    Command::new("sysctl")
        .args(["-n", "hw.optional.arm64"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn detect_macos_intel_vram() -> Option<(usize, usize)> {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let displays = json.get("SPDisplaysDataType")?.as_array()?;
    let display = displays.first()?;
    let vram = display.get("spdisplays_vram").and_then(|v| v.as_str())?;
    let total = parse_vram_string(vram)?;
    // No way to read "free" without Metal API; assume the user has it all.
    Some((total, total))
}

/// Parses strings like `"8192 MB"`, `"8 GB"`, `"16384"` into MB.
///
/// Only used by `detect_macos_intel_vram`, which is itself
/// `cfg(target_os = "macos")` — gating this function the same way prevents
/// `clippy::dead_code` from firing on Linux/Windows builds.
#[cfg(target_os = "macos")]
pub(crate) fn parse_vram_string(s: &str) -> Option<usize> {
    let s = s.trim();
    let upper = s.to_ascii_uppercase();
    let (num_part, multiplier) = if let Some(stripped) = upper.strip_suffix("GB") {
        (stripped.trim(), 1024usize)
    } else if let Some(stripped) = upper.strip_suffix("MB") {
        (stripped.trim(), 1usize)
    } else {
        (upper.as_str(), 1usize)
    };
    num_part.parse::<usize>().ok().map(|n| n * multiplier)
}

fn generate_recommendations(profile: &HardwareProfile, vram_status: ResourceStatus) -> Vec<String> {
    let mut out = Vec::new();
    if profile.gpu_kind == GpuKind::Cpu {
        out.push(
            "no GPU detected — running on CPU. Expect ~5-15 tok/s on a small \
             model. Pull `qwen3.5:0.8b` and use `--num_ctx 4096`."
                .to_string(),
        );
        return out;
    }
    match vram_status {
        ResourceStatus::Critical => {
            out.push(format!(
                "CRITICAL: only {} MB free VRAM. Unload other models with `ollama stop`.",
                profile.free_vram_mb
            ));
        }
        ResourceStatus::Warning => {
            out.push(format!(
                "warning: {} MB free VRAM. set `num_gpu={}` to keep some layers on CPU.",
                profile.free_vram_mb, profile.optimal_gpu_layers
            ));
        }
        ResourceStatus::Healthy => {
            out.push(format!(
                "healthy. recommended: {} with num_ctx={}",
                profile.recommended_model, profile.optimal_context
            ));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStatus {
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub hardware_profile: HardwareProfile,
    pub vram_status: ResourceStatus,
    pub ram_status: ResourceStatus,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OptimizationPlan {
    pub recommended_model: String,
    pub optimal_context: usize,
    pub optimal_num_gpu: i32,
    pub keep_alive_duration: i64,
    pub unload_threshold_mb: usize,
    pub apply_onnx: bool,
}

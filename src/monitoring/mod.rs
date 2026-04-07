use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub struct VramSentinel {
    min_free_vram_mb: usize,
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
    pub recommended_model: String,
    pub optimal_context: usize,
    pub optimal_gpu_layers: i32,
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
        
        let total_ram = sys.total_memory() / (1024 * 1024);
        let used_ram = (sys.total_memory() - sys.available_memory()) / (1024 * 1024);
        let free_ram = sys.available_memory() / (1024 * 1024);
        
        let cpu_cores = sys.cpus().len();
        
        let os = std::env::consts::OS.to_string();
        
        let (total_vram, free_vram) = self.detect_vram_internal(&os);
        
        let recommended_model = self.suggest_model(free_vram);
        let optimal_context = self.calculate_optimal_context(free_vram);
        let optimal_gpu_layers = self.calculate_gpu_layers(free_vram);

        HardwareProfile {
            total_vram_mb: total_vram,
            free_vram_mb: free_vram,
            used_vram_mb: total_vram.saturating_sub(free_vram),
            total_ram_mb: total_ram as usize,
            cpu_cores,
            os,
            recommended_model,
            optimal_context,
            optimal_gpu_layers,
        }
    }

    fn detect_vram_internal(&self, os: &str) -> (usize, usize) {
        #[cfg(target_os = "macos")]
        {
            self.detect_macos_vram()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.detect_linux_vram()
        }
        #[cfg(target_os = "windows")]
        {
            self.detect_windows_vram()
        }
        #[cfg(not(any(target_os = "macos", unix, windows)))]
        {
            (8192, 4096)
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_macos_vram(&self) -> (usize, usize) {
        use std::process::Command;
        
        if let Ok(output) = Command::new("system_profiler")
            .args(["SPDisplaysDataType", "-json"])
            .output()
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(displays) = json.get("SPDisplaysDataType") {
                    if let Some(display) = displays.as_array().and_then(|a| a.first()) {
                        if let Some(vram) = display.get("spdisplays_vram")
                            .and_then(|v| v.as_str())
                        {
                            let mb = parse_vram_string(vram);
                            let total = mb.unwrap_or(8192);
                            return (total, total / 2);
                        }
                    }
                }
            }
        }
        
        if let Ok(output) = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if let Ok(mem) = String::from_utf8(output.stdout) {
                let bytes: u64 = mem.trim().parse().unwrap_or(8_000_000_000);
                let mb = (bytes / (1024 * 1024)) as usize;
                return (mb / 4, mb / 8);
            }
        }
        
        (8192, 4096)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn detect_linux_vram(&self) -> (usize, usize) {
        use std::process::Command;
        
        if let Ok(output) = Command::new("nvidia-smi")
            .args(["--query-gpu=memory.total,memory.free", "--format=csv,noheader,nounits"])
            .output()
        {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                if let Some(line) = stdout.lines().next() {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() == 2 {
                        let total = parts[0].parse().unwrap_or(0);
                        let free = parts[1].parse().unwrap_or(0);
                        return (total, free);
                    }
                }
            }
        }
        
        (16384, 8192)
    }

    #[cfg(target_os = "windows")]
    fn detect_windows_vram(&self) -> (usize, usize) {
        use std::process::Command;
        
        if let Ok(output) = Command::new("wmic")
            .args(["path", "win32_VideoController", "get", "AdapterRAM", "/format:value"])
            .output()
        {
            if let Ok(stdout) = String::from_utf8_lossy(&output.stdout).to_string().lines().next() {
                if let Some(value) = stdout.split('=').nth(1) {
                    if let Ok(bytes) = value.trim().parse::<u64>() {
                        return ((bytes / (1024 * 1024)) as usize, (bytes / (2 * 1024 * 1024)) as usize);
                    }
                }
            }
        }
        
        (16384, 8192)
    }

    fn suggest_model(&self, free_vram_mb: usize) -> String {
        match free_vram_mb {
            v if v >= 48_000 => "llama3.3:70b".to_string(),
            v if v >= 32_000 => "deepseek-coder-v2:16b".to_string(),
            v if v >= 24_000 => "qwen2.5-coder:14b".to_string(),
            v if v >= 16_000 => "codellama:13b".to_string(),
            v if v >= 12_000 => "qwen2.5-coder:7b".to_string(),
            v if v >= 8_000 => "codellama:7b".to_string(),
            v if v >= 6_000 => "llama3.2:3b".to_string(),
            v if v >= 4_000 => "phi4:4b".to_string(),
            _ => "llama3.2:1b".to_string(),
        }
    }

    fn calculate_optimal_context(&self, free_vram_mb: usize) -> usize {
        match free_vram_mb {
            v if v >= 32_000 => 131072,
            v if v >= 24_000 => 65536,
            v if v >= 16_000 => 32768,
            v if v >= 12_000 => 16384,
            v if v >= 8_000 => 8192,
            _ => 4096,
        }
    }

    fn calculate_gpu_layers(&self, free_vram_mb: usize) -> i32 {
        match free_vram_mb {
            v if v >= 32_000 => -1,
            v if v >= 24_000 => 100,
            v if v >= 16_000 => 75,
            v if v >= 12_000 => 50,
            v if v >= 8_000 => 33,
            _ => 0,
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
        
        HealthStatus {
            hardware_profile: profile.clone(),
            vram_status,
            ram_status: if profile.total_ram_mb - profile.free_vram_mb < profile.total_ram_mb / 2 {
                ResourceStatus::Healthy
            } else {
                ResourceStatus::Warning
            },
            recommendations: self.generate_recommendations(&profile, vram_status),
        }
    }

    fn generate_recommendations(&self, profile: &HardwareProfile, vram_status: ResourceStatus) -> Vec<String> {
        let mut recommendations = Vec::new();
        
        match vram_status {
            ResourceStatus::Critical => {
                recommendations.push(format!(
                    "CRITICAL: Only {} MB VRAM free. Consider unloading unused models.",
                    profile.free_vram_mb
                ));
                recommendations.push(format!(
                    "Recommended model: {}. Consider quantized versions.",
                    self.suggest_model(profile.free_vram_mb)
                ));
            }
            ResourceStatus::Warning => {
                recommendations.push(format!(
                    "Warning: {} MB VRAM free. Optimize with num_gpu={}",
                    profile.free_vram_mb, profile.optimal_gpu_layers
                ));
            }
            ResourceStatus::Healthy => {
                recommendations.push(format!(
                    "VRAM healthy. Using context={} for optimal performance.",
                    profile.optimal_context
                ));
            }
        }
        
        recommendations
    }

    pub async fn auto_optimize(&self) -> Result<OptimizationPlan> {
        let profile = self.detect_hardware().await;
        
        let plan = OptimizationPlan {
            recommended_model: self.suggest_model(profile.free_vram_mb),
            optimal_context: profile.optimal_context,
            optimal_num_gpu: profile.optimal_gpu_layers,
            keep_alive_duration: if profile.free_vram_mb < 8_000 { 300 } else { 3600 },
            unload_threshold_mb: profile.free_vram_mb / 4,
            apply_onnx: profile.free_vram_mb < 8_000,
        };
        
        info!("Generated optimization plan: {:?}", plan);
        Ok(plan)
    }
}

fn parse_vram_string(s: &str) -> Option<usize> {
    let cleaned = s.replace(" MB", "").replace("MB", "").replace("GB", "");
    cleaned.trim().parse().ok().map(|mb| {
        if s.contains("GB") { mb * 1024 } else { mb }
    })
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

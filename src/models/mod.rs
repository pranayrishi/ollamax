//! Curated, hardware-tiered registry of current open-weight models.
//!
//! ## What this catalog promises
//!
//! Model names, tags, and hardware requirements change quickly. The catalog is
//! deliberately data-driven and records enough information for callers to be
//! honest about each option:
//!
//! - [`LocalAvailability`] distinguishes a model that can be pulled through
//!   local Ollama, a model that must be self-hosted through a separate local
//!   server, and a cloud-only offering.
//! - [`ModelRuntime`] identifies the API a caller must use. A self-hosted
//!   vLLM/SGLang/llama.cpp server is OpenAI-compatible, but is not an Ollama
//!   pull target.
//! - [`ModelCapabilities`] keeps text-only reasoners out of screenshot and
//!   spatial-routing paths.
//! - [`ModelRegistry::fits`] and [`ModelRegistry::recommend`] only operate on
//!   genuinely local, Ollama-pullable models. A cloud model can never become an
//!   offline recommendation by accident.
//!
//! [`ModelRegistry::all`] deliberately retains its historical, Ollama-only
//! behavior for existing callers. Use [`ModelRegistry::catalog`] to enumerate
//! the full catalog, including explicit self-hosted and cloud-only disclosures.

use serde::{Deserialize, Serialize};

/// Hardware bracket a model is realistic on. Coarse on purpose — the precise
/// gate is [`ModelRegistry::fits`] using the per-model VRAM estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HardwareTier {
    /// Runs on modest hardware (~8 GB of VRAM/unified memory, or CPU).
    Modest,
    /// A single consumer GPU (~16–24 GB).
    Single,
    /// High-end / multi-GPU / enterprise (large MoE, 200B+).
    HighEnd,
}

impl HardwareTier {
    pub fn label(self) -> &'static str {
        match self {
            HardwareTier::Modest => "Modest (~8 GB)",
            HardwareTier::Single => "Single GPU (~16–24 GB)",
            HardwareTier::HighEnd => "High-end / multi-GPU",
        }
    }

    /// Which bracket a machine's free VRAM lands in. `0` (unknown / CPU) maps to
    /// `Modest` so we never default someone into a model they can't run.
    pub fn for_vram(free_vram_mb: usize) -> HardwareTier {
        match free_vram_mb {
            v if v >= 24_000 => HardwareTier::HighEnd,
            v if v >= 12_000 => HardwareTier::Single,
            _ => HardwareTier::Modest,
        }
    }
}

/// Model license, surfaced so commercial caveats are visible. We prefer
/// Apache-2.0 / MIT for a commercial product; custom licenses require review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum License {
    Apache2,
    Mit,
    /// e.g. Kimi K2's modified MIT.
    ModifiedMit,
    /// Meta Llama community license (custom; has acceptable-use + scale clauses).
    Llama,
    /// Older Google Gemma terms (Gemma 4 itself is Apache-2.0).
    Gemma,
    /// Mistral non-production / research license (NOT free for commercial use).
    MistralResearch,
    /// MiniMax M3 Community License: notices and commercial-use conditions apply.
    MiniMaxCommunity,
    Other,
}

impl License {
    pub fn spdx(self) -> &'static str {
        match self {
            License::Apache2 => "Apache-2.0",
            License::Mit => "MIT",
            License::ModifiedMit => "MIT (modified)",
            License::Llama => "Llama Community",
            License::Gemma => "Gemma Terms",
            License::MistralResearch => "Mistral Research (non-commercial)",
            License::MiniMaxCommunity => "MiniMax Community License",
            License::Other => "see model card",
        }
    }

    /// True for licenses that are unambiguously fine for a commercial product.
    /// Custom licenses need the user to read their terms, so they return false
    /// and the UI can flag them.
    pub fn commercial_friendly(self) -> bool {
        matches!(self, License::Apache2 | License::Mit | License::ModifiedMit)
    }
}

/// API/runtime used to invoke a model once it has been installed or deployed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelRuntime {
    /// Use the local Ollama HTTP API after `ollama pull <tag>`.
    #[default]
    Ollama,
    /// Use an OpenAI-compatible endpoint from vLLM, SGLang, llama.cpp, MLX, etc.
    OpenAiCompatible,
}

impl ModelRuntime {
    pub fn label(self) -> &'static str {
        match self {
            ModelRuntime::Ollama => "Ollama",
            ModelRuntime::OpenAiCompatible => "OpenAI-compatible local endpoint",
        }
    }
}

/// Whether a catalog entry is actually available offline on the user's machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalAvailability {
    /// The model has a verified local Ollama tag and can be installed with pull.
    #[default]
    OllamaLocal,
    /// Open weights exist, but the user must operate a separate local server.
    SelfHostedLocal,
    /// The known offering is remote/cloud-only. Never recommend it as offline.
    CloudOnly,
}

impl LocalAvailability {
    pub fn label(self) -> &'static str {
        match self {
            LocalAvailability::OllamaLocal => "Local via Ollama",
            LocalAvailability::SelfHostedLocal => "Local when self-hosted",
            LocalAvailability::CloudOnly => "Cloud only (not offline)",
        }
    }

    pub fn is_local(self) -> bool {
        !matches!(self, LocalAvailability::CloudOnly)
    }

    pub fn is_ollama_pullable(self) -> bool {
        matches!(self, LocalAvailability::OllamaLocal)
    }
}

/// Return whether an Ollama model selector names a hosted Cloud variant.
///
/// Ollama publishes both the simple `model:cloud` form and parameterized
/// variants such as `model:397b-cloud` / `model:31b-cloud`. The latter must
/// not slip through a local-only picker merely because it does not end in the
/// exact string `:cloud`. Only a final tag segment is considered, so a local
/// model whose repository name happens to contain `cloud` is unaffected.
pub fn is_ollama_cloud_tag(model: &str) -> bool {
    let Some((_, tag)) = model.trim().rsplit_once(':') else {
        return false;
    };
    let tag = tag.to_ascii_lowercase();
    tag == "cloud" || tag.ends_with("-cloud")
}

/// Return whether an Ollama tag is eligible for an offline execution path.
/// Keep this small predicate shared by CLI, server, and Build routing so a
/// future `/api/tags` response cannot reintroduce a hosted Cloud variant into
/// one of their automatic fallbacks.
pub fn is_offline_ollama_tag(model: &str) -> bool {
    !model.trim().is_empty() && !is_ollama_cloud_tag(model)
}

/// Runtime-level capabilities. `screen_grounding` means a vision model may be
/// used to propose a UI target; it does *not* authorize an action without DOM,
/// accessibility, or coordinate validation by the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub text: bool,
    pub vision: bool,
    pub audio: bool,
    pub video: bool,
    pub tools: bool,
    pub thinking: bool,
    pub screen_grounding: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            text: true,
            vision: false,
            audio: false,
            video: false,
            tools: false,
            thinking: false,
            screen_grounding: false,
        }
    }
}

const CAPS_TEXT: ModelCapabilities = ModelCapabilities {
    text: true,
    vision: false,
    audio: false,
    video: false,
    tools: false,
    thinking: false,
    screen_grounding: false,
};

const CAPS_REASONING: ModelCapabilities = ModelCapabilities {
    tools: true,
    thinking: true,
    ..CAPS_TEXT
};

const CAPS_VISION_AGENT: ModelCapabilities = ModelCapabilities {
    vision: true,
    tools: true,
    thinking: true,
    screen_grounding: true,
    ..CAPS_TEXT
};

const CAPS_OCR: ModelCapabilities = ModelCapabilities {
    vision: true,
    ..CAPS_TEXT
};

const CAPS_MINIMAX_M3: ModelCapabilities = ModelCapabilities {
    vision: true,
    video: true,
    tools: true,
    thinking: true,
    screen_grounding: true,
    ..CAPS_TEXT
};

/// One curated open-weight model. Dynamic fields are reconciled at runtime;
/// the seed carries only static, reviewed facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedModel {
    /// Human family name, e.g. "Qwen3.6".
    pub family: String,
    /// The `ollama pull` target. Empty for self-hosted-only entries; callers
    /// must check [`Self::can_pull_from_ollama`] before rendering a pull command.
    pub ollama_tag: String,
    /// Canonical model identifier or official catalog reference.
    #[serde(default)]
    pub source_ref: String,
    /// Invocation API expected by this catalog entry.
    #[serde(default)]
    pub runtime: ModelRuntime,
    /// Whether this is local Ollama, separate self-hosting, or cloud-only.
    #[serde(default)]
    pub local_availability: LocalAvailability,
    pub params: String,
    pub tier: HardwareTier,
    /// Rough resident VRAM at the stated local precision, in MB. It is a lower
    /// bound for fit checks; KV cache and long contexts require additional room.
    pub approx_vram_mb: usize,
    /// Download size when an official local artifact is known. It is not a RAM
    /// estimate and is intentionally absent for self-hosted server models whose
    /// official artifact format/hardware configuration varies.
    #[serde(default)]
    pub download_size_mb: Option<usize>,
    /// Official/native context window when known. The caller must still choose
    /// a context that fits the current machine's KV-cache budget.
    #[serde(default)]
    pub context_window_tokens: Option<usize>,
    pub license: License,
    /// What it's good at, terse: "coding", "agentic", "general", "reasoning".
    pub strengths: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Higher is preferred when otherwise eligible. This lets the current,
    /// hardware-appropriate lineup win over a larger legacy model.
    #[serde(default)]
    pub recommendation_priority: u16,
    /// Important deployment, runtime, or license caveat shown verbatim by UIs.
    #[serde(default)]
    pub caveat: String,
    /// Mutable Ollama aliases currently known to resolve to this exact variant.
    /// Tags such as `:latest` are reconciled at runtime and should never be used
    /// as a packaged/pinned artifact identity.
    #[serde(default)]
    pub installed_aliases: Vec<String>,
    /// True once reconciled against local Ollama `/api/tags`.
    #[serde(default)]
    pub installed: bool,
    /// `Some(true/false)` after a live library check; `None` if not checked.
    #[serde(default)]
    pub library_verified: Option<bool>,
}

impl CuratedModel {
    /// Whether this entry can truthfully be installed with `ollama pull`.
    pub fn can_pull_from_ollama(&self) -> bool {
        self.local_availability.is_ollama_pullable() && !self.ollama_tag.is_empty()
    }

    /// A pull command only for actual local Ollama artifacts.
    pub fn pull_command(&self) -> Option<String> {
        self.can_pull_from_ollama()
            .then(|| format!("ollama pull {}", self.ollama_tag))
    }

    /// Whether a model can operate without a remote inference service. This is
    /// true for self-hosted entries too, but they are deliberately not automatic
    /// recommendations because this application does not provision their server.
    pub fn is_offline_local(&self) -> bool {
        self.local_availability.is_local()
    }

    fn matches_installed(&self, installed: &[String]) -> bool {
        self.can_pull_from_ollama()
            && std::iter::once(self.ollama_tag.as_str())
                .chain(self.installed_aliases.iter().map(String::as_str))
                .any(|tag| installed.iter().any(|item| tag_matches(tag, item)))
    }
}

/// Static facts used to materialize [`CuratedModel`] values. Keeping the seed
/// declarative makes model/version review a data edit rather than routing logic.
#[derive(Clone, Copy)]
struct ModelSeed {
    family: &'static str,
    ollama_tag: &'static str,
    source_ref: &'static str,
    runtime: ModelRuntime,
    local_availability: LocalAvailability,
    params: &'static str,
    tier: HardwareTier,
    approx_vram_mb: usize,
    download_size_mb: Option<usize>,
    context_window_tokens: Option<usize>,
    license: License,
    strengths: &'static str,
    capabilities: ModelCapabilities,
    recommendation_priority: u16,
    caveat: &'static str,
    installed_aliases: &'static [&'static str],
}

impl ModelSeed {
    fn ollama(
        family: &'static str,
        ollama_tag: &'static str,
        params: &'static str,
        tier: HardwareTier,
        approx_vram_mb: usize,
        license: License,
        strengths: &'static str,
    ) -> Self {
        Self {
            family,
            ollama_tag,
            source_ref: ollama_tag,
            runtime: ModelRuntime::Ollama,
            local_availability: LocalAvailability::OllamaLocal,
            params,
            tier,
            approx_vram_mb,
            download_size_mb: None,
            context_window_tokens: None,
            license,
            strengths,
            capabilities: CAPS_TEXT,
            recommendation_priority: 1,
            caveat: "",
            installed_aliases: &[],
        }
    }

    fn self_hosted(
        family: &'static str,
        source_ref: &'static str,
        params: &'static str,
        tier: HardwareTier,
        approx_vram_mb: usize,
        license: License,
        strengths: &'static str,
    ) -> Self {
        Self {
            family,
            ollama_tag: "",
            source_ref,
            runtime: ModelRuntime::OpenAiCompatible,
            local_availability: LocalAvailability::SelfHostedLocal,
            params,
            tier,
            approx_vram_mb,
            download_size_mb: None,
            context_window_tokens: None,
            license,
            strengths,
            capabilities: CAPS_TEXT,
            recommendation_priority: 0,
            caveat: "Requires a separately managed local inference server.",
            installed_aliases: &[],
        }
    }

    fn cloud(
        family: &'static str,
        ollama_tag: &'static str,
        source_ref: &'static str,
        params: &'static str,
        tier: HardwareTier,
        license: License,
        strengths: &'static str,
    ) -> Self {
        Self {
            family,
            ollama_tag,
            source_ref,
            runtime: ModelRuntime::Ollama,
            local_availability: LocalAvailability::CloudOnly,
            params,
            tier,
            approx_vram_mb: 0,
            download_size_mb: None,
            context_window_tokens: None,
            license,
            strengths,
            capabilities: CAPS_TEXT,
            recommendation_priority: 0,
            caveat: "Cloud-only entry; never present it as a free offline model.",
            installed_aliases: &[],
        }
    }

    // The reviewed catalog is deliberately written as compact, auditable
    // declarative entries. Keeping all metadata beside the seed constructor is
    // clearer than splitting every record across a transient options struct.
    #[allow(clippy::too_many_arguments)]
    fn details(
        mut self,
        source_ref: &'static str,
        capabilities: ModelCapabilities,
        context_window_tokens: Option<usize>,
        download_size_mb: Option<usize>,
        recommendation_priority: u16,
        caveat: &'static str,
        installed_aliases: &'static [&'static str],
    ) -> Self {
        self.source_ref = source_ref;
        self.capabilities = capabilities;
        self.context_window_tokens = context_window_tokens;
        self.download_size_mb = download_size_mb;
        self.recommendation_priority = recommendation_priority;
        self.caveat = caveat;
        self.installed_aliases = installed_aliases;
        self
    }

    fn materialize(self) -> CuratedModel {
        CuratedModel {
            family: self.family.to_string(),
            ollama_tag: self.ollama_tag.to_string(),
            source_ref: self.source_ref.to_string(),
            runtime: self.runtime,
            local_availability: self.local_availability,
            params: self.params.to_string(),
            tier: self.tier,
            approx_vram_mb: self.approx_vram_mb,
            download_size_mb: self.download_size_mb,
            context_window_tokens: self.context_window_tokens,
            license: self.license,
            strengths: self.strengths.to_string(),
            capabilities: self.capabilities,
            recommendation_priority: self.recommendation_priority,
            caveat: self.caveat.to_string(),
            installed_aliases: self
                .installed_aliases
                .iter()
                .map(|alias| (*alias).to_string())
                .collect(),
            installed: false,
            library_verified: None,
        }
    }
}

/// The curated set. Cheap to construct; safe to call often.
pub struct ModelRegistry {
    // Kept separate so legacy callers of `all`, `fits`, and `recommend` can
    // only display safe, local-Ollama options and cannot accidentally render a
    // self-hosted model as an `ollama pull` command.
    ollama_entries: Vec<CuratedModel>,
    additional_entries: Vec<CuratedModel>,
}

impl ModelRegistry {
    /// The reviewed mid-2026 seed lineup. `:latest` aliases are only used to
    /// reconcile an already-installed model; explicit size tags remain the
    /// catalog's install targets.
    pub fn seed() -> Self {
        use HardwareTier::*;
        use License::*;

        let ollama_entries = vec![
            // --- Current laptop / modest-hardware defaults ---
            ModelSeed::ollama(
                "Qwen3.5",
                "qwen3.5:4b",
                "4B",
                Modest,
                3_400,
                Apache2,
                "general, coding, vision",
            )
            .details(
                "ollama.com/library/qwen3.5",
                CAPS_VISION_AGENT,
                Some(256_000),
                Some(3_400),
                145,
                "Current local Qwen visual generalist; reserve KV-cache headroom for long context.",
                &[],
            ),
            ModelSeed::ollama(
                "Qwen3.5",
                "qwen3.5:9b",
                "9B",
                Single,
                6_600,
                Apache2,
                "general, coding, vision",
            )
            .details(
                "ollama.com/library/qwen3.5",
                CAPS_VISION_AGENT,
                Some(256_000),
                Some(6_600),
                185,
                "Current consumer Qwen visual generalist; the mutable :latest alias currently maps here.",
                &["qwen3.5:latest"],
            ),
            ModelSeed::ollama(
                "Gemma 4",
                "gemma4:e2b",
                "E2B (5.1B incl. embeddings)",
                Modest,
                2_900,
                Apache2,
                "edge general, coding, vision",
            )
            .details(
                "google/gemma-4-E2B-it",
                CAPS_VISION_AGENT,
                Some(128_000),
                Some(7_200),
                120,
                "Official Q4 static-weight estimate is ~2.9 GB, while the Ollama download is ~7.2 GB; current Ollama listing is Text/Image, so do not assume audio routing without a runtime probe.",
                &[],
            ),
            ModelSeed::ollama(
                "Gemma 4",
                "gemma4:e4b",
                "E4B (8B incl. embeddings)",
                Modest,
                4_500,
                Apache2,
                "edge general, coding, vision",
            )
            .details(
                "google/gemma-4-E4B-it",
                CAPS_VISION_AGENT,
                Some(128_000),
                Some(9_600),
                155,
                "Official Q4 static-weight estimate is ~4.5 GB; current Ollama listing is Text/Image. Gemma UI understanding is useful for target proposals, never direct execution.",
                &["gemma4:latest"],
            ),
            ModelSeed::ollama(
                "DeepSeek-R1",
                "deepseek-r1:1.5b",
                "1.5B",
                Modest,
                1_100,
                Mit,
                "compact reasoning (text-only)",
            )
            .details(
                "ollama.com/library/deepseek-r1",
                CAPS_REASONING,
                Some(128_000),
                Some(1_100),
                45,
                "Text-only distilled reasoner; never route screenshots or spatial targeting to it.",
                &[],
            ),
            ModelSeed::ollama(
                "DeepSeek-R1-0528-Qwen3",
                "deepseek-r1:8b",
                "8B",
                Modest,
                5_200,
                Mit,
                "reasoning, coding (text-only)",
            )
            .details(
                "deepseek-ai/DeepSeek-R1-0528-Qwen3-8B",
                CAPS_REASONING,
                Some(128_000),
                Some(5_200),
                165,
                "The current local DeepSeek default is R1-0528-Qwen3-8B. It is text-only, so pair it with a vision model for spatial work.",
                &["deepseek-r1:latest"],
            ),
            // --- Current workstation models ---
            ModelSeed::ollama(
                "Gemma 4",
                "gemma4:12b",
                "12B",
                Single,
                6_700,
                Apache2,
                "general, coding, vision",
            )
            .details(
                "google/gemma-4-12B-it",
                CAPS_VISION_AGENT,
                Some(256_000),
                Some(7_600),
                200,
                "Official Q4 static-weight estimate is ~6.7 GB; long context substantially increases the required KV cache.",
                &[],
            ),
            ModelSeed::ollama(
                "DeepSeek-R1",
                "deepseek-r1:14b",
                "14B",
                Single,
                9_000,
                Mit,
                "reasoning, coding (text-only)",
            )
            .details(
                "ollama.com/library/deepseek-r1",
                CAPS_REASONING,
                Some(128_000),
                Some(9_000),
                175,
                "Text-only reasoning model; it is not a spatial or image-understanding worker.",
                &[],
            ),
            ModelSeed::ollama(
                "Gemma 4",
                "gemma4:26b",
                "26B A4B (MoE)",
                Single,
                14_400,
                Apache2,
                "fast workstation reasoning, coding, vision",
            )
            .details(
                "google/gemma-4-26B-A4B-it",
                CAPS_VISION_AGENT,
                Some(256_000),
                Some(18_000),
                225,
                "Only ~3.8B parameters are active per token, but all ~25B parameters must be loaded; do not size hardware from active parameters alone.",
                &[],
            ),
            ModelSeed::ollama(
                "Qwen3.6",
                "qwen3.6:27b",
                "27B",
                Single,
                17_000,
                Apache2,
                "agentic coding, reasoning, vision",
            )
            .details(
                "Qwen/Qwen3.6-27B",
                CAPS_VISION_AGENT,
                Some(262_144),
                Some(17_000),
                255,
                "For self-hosted vLLM/SGLang, use the Qwen reasoning parser and qwen3_coder tool parser. Do not promise its full 256K context on a small GPU.",
                &[],
            ),
            ModelSeed::ollama(
                "DeepSeek-R1",
                "deepseek-r1:32b",
                "32B",
                HighEnd,
                20_000,
                Mit,
                "high-end reasoning, coding (text-only)",
            )
            .details(
                "ollama.com/library/deepseek-r1",
                CAPS_REASONING,
                Some(128_000),
                Some(20_000),
                230,
                "Text-only model; retain separate visual grounding and action validation workers.",
                &[],
            ),
            ModelSeed::ollama(
                "Gemma 4",
                "gemma4:31b",
                "31B",
                HighEnd,
                17_500,
                Apache2,
                "frontier local reasoning, coding, vision",
            )
            .details(
                "google/gemma-4-31B-it",
                CAPS_VISION_AGENT,
                Some(256_000),
                Some(20_000),
                245,
                "Official Q4 static-weight estimate is ~17.5 GB. Treat it as a workstation model once KV-cache headroom is included.",
                &[],
            ),
            ModelSeed::ollama(
                "Qwen3.6",
                "qwen3.6:35b",
                "35B A3B (MoE)",
                HighEnd,
                24_000,
                Apache2,
                "frontier agentic coding, reasoning, vision",
            )
            .details(
                "Qwen/Qwen3.6-35B-A3B",
                CAPS_VISION_AGENT,
                Some(262_144),
                Some(24_000),
                270,
                "Current open-weight Qwen flagship local tag. Full native context needs much more than the weight download because of KV cache.",
                &["qwen3.6:latest"],
            ),
            // --- Existing catalog compatibility / secondary choices ---
            ModelSeed::ollama(
                "Qwen3",
                "qwen3:4b",
                "4B",
                Modest,
                3_500,
                Apache2,
                "general, coding",
            )
            .details(
                "ollama.com/library/qwen3",
                CAPS_REASONING,
                Some(256_000),
                None,
                95,
                "Previous Qwen generation retained for installed-model compatibility; prefer Qwen3.5/3.6 for a new install.",
                &[],
            ),
            ModelSeed::ollama(
                "Phi-4-mini",
                "phi4-mini",
                "3.8B",
                Modest,
                3_300,
                Mit,
                "reasoning, compact",
            ),
            ModelSeed::ollama(
                "Qwen2.5-Coder",
                "qwen2.5-coder:7b",
                "7B",
                Modest,
                5_500,
                Apache2,
                "coding",
            )
            .details(
                "ollama.com/library/qwen2.5-coder",
                CAPS_TEXT,
                None,
                None,
                105,
                "Retained for installed-model compatibility; prefer Qwen3.5/3.6 for new work.",
                &[],
            ),
            ModelSeed::ollama(
                "Gemma 3",
                "gemma3:4b",
                "4B",
                Modest,
                4_000,
                Gemma,
                "general",
            ),
            ModelSeed::ollama(
                "Qwen3",
                "qwen3:14b",
                "14B",
                Single,
                9_500,
                Apache2,
                "general, coding",
            ),
            ModelSeed::ollama(
                "Qwen3-Coder",
                "qwen3-coder:30b",
                "30B (MoE)",
                Single,
                20_000,
                Apache2,
                "coding, agentic",
            ),
            ModelSeed::ollama(
                "Devstral Small",
                "devstral",
                "24B",
                Single,
                15_000,
                Apache2,
                "agentic coding",
            ),
            ModelSeed::ollama(
                "DeepSeek-Coder-V2",
                "deepseek-coder-v2:16b",
                "16B (MoE)",
                Single,
                10_500,
                Other,
                "coding",
            ),
            ModelSeed::ollama(
                "Gemma 3",
                "gemma3:27b",
                "27B",
                Single,
                17_000,
                Gemma,
                "general",
            ),
            ModelSeed::ollama(
                "Codestral",
                "codestral",
                "22B",
                Single,
                13_500,
                MistralResearch,
                "coding (non-commercial license)",
            ),
            ModelSeed::ollama(
                "DeepSeek-R1",
                "deepseek-r1:671b",
                "671B (MoE)",
                HighEnd,
                400_000,
                Mit,
                "reasoning, coding",
            )
            .details(
                "deepseek-ai/DeepSeek-R1",
                CAPS_REASONING,
                Some(160_000),
                Some(404_000),
                10,
                "Official local Ollama artifact, but a 404 GB download: server-class only, never a default for a personal computer.",
                &[],
            ),
            ModelSeed::ollama(
                "MiniMax M2.5",
                "hf.co/unsloth/MiniMax-M2.5-GGUF:Q4_K_M",
                "230B total / 10B active (MoE)",
                HighEnd,
                139_000,
                ModifiedMit,
                "coding, agentic, tool use",
            )
            .details(
                "MiniMaxAI/MiniMax-M2.5",
                CAPS_REASONING,
                None,
                Some(139_000),
                10,
                "The only free OFFLINE MiniMax install path: Ollama pulls this community Q4_K_M GGUF (unsloth) straight from Hugging Face — official `minimax-*` Ollama tags are cloud-only. ~139 GB download; realistic on 192 GB-class unified memory or multi-GPU rigs only. Weights are modified-MIT (MiniMax-AI/MiniMax-M2.5 LICENSE).",
                &["hf.co/unsloth/minimax-m2.5-gguf:q4_k_m"],
            ),
            ModelSeed::ollama(
                "Qwen3",
                "qwen3:235b",
                "235B (MoE)",
                HighEnd,
                150_000,
                Apache2,
                "frontier general",
            ),
            ModelSeed::ollama(
                "Llama 4 Scout",
                "llama4:scout",
                "109B (MoE)",
                HighEnd,
                70_000,
                Llama,
                "huge context",
            ),
            ModelSeed::ollama(
                "GLM-4.6",
                "glm4:latest",
                "large",
                HighEnd,
                60_000,
                Mit,
                "coding, agentic",
            ),
            ModelSeed::ollama(
                "Mistral Large",
                "mistral-large",
                "123B",
                HighEnd,
                73_000,
                MistralResearch,
                "general (non-commercial license)",
            ),
        ]
        .into_iter()
        .map(ModelSeed::materialize)
        .collect();

        let additional_entries = vec![
            // Open weights, but not valid `ollama pull` targets. These stay in
            // the catalog as explicit advanced options rather than being omitted
            // or mislabeled as laptop-local models.
            ModelSeed::self_hosted(
                "DeepSeek V4 Flash",
                "deepseek-ai/DeepSeek-V4-Flash",
                "284B total / 13B active (MoE)",
                HighEnd,
                250_000,
                Mit,
                "server reasoning, coding",
            )
            .details(
                "deepseek-ai/DeepSeek-V4-Flash",
                CAPS_REASONING,
                Some(1_000_000),
                None,
                0,
                "Latest DeepSeek open weights are server-class. Use the official DeepSeek V4 encoding adapter (not a generic Jinja chat template) and preserve reasoning_content; Ollamax's generic local endpoint path is experimental text/image Chat Completions only and does not expose that model-specific reasoning channel. No verified official local Ollama tag exists.",
                &[],
            ),
            ModelSeed::self_hosted(
                "DeepSeek V4 Pro",
                "deepseek-ai/DeepSeek-V4-Pro",
                "1.6T total / 49B active (MoE)",
                HighEnd,
                1_000_000,
                Mit,
                "frontier server reasoning, coding",
            )
            .details(
                "deepseek-ai/DeepSeek-V4-Pro",
                CAPS_REASONING,
                Some(1_000_000),
                None,
                0,
                "Server/cluster-only. It is open-weight but not a consumer offline installation and has no verified official local Ollama tag. Ollamax's generic local endpoint path is experimental text/image Chat Completions only; model-specific encoding and reasoning streams are not exposed.",
                &[],
            ),
            ModelSeed::self_hosted(
                "DeepSeek OCR 2",
                "deepseek-ai/DeepSeek-OCR-2",
                "3B",
                Single,
                6_500,
                Apache2,
                "OCR, document extraction",
            )
            .details(
                "deepseek-ai/DeepSeek-OCR-2",
                CAPS_OCR,
                None,
                None,
                0,
                "Optional OCR sidecar, not a general action agent. Official example targets Transformers on NVIDIA CUDA and uses trust_remote_code.",
                &[],
            ),
            ModelSeed::self_hosted(
                "MiniMax M3",
                "MiniMaxAI/MiniMax-M3",
                "~427B total / ~23B active (MoE)",
                HighEnd,
                512_000,
                MiniMaxCommunity,
                "frontier coding, agentic, vision/video",
            )
            .details(
                "MiniMaxAI/MiniMax-M3",
                CAPS_MINIMAX_M3,
                Some(1_000_000),
                None,
                0,
                "Open weights require separately operated SGLang/vLLM/Transformers-class infrastructure (validated vLLM paths use multi-accelerator server hardware). Ollamax's generic local endpoint path currently supports only text and declared-image Chat Completions; MiniMax video, native tool, and structured reasoning adapters are not enabled. Commercial use must retain MiniMax's required notices, including “Built with MiniMax M3”.",
                &[],
            ),
            ModelSeed::cloud(
                "MiniMax M3 (Ollama Cloud)",
                "minimax-m3:cloud",
                "ollama.com/library/minimax-m3",
                "cloud service",
                HighEnd,
                MiniMaxCommunity,
                "remote coding, agentic, vision",
            )
            .details(
                "ollama.com/library/minimax-m3",
                CAPS_MINIMAX_M3,
                Some(512_000),
                None,
                0,
                "Ollama publishes only minimax-m3:cloud. It is intentionally disclosed here to prevent it from being mistaken for a free offline MiniMax installation.",
                &[],
            ),
        ]
        .into_iter()
        .map(ModelSeed::materialize)
        .collect();

        Self {
            ollama_entries,
            additional_entries,
        }
    }

    /// Local, Ollama-pullable entries only. This is kept for compatibility with
    /// existing model pickers that render a pull command for every returned row.
    pub fn all(&self) -> &[CuratedModel] {
        &self.ollama_entries
    }

    /// The full reviewed catalog, including advanced self-hosted and cloud-only
    /// disclosure entries. Check [`CuratedModel::can_pull_from_ollama`] before
    /// presenting any install command.
    pub fn catalog(&self) -> impl Iterator<Item = &CuratedModel> {
        self.ollama_entries
            .iter()
            .chain(self.additional_entries.iter())
    }

    /// Entries that cannot be installed through the local Ollama pull flow.
    pub fn additional(&self) -> &[CuratedModel] {
        &self.additional_entries
    }

    /// Models whose estimated resident size fits in `free_vram_mb`, biggest
    /// first. Only genuine local Ollama entries participate. `0` (unknown VRAM
    /// / CPU) returns the Modest tier only — we'd rather under-promise than
    /// recommend a model the machine cannot load. A 25% headroom factor accounts
    /// for KV cache + runtime overhead.
    pub fn fits(&self, free_vram_mb: usize) -> Vec<&CuratedModel> {
        let mut models: Vec<&CuratedModel> = if free_vram_mb == 0 {
            self.ollama_entries
                .iter()
                .filter(|m| m.tier == HardwareTier::Modest && m.can_pull_from_ollama())
                .collect()
        } else {
            self.ollama_entries
                .iter()
                .filter(|m| {
                    m.can_pull_from_ollama()
                        && (m.approx_vram_mb as f64 * 1.25) as usize <= free_vram_mb
                })
                .collect()
        };
        models.sort_by_key(|model| std::cmp::Reverse(model.approx_vram_mb));
        models
    }

    /// Reconcile with the user's installed Ollama models. Matching uses the
    /// explicit tag plus reviewed aliases (for example `deepseek-r1:latest`).
    /// It never matches across parameter sizes.
    pub fn mark_installed(&mut self, installed: &[String]) {
        for model in &mut self.ollama_entries {
            model.installed = model.matches_installed(installed);
        }
    }

    /// Pick a sensible local default the machine can actually run. Preference:
    ///
    /// 1. installed, commercial-friendly coding model;
    /// 2. any installed local model;
    /// 3. highest-priority commercial-friendly current local model;
    /// 4. highest-priority remaining local model;
    /// 5. smallest local model only when VRAM is unknown (CPU / `0`).
    ///
    /// `SelfHostedLocal` and `CloudOnly` entries are deliberately excluded.
    pub fn recommend(&self, free_vram_mb: usize, installed: &[String]) -> Option<&CuratedModel> {
        // A zero value means we could not establish a dedicated VRAM budget
        // (CPU-only, integrated graphics, or an unsupported driver). In that
        // case do not let a recommendation priority turn an unknown machine
        // into a 5–9 GB load: choose the smallest reviewed Ollama-local model
        // outright. A user can still manually choose a stronger visual model
        // after seeing its actual hardware fit.
        if free_vram_mb == 0 {
            return self
                .ollama_entries
                .iter()
                .filter(|model| model.can_pull_from_ollama())
                .min_by_key(|model| model.approx_vram_mb);
        }
        let fits = self.fits(free_vram_mb);

        preferred(fits.iter().copied().filter(|model| {
            model.matches_installed(installed)
                && model.license.commercial_friendly()
                && model.strengths.contains("coding")
        }))
        .or_else(|| {
            preferred(
                fits.iter()
                    .copied()
                    .filter(|model| model.matches_installed(installed)),
            )
        })
        .or_else(|| {
            preferred(
                fits.iter()
                    .copied()
                    .filter(|model| model.license.commercial_friendly()),
            )
        })
        .or_else(|| preferred(fits.iter().copied()))
    }
}

fn preferred<'a>(models: impl Iterator<Item = &'a CuratedModel>) -> Option<&'a CuratedModel> {
    models.max_by_key(|model| (model.recommendation_priority, model.approx_vram_mb))
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::seed()
    }
}

/// True if an installed Ollama tag corresponds to a curated seed tag. Matches
/// the seed EXACTLY, or as a prefix up to a separator (`-` for a quant suffix,
/// `:` for a tag-less seed). So `qwen2.5-coder:7b` matches an installed
/// `qwen2.5-coder:7b-instruct-q4_K_M`, and `codestral` matches `codestral:22b`,
/// but `qwen3:4b` does NOT match `qwen3:14b` or `qwen3:235b` (different sizes).
fn tag_matches(seed: &str, installed: &str) -> bool {
    // Ollama's `/api/tags` can list hosted Cloud entries alongside pulled
    // local artifacts. A parameterized Cloud tag (for example
    // `gemma4:31b-cloud`) otherwise looks like a quantized suffix of the
    // local `gemma4:31b` seed. Keep the offline catalog boundary here as a
    // second line of defense for every caller that reconciles installed tags.
    !is_ollama_cloud_tag(installed)
        && !seed.is_empty()
        && (installed == seed
            || installed.starts_with(&format!("{seed}-"))
            || installed.starts_with(&format!("{seed}:")))
}

/// Best-effort live check that an Ollama tag still resolves in the public
/// library. Returns `None` on any network/parse error (never panics, never
/// blocks the catalog). This check establishes library presence only; callers
/// must use [`LocalAvailability`] to determine whether it is actually local.
pub async fn verify_in_library(ollama_tag: &str) -> Option<bool> {
    if ollama_tag.trim().is_empty() {
        return None;
    }
    // Two verifiable shapes: plain Ollama library tags, and `hf.co/org/repo`
    // direct-pull tags (checked against the Hugging Face repo page). Anything
    // else with a '/' (user namespaces etc.) stays "unverified" rather than
    // guessed at.
    let url = if let Some(hf_path) = ollama_tag.strip_prefix("hf.co/") {
        // `hf.co/org/repo:Q4_K_M` → strip the quant suffix after the LAST
        // colon (repo names themselves never contain a colon).
        let repo = hf_path.rsplit_once(':').map(|(r, _)| r).unwrap_or(hf_path);
        format!("https://huggingface.co/{repo}")
    } else if ollama_tag.contains('/') {
        return None;
    } else {
        let name = ollama_tag.split(':').next().unwrap_or(ollama_tag);
        format!("https://ollama.com/library/{name}")
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .user_agent("ollama-forge")
        .build()
        .ok()?;
    match client.get(&url).send().await {
        Ok(resp) => Some(resp.status().is_success()),
        Err(_) => None,
    }
}

// Small test helper kept out of the public surface.
#[cfg(test)]
impl CuratedModel {
    fn installed_matches(&self, installed: &[String]) -> bool {
        self.matches_installed(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_maps_vram_without_overpromising() {
        assert_eq!(HardwareTier::for_vram(0), HardwareTier::Modest);
        assert_eq!(HardwareTier::for_vram(8_000), HardwareTier::Modest);
        assert_eq!(HardwareTier::for_vram(16_000), HardwareTier::Single);
        assert_eq!(HardwareTier::for_vram(48_000), HardwareTier::HighEnd);
    }

    #[test]
    fn recognizes_all_ollama_cloud_tag_forms() {
        for tag in [
            "minimax-m3:cloud",
            "qwen3.5:397b-cloud",
            "gemma4:31b-cloud",
            " MODEL:Cloud ",
        ] {
            assert!(is_ollama_cloud_tag(tag), "should reject hosted tag {tag}");
        }
        for tag in ["cloud-local:4b", "qwen3.5:4b", "my-cloud-model"] {
            assert!(!is_ollama_cloud_tag(tag), "should retain local tag {tag}");
            assert!(
                is_offline_ollama_tag(tag),
                "should retain offline tag {tag}"
            );
        }
        assert!(!is_offline_ollama_tag("  "));
    }

    #[test]
    fn license_commercial_flags_are_honest() {
        assert!(License::Apache2.commercial_friendly());
        assert!(License::Mit.commercial_friendly());
        // Custom licenses must NOT be advertised as commercial-safe.
        assert!(!License::Gemma.commercial_friendly());
        assert!(!License::Llama.commercial_friendly());
        assert!(!License::MistralResearch.commercial_friendly());
        assert!(!License::MiniMaxCommunity.commercial_friendly());
    }

    #[test]
    fn latest_local_families_have_honest_capability_metadata() {
        let reg = ModelRegistry::seed();

        let gemma = reg
            .all()
            .iter()
            .find(|model| model.ollama_tag == "gemma4:e4b")
            .unwrap();
        assert_eq!(gemma.license, License::Apache2);
        assert!(gemma.capabilities.vision);
        assert!(gemma.capabilities.screen_grounding);
        assert_eq!(gemma.local_availability, LocalAvailability::OllamaLocal);
        assert_eq!(
            gemma.pull_command().as_deref(),
            Some("ollama pull gemma4:e4b")
        );

        let r1 = reg
            .all()
            .iter()
            .find(|model| model.ollama_tag == "deepseek-r1:8b")
            .unwrap();
        assert!(r1.capabilities.thinking);
        assert!(
            !r1.capabilities.vision,
            "R1 8B must not receive screenshots"
        );

        let qwen = reg
            .all()
            .iter()
            .find(|model| model.ollama_tag == "qwen3.6:27b")
            .unwrap();
        assert!(qwen.capabilities.vision);
        assert!(qwen.capabilities.tools);
        assert_eq!(qwen.context_window_tokens, Some(262_144));
    }

    #[test]
    fn advanced_entries_are_disclosed_but_not_rendered_as_ollama_pull_targets() {
        let reg = ModelRegistry::seed();
        assert!(reg
            .all()
            .iter()
            .all(|model| model.local_availability == LocalAvailability::OllamaLocal));

        let v4 = reg
            .catalog()
            .find(|model| model.source_ref == "deepseek-ai/DeepSeek-V4-Flash")
            .unwrap();
        assert_eq!(v4.runtime, ModelRuntime::OpenAiCompatible);
        assert_eq!(v4.local_availability, LocalAvailability::SelfHostedLocal);
        assert!(v4.is_offline_local());
        assert!(!v4.can_pull_from_ollama());
        assert!(v4.pull_command().is_none());

        let m3 = reg
            .catalog()
            .find(|model| model.source_ref == "MiniMaxAI/MiniMax-M3")
            .unwrap();
        assert_eq!(m3.local_availability, LocalAvailability::SelfHostedLocal);
        assert_eq!(m3.license, License::MiniMaxCommunity);
        assert!(m3.capabilities.video);
        assert!(m3.caveat.contains("Built with MiniMax M3"));
    }

    #[test]
    fn cloud_only_models_never_become_offline_recommendations() {
        let reg = ModelRegistry::seed();
        let cloud = reg
            .catalog()
            .find(|model| model.ollama_tag == "minimax-m3:cloud")
            .unwrap();
        assert_eq!(cloud.local_availability, LocalAvailability::CloudOnly);
        assert!(!cloud.is_offline_local());
        assert!(!cloud.can_pull_from_ollama());
        assert!(cloud.pull_command().is_none());

        for free_vram_mb in [0, 8_000, 24_000, 1_000_000] {
            let recommendation = reg.recommend(free_vram_mb, &[]).unwrap();
            assert_eq!(
                recommendation.local_availability,
                LocalAvailability::OllamaLocal
            );
            assert_ne!(recommendation.ollama_tag, "minimax-m3:cloud");
        }
    }

    #[test]
    fn fits_filters_to_what_actually_runs() {
        let reg = ModelRegistry::seed();
        // 8 GB machine: only small models, and nothing high-end.
        let small = reg.fits(8_000);
        assert!(!small.is_empty());
        assert!(small.iter().all(|model| {
            model.can_pull_from_ollama() && (model.approx_vram_mb as f64 * 1.25) as usize <= 8_000
        }));
        assert!(small
            .iter()
            .all(|model| model.tier != HardwareTier::HighEnd));
        // Biggest-first ordering.
        assert!(small
            .windows(2)
            .all(|window| window[0].approx_vram_mb >= window[1].approx_vram_mb));
    }

    #[test]
    fn fits_unknown_vram_returns_modest_ollama_only() {
        let reg = ModelRegistry::seed();
        let fitting = reg.fits(0);
        assert!(!fitting.is_empty());
        assert!(fitting
            .iter()
            .all(|model| { model.tier == HardwareTier::Modest && model.can_pull_from_ollama() }));
    }

    #[test]
    fn recommend_prefers_installed_coding_model_that_fits() {
        let reg = ModelRegistry::seed();
        let installed = vec!["qwen2.5-coder:7b".to_string()];
        let recommendation = reg.recommend(12_000, &installed).unwrap();
        assert_eq!(recommendation.ollama_tag, "qwen2.5-coder:7b");
        assert!(recommendation.installed_matches(&installed));
    }

    #[test]
    fn recommend_uses_current_priority_when_nothing_is_installed() {
        let reg = ModelRegistry::seed();
        let recommendation = reg.recommend(12_000, &[]).unwrap();
        assert_eq!(recommendation.ollama_tag, "gemma4:12b");
        assert!(recommendation.license.commercial_friendly());
    }

    #[test]
    fn recommend_returns_none_when_known_vram_fits_nothing() {
        let reg = ModelRegistry::seed();
        // 1 GB known free VRAM: even the smallest model's headroom doesn't fit.
        // Must be None (caller shows "nothing fits") — never a model that can't run.
        assert!(reg.recommend(1_000, &[]).is_none());
        // But unknown VRAM (0 = CPU) still suggests a modest local model.
        assert!(reg.recommend(0, &[]).is_some());
    }

    #[test]
    fn unknown_vram_uses_the_smallest_safe_local_model() {
        let reg = ModelRegistry::seed();
        let recommendation = reg.recommend(0, &[]).unwrap();
        assert_eq!(recommendation.ollama_tag, "deepseek-r1:1.5b");
        assert_eq!(recommendation.approx_vram_mb, 1_100);
    }

    #[test]
    fn mark_installed_matches_quantized_suffix_and_reviewed_latest_alias() {
        let mut reg = ModelRegistry::seed();
        reg.mark_installed(&[
            "qwen2.5-coder:7b-instruct-q4_K_M".to_string(),
            "deepseek-r1:latest".to_string(),
        ]);
        let coder = reg
            .all()
            .iter()
            .find(|model| model.ollama_tag == "qwen2.5-coder:7b")
            .unwrap();
        assert!(
            coder.installed,
            "quantized suffix should match the seed tag"
        );
        let r1 = reg
            .all()
            .iter()
            .find(|model| model.ollama_tag == "deepseek-r1:8b")
            .unwrap();
        assert!(
            r1.installed,
            "reviewed :latest alias should reconcile to R1 8B"
        );
    }

    #[test]
    fn mark_installed_does_not_match_across_param_sizes() {
        let mut reg = ModelRegistry::seed();
        reg.mark_installed(&["qwen3.5:4b".to_string()]);
        let installed: Vec<&str> = reg
            .all()
            .iter()
            .filter(|model| model.installed)
            .map(|model| model.ollama_tag.as_str())
            .collect();
        assert_eq!(
            installed,
            vec!["qwen3.5:4b"],
            "only the exact size pulled is installed"
        );
        assert!(!reg
            .all()
            .iter()
            .any(|model| model.ollama_tag == "qwen3.5:9b" && model.installed));
    }

    #[test]
    fn cloud_tags_never_mark_a_local_catalog_entry_installed() {
        let mut reg = ModelRegistry::seed();
        let cloud_only_installed = vec![
            "gemma4:31b-cloud".to_string(),
            "qwen3.5:397b-cloud".to_string(),
        ];
        reg.mark_installed(&cloud_only_installed);

        assert!(
            !reg.all().iter().any(|model| model.installed),
            "hosted Cloud tags must not become local catalog matches"
        );
        assert!(
            reg.recommend(128_000, &cloud_only_installed)
                .is_some_and(|model| !model.installed),
            "a Cloud-only installed list must not influence an offline recommendation"
        );
    }

    #[test]
    fn empty_self_hosted_reference_never_matches_an_ollama_install() {
        assert!(!tag_matches("", "anything"));
    }

    // The one free OFFLINE MiniMax path: a direct Hugging Face GGUF pull.
    // Official `minimax-*` Ollama tags are cloud-only, so this entry must be
    // pullable, offline-eligible, and honestly licensed (modified MIT).
    #[test]
    fn offline_minimax_ships_as_a_direct_hf_gguf_pull() {
        let reg = ModelRegistry::seed();
        let m25 = reg
            .all()
            .iter()
            .find(|m| m.family == "MiniMax M2.5")
            .expect("MiniMax M2.5 must be curated as a local entry");
        assert!(m25.ollama_tag.starts_with("hf.co/"));
        assert!(
            m25.can_pull_from_ollama(),
            "hf.co GGUFs are valid pull targets"
        );
        assert!(is_offline_ollama_tag(&m25.ollama_tag));
        assert_eq!(m25.tier, HardwareTier::HighEnd);
        assert_eq!(m25.license, License::ModifiedMit);
        assert!(m25.license.commercial_friendly());
        assert!(
            m25.caveat.to_lowercase().contains("cloud-only"),
            "the caveat must warn that official Ollama MiniMax tags are cloud-only"
        );
    }
}

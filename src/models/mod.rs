//! Curated, hardware-tiered registry of **free, open-weight** models pulled via
//! Ollama.
//!
//! ## Why this is data-driven, not a hardcoded list
//!
//! Open-weight leaderboards churn monthly and Ollama tags drift fast, so the
//! `seed()` list below is explicitly a **starting point that is reconciled at
//! runtime**, not gospel:
//!
//! - [`ModelRegistry::mark_installed`] reconciles against what Ollama actually
//!   has locally (`/api/tags`), so the catalog reflects the user's machine.
//! - [`verify_in_library`] does a best-effort live check that a tag still
//!   exists in the Ollama library, so stale names surface instead of 404-ing at
//!   pull time.
//! - [`ModelRegistry::fits`] + [`HardwareTier::for_vram`] filter to what the
//!   user's VRAM can actually run (driven by [`crate::monitoring::VramSentinel`]).
//!
//! ## Free vs. paid, local vs. cloud (honest)
//!
//! Everything here is **free** and runs **locally** through Ollama —
//! `OllamaProvider` stays the single local choke point. Paid cloud models
//! (OpenAI / Anthropic / Gemini) are deliberately **not** in this registry:
//! they are bring-your-own-key and billed per token, and are surfaced
//! separately (and opt-in) — never mixed into the "free" lineup.

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

/// Model license, surfaced so the commercial caveats are visible. We prefer
/// Apache-2.0 / MIT for a commercial product; others carry custom terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum License {
    Apache2,
    Mit,
    /// e.g. Kimi K2's modified MIT.
    ModifiedMit,
    /// Meta Llama community license (custom; has acceptable-use + scale clauses).
    Llama,
    /// Google Gemma terms (custom; permissive-ish but not OSI).
    Gemma,
    /// Mistral non-production / research license (NOT free for commercial use).
    MistralResearch,
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
            License::Other => "see model card",
        }
    }

    /// True for licenses that are unambiguously fine for a commercial product.
    /// The custom licenses (Llama/Gemma/Mistral-research) need the user to read
    /// the terms, so they return false and the UI flags them.
    pub fn commercial_friendly(self) -> bool {
        matches!(self, License::Apache2 | License::Mit | License::ModifiedMit)
    }
}

/// One curated open-weight model. `installed` and `library_verified` are
/// reconciled at runtime — the seed only carries the static facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedModel {
    /// Human family name, e.g. "Qwen3-Coder".
    pub family: String,
    /// The `ollama pull` target, e.g. "qwen2.5-coder:7b". Tags prefixed with
    /// `hf.co/` are direct Hugging Face GGUF pulls (Ollama supports these
    /// natively) — used for open-weight families whose official Ollama library
    /// entries are cloud-only (e.g. MiniMax).
    pub ollama_tag: String,
    pub params: String,
    pub tier: HardwareTier,
    /// Rough resident VRAM at Q4_K_M, in MB. A *lower bound for fit-checking*;
    /// KV cache for long context pushes it higher (see [`ModelRegistry::fits`]).
    pub approx_vram_mb: usize,
    pub license: License,
    /// What it's good at, terse: "coding", "agentic", "general", "reasoning".
    pub strengths: String,
    /// True for multimodal models that accept image input (screenshots,
    /// diagrams). Drives [`ModelRegistry::recommend_vision`] and the desktop
    /// companion's screen-context pipeline.
    #[serde(default)]
    pub vision: bool,
    /// True once reconciled against local Ollama `/api/tags`.
    #[serde(default)]
    pub installed: bool,
    /// `Some(true/false)` after a live library check; `None` if not checked.
    #[serde(default)]
    pub library_verified: Option<bool>,
}

impl CuratedModel {
    fn new(
        family: &str,
        ollama_tag: &str,
        params: &str,
        tier: HardwareTier,
        approx_vram_mb: usize,
        license: License,
        strengths: &str,
    ) -> Self {
        Self {
            family: family.to_string(),
            ollama_tag: ollama_tag.to_string(),
            params: params.to_string(),
            tier,
            approx_vram_mb,
            license,
            strengths: strengths.to_string(),
            vision: false,
            installed: false,
            library_verified: None,
        }
    }

    /// Marks this entry as accepting image input (multimodal).
    fn with_vision(mut self) -> Self {
        self.vision = true;
        self
    }
}

/// The curated set. Cheap to construct; safe to call often.
pub struct ModelRegistry {
    entries: Vec<CuratedModel>,
}

impl ModelRegistry {
    /// The **seed** lineup (July-2026 snapshot; every tag below verified live
    /// against `ollama.com/library` / Hugging Face at curation time). Treat
    /// names/tags as a starting point — `mark_installed` + `verify_in_library`
    /// keep it honest at runtime. Apache-2.0 / MIT are listed first within
    /// each tier (commercial-friendly).
    ///
    /// Notes on the 2026 families:
    /// - **Qwen 3.6** is the newest open-weight Qwen (3.7 is hosted-only);
    ///   `qwen3.6:27b` is the strongest open dense coder at time of curation.
    /// - **Gemma 4** (April 2026) switched to Apache-2.0 — unlike Gemma 3,
    ///   which stays under the custom Gemma terms. All Gemma 4 sizes are
    ///   multimodal (image input) with 128K–256K context.
    /// - **DeepSeek**: the R1 distills remain the practical local reasoning
    ///   ladder; V3.1 (671B, MIT) is the largest local DeepSeek on Ollama.
    ///   V3.2 / V4 exist only as `:cloud` tags, so they are NOT listed here.
    /// - **MiniMax M2.5**: official Ollama tags are cloud-only, but the open
    ///   weights are freely downloadable — we pull the GGUF straight from
    ///   Hugging Face (`hf.co/...`), which Ollama supports natively.
    pub fn seed() -> Self {
        use HardwareTier::*;
        use License::*;
        let entries = vec![
            // --- Modest (~8 GB): runs on a laptop / small GPU / Apple Silicon ---
            CuratedModel::new(
                "Qwen3.5",
                "qwen3.5:9b",
                "9B",
                Modest,
                5_800,
                Apache2,
                "general, coding",
            ),
            CuratedModel::new(
                "Qwen3.5",
                "qwen3.5:4b",
                "4B",
                Modest,
                3_000,
                Apache2,
                "general, compact",
            ),
            CuratedModel::new(
                "Qwen3-VL",
                "qwen3-vl:4b",
                "4B",
                Modest,
                3_500,
                Apache2,
                "vision, screen understanding",
            )
            .with_vision(),
            CuratedModel::new(
                "Qwen3-VL",
                "qwen3-vl:8b",
                "8B",
                Modest,
                6_000,
                Apache2,
                "vision, OCR, screen understanding",
            )
            .with_vision(),
            CuratedModel::new(
                "Gemma 4",
                "gemma4:e2b",
                "2.3B effective",
                Modest,
                5_000,
                Apache2,
                "general, vision, audio-in",
            )
            .with_vision(),
            CuratedModel::new(
                "Gemma 4",
                "gemma4:e4b",
                "4.5B effective",
                Modest,
                7_000,
                Apache2,
                "general, vision, audio-in",
            )
            .with_vision(),
            CuratedModel::new(
                "DeepSeek-R1",
                "deepseek-r1:8b",
                "8B (distill)",
                Modest,
                5_500,
                Mit,
                "reasoning",
            ),
            CuratedModel::new(
                "Qwen2.5-Coder",
                "qwen2.5-coder:7b",
                "7B",
                Modest,
                5_500,
                Apache2,
                "coding",
            ),
            CuratedModel::new(
                "Phi-4-mini",
                "phi4-mini",
                "3.8B",
                Modest,
                3_300,
                Mit,
                "reasoning, compact",
            ),
            CuratedModel::new(
                "Gemma 3",
                "gemma3:4b",
                "4B",
                Modest,
                4_000,
                Gemma,
                "general",
            )
            .with_vision(),
            // --- Single consumer GPU (~16–24 GB) ---
            CuratedModel::new(
                "Qwen3.6",
                "qwen3.6:27b",
                "27B",
                Single,
                17_000,
                Apache2,
                "coding, agentic (best open dense coder)",
            ),
            CuratedModel::new(
                "Qwen3.6",
                "qwen3.6:35b",
                "35B (MoE, 3B active)",
                Single,
                20_000,
                Apache2,
                "general, agentic",
            ),
            CuratedModel::new(
                "Gemma 4",
                "gemma4:12b",
                "12B",
                Single,
                8_000,
                Apache2,
                "general, vision, 256K context",
            )
            .with_vision(),
            CuratedModel::new(
                "Gemma 4",
                "gemma4:26b",
                "25.2B (MoE, 3.8B active)",
                Single,
                18_000,
                Apache2,
                "general, vision, reasoning",
            )
            .with_vision(),
            CuratedModel::new(
                "Gemma 4",
                "gemma4:31b",
                "30.7B",
                Single,
                20_000,
                Apache2,
                "general, vision, reasoning",
            )
            .with_vision(),
            CuratedModel::new(
                "Qwen3-VL",
                "qwen3-vl:30b",
                "30B (MoE, 3B active)",
                Single,
                16_000,
                Apache2,
                "vision, agentic screen use",
            )
            .with_vision(),
            CuratedModel::new(
                "DeepSeek-R1",
                "deepseek-r1:14b",
                "14B (distill)",
                Single,
                9_500,
                Mit,
                "reasoning",
            ),
            CuratedModel::new(
                "DeepSeek-R1",
                "deepseek-r1:32b",
                "32B (distill)",
                Single,
                20_000,
                Mit,
                "reasoning, coding",
            ),
            CuratedModel::new(
                "Qwen3-Coder",
                "qwen3-coder:30b",
                "30B (MoE)",
                Single,
                20_000,
                Apache2,
                "coding, agentic",
            ),
            CuratedModel::new(
                "Devstral Small",
                "devstral",
                "24B",
                Single,
                15_000,
                Apache2,
                "agentic coding",
            ),
            CuratedModel::new(
                "DeepSeek-Coder-V2",
                "deepseek-coder-v2:16b",
                "16B (MoE)",
                Single,
                10_500,
                Other,
                "coding",
            ),
            CuratedModel::new(
                "Gemma 3",
                "gemma3:27b",
                "27B",
                Single,
                17_000,
                Gemma,
                "general",
            )
            .with_vision(),
            CuratedModel::new(
                "Codestral",
                "codestral",
                "22B",
                Single,
                13_500,
                MistralResearch,
                "coding (non-commercial license)",
            ),
            // --- High-end / multi-GPU (large MoE / 64 GB+ unified memory) ---
            CuratedModel::new(
                "Qwen3-Coder-Next",
                "qwen3-coder-next",
                "80B (MoE, 3B active)",
                HighEnd,
                52_000,
                Apache2,
                "coding, agentic",
            ),
            CuratedModel::new(
                "DeepSeek-R1",
                "deepseek-r1:70b",
                "70B (distill)",
                HighEnd,
                43_000,
                Mit,
                "reasoning, coding",
            ),
            CuratedModel::new(
                "Qwen3.5",
                "qwen3.5:122b",
                "122B (MoE, 10B active)",
                HighEnd,
                75_000,
                Apache2,
                "frontier general",
            ),
            CuratedModel::new(
                "MiniMax-M2.5",
                "hf.co/unsloth/MiniMax-M2.5-GGUF:Q4_K_M",
                "230B (MoE, 10B active)",
                HighEnd,
                139_000,
                Other,
                "coding, agentic, tool use",
            ),
            CuratedModel::new(
                "DeepSeek-V3.1",
                "deepseek-v3.1:671b",
                "671B (MoE)",
                HighEnd,
                400_000,
                Mit,
                "frontier general, coding",
            ),
            CuratedModel::new(
                "DeepSeek-R1",
                "deepseek-r1:671b",
                "671B (MoE)",
                HighEnd,
                400_000,
                Mit,
                "reasoning, coding",
            ),
            CuratedModel::new(
                "Qwen3-VL",
                "qwen3-vl:235b",
                "235B (MoE, 22B active)",
                HighEnd,
                150_000,
                Apache2,
                "frontier vision",
            )
            .with_vision(),
            CuratedModel::new(
                "Qwen3",
                "qwen3:235b",
                "235B (MoE)",
                HighEnd,
                150_000,
                Apache2,
                "frontier general",
            ),
            CuratedModel::new(
                "GLM-4.6",
                "glm4:latest",
                "large",
                HighEnd,
                60_000,
                Mit,
                "coding, agentic",
            ),
            CuratedModel::new(
                "Llama 4 Scout",
                "llama4:scout",
                "109B (MoE)",
                HighEnd,
                70_000,
                Llama,
                "huge context",
            )
            .with_vision(),
            CuratedModel::new(
                "Mistral Large",
                "mistral-large",
                "123B",
                HighEnd,
                73_000,
                MistralResearch,
                "general (non-commercial license)",
            ),
        ];
        Self { entries }
    }

    pub fn all(&self) -> &[CuratedModel] {
        &self.entries
    }

    /// Models whose estimated resident size fits in `free_vram_mb`, biggest
    /// first. `0` (unknown VRAM / CPU) returns the Modest tier only — we'd
    /// rather under-promise than recommend a model the machine can't load.
    /// A 25% headroom factor accounts for KV cache + runtime overhead.
    pub fn fits(&self, free_vram_mb: usize) -> Vec<&CuratedModel> {
        let mut v: Vec<&CuratedModel> = if free_vram_mb == 0 {
            self.entries
                .iter()
                .filter(|m| m.tier == HardwareTier::Modest)
                .collect()
        } else {
            self.entries
                .iter()
                .filter(|m| (m.approx_vram_mb as f64 * 1.25) as usize <= free_vram_mb)
                .collect()
        };
        v.sort_by_key(|model| std::cmp::Reverse(model.approx_vram_mb));
        v
    }

    /// Reconcile with the user's installed Ollama models. Matches the seed tag
    /// EXACTLY or as a prefix up to a separator (`-` quant suffix, or `:` for
    /// tag-less seeds) — see [`tag_matches`]. Crucially this does NOT match
    /// across parameter sizes: installing `qwen3:4b` never marks `qwen3:14b`.
    pub fn mark_installed(&mut self, installed: &[String]) {
        for m in self.entries.iter_mut() {
            m.installed = installed.iter().any(|i| tag_matches(&m.ollama_tag, i));
        }
    }

    /// Pick a sensible default the machine can actually run. Preference order:
    /// 1. an **installed**, commercial-friendly **coding** model that fits;
    /// 2. an installed model that fits (any);
    /// 3. the largest commercial-friendly model that fits;
    /// 4. the largest model that fits;
    /// 5. the smallest curated model overall (last resort).
    pub fn recommend(&self, free_vram_mb: usize, installed: &[String]) -> Option<&CuratedModel> {
        let is_installed =
            |m: &CuratedModel| installed.iter().any(|i| tag_matches(&m.ollama_tag, i));
        let fits = self.fits(free_vram_mb); // biggest-first
        fits.iter()
            .find(|m| {
                is_installed(m) && m.license.commercial_friendly() && m.strengths.contains("coding")
            })
            .or_else(|| fits.iter().find(|m| is_installed(m)))
            .or_else(|| fits.iter().find(|m| m.license.commercial_friendly()))
            .or_else(|| fits.first())
            .copied()
            .or_else(|| {
                // Last resort ONLY when VRAM is unknown (CPU / 0): suggest the
                // smallest curated model. When VRAM is *known* but nothing fits,
                // return None — never recommend a model the machine can't load
                // (the catalog/CLI then says "nothing fits; smallest is X").
                if free_vram_mb == 0 {
                    self.entries.iter().min_by_key(|m| m.approx_vram_mb)
                } else {
                    None
                }
            })
    }

    /// Pick the best **vision-capable** model for screenshot/image turns
    /// (desktop companion, spatial context). Same honesty rules as
    /// [`recommend`]: prefer what's installed and fits, never suggest a model
    /// known not to fit. Preference order:
    /// 1. the largest **installed** vision model that fits;
    /// 2. the largest commercial-friendly vision model that fits;
    /// 3. the largest vision model that fits;
    /// 4. (unknown VRAM only) the smallest vision model overall.
    pub fn recommend_vision(
        &self,
        free_vram_mb: usize,
        installed: &[String],
    ) -> Option<&CuratedModel> {
        let is_installed =
            |m: &CuratedModel| installed.iter().any(|i| tag_matches(&m.ollama_tag, i));
        let fits: Vec<&CuratedModel> = self
            .fits(free_vram_mb) // biggest-first
            .into_iter()
            .filter(|m| m.vision)
            .collect();
        fits.iter()
            .find(|m| is_installed(m))
            .or_else(|| fits.iter().find(|m| m.license.commercial_friendly()))
            .or_else(|| fits.first())
            .copied()
            .or_else(|| {
                if free_vram_mb == 0 {
                    self.entries
                        .iter()
                        .filter(|m| m.vision)
                        .min_by_key(|m| m.approx_vram_mb)
                } else {
                    None
                }
            })
    }

    /// Pick the best **installed** model for a multi-agent team role, so
    /// heterogeneous orchestration can put each role on the model family that
    /// is actually good at that job (instead of one writer model everywhere):
    ///
    /// - `Planner` prefers reasoning-tilted families (DeepSeek-R1 distills,
    ///   Gemma 4 thinking, Qwen 3.6) — the plan is where chain-of-thought pays.
    /// - `Writer` prefers coding families (Qwen 3.6/Qwen-Coder, Devstral,
    ///   DeepSeek-Coder) — the diff is where code specialization pays.
    /// - `Scout` prefers the *smallest* fitting model — read-only
    ///   reconnaissance doesn't need the big gun and shouldn't hog VRAM the
    ///   writer will want.
    /// - `Vision` delegates to [`recommend_vision`].
    ///
    /// Returns `None` when nothing installed matches — callers keep their
    /// existing default (this is an upgrade path, never a downgrade).
    pub fn best_installed_for_role(
        &self,
        role: ModelRole,
        free_vram_mb: usize,
        installed: &[String],
    ) -> Option<String> {
        if role == ModelRole::Vision {
            return self
                .recommend_vision(free_vram_mb, installed)
                .filter(|m| m.installed_in(installed))
                .map(|m| m.ollama_tag.clone());
        }
        let fits: Vec<&CuratedModel> = self
            .fits(free_vram_mb) // biggest-first
            .into_iter()
            .filter(|m| m.installed_in(installed))
            .collect();
        if fits.is_empty() {
            return None;
        }
        let strength_pick = |needles: &[&str]| -> Option<&CuratedModel> {
            fits.iter()
                .find(|m| needles.iter().any(|n| m.strengths.contains(n)))
                .copied()
        };
        match role {
            ModelRole::Planner => strength_pick(&["reasoning"])
                .or_else(|| strength_pick(&["agentic", "general"]))
                .or_else(|| fits.first().copied())
                .map(|m| m.ollama_tag.clone()),
            ModelRole::Writer => strength_pick(&["coding"])
                .or_else(|| fits.first().copied())
                .map(|m| m.ollama_tag.clone()),
            // Smallest fitting installed model — `fits` is biggest-first.
            ModelRole::Scout => fits.last().map(|m| m.ollama_tag.clone()),
            ModelRole::Vision => unreachable!("handled above"),
        }
    }
}

/// A role in the multi-agent topology, used to bias model-family selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelRole {
    /// Read-only synthesis / planning — reasoning families shine here.
    Planner,
    /// The single controlled writer — coding families shine here.
    Writer,
    /// Read-only reconnaissance — smallest fitting model wins.
    Scout,
    /// Screenshot / image understanding — vision families only.
    Vision,
}

impl CuratedModel {
    /// True if this curated entry matches any installed Ollama tag.
    pub fn installed_in(&self, installed: &[String]) -> bool {
        installed.iter().any(|i| tag_matches(&self.ollama_tag, i))
    }
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
    installed == seed
        || installed.starts_with(&format!("{seed}-"))
        || installed.starts_with(&format!("{seed}:"))
}

/// Best-effort live check that an Ollama tag still resolves in the public
/// library. Returns `None` on any network/parse error (never panics, never
/// blocks the catalog) so a curated tag that *can't be verified* is shown as
/// "unverified" rather than wrongly dropped.
///
/// Two tag shapes are supported:
/// - Ollama library tags verify against the model page
///   (`https://ollama.com/library/<name>`), which 200s for a live model.
/// - `hf.co/<org>/<repo>[:quant]` direct-pull tags verify against the
///   Hugging Face repo page (`https://huggingface.co/<org>/<repo>`).
pub async fn verify_in_library(ollama_tag: &str) -> Option<bool> {
    let url = if let Some(hf_path) = ollama_tag.strip_prefix("hf.co/") {
        // `hf.co/org/repo:Q4_K_M` → strip the quant suffix after the LAST
        // colon (repo names themselves never contain a colon).
        let repo = hf_path.rsplit_once(':').map(|(r, _)| r).unwrap_or(hf_path);
        format!("https://huggingface.co/{repo}")
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
        installed.iter().any(|i| tag_matches(&self.ollama_tag, i))
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
    fn license_commercial_flags_are_honest() {
        assert!(License::Apache2.commercial_friendly());
        assert!(License::Mit.commercial_friendly());
        // Custom licenses must NOT be advertised as commercial-safe.
        assert!(!License::Gemma.commercial_friendly());
        assert!(!License::Llama.commercial_friendly());
        assert!(!License::MistralResearch.commercial_friendly());
    }

    #[test]
    fn fits_filters_to_what_actually_runs() {
        let reg = ModelRegistry::seed();
        // 8 GB machine: only small models, and nothing high-end.
        let small = reg.fits(8_000);
        assert!(!small.is_empty());
        assert!(small
            .iter()
            .all(|m| (m.approx_vram_mb as f64 * 1.25) as usize <= 8_000));
        assert!(small.iter().all(|m| m.tier != HardwareTier::HighEnd));
        // Biggest-first ordering.
        assert!(small
            .windows(2)
            .all(|w| w[0].approx_vram_mb >= w[1].approx_vram_mb));
    }

    #[test]
    fn fits_unknown_vram_returns_modest_only() {
        let reg = ModelRegistry::seed();
        let f = reg.fits(0);
        assert!(!f.is_empty());
        assert!(f.iter().all(|m| m.tier == HardwareTier::Modest));
    }

    #[test]
    fn recommend_prefers_installed_coding_model_that_fits() {
        let reg = ModelRegistry::seed();
        let installed = vec!["qwen2.5-coder:7b".to_string()];
        let rec = reg.recommend(12_000, &installed).unwrap();
        assert_eq!(rec.ollama_tag, "qwen2.5-coder:7b");
        assert!(rec.installed_matches(&installed));
    }

    #[test]
    fn recommend_returns_a_fitting_commercial_model_when_one_fits() {
        let reg = ModelRegistry::seed();
        let rec = reg.recommend(6_000, &[]).unwrap();
        assert!((rec.approx_vram_mb as f64 * 1.25) as usize <= 6_000);
        assert!(
            rec.license.commercial_friendly(),
            "default should be commercial-safe"
        );
    }

    #[test]
    fn recommend_returns_none_when_known_vram_fits_nothing() {
        let reg = ModelRegistry::seed();
        // 2 GB known free VRAM: even the smallest model's headroom doesn't fit.
        // Must be None (caller shows "nothing fits") — never a model that can't run.
        assert!(reg.recommend(2_000, &[]).is_none());
        // But unknown VRAM (0 = CPU) still suggests the smallest as best-effort.
        assert!(reg.recommend(0, &[]).is_some());
    }

    #[test]
    fn mark_installed_matches_quantized_suffix() {
        let mut reg = ModelRegistry::seed();
        reg.mark_installed(&["qwen2.5-coder:7b-instruct-q4_K_M".to_string()]);
        let m = reg
            .all()
            .iter()
            .find(|m| m.ollama_tag == "qwen2.5-coder:7b")
            .unwrap();
        assert!(
            m.installed,
            "should match the seed tag as a prefix despite quant suffix"
        );
    }

    #[test]
    fn mark_installed_does_not_match_across_param_sizes() {
        let mut reg = ModelRegistry::seed();
        reg.mark_installed(&["qwen3.5:4b".to_string()]);
        let installed: Vec<&str> = reg
            .all()
            .iter()
            .filter(|m| m.installed)
            .map(|m| m.ollama_tag.as_str())
            .collect();
        assert_eq!(
            installed,
            vec!["qwen3.5:4b"],
            "only the exact size pulled is installed"
        );
        // The other qwen3.5 sizes must NOT be marked installed.
        assert!(!reg
            .all()
            .iter()
            .any(|m| m.ollama_tag == "qwen3.5:9b" && m.installed));
        assert!(!reg
            .all()
            .iter()
            .any(|m| m.ollama_tag == "qwen3.5:122b" && m.installed));
    }

    // ------------------------------------------------------------------
    // July-2026 lineup: new families, vision picks, and role affinity
    // ------------------------------------------------------------------

    #[test]
    fn seed_includes_the_2026_families() {
        let reg = ModelRegistry::seed();
        let has_family = |needle: &str| reg.all().iter().any(|m| m.family.contains(needle));
        assert!(has_family("Gemma 4"), "Gemma 4 must be curated");
        assert!(has_family("DeepSeek-R1"), "DeepSeek R1 must be curated");
        assert!(has_family("DeepSeek-V3.1"), "DeepSeek V3.1 must be curated");
        assert!(has_family("MiniMax"), "MiniMax must be curated");
        assert!(has_family("Qwen3.6"), "Qwen 3.6 must be curated");
        // Gemma 4 switched to Apache-2.0 in April 2026 — it must NOT carry
        // the old custom Gemma terms (that would wrongly hide it from the
        // commercial-friendly default picker).
        assert!(reg
            .all()
            .iter()
            .filter(|m| m.family == "Gemma 4")
            .all(|m| m.license == License::Apache2));
        // MiniMax open weights ship via a direct Hugging Face GGUF pull.
        let minimax = reg
            .all()
            .iter()
            .find(|m| m.family.contains("MiniMax"))
            .unwrap();
        assert!(minimax.ollama_tag.starts_with("hf.co/"));
        assert_eq!(minimax.tier, HardwareTier::HighEnd);
    }

    #[test]
    fn recommend_vision_prefers_installed_vision_model_that_fits() {
        let reg = ModelRegistry::seed();
        let installed = vec!["qwen3-vl:4b".to_string(), "qwen3.5:9b".to_string()];
        let rec = reg.recommend_vision(8_000, &installed).unwrap();
        assert_eq!(
            rec.ollama_tag, "qwen3-vl:4b",
            "installed vision model must win over larger uninstalled ones"
        );
        assert!(rec.vision);
    }

    #[test]
    fn recommend_vision_never_returns_a_text_only_model() {
        let reg = ModelRegistry::seed();
        // No vision model installed: the pick must still be vision-capable.
        let rec = reg.recommend_vision(24_000, &[]).unwrap();
        assert!(rec.vision, "{} is not vision-capable", rec.ollama_tag);
        // Known-too-small VRAM: nothing fits, so no recommendation at all.
        assert!(reg.recommend_vision(2_000, &[]).is_none());
    }

    #[test]
    fn role_affinity_routes_planner_writer_scout_to_different_families() {
        let reg = ModelRegistry::seed();
        let installed = vec![
            "deepseek-r1:14b".to_string(),
            "qwen3.6:27b".to_string(),
            "qwen3.5:4b".to_string(),
        ];
        let free_vram = 24_000;
        let planner = reg
            .best_installed_for_role(ModelRole::Planner, free_vram, &installed)
            .unwrap();
        let writer = reg
            .best_installed_for_role(ModelRole::Writer, free_vram, &installed)
            .unwrap();
        let scout = reg
            .best_installed_for_role(ModelRole::Scout, free_vram, &installed)
            .unwrap();
        assert_eq!(planner, "deepseek-r1:14b", "planner should get reasoning");
        assert_eq!(writer, "qwen3.6:27b", "writer should get the coder");
        assert_eq!(scout, "qwen3.5:4b", "scout should get the smallest");
    }

    #[test]
    fn role_affinity_returns_none_when_nothing_installed_matches() {
        let reg = ModelRegistry::seed();
        assert!(reg
            .best_installed_for_role(ModelRole::Planner, 24_000, &[])
            .is_none());
        // Vision role with only text models installed: no forced downgrade.
        assert!(reg
            .best_installed_for_role(
                ModelRole::Vision,
                24_000,
                &["qwen3.6:27b".to_string()]
            )
            .is_none());
    }
}

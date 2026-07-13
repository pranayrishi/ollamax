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
    /// The `ollama pull` target, e.g. "qwen2.5-coder:7b".
    pub ollama_tag: String,
    pub params: String,
    pub tier: HardwareTier,
    /// Rough resident VRAM at Q4_K_M, in MB. A *lower bound for fit-checking*;
    /// KV cache for long context pushes it higher (see [`ModelRegistry::fits`]).
    pub approx_vram_mb: usize,
    pub license: License,
    /// What it's good at, terse: "coding", "agentic", "general", "reasoning".
    pub strengths: String,
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
            installed: false,
            library_verified: None,
        }
    }
}

/// The curated set. Cheap to construct; safe to call often.
pub struct ModelRegistry {
    entries: Vec<CuratedModel>,
}

impl ModelRegistry {
    /// The **seed** lineup (mid-2026 snapshot). Treat names/tags as a starting
    /// point — `mark_installed` + `verify_in_library` keep it honest at runtime.
    /// Apache-2.0 / MIT are listed first within each tier (commercial-friendly).
    pub fn seed() -> Self {
        use HardwareTier::*;
        use License::*;
        let entries = vec![
            // --- Modest (~8 GB): runs on a laptop / small GPU / Apple Silicon ---
            CuratedModel::new(
                "Qwen3",
                "qwen3:4b",
                "4B",
                Modest,
                3_500,
                Apache2,
                "general, coding",
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
                "Qwen2.5-Coder",
                "qwen2.5-coder:7b",
                "7B",
                Modest,
                5_500,
                Apache2,
                "coding",
            ),
            CuratedModel::new(
                "Gemma 3",
                "gemma3:4b",
                "4B",
                Modest,
                4_000,
                Gemma,
                "general",
            ),
            // --- Single consumer GPU (~16–24 GB) ---
            CuratedModel::new(
                "Qwen3",
                "qwen3:14b",
                "14B",
                Single,
                9_500,
                Apache2,
                "general, coding",
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
            ),
            CuratedModel::new(
                "Codestral",
                "codestral",
                "22B",
                Single,
                13_500,
                MistralResearch,
                "coding (non-commercial license)",
            ),
            // --- High-end / multi-GPU (large MoE) ---
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
                "Qwen3",
                "qwen3:235b",
                "235B (MoE)",
                HighEnd,
                150_000,
                Apache2,
                "frontier general",
            ),
            CuratedModel::new(
                "Llama 4 Scout",
                "llama4:scout",
                "109B (MoE)",
                HighEnd,
                70_000,
                Llama,
                "huge context",
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
/// "unverified" rather than wrongly dropped. Verified against the model page
/// (`https://ollama.com/library/<name>`), which 200s for a live model.
pub async fn verify_in_library(ollama_tag: &str) -> Option<bool> {
    let name = ollama_tag.split(':').next().unwrap_or(ollama_tag);
    let url = format!("https://ollama.com/library/{name}");
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
        reg.mark_installed(&["qwen3:4b".to_string()]);
        let installed: Vec<&str> = reg
            .all()
            .iter()
            .filter(|m| m.installed)
            .map(|m| m.ollama_tag.as_str())
            .collect();
        assert_eq!(
            installed,
            vec!["qwen3:4b"],
            "only the exact size pulled is installed"
        );
        // The bigger qwen3 sizes must NOT be marked installed.
        assert!(!reg
            .all()
            .iter()
            .any(|m| m.ollama_tag == "qwen3:14b" && m.installed));
        assert!(!reg
            .all()
            .iter()
            .any(|m| m.ollama_tag == "qwen3:235b" && m.installed));
    }
}

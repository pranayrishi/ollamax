//! Pure-function tests for the monitoring tier logic.
//!
//! These exist so that any future tweak to the model-tier ladder
//! (`suggest_model`/`calculate_optimal_context`/`calculate_gpu_layers`) shows
//! up as a diff in this file. The tier boundaries are user-visible defaults
//! — moving them silently would change which model gets pulled on someone's
//! 12 GB card.

use ollama_forge::monitoring::{calculate_gpu_layers, calculate_optimal_context, suggest_model};

#[test]
fn suggest_model_picks_smallest_for_cpu_only() {
    // 0 free VRAM = no GPU detected. Don't lie about a 7B being viable.
    assert_eq!(suggest_model(0), "qwen2.5-coder:1.5b");
    assert_eq!(suggest_model(2_999), "qwen2.5-coder:1.5b");
}

#[test]
fn suggest_model_tier_boundaries_are_inclusive_lower() {
    // Each tier's lower bound should jump to the next size.
    assert_eq!(suggest_model(3_000), "qwen2.5-coder:1.5b");
    assert_eq!(suggest_model(6_000), "qwen2.5-coder:3b");
    assert_eq!(suggest_model(10_000), "qwen2.5-coder:7b");
    assert_eq!(suggest_model(16_000), "qwen2.5-coder:14b");
    assert_eq!(suggest_model(24_000), "qwen2.5-coder:32b");
    assert_eq!(suggest_model(48_000), "llama3.3:70b");
}

#[test]
fn suggest_model_does_not_recommend_70b_on_3060() {
    // RTX 3060 12GB — the most common r/LocalLLaMA card. Must NOT route to a
    // 70B (would OOM hard). Must route to a 7B-class.
    assert_eq!(suggest_model(11_000), "qwen2.5-coder:7b");
}

#[test]
fn optimal_context_grows_monotonically_with_vram() {
    let samples = [
        0, 4_000, 8_000, 12_000, 16_000, 24_000, 32_000, 48_000, 80_000,
    ];
    let ctxs: Vec<usize> = samples
        .iter()
        .map(|v| calculate_optimal_context(*v))
        .collect();
    for w in ctxs.windows(2) {
        assert!(
            w[0] <= w[1],
            "context size regressed as VRAM increased: {ctxs:?}"
        );
    }
}

#[test]
fn gpu_layers_zero_at_zero_vram() {
    assert_eq!(calculate_gpu_layers(0), 0);
    assert_eq!(calculate_gpu_layers(2_999), 0);
}

#[test]
fn gpu_layers_all_layers_at_24gb_plus() {
    // -1 is the Ollama/llama.cpp magic value for "every layer on GPU".
    assert_eq!(calculate_gpu_layers(24_000), -1);
    assert_eq!(calculate_gpu_layers(80_000), -1);
}

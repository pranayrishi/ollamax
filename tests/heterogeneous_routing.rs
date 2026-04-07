//! Tests for the tiered routing decision in `TaskRouter::split_into_tiered_subtasks`.
//!
//! These pin the heterogeneous-parallel contract:
//!
//! - When two distinct models are installed, architecture work goes to the
//!   biggest, boilerplate (tests/UI) goes to the smallest, and balanced work
//!   uses the analyzer's pick.
//! - When only one model is installed, no overrides are set (we wouldn't
//!   know what to route to anyway).
//! - The total number of subtasks should be ≥ what `split_into_subtasks`
//!   produced — tiered routing may *insert* an Architecture task on top of
//!   the boilerplate split.

use ollama_forge::providers::ModelInfo;
use ollama_forge::router::{ModelConfig, TaskRouter};

fn fake_model(name: &str, size: u64) -> ModelInfo {
    ModelInfo {
        name: name.to_string(),
        size,
        size_human: format!("{} B", size),
        modified_at: String::new(),
        digest: String::new(),
    }
}

#[test]
fn single_model_yields_no_overrides() {
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![fake_model("only:7b", 7_000_000_000)];
    let subs = r.split_into_tiered_subtasks(
        "build a frontend with backend api and tests",
        &models,
        "only:7b",
    );
    assert!(
        subs.iter().all(|s| s.model_override.is_none()),
        "with one model installed, tiered routing should not override anything: {subs:#?}"
    );
}

#[test]
fn two_models_route_arch_to_largest_and_boilerplate_to_smallest() {
    let r = TaskRouter::new(ModelConfig::default());
    // small + large, deliberately different sizes
    let models = vec![
        fake_model("tiny:1b", 1_000_000_000),
        fake_model("big:32b", 32_000_000_000),
    ];
    let subs = r.split_into_tiered_subtasks(
        "build a frontend ui with backend api and tests",
        &models,
        "big:32b",
    );

    // The router should have inserted an Architecture subtask.
    let arch = subs
        .iter()
        .find(|s| s.name.to_lowercase().contains("arch"))
        .expect("expected an Architecture subtask to be inserted");
    assert_eq!(
        arch.model_override.as_deref(),
        Some("big:32b"),
        "architecture should run on the biggest installed model"
    );

    // Frontend should land on the small model.
    let frontend = subs
        .iter()
        .find(|s| s.name.to_lowercase().contains("frontend"))
        .expect("Frontend subtask missing");
    assert_eq!(
        frontend.model_override.as_deref(),
        Some("tiny:1b"),
        "frontend boilerplate should run on the smallest model"
    );

    // Tests subtask should also land on the small model.
    let tests = subs
        .iter()
        .find(|s| s.name.to_lowercase().contains("test"))
        .expect("Tests subtask missing");
    assert_eq!(
        tests.model_override.as_deref(),
        Some("tiny:1b"),
        "test boilerplate should run on the smallest model"
    );

    // Backend (balanced work) should fall back to the analyzer's choice.
    let backend = subs
        .iter()
        .find(|s| s.name.to_lowercase().contains("backend"))
        .expect("Backend subtask missing");
    assert_eq!(
        backend.model_override.as_deref(),
        Some("big:32b"),
        "balanced work should fall back to the default tier model"
    );
}

#[test]
fn architecture_task_not_duplicated_when_user_already_asked_for_it() {
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![
        fake_model("tiny:1b", 1_000_000_000),
        fake_model("big:32b", 32_000_000_000),
    ];
    // Task already mentions "design", but the splitter doesn't pick that
    // up as an "Architecture" subtask name — it will create
    // Frontend/Backend/Tests and we'll insert exactly one Architecture
    // wrapper.
    let subs = r.split_into_tiered_subtasks(
        "design a frontend with backend and tests",
        &models,
        "big:32b",
    );
    let arch_count = subs
        .iter()
        .filter(|s| s.name.to_lowercase().contains("arch"))
        .count();
    assert_eq!(arch_count, 1, "exactly one Architecture subtask expected");
}

#[test]
fn unique_models_in_subtasks_can_load_in_parallel() {
    // Sanity for the heterogeneous-preload contract: the executor will
    // gather distinct model_overrides into a set and preload concurrently.
    // We just verify that with two installed models we end up with at
    // least 2 distinct overrides in the subtask list.
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![
        fake_model("tiny:1b", 1_000_000_000),
        fake_model("big:32b", 32_000_000_000),
    ];
    let subs = r.split_into_tiered_subtasks(
        "build a frontend ui with backend and tests",
        &models,
        "big:32b",
    );
    let distinct: std::collections::BTreeSet<&str> = subs
        .iter()
        .filter_map(|s| s.model_override.as_deref())
        .collect();
    assert!(
        distinct.len() >= 2,
        "expected at least 2 distinct model overrides in the heterogeneous split, got {distinct:?}"
    );
}

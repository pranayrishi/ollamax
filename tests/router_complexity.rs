//! Tests for the task-router complexity classifier.
//!
//! These pin the user-visible behavior: a one-line "rename files" task
//! should land in the `Simple` tier (small model, fast), and a multi-clause
//! "design distributed microservices" task should land in `Complex` or
//! higher (which is what triggers parallel execution downstream).

use ollama_forge::providers::ModelInfo;
use ollama_forge::router::{ModelConfig, TaskRouter, TaskType};

fn fake_model(name: &str) -> ModelInfo {
    ModelInfo {
        name: name.to_string(),
        size: 0,
        size_human: "0 B".to_string(),
        modified_at: String::new(),
        digest: String::new(),
    }
}

#[tokio::test]
async fn one_line_rename_is_simple() {
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![fake_model("llama3.2:3b")];
    let c = r
        .analyze_complexity("rename all .txt files to .md", &models)
        .await
        .unwrap();
    assert_eq!(c.task_type, TaskType::Simple);
    assert!(c.score < 0.3, "score={}", c.score);
}

#[tokio::test]
async fn distributed_microservices_is_at_least_complex() {
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![fake_model("llama3.3:70b")];
    let c = r
        .analyze_complexity(
            "design a distributed microservices architecture with auth, \
             database, and a parallel job queue, including security review",
            &models,
        )
        .await
        .unwrap();
    assert!(
        matches!(c.task_type, TaskType::Complex | TaskType::Architect),
        "expected Complex/Architect, got {:?} (score={})",
        c.task_type,
        c.score
    );
}

#[tokio::test]
async fn can_parallelize_only_above_medium_threshold() {
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![fake_model("qwen2.5-coder:7b")];

    let trivial = r.analyze_complexity("print hello", &models).await.unwrap();
    assert!(!r.can_parallelize(&trivial));

    let big = r
        .analyze_complexity(
            "build a full-stack app with frontend ui, backend api, database, \
             auth, security review, and tests",
            &models,
        )
        .await
        .unwrap();
    assert!(r.can_parallelize(&big));
}

#[tokio::test]
async fn split_into_subtasks_for_full_stack() {
    let r = TaskRouter::new(ModelConfig::default());
    let subs = r.split_into_subtasks("build a frontend ui with backend api and tests");
    let names: Vec<&str> = subs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Frontend/UI"), "names={names:?}");
    assert!(names.contains(&"Backend/Logic"), "names={names:?}");
    assert!(names.contains(&"Tests"), "names={names:?}");
}

#[tokio::test]
async fn split_into_subtasks_falls_back_to_implementation() {
    let r = TaskRouter::new(ModelConfig::default());
    let subs = r.split_into_subtasks("explain this regex");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].name, "Implementation");
}

#[tokio::test]
async fn route_to_model_never_returns_a_model_the_user_does_not_have() {
    // The router currently has tier-specific patterns that can fall through
    // to a hardcoded default model that may not be installed. The minimum
    // contract we want to defend: `route_to_model` should *never* hand back
    // a model name that isn't in `available_models` when at least one model
    // exists. (Without this, downstream code calls Ollama with a model tag
    // it doesn't have, and the user sees a confusing 404.)
    let r = TaskRouter::new(ModelConfig::default());
    let models = vec![fake_model("deepseek-coder-v2:16b")];
    let c = r
        .analyze_complexity(
            "refactor this distributed concurrent algorithm for performance and security",
            &models,
        )
        .await
        .unwrap();
    let routed = r.route_to_model(&c, &models);
    assert!(
        models.iter().any(|m| m.name == routed),
        "router returned `{routed}` which is not in the available list — \
         downstream Ollama call will 404. \
         Fix: route_to_model() must fall back through *available* tiers, \
         not through hardcoded defaults."
    );
}

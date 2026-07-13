use crate::models::is_offline_ollama_tag;
use crate::providers::ModelInfo;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityScore {
    pub score: f32,
    pub reasoning: String,
    pub suggested_model: String,
    pub task_type: TaskType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    Simple,
    Medium,
    Complex,
    Architect,
}

impl ComplexityScore {
    pub fn new(
        score: f32,
        reasoning: String,
        suggested_model: String,
        task_type: TaskType,
    ) -> Self {
        Self {
            score,
            reasoning,
            suggested_model,
            task_type,
        }
    }
}

pub struct TaskRouter {
    model_config: ModelConfig,
    complexity_thresholds: ComplexityThresholds,
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub small_model: String,
    pub medium_model: String,
    pub large_model: String,
    pub planner_model: String,
    pub code_models: Vec<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        // Aligned with Config and monitoring's current local line-up. These are
        // only fallbacks: route_to_model always returns an installed model when
        // one is available, avoiding a surprise pull/404 at execution time.
        Self {
            small_model: "gemma4:e2b".to_string(),
            medium_model: "qwen3.5:4b".to_string(),
            large_model: "gemma4:12b".to_string(),
            planner_model: "deepseek-r1:8b".to_string(),
            code_models: vec![
                "gemma4:e2b".to_string(),
                "qwen3.5:4b".to_string(),
                "deepseek-r1:8b".to_string(),
                "gemma4:12b".to_string(),
                "qwen3.6:27b".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComplexityThresholds {
    pub simple_threshold: f32,
    pub medium_threshold: f32,
    pub complex_threshold: f32,
}

impl Default for ComplexityThresholds {
    fn default() -> Self {
        Self {
            simple_threshold: 0.3,
            medium_threshold: 0.6,
            complex_threshold: 0.8,
        }
    }
}

impl TaskRouter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            model_config: config,
            complexity_thresholds: ComplexityThresholds::default(),
        }
    }

    pub async fn analyze_complexity(
        &self,
        task: &str,
        available_models: &[ModelInfo],
    ) -> Result<ComplexityScore> {
        let task_lower = task.to_lowercase();

        let mut score_factors = Vec::new();

        let file_indicators = ["file", "read", "write", "rename", "copy", "delete"];
        if file_indicators.iter().any(|i| task_lower.contains(i)) {
            score_factors.push(0.1);
        }

        let regex_indicators = ["regex", "pattern", "match", "validate"];
        if regex_indicators.iter().any(|i| task_lower.contains(i)) && task_lower.len() < 100 {
            score_factors.push(0.15);
        }

        let lint_indicators = ["lint", "format", "style", "prettier", "eslint"];
        if lint_indicators.iter().any(|i| task_lower.contains(i)) {
            score_factors.push(0.2);
        }

        let medium_indicators = [
            "api",
            "endpoint",
            "function",
            "class",
            "module",
            "component",
            "route",
            "query",
            "database",
            "auth",
        ];
        let medium_count = medium_indicators
            .iter()
            .filter(|i| task_lower.contains(*i))
            .count();
        if medium_count > 0 {
            score_factors.push(0.3 + (medium_count as f32 * 0.1).min(0.3));
        }

        let complex_indicators = [
            "architecture",
            "system",
            "distributed",
            "microservice",
            "optimize",
            "refactor",
            "algorithm",
            "concurrent",
            "parallel",
            "security",
            "performance",
            "scale",
        ];
        let complex_count = complex_indicators
            .iter()
            .filter(|i| task_lower.contains(*i))
            .count();
        if complex_count > 0 {
            score_factors.push(0.5 + (complex_count as f32 * 0.1).min(0.3));
        }

        let build_indicators = ["build", "create", "implement", "design", "architect"];
        if build_indicators.iter().any(|i| task_lower.contains(i)) {
            score_factors.push(0.2);
        }

        if task_lower.contains("full-stack")
            || task_lower.contains("complete")
            || task_lower.contains("app")
        {
            score_factors.push(0.3);
        }

        let base_score = if score_factors.is_empty() {
            0.2
        } else {
            score_factors.iter().sum::<f32>() / score_factors.len().max(1) as f32
        };

        let length_factor = (task.len() as f32 / 500.0).min(0.3);
        let final_score = (base_score + length_factor).min(1.0);

        let task_type = if final_score < self.complexity_thresholds.simple_threshold {
            TaskType::Simple
        } else if final_score < self.complexity_thresholds.medium_threshold {
            TaskType::Medium
        } else if final_score < self.complexity_thresholds.complex_threshold {
            TaskType::Complex
        } else {
            TaskType::Architect
        };

        let suggested_model = self.select_model_for_task(&task_type, available_models);

        let reasoning = format!(
            "Analyzed task with {} scoring factors: {:?}. Length contribution: {:.2}. Final: {:.2}",
            score_factors.len(),
            score_factors,
            length_factor,
            final_score
        );

        debug!("{}", reasoning);

        Ok(ComplexityScore::new(
            final_score,
            reasoning,
            suggested_model,
            task_type,
        ))
    }

    fn select_model_for_task(
        &self,
        task_type: &TaskType,
        available_models: &[ModelInfo],
    ) -> String {
        let available: Vec<&str> = available_models.iter().map(|m| m.name.as_str()).collect();

        match task_type {
            TaskType::Simple => available
                .iter()
                .find(|m| m.contains("3b") || m.contains("1b"))
                .copied()
                .unwrap_or(&self.model_config.small_model)
                .to_string(),
            TaskType::Medium => available
                .iter()
                .find(|m| m.contains("7b") || m.contains("qwen"))
                .copied()
                .unwrap_or(&self.model_config.medium_model)
                .to_string(),
            TaskType::Complex => available
                .iter()
                .find(|m| m.contains("16b") || m.contains("coder"))
                .copied()
                .unwrap_or(&self.model_config.large_model)
                .to_string(),
            TaskType::Architect => available
                .iter()
                .find(|m| m.contains("70b") || m.contains("671b") || m.contains("llama3.3"))
                .copied()
                .unwrap_or(&self.model_config.planner_model)
                .to_string(),
        }
    }

    pub fn route_to_model(
        &self,
        complexity: &ComplexityScore,
        available_models: &[ModelInfo],
    ) -> String {
        let eligible: Vec<&ModelInfo> = available_models
            .iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect();
        if eligible.is_empty() {
            return complexity.suggested_model.clone();
        }
        // Honor the analyzer's pick if the user actually has that model.
        if is_offline_ollama_tag(&complexity.suggested_model)
            && eligible
                .iter()
                .any(|m| m.name == complexity.suggested_model)
        {
            return complexity.suggested_model.clone();
        }
        // Otherwise: walk *available* models in tier order. Bigger first when
        // the task is hard, smaller first when it's easy. The previous
        // implementation fell back to a hardcoded default that the user
        // might not have installed, which produced a misleading 404 from
        // Ollama at call time.
        let by_size_desc: Vec<&str> = {
            let mut v = eligible;
            v.sort_by_key(|model| std::cmp::Reverse(model.size));
            v.into_iter().map(|m| m.name.as_str()).collect()
        };
        let pick = match complexity.task_type {
            TaskType::Architect | TaskType::Complex => by_size_desc.first(),
            TaskType::Medium | TaskType::Simple => by_size_desc.last(),
        };
        pick.copied().unwrap_or_default().to_string()
    }

    pub fn can_parallelize(&self, complexity: &ComplexityScore) -> bool {
        complexity.score >= self.complexity_thresholds.medium_threshold
    }

    pub fn split_into_subtasks(&self, task: &str) -> Vec<SubTask> {
        let task_lower = task.to_lowercase();
        let mut subtasks = Vec::new();

        let needs_frontend = task_lower.contains("frontend")
            || task_lower.contains("ui")
            || task_lower.contains("react")
            || task_lower.contains("vue")
            || task_lower.contains("css")
            || task_lower.contains("component");

        let needs_backend = task_lower.contains("backend")
            || task_lower.contains("api")
            || task_lower.contains("server")
            || task_lower.contains("database")
            || task_lower.contains("auth");

        let needs_tests = task_lower.contains("test")
            || task_lower.contains("spec")
            || task_lower.contains("tdd")
            || task_lower.contains("build");

        if needs_frontend {
            let mut s = SubTask::parallel(
                "Frontend/UI",
                "Build the user interface and frontend components",
            );
            s.skill_tags = vec!["frontend".to_string(), "ui".to_string()];
            subtasks.push(s);
        }

        if needs_backend {
            let mut s = SubTask::parallel(
                "Backend/Logic",
                "Build the backend logic, API endpoints, and data models",
            );
            s.skill_tags = vec!["backend".to_string(), "api".to_string()];
            subtasks.push(s);
        }

        if needs_tests {
            let mut s = SubTask::parallel("Tests", "Write comprehensive tests for all components");
            s.skill_tags = vec!["testing".to_string(), "tdd".to_string()];
            subtasks.push(s);
        }

        if subtasks.is_empty() {
            let mut s = SubTask::parallel("Implementation", task);
            s.parallel = false;
            subtasks.push(s);
        }

        subtasks
    }

    /// VRAM-aware version of `split_into_tiered_subtasks`. Caller passes the
    /// free VRAM in MB; we only assign two distinct models if their combined
    /// size fits. Otherwise we fall back to a uniform model assignment so
    /// the second model load doesn't OOM Ollama.
    ///
    /// Rule of thumb: a 7B Q4 needs ~5 GB resident, a 14B needs ~9 GB,
    /// a 32B needs ~20 GB. Combined headroom matters because Ollama
    /// loads each model independently into the same VRAM budget.
    pub fn split_into_tiered_subtasks_vram_aware(
        &self,
        task: &str,
        available_models: &[ModelInfo],
        default_tier_model: &str,
        free_vram_mb: usize,
    ) -> Vec<SubTask> {
        let eligible: Vec<&ModelInfo> = available_models
            .iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect();
        let mut subs =
            self.split_into_tiered_subtasks_from_eligible(task, &eligible, default_tier_model);

        // Distinct models requested, with their advertised disk sizes.
        let distinct_overrides: std::collections::BTreeSet<String> = subs
            .iter()
            .filter_map(|s| s.model_override.clone())
            .collect();
        if distinct_overrides.len() < 2 {
            return subs;
        }

        // Disk size is a *lower bound* on resident size — Q4 models are
        // about 80-90% of disk in RAM. We add a 30% safety margin to
        // account for KV cache (the actual memory hog) and runtime overhead.
        let total_resident_mb: usize = distinct_overrides
            .iter()
            .filter_map(|name| eligible.iter().find(|m| m.name == *name))
            .map(|m| ((m.size as f64 * 1.3) / (1024.0 * 1024.0)) as usize)
            .sum();

        if total_resident_mb <= free_vram_mb || free_vram_mb == 0 {
            return subs;
        }

        // Doesn't fit. Pick the largest single model that DOES fit and
        // collapse every override to it.
        let single = eligible
            .iter()
            .filter(|m| {
                let resident = ((m.size as f64 * 1.3) / (1024.0 * 1024.0)) as usize;
                resident <= free_vram_mb
            })
            .max_by_key(|m| m.size)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| default_tier_model.to_string());

        for s in subs.iter_mut() {
            s.model_override = Some(single.clone());
        }
        subs
    }

    /// Like `split_into_subtasks`, but each subtask is also tagged with a
    /// **complexity tier** and a model override is selected from the
    /// available models.
    ///
    /// **This is the heterogeneous-parallel routing decision.** It says:
    /// architecture/planning work goes to the biggest model installed,
    /// boilerplate goes to the smallest, balanced work goes to whatever
    /// the analyzer originally picked.
    ///
    /// Example: a "build a chat app with auth, frontend, backend, tests"
    /// task gets split as:
    /// - `Architecture` → biggest model (designs the schema, picks libs)
    /// - `Frontend/UI`  → smallest model (boilerplate JSX)
    /// - `Backend/Logic`→ original model (medium tier)
    /// - `Tests`        → smallest model (boilerplate test stubs)
    ///
    /// All four run *concurrently* in `ParallelExecutor::execute_parallel`,
    /// each on its own model. The big model loads in VRAM in parallel with
    /// the small one (Ollama serializes per-model, not across models).
    pub fn split_into_tiered_subtasks(
        &self,
        task: &str,
        available_models: &[ModelInfo],
        default_tier_model: &str,
    ) -> Vec<SubTask> {
        let eligible: Vec<&ModelInfo> = available_models
            .iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect();
        self.split_into_tiered_subtasks_from_eligible(task, &eligible, default_tier_model)
    }

    fn split_into_tiered_subtasks_from_eligible(
        &self,
        task: &str,
        eligible: &[&ModelInfo],
        default_tier_model: &str,
    ) -> Vec<SubTask> {
        let mut subtasks = self.split_into_subtasks(task);

        // Sort installed models by size to derive `small`/`medium`/`large` slots.
        let mut by_size: Vec<&ModelInfo> = eligible.to_vec();
        by_size.sort_by_key(|m| m.size);
        let small = by_size.first().map(|m| m.name.clone());
        let large = by_size.last().map(|m| m.name.clone());

        // If we don't have at least two distinct models installed,
        // heterogeneous routing is meaningless — leave overrides empty.
        if small == large {
            return subtasks;
        }

        // Bonus: insert an explicit Architecture subtask at position 0 if
        // none of the existing subtasks already covers planning. This is
        // what gives the big model something to do that's *different* from
        // the boilerplate work the small model is doing in parallel.
        let has_arch = subtasks
            .iter()
            .any(|s| s.name.to_lowercase().contains("arch"));
        if !has_arch && subtasks.len() >= 2 {
            let mut arch = SubTask::parallel(
                "Architecture",
                format!(
                    "Design the high-level architecture for: {task}. \
                     Pick libraries, sketch the data model, identify the \
                     critical edges, and list the assumptions the workers \
                     should make."
                ),
            );
            arch.skill_tags = vec!["architecture".to_string(), "planning".to_string()];
            subtasks.insert(0, arch);
        }

        for s in subtasks.iter_mut() {
            let n = s.name.to_lowercase();
            if n.contains("arch") || n.contains("plan") || n.contains("design") {
                s.model_override = large.clone();
            } else if n.contains("test")
                || n.contains("frontend")
                || n.contains("ui")
                || n.contains("boilerplate")
            {
                s.model_override = small.clone();
            } else {
                s.model_override = Some(default_tier_model.to_string());
            }
        }

        subtasks
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: String,
    pub name: String,
    pub description: String,
    pub skill_tags: Vec<String>,
    pub parallel: bool,
    /// Override the default model for this specific subtask. `None` means
    /// "use the executor's default model." This is what enables
    /// heterogeneous parallel execution: an architecture subtask can run
    /// on a 32B at the same time a frontend boilerplate subtask runs on a
    /// 3B, on the same physical machine, on different model loads in
    /// Ollama.
    #[serde(default)]
    pub model_override: Option<String>,
    /// Override `num_ctx` for this subtask. Smaller models often have
    /// lower native context limits, so when we route a subtask to a 3B
    /// while keeping the planner on a 14B, the 3B's context shouldn't be
    /// the planner's 32k.
    #[serde(default)]
    pub num_ctx_override: Option<usize>,
}

impl SubTask {
    /// Constructor for the common (non-tiered) case. Defaults
    /// `parallel=true`, no model override.
    pub fn parallel(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            skill_tags: Vec::new(),
            parallel: true,
            model_override: None,
            num_ctx_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_task_routing() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![ModelInfo {
            name: "llama3.2:3b".to_string(),
            size: 2_000_000_000,
            size_human: "2.0 GB".to_string(),
            modified_at: "2024-01-01".to_string(),
            digest: "abc123".to_string(),
        }];

        let complexity = router
            .analyze_complexity("rename all .txt files to .md", &models)
            .await
            .unwrap();
        assert!(complexity.score < 0.3);
        assert_eq!(complexity.task_type, TaskType::Simple);
    }

    #[tokio::test]
    async fn test_complex_task_routing() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![ModelInfo {
            name: "llama3.3:70b".to_string(),
            size: 40_000_000_000,
            size_human: "40 GB".to_string(),
            modified_at: "2024-01-01".to_string(),
            digest: "xyz789".to_string(),
        }];

        let complexity = router
            .analyze_complexity(
                "Design a distributed microservices architecture with API gateway",
                &models,
            )
            .await
            .unwrap();
        assert!(complexity.score >= 0.5);
    }

    fn mi(name: &str, size: u64) -> ModelInfo {
        ModelInfo {
            name: name.into(),
            size,
            size_human: String::new(),
            modified_at: String::new(),
            digest: String::new(),
        }
    }

    // Feature 3: heterogeneous routing — architecture/planning goes to the
    // biggest installed model, boilerplate (frontend/tests) to the smallest.
    #[test]
    fn tiered_routing_assigns_big_to_architecture_small_to_boilerplate() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![
            mi("qwen2.5-coder:1.5b", 1_000_000_000),
            mi("qwen2.5-coder:32b", 20_000_000_000),
        ];
        let subs = router.split_into_tiered_subtasks(
            "build a full-stack app with frontend, backend, auth, and tests",
            &models,
            "qwen2.5-coder:7b",
        );
        let arch = subs
            .iter()
            .find(|s| s.name.to_lowercase().contains("arch"))
            .expect("an architecture subtask should be inserted");
        assert_eq!(arch.model_override.as_deref(), Some("qwen2.5-coder:32b"));
        for s in subs.iter().filter(|s| {
            let n = s.name.to_lowercase();
            n.contains("front") || n.contains("test") || n.contains("ui")
        }) {
            assert_eq!(
                s.model_override.as_deref(),
                Some("qwen2.5-coder:1.5b"),
                "boilerplate `{}` should route to the smallest model",
                s.name
            );
        }
    }

    // Feature 3: VRAM safety — if two distinct models wouldn't fit together,
    // every subtask collapses onto a single model that does fit.
    #[test]
    fn tiered_routing_collapses_when_vram_too_small() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![
            mi("small:1.5b", 1_000_000_000),
            mi("big:32b", 20_000_000_000),
        ];
        let subs = router.split_into_tiered_subtasks_vram_aware(
            "build a frontend and backend with tests",
            &models,
            "small:1.5b",
            2_000, // ~2 GB free — can't hold both models
        );
        let overrides: std::collections::BTreeSet<String> = subs
            .iter()
            .filter_map(|s| s.model_override.clone())
            .collect();
        assert!(
            overrides.len() <= 1,
            "VRAM-too-small must collapse to one model, got {overrides:?}"
        );
    }

    // With a single installed model, heterogeneous routing is meaningless and
    // we must not invent overrides the executor would choke on.
    #[test]
    fn single_model_lineup_skips_heterogeneous_overrides() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![mi("only:7b", 5_000_000_000)];
        let subs =
            router.split_into_tiered_subtasks("build a frontend and backend", &models, "only:7b");
        assert!(subs.iter().all(|s| s.model_override.is_none()));
    }

    #[test]
    fn cloud_tags_are_never_selected_when_a_local_model_is_available() {
        let router = TaskRouter::new(ModelConfig::default());
        let models = vec![
            mi("qwen3.5:397b-cloud", 250_000_000_000),
            mi("qwen3.5:4b", 3_000_000_000),
        ];
        let complexity = ComplexityScore::new(
            0.95,
            "test".to_string(),
            "qwen3.5:397b-cloud".to_string(),
            TaskType::Architect,
        );
        assert_eq!(router.route_to_model(&complexity, &models), "qwen3.5:4b");

        let subtasks = router.split_into_tiered_subtasks(
            "build a frontend and backend",
            &models,
            "qwen3.5:4b",
        );
        assert!(subtasks
            .iter()
            .all(|subtask| subtask.model_override.as_deref() != Some("qwen3.5:397b-cloud")));
    }
}

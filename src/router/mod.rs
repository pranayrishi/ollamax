use crate::providers::{LlmProvider, ModelInfo};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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
    pub fn new(score: f32, reasoning: String, suggested_model: String, task_type: TaskType) -> Self {
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
        Self {
            small_model: "llama3.2:3b".to_string(),
            medium_model: "qwen2.5-coder:7b".to_string(),
            large_model: "deepseek-coder-v2:16b".to_string(),
            planner_model: "qwen2.5-coder:7b".to_string(),
            code_models: vec![
                "llama3.2:3b".to_string(),
                "qwen2.5-coder:7b".to_string(),
                "deepseek-coder-v2:16b".to_string(),
                "llama3.3:70b".to_string(),
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

    pub async fn analyze_complexity(&self, task: &str, available_models: &[ModelInfo]) -> Result<ComplexityScore> {
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
            "api", "endpoint", "function", "class", "module", 
            "component", "route", "query", "database", "auth"
        ];
        let medium_count = medium_indicators
            .iter()
            .filter(|i| task_lower.contains(*i))
            .count();
        if medium_count > 0 {
            score_factors.push(0.3 + (medium_count as f32 * 0.1).min(0.3));
        }

        let complex_indicators = [
            "architecture", "system", "distributed", "microservice",
            "optimize", "refactor", "algorithm", "concurrent", "parallel",
            "security", "performance", "scale"
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

        if task_lower.contains("full-stack") || task_lower.contains("complete") || task_lower.contains("app") {
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

    fn select_model_for_task(&self, task_type: &TaskType, available_models: &[ModelInfo]) -> String {
        let available: Vec<&str> = available_models.iter().map(|m| m.name.as_str()).collect();
        
        match task_type {
            TaskType::Simple => {
                available.iter()
                    .find(|m| m.contains("3b") || m.contains("1b"))
                    .copied()
                    .unwrap_or(&self.model_config.small_model)
                    .to_string()
            }
            TaskType::Medium => {
                available.iter()
                    .find(|m| m.contains("7b") || m.contains("qwen"))
                    .copied()
                    .unwrap_or(&self.model_config.medium_model)
                    .to_string()
            }
            TaskType::Complex => {
                available.iter()
                    .find(|m| m.contains("16b") || m.contains("coder"))
                    .copied()
                    .unwrap_or(&self.model_config.large_model)
                    .to_string()
            }
            TaskType::Architect => {
                available.iter()
                    .find(|m| m.contains("70b") || m.contains("671b") || m.contains("llama3.3"))
                    .copied()
                    .unwrap_or(&self.model_config.planner_model)
                    .to_string()
            }
        }
    }

    pub fn route_to_model(&self, complexity: &ComplexityScore, available_models: &[ModelInfo]) -> String {
        if complexity.suggested_model.is_empty() {
            return self.select_model_for_task(&complexity.task_type, available_models);
        }
        
        if available_models.iter().any(|m| m.name == complexity.suggested_model) {
            complexity.suggested_model.clone()
        } else {
            self.select_model_for_task(&complexity.task_type, available_models)
        }
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
            subtasks.push(SubTask {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Frontend/UI".to_string(),
                description: "Build the user interface and frontend components".to_string(),
                skill_tags: vec!["frontend".to_string(), "ui".to_string()],
                parallel: true,
            });
        }

        if needs_backend {
            subtasks.push(SubTask {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Backend/Logic".to_string(),
                description: "Build the backend logic, API endpoints, and data models".to_string(),
                skill_tags: vec!["backend".to_string(), "api".to_string()],
                parallel: true,
            });
        }

        if needs_tests {
            subtasks.push(SubTask {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Tests".to_string(),
                description: "Write comprehensive tests for all components".to_string(),
                skill_tags: vec!["testing".to_string(), "tdd".to_string()],
                parallel: true,
            });
        }

        if subtasks.is_empty() {
            subtasks.push(SubTask {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Implementation".to_string(),
                description: task.to_string(),
                skill_tags: vec![],
                parallel: false,
            });
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
        
        let complexity = router.analyze_complexity("rename all .txt files to .md", &models).await.unwrap();
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
        
        let complexity = router.analyze_complexity(
            "Design a distributed microservices architecture with API gateway",
            &models
        ).await.unwrap();
        assert!(complexity.score >= 0.5);
    }
}

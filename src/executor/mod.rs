use crate::providers::{GenerateOptions, LlmProvider, LlmResponse};
use crate::router::{SubTask, TaskRouter};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

pub struct ParallelExecutor {
    /// Held for re-routing on retry/fallback. Not currently used inside the
    /// executor itself — the orchestrator does the routing — but kept so
    /// adaptive retry doesn't need a constructor change.
    #[allow(dead_code)]
    router: Arc<TaskRouter>,
    /// Soft cap on concurrent subtasks. Not enforced yet — Ollama already
    /// serializes per-model — but the field is part of the public API
    /// surface for `ParallelExecutor::new`.
    #[allow(dead_code)]
    workers: usize,
    provider: Arc<dyn LlmProvider>,
    active_tasks: Arc<RwLock<HashMap<String, TaskStatus>>>,
}

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub task_id: String,
    pub output: String,
    pub tokens_generated: usize,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl ParallelExecutor {
    pub fn new(router: Arc<TaskRouter>, provider: Arc<dyn LlmProvider>, workers: usize) -> Self {
        Self {
            router,
            workers,
            provider,
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn execute_parallel(
        &self,
        task: &str,
        subtasks: Vec<SubTask>,
        system_prompt: Option<&str>,
        model: &str,
        num_ctx: usize,
    ) -> Result<Vec<WorkerResult>> {
        info!(
            "executing {} subtasks in parallel on `{model}` (num_ctx={num_ctx})",
            subtasks.len()
        );

        // Warm-load the model BEFORE fanning out — otherwise the first parallel
        // worker pays the cold-start tax (5-30s for a 7-14B) and Ollama
        // serializes the rest of the workers behind it. One preload, all
        // workers benefit. `1h` keep_alive lets follow-up calls stay warm too.
        if let Err(e) = self.provider.preload(model, "1h").await {
            warn!("preload of `{model}` failed (continuing anyway): {e}");
        }

        let (tx, mut rx) = mpsc::channel::<WorkerResult>(subtasks.len().max(1));

        let task_handles: Vec<_> = subtasks
            .into_iter()
            .map(|subtask| {
                let provider = self.provider.clone();
                let tx = tx.clone();
                let system = system_prompt.map(|s| s.to_string());
                let task_text = task.to_string();
                let model = model.to_string();

                tokio::spawn(async move {
                    let result = Self::execute_subtask(
                        provider,
                        &task_text,
                        &subtask,
                        system.as_deref(),
                        &model,
                        num_ctx,
                    )
                    .await;

                    let _ = tx.send(result).await;
                })
            })
            .collect();

        let mut results = Vec::new();
        for _ in 0..task_handles.len() {
            if let Some(result) = rx.recv().await {
                results.push(result);
            }
        }

        for handle in task_handles {
            if let Err(e) = handle.await {
                error!("Worker task panicked: {}", e);
            }
        }

        Ok(results)
    }

    async fn execute_subtask(
        provider: Arc<dyn LlmProvider>,
        original_task: &str,
        subtask: &SubTask,
        system: Option<&str>,
        model: &str,
        num_ctx: usize,
    ) -> WorkerResult {
        info!(
            "worker {}: starting '{}' on `{model}`",
            subtask.id, subtask.name
        );

        let mut prompt = String::new();

        if let Some(sys) = system {
            prompt.push_str(&format!("{sys}\n\n"));
        }

        prompt.push_str(&format!(
            "Original Task: {original_task}\n\nSubtask: {}\nDescription: {}\n\n",
            subtask.name, subtask.description
        ));

        if !subtask.skill_tags.is_empty() {
            prompt.push_str(&format!("Focus on: {}\n\n", subtask.skill_tags.join(", ")));
        }

        prompt.push_str("Provide the complete implementation:\n");

        let opts = GenerateOptions {
            model: model.to_string(),
            prompt,
            system: Some(
                "You are a specialized coding assistant. Return only code with minimal explanation."
                    .to_string(),
            ),
            temperature: Some(0.7),
            num_ctx: Some(num_ctx),
            // Reuse the model that's already warm from `preload()`. The
            // executor's parent call passed the same `keep_alive` so all
            // workers share the same residency window.
            keep_alive: Some("1h".to_string()),
            stream: false,
            ..Default::default()
        };

        match provider.generate(opts).await {
            Ok(response) => {
                info!(
                    "Worker {}: Completed successfully ({} tokens)",
                    subtask.id, response.tokens_generated
                );
                WorkerResult {
                    task_id: subtask.id.clone(),
                    output: response.content,
                    tokens_generated: response.tokens_generated,
                    duration_ms: response.duration_ms,
                    success: true,
                    error: None,
                }
            }
            Err(e) => {
                error!("Worker {}: Failed - {}", subtask.id, e);
                WorkerResult {
                    task_id: subtask.id.clone(),
                    output: String::new(),
                    tokens_generated: 0,
                    duration_ms: 0,
                    success: false,
                    error: Some(e.to_string()),
                }
            }
        }
    }

    pub async fn execute_single(
        &self,
        task: &str,
        model: &str,
        system: Option<&str>,
    ) -> Result<LlmResponse> {
        let opts = GenerateOptions {
            model: model.to_string(),
            prompt: task.to_string(),
            system: system.map(|s| s.to_string()),
            temperature: Some(0.7),
            num_ctx: Some(8192),
            stream: false,
            ..Default::default()
        };

        self.provider.generate(opts).await
    }

    pub async fn merge_results(&self, results: Vec<WorkerResult>) -> Result<String> {
        let successful: Vec<_> = results.iter().filter(|r| r.success).collect();

        if successful.is_empty() {
            anyhow::bail!("All workers failed, cannot merge results");
        }

        if successful.len() == 1 {
            return Ok(successful[0].output.clone());
        }

        let merge_prompt = format!(
            "Merge the following code outputs into a single coherent implementation:\n\n{}",
            successful
                .iter()
                .map(|r| format!("=== {} ===\n{}\n", r.task_id, r.output))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let opts = GenerateOptions {
            model: "qwen2.5-coder:7b".to_string(),
            prompt: merge_prompt,
            system: Some(
                "You are a code merging expert. Combine the provided code snippets into a single, \
                cohesive implementation. Resolve any conflicts by keeping the best version of each \
                section. Return only the merged code without explanation."
                    .to_string(),
            ),
            temperature: Some(0.3),
            num_ctx: Some(16384),
            stream: false,
            ..Default::default()
        };

        self.provider.generate(opts).await.map(|r| r.content)
    }

    pub async fn get_active_tasks(&self) -> Vec<(String, TaskStatus)> {
        let tasks = self.active_tasks.read().await;
        tasks
            .iter()
            .map(|(id, status)| (id.clone(), status.clone()))
            .collect()
    }
}

pub struct MergingAgent {
    provider: Arc<dyn LlmProvider>,
}

impl MergingAgent {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }

    pub async fn reconcile(&self, outputs: Vec<&str>, language: Option<&str>) -> Result<String> {
        let lang_context = language
            .map(|l| format!("Language: {}\n", l))
            .unwrap_or_default();

        let merge_prompt = format!(
            "{}You are a code reconciliation expert. Review the following code outputs and \
            produce a single, clean, production-ready implementation.\n\n\
            Resolve any conflicts by selecting the most correct and efficient solution.\n\n\
            Code Outputs:\n{}\n\n\
            Provide the final merged implementation:",
            lang_context,
            outputs
                .iter()
                .enumerate()
                .map(|(i, o)| format!("--- Output {} ---\n{}\n", i + 1, o))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let opts = GenerateOptions {
            model: "deepseek-coder-v2:16b".to_string(),
            prompt: merge_prompt,
            system: Some(
                "You are an expert code reviewer and merger. Produce clean, well-structured, \
                production-ready code. Remove any duplicates or conflicting code. Ensure the \
                final output is syntactically correct and follows best practices."
                    .to_string(),
            ),
            temperature: Some(0.2),
            num_ctx: Some(16384),
            stream: false,
            ..Default::default()
        };

        self.provider.generate(opts).await.map(|r| r.content)
    }

    pub async fn resolve_conflicts(&self, conflicting_outputs: Vec<&str>) -> Result<String> {
        let conflict_prompt = format!(
            "The following code snippets have conflicts. Analyze each one and produce \
            the correct, merged version:\n\n{}",
            conflicting_outputs
                .iter()
                .enumerate()
                .map(|(i, o)| format!("=== Version {} ===\n{}\n", i + 1, o))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let opts = GenerateOptions {
            model: "qwen2.5-coder:7b".to_string(),
            prompt: conflict_prompt,
            system: Some(
                "Analyze the conflicting code versions and produce the correct merged output. \
                If there are bugs, fix them. If there are style differences, follow standard conventions. \
                Return only the final code.".to_string()
            ),
            temperature: Some(0.3),
            num_ctx: Some(8192),
            stream: false,
            ..Default::default()
        };

        self.provider.generate(opts).await.map(|r| r.content)
    }
}

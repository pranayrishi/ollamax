use crate::providers::{GenerateOptions, LlmProvider, LlmResponse};
use crate::router::{SubTask, TaskRouter};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

/// Progress event emitted by `ParallelExecutor::execute_parallel` so the CLI
/// can stream a live status to stderr while a long parallel build runs.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// A unique model is being preloaded into Ollama.
    PreloadStarted { model: String },
    /// A model finished preloading.
    PreloadFinished {
        model: String,
        ok: bool,
        elapsed_ms: u64,
    },
    /// A subtask worker started.
    WorkerStarted {
        subtask_id: String,
        subtask_name: String,
        model: String,
    },
    /// A subtask worker finished.
    WorkerFinished {
        subtask_id: String,
        subtask_name: String,
        ok: bool,
        elapsed_ms: u64,
        tokens: usize,
    },
}

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

    /// Backwards-compatible thin wrapper that drops progress events on the
    /// floor. Existing call sites stay unchanged.
    pub async fn execute_parallel(
        &self,
        task: &str,
        subtasks: Vec<SubTask>,
        system_prompt: Option<&str>,
        default_model: &str,
        default_num_ctx: usize,
    ) -> Result<Vec<WorkerResult>> {
        self.execute_parallel_with_progress(
            task,
            subtasks,
            system_prompt,
            default_model,
            default_num_ctx,
            None,
        )
        .await
    }

    /// Like `execute_parallel`, plus a progress channel. Each preload and
    /// each worker emits a `ProgressEvent` so the CLI can render a
    /// real-time status board on stderr instead of staring at a blank
    /// terminal for 5 minutes.
    pub async fn execute_parallel_with_progress(
        &self,
        task: &str,
        subtasks: Vec<SubTask>,
        system_prompt: Option<&str>,
        default_model: &str,
        default_num_ctx: usize,
        progress: Option<mpsc::UnboundedSender<ProgressEvent>>,
    ) -> Result<Vec<WorkerResult>> {
        info!(
            "executing {} subtasks in parallel (default model: `{default_model}`)",
            subtasks.len()
        );

        // **Heterogeneous parallel preload.** Each subtask may carry its
        // own `model_override`. We collect every distinct model that will
        // be used in this batch and preload them *concurrently* — Ollama
        // serializes per-model but not across models, so a 32B and a 3B
        // can load in parallel. This is the entire reason this method
        // exists rather than the previous "one model name for everyone"
        // version.
        let mut needed_models: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for s in &subtasks {
            needed_models.insert(
                s.model_override
                    .clone()
                    .unwrap_or_else(|| default_model.to_string()),
            );
        }
        info!(
            "preloading {} unique model(s) in parallel: {:?}",
            needed_models.len(),
            needed_models
        );
        let preload_futures = needed_models.into_iter().map(|m| {
            let provider = self.provider.clone();
            let progress = progress.clone();
            async move {
                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::PreloadStarted { model: m.clone() });
                }
                let start = std::time::Instant::now();
                let result = provider.preload(&m, "1h").await;
                let ok = result.is_ok();
                if let Err(e) = result {
                    warn!("preload of `{m}` failed (continuing anyway): {e}");
                }
                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::PreloadFinished {
                        model: m,
                        ok,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
            }
        });
        let preload_handles: Vec<_> = preload_futures.map(tokio::spawn).collect();
        for h in preload_handles {
            let _ = h.await;
        }

        let (tx, mut rx) = mpsc::channel::<WorkerResult>(subtasks.len().max(1));

        let task_handles: Vec<_> = subtasks
            .into_iter()
            .map(|subtask| {
                let provider = self.provider.clone();
                let tx = tx.clone();
                let system = system_prompt.map(|s| s.to_string());
                let task_text = task.to_string();
                let model = subtask
                    .model_override
                    .clone()
                    .unwrap_or_else(|| default_model.to_string());
                let num_ctx = subtask.num_ctx_override.unwrap_or(default_num_ctx);
                let progress = progress.clone();
                let subtask_id = subtask.id.clone();
                let subtask_name = subtask.name.clone();
                let model_name = model.clone();

                tokio::spawn(async move {
                    if let Some(tx) = &progress {
                        let _ = tx.send(ProgressEvent::WorkerStarted {
                            subtask_id: subtask_id.clone(),
                            subtask_name: subtask_name.clone(),
                            model: model_name.clone(),
                        });
                    }
                    let start = std::time::Instant::now();
                    let result = Self::execute_subtask(
                        provider,
                        &task_text,
                        &subtask,
                        system.as_deref(),
                        &model,
                        num_ctx,
                    )
                    .await;
                    if let Some(tx) = &progress {
                        let _ = tx.send(ProgressEvent::WorkerFinished {
                            subtask_id,
                            subtask_name,
                            ok: result.success,
                            elapsed_ms: start.elapsed().as_millis() as u64,
                            tokens: result.tokens_generated,
                        });
                    }

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

    /// Merge subtask outputs back into a single artifact.
    ///
    /// **Strategy: section-aware concatenation, then a model-based dedup
    /// pass.** The previous merger asked the model to "combine these code
    /// snippets" with no structure, which produced hallucinated stitching
    /// (the model would invent imports that no worker emitted, drop sections
    /// of one worker's output, etc.). The new strategy:
    ///
    /// 1. If one worker succeeded → return its output verbatim. No model
    ///    call. Cheapest, most reliable path.
    /// 2. If multiple workers succeeded → concatenate them with explicit
    ///    section markers (`// === Frontend/UI ===`) and ask the model to
    ///    *only* deduplicate imports and resolve obvious conflicts. The
    ///    structure stays intact. The model is told the section markers are
    ///    load-bearing.
    /// 3. The merger uses the same model the workers used (passed in by
    ///    `model`) so we don't pay an extra cold-start to load a different
    ///    one mid-build.
    /// 4. `temperature=0.1` because merging is not creative work.
    pub async fn merge_results(
        &self,
        results: Vec<WorkerResult>,
        model: &str,
        num_ctx: usize,
    ) -> Result<String> {
        let successful: Vec<_> = results.iter().filter(|r| r.success).collect();

        if successful.is_empty() {
            // Surface the actual worker errors so the user can debug.
            let errors: Vec<_> = results
                .iter()
                .filter(|r| !r.success)
                .filter_map(|r| r.error.as_ref())
                .collect();
            anyhow::bail!(
                "all {} workers failed; first error: {}",
                results.len(),
                errors.first().map(|s| s.as_str()).unwrap_or("(no error)")
            );
        }

        if successful.len() == 1 {
            return Ok(successful[0].output.clone());
        }

        // Concatenate with section markers. The fence markers tell the model
        // "these are not optional, do not delete them" — past behavior was
        // to silently drop a worker's output if the model couldn't figure
        // out how it fit.
        let mut concatenated = String::new();
        for r in &successful {
            concatenated.push_str(&format!(
                "// === BEGIN section {} ===\n{}\n// === END section {} ===\n\n",
                r.task_id, r.output, r.task_id
            ));
        }

        let merge_prompt = format!(
            "Below are {} code sections produced by separate worker agents. Each \
             section is wrapped in BEGIN/END markers. Your job:\n\
             \n\
             1. Combine all sections into one coherent file.\n\
             2. Deduplicate imports/use statements at the top.\n\
             3. If two sections define the same symbol, keep the more complete \
                version (longer, with more error handling).\n\
             4. Do NOT invent imports, types, or functions that are not in any \
                input section.\n\
             5. Do NOT delete entire sections — every BEGIN section must be \
                represented in the output, even if just as a function the rest \
                of the file calls.\n\
             6. Strip the BEGIN/END markers from the final output.\n\
             7. Return only the merged code. No markdown fences. No prose.\n\
             \n\
             Sections:\n\
             {concatenated}",
            successful.len()
        );

        let opts = GenerateOptions {
            model: model.to_string(),
            prompt: merge_prompt,
            system: Some(
                "You are a code merger. You combine multiple working drafts into \
                 one coherent file by deduplicating imports, resolving conflicts \
                 conservatively, and preserving every input section. You never \
                 invent code that wasn't in the inputs."
                    .to_string(),
            ),
            // Merging is mechanical, not creative. Low temp + low top_p so
            // the same inputs produce the same merge.
            temperature: Some(0.1),
            top_p: Some(0.5),
            num_ctx: Some(num_ctx),
            stream: false,
            keep_alive: Some("1h".to_string()),
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

use crate::providers::{GenerateOptions, LlmProvider, LlmResponse};
use crate::router::{SubTask, TaskRouter};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::task::JoinSet;
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
    /// Maximum number of worker generations (and model preloads) that may
    /// be in flight for this executor. A value of zero is normalized to one
    /// at construction, so a malformed configuration cannot deadlock a run.
    workers: usize,
    provider: Arc<dyn LlmProvider>,
    /// Global resource capacity shared by generation and model loading across
    /// all overlapping builds. Either operation can consume substantial
    /// RAM/VRAM, so separate lanes would allow their combined load to exceed
    /// the configured worker budget.
    resource_permits: Arc<Semaphore>,
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
        let requested_workers = workers;
        let workers = requested_workers.max(1);
        if requested_workers == 0 {
            warn!("parallel executor worker count was normalized to one");
        }
        Self {
            router,
            workers,
            provider,
            resource_permits: Arc::new(Semaphore::new(workers)),
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
            "executing {} subtasks with a concurrency cap of {} (default model: `{default_model}`)",
            subtasks.len(),
            self.workers,
        );

        // Publish the entire batch as pending before any preload or worker
        // starts. The single write lock makes this a coherent snapshot for
        // dashboards polling `get_active_tasks` while a build is queued.
        let expected_task_ids: Vec<String> = subtasks.iter().map(|s| s.id.clone()).collect();
        {
            let mut active_tasks = self.active_tasks.write().await;
            for task_id in &expected_task_ids {
                active_tasks.insert(task_id.clone(), TaskStatus::Pending);
            }
        }

        // **Heterogeneous parallel preload.** Each subtask may carry its
        // own `model_override`. We collect every distinct model that will
        // be used in this batch and preload them in parallel up to the same
        // configured worker budget. A large batch with many model overrides
        // therefore cannot stampede Ollama with unbounded RAM/VRAM loads.
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
            "preloading {} unique model(s), at most {} at once: {:?}",
            needed_models.len(),
            self.workers,
            needed_models
        );
        let mut preload_tasks = JoinSet::new();
        for model in needed_models {
            let provider = self.provider.clone();
            let progress = progress.clone();
            let permits = self.resource_permits.clone();
            preload_tasks.spawn(async move {
                let permit = match permits.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        // The executor never closes its semaphore, but make
                        // a future shutdown path fail visibly rather than
                        // silently dropping a preload event.
                        if let Some(tx) = &progress {
                            let _ = tx.send(ProgressEvent::PreloadFinished {
                                model,
                                ok: false,
                                elapsed_ms: 0,
                            });
                        }
                        return;
                    }
                };
                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::PreloadStarted {
                        model: model.clone(),
                    });
                }
                let start = std::time::Instant::now();
                let result = provider.preload(&model, "1h").await;
                let ok = result.is_ok();
                if let Err(e) = result {
                    warn!("preload of `{model}` failed (continuing anyway): {e}");
                }
                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::PreloadFinished {
                        model,
                        ok,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
                drop(permit);
            });
        }
        while let Some(joined) = preload_tasks.join_next().await {
            if let Err(e) = joined {
                error!("Model preload task panicked or was cancelled: {e}");
            }
        }

        // A JoinSet, instead of a result channel, keeps this method from
        // waiting forever when a task panics before sending its result. It
        // also lets us preserve the router's input order, rather than merging
        // files in nondeterministic completion order.
        let task_count = subtasks.len();
        let mut results_by_index: Vec<Option<WorkerResult>> =
            (0..task_count).map(|_| None).collect();
        let mut worker_tasks = JoinSet::new();

        for (index, subtask) in subtasks.into_iter().enumerate() {
            let provider = self.provider.clone();
            let system = system_prompt.map(|s| s.to_string());
            let task_text = task.to_string();
            let model = subtask
                .model_override
                .clone()
                .unwrap_or_else(|| default_model.to_string());
            let num_ctx = subtask.num_ctx_override.unwrap_or(default_num_ctx);
            let progress = progress.clone();
            let active_tasks = self.active_tasks.clone();
            let permits = self.resource_permits.clone();
            let subtask_id = subtask.id.clone();
            let subtask_name = subtask.name.clone();
            let model_name = model.clone();

            worker_tasks.spawn(async move {
                let permit = match permits.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        let error_message = "worker executor shut down before the task could start";
                        Self::set_task_status(
                            &active_tasks,
                            &subtask_id,
                            TaskStatus::Failed(error_message.to_string()),
                        )
                        .await;
                        if let Some(tx) = &progress {
                            let _ = tx.send(ProgressEvent::WorkerFinished {
                                subtask_id: subtask_id.clone(),
                                subtask_name: subtask_name.clone(),
                                ok: false,
                                elapsed_ms: 0,
                                tokens: 0,
                            });
                        }
                        return (
                            index,
                            WorkerResult {
                                task_id: subtask_id,
                                output: String::new(),
                                tokens_generated: 0,
                                duration_ms: 0,
                                success: false,
                                error: Some(error_message.to_string()),
                            },
                        );
                    }
                };

                Self::set_task_status(&active_tasks, &subtask_id, TaskStatus::Running).await;
                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::WorkerStarted {
                        subtask_id: subtask_id.clone(),
                        subtask_name: subtask_name.clone(),
                        model: model_name,
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
                let status =
                    if result.success {
                        // Keep a useful terminal state without retaining a second
                        // copy of what may be a very large generated artifact.
                        TaskStatus::Completed(format!(
                            "completed: {} tokens in {} ms",
                            result.tokens_generated, result.duration_ms
                        ))
                    } else {
                        TaskStatus::Failed(result.error.clone().unwrap_or_else(|| {
                            "worker failed without an error message".to_string()
                        }))
                    };
                Self::set_task_status(&active_tasks, &subtask_id, status).await;

                if let Some(tx) = &progress {
                    let _ = tx.send(ProgressEvent::WorkerFinished {
                        subtask_id,
                        subtask_name,
                        ok: result.success,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        tokens: result.tokens_generated,
                    });
                }
                drop(permit);
                (index, result)
            });
        }

        while let Some(joined) = worker_tasks.join_next().await {
            match joined {
                Ok((index, result)) => results_by_index[index] = Some(result),
                Err(e) => error!("Worker task panicked or was cancelled: {e}"),
            }
        }

        // Every submitted subtask gets a terminal result, even if a spawned
        // task panicked. The old channel-based loop could instead block
        // forever on that missing send and hide the failure from callers.
        let mut results = Vec::with_capacity(task_count);
        for (index, result) in results_by_index.into_iter().enumerate() {
            match result {
                Some(result) => results.push(result),
                None => {
                    let task_id = expected_task_ids[index].clone();
                    let error_message = "worker task ended before producing a result".to_string();
                    Self::set_task_status(
                        &self.active_tasks,
                        &task_id,
                        TaskStatus::Failed(error_message.clone()),
                    )
                    .await;
                    results.push(WorkerResult {
                        task_id,
                        output: String::new(),
                        tokens_generated: 0,
                        duration_ms: 0,
                        success: false,
                        error: Some(error_message),
                    });
                }
            }
        }

        Ok(results)
    }

    async fn set_task_status(
        active_tasks: &Arc<RwLock<HashMap<String, TaskStatus>>>,
        task_id: &str,
        status: TaskStatus,
    ) {
        active_tasks
            .write()
            .await
            .insert(task_id.to_string(), status);
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
        let mut statuses: Vec<_> = tasks
            .iter()
            .map(|(id, status)| (id.clone(), status.clone()))
            .collect();
        // HashMap iteration order changes between calls; stable ordering makes
        // this snapshot safe for status UIs and tests to compare directly.
        statuses.sort_by(|(left, _), (right, _)| left.cmp(right));
        statuses
    }
}

pub struct MergingAgent {
    provider: Arc<dyn LlmProvider>,
    model: String,
}

impl MergingAgent {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self::with_model(provider, "qwen3.5:4b")
    }

    /// Use the same installed writer/planner model selected by the caller when
    /// available. This avoids hidden requests to stale, uninstalled tags.
    pub fn with_model(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
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
            model: self.model.clone(),
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
            model: self.model.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatOptions;
    use crate::router::ModelConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{timeout, Duration};

    /// A controllable provider which holds requests at a semaphore gate. It
    /// lets these tests observe true in-flight concurrency rather than merely
    /// counting spawned futures.
    struct ConcurrencyProvider {
        block_generates: bool,
        block_preloads: bool,
        generate_active: AtomicUsize,
        generate_max: AtomicUsize,
        preload_active: AtomicUsize,
        preload_max: AtomicUsize,
        resource_active: AtomicUsize,
        resource_max: AtomicUsize,
        generate_gate: Arc<Semaphore>,
        preload_gate: Arc<Semaphore>,
    }

    impl ConcurrencyProvider {
        fn new(block_generates: bool, block_preloads: bool) -> Self {
            Self {
                block_generates,
                block_preloads,
                generate_active: AtomicUsize::new(0),
                generate_max: AtomicUsize::new(0),
                preload_active: AtomicUsize::new(0),
                preload_max: AtomicUsize::new(0),
                resource_active: AtomicUsize::new(0),
                resource_max: AtomicUsize::new(0),
                generate_gate: Arc::new(Semaphore::new(0)),
                preload_gate: Arc::new(Semaphore::new(0)),
            }
        }

        fn record_high_water(maximum: &AtomicUsize, current: usize) {
            let mut previous = maximum.load(Ordering::SeqCst);
            while current > previous {
                match maximum.compare_exchange_weak(
                    previous,
                    current,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break,
                    Err(actual) => previous = actual,
                }
            }
        }

        async fn pass_gate(
            active: &AtomicUsize,
            maximum: &AtomicUsize,
            resource_active: &AtomicUsize,
            resource_maximum: &AtomicUsize,
            gate: &Arc<Semaphore>,
            should_block: bool,
        ) -> Result<()> {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            Self::record_high_water(maximum, current);
            let resource_current = resource_active.fetch_add(1, Ordering::SeqCst) + 1;
            Self::record_high_water(resource_maximum, resource_current);

            let outcome = if should_block {
                match gate.acquire().await {
                    Ok(permit) => {
                        // Test release permits represent one completed provider
                        // call, so do not return them to the gate when this
                        // future exits.
                        permit.forget();
                        Ok(())
                    }
                    Err(_) => Err(anyhow::anyhow!("test gate closed")),
                }
            } else {
                Ok(())
            };

            active.fetch_sub(1, Ordering::SeqCst);
            resource_active.fetch_sub(1, Ordering::SeqCst);
            outcome
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ConcurrencyProvider {
        fn name(&self) -> &str {
            "concurrency-test"
        }

        async fn generate(&self, options: GenerateOptions) -> Result<LlmResponse> {
            Self::pass_gate(
                &self.generate_active,
                &self.generate_max,
                &self.resource_active,
                &self.resource_max,
                &self.generate_gate,
                self.block_generates,
            )
            .await?;

            Ok(LlmResponse {
                content: format!("output for {}", options.model),
                model: options.model,
                tokens_generated: 1,
                context_used: 0,
                duration_ms: 1,
            })
        }

        async fn chat(&self, _options: ChatOptions) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: String::new(),
                model: "concurrency-test".to_string(),
                tokens_generated: 0,
                context_used: 0,
                duration_ms: 0,
            })
        }

        async fn list_models(&self) -> Result<Vec<crate::providers::ModelInfo>> {
            Ok(Vec::new())
        }

        async fn preload(&self, _model: &str, _keep_alive: &str) -> Result<()> {
            Self::pass_gate(
                &self.preload_active,
                &self.preload_max,
                &self.resource_active,
                &self.resource_max,
                &self.preload_gate,
                self.block_preloads,
            )
            .await
        }
    }

    fn test_executor(provider: Arc<ConcurrencyProvider>, workers: usize) -> Arc<ParallelExecutor> {
        Arc::new(ParallelExecutor::new(
            Arc::new(TaskRouter::new(ModelConfig::default())),
            provider,
            workers,
        ))
    }

    fn subtasks(count: usize, distinct_models: bool) -> Vec<SubTask> {
        (0..count)
            .map(|index| {
                let mut subtask = SubTask::parallel(
                    format!("worker-{index}"),
                    format!("implement component {index}"),
                );
                if distinct_models {
                    subtask.model_override = Some(format!("model-{index}:test"));
                }
                subtask
            })
            .collect()
    }

    async fn wait_until_at_least(counter: &AtomicUsize, expected: usize) {
        timeout(Duration::from_secs(2), async {
            while counter.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for provider calls to start");
    }

    #[tokio::test]
    async fn worker_concurrency_and_task_states_are_bounded() {
        let provider = Arc::new(ConcurrencyProvider::new(true, false));
        let executor = test_executor(provider.clone(), 2);
        let tasks = subtasks(4, false);
        let expected_ids: Vec<String> = tasks.iter().map(|task| task.id.clone()).collect();

        let runner = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute_parallel("build test components", tasks, None, "default:test", 1024)
                    .await
            })
        };

        wait_until_at_least(&provider.generate_active, 2).await;
        assert_eq!(provider.generate_max.load(Ordering::SeqCst), 2);

        let active = executor.get_active_tasks().await;
        assert_eq!(
            active
                .iter()
                .filter(|(_, status)| matches!(status, TaskStatus::Running))
                .count(),
            2,
            "only permit holders may report as running"
        );
        assert_eq!(
            active
                .iter()
                .filter(|(_, status)| matches!(status, TaskStatus::Pending))
                .count(),
            2,
            "queued subtasks must remain visible as pending"
        );

        provider.generate_gate.add_permits(4);
        let results = timeout(Duration::from_secs(2), runner)
            .await
            .expect("parallel execution timed out")
            .expect("executor task panicked")
            .expect("parallel execution failed");

        let result_ids: Vec<String> = results.into_iter().map(|result| result.task_id).collect();
        assert_eq!(
            result_ids, expected_ids,
            "results follow router input order"
        );
        assert!(
            executor
                .get_active_tasks()
                .await
                .iter()
                .all(|(_, status)| matches!(status, TaskStatus::Completed(_))),
            "every submitted task should have a terminal state"
        );
    }

    #[tokio::test]
    async fn model_preloads_use_the_same_bounded_budget() {
        let provider = Arc::new(ConcurrencyProvider::new(false, true));
        let executor = test_executor(provider.clone(), 2);

        let runner = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute_parallel(
                        "build heterogeneous components",
                        subtasks(4, true),
                        None,
                        "default:test",
                        1024,
                    )
                    .await
            })
        };

        wait_until_at_least(&provider.preload_active, 2).await;
        assert_eq!(provider.preload_max.load(Ordering::SeqCst), 2);

        provider.preload_gate.add_permits(4);
        timeout(Duration::from_secs(2), runner)
            .await
            .expect("parallel execution timed out")
            .expect("executor task panicked")
            .expect("parallel execution failed");

        assert_eq!(provider.preload_max.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn preloads_and_generations_share_global_capacity_across_overlapping_builds() {
        let provider = Arc::new(ConcurrencyProvider::new(true, true));
        let executor = test_executor(provider.clone(), 2);

        // Let the first build finish its single preload, then hold both of its
        // generation requests. They should occupy the entire global capacity.
        let first_runner = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute_parallel(
                        "first overlapping build",
                        subtasks(2, false),
                        None,
                        "default:test",
                        1024,
                    )
                    .await
            })
        };

        wait_until_at_least(&provider.preload_active, 1).await;
        provider.preload_gate.add_permits(1);
        wait_until_at_least(&provider.generate_active, 2).await;
        assert_eq!(provider.resource_active.load(Ordering::SeqCst), 2);

        // A second build needs a distinct model preload. With separate
        // preload and generation semaphores it would enter the provider here,
        // producing three simultaneous resource-heavy operations. The shared
        // semaphore must keep it queued until the first build releases a slot.
        let second_runner = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute_parallel(
                        "second overlapping build",
                        subtasks(1, true),
                        None,
                        "default:test",
                        1024,
                    )
                    .await
            })
        };

        assert!(
            timeout(
                Duration::from_millis(100),
                wait_until_at_least(&provider.preload_active, 1),
            )
            .await
            .is_err(),
            "the second build's preload must wait for shared generation capacity"
        );
        assert_eq!(provider.resource_max.load(Ordering::SeqCst), 2);

        provider.generate_gate.add_permits(2);
        wait_until_at_least(&provider.preload_active, 1).await;
        provider.preload_gate.add_permits(1);
        wait_until_at_least(&provider.generate_active, 1).await;
        provider.generate_gate.add_permits(1);

        timeout(Duration::from_secs(2), first_runner)
            .await
            .expect("first parallel execution timed out")
            .expect("first executor task panicked")
            .expect("first parallel execution failed");
        timeout(Duration::from_secs(2), second_runner)
            .await
            .expect("second parallel execution timed out")
            .expect("second executor task panicked")
            .expect("second parallel execution failed");

        assert_eq!(provider.resource_max.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn zero_workers_is_normalized_to_one() {
        let provider = Arc::new(ConcurrencyProvider::new(false, false));
        let executor = test_executor(provider, 0);

        assert_eq!(executor.workers, 1);
        assert_eq!(executor.resource_permits.available_permits(), 1);
    }
}

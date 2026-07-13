use crate::context::ContextManager;
use crate::executor::ParallelExecutor;
use crate::models::{is_offline_ollama_tag, is_ollama_cloud_tag, LocalAvailability, ModelRegistry};
use crate::monitoring::VramSentinel;
use crate::providers::{
    parse_local_model_selector, GenerateOptions, LlmProvider, ModelInfo, OllamaProvider,
};
use crate::router::TaskRouter;
use crate::security::{AuditReport, SecurityGuard, TddEnforcer};
use crate::skills::SkillsEngine;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

pub struct Orchestrator {
    config: OrchestratorConfig,
    ollama: Arc<OllamaProvider>,
    router: Arc<TaskRouter>,
    context: Arc<ContextManager>,
    executor: Arc<ParallelExecutor>,
    sentinel: Arc<VramSentinel>,
    security: Arc<SecurityGuard>,
    /// TDD enforcement is constructed but not yet wired into the build path —
    /// the enforce-tests-on-every-change feature is tracked in the roadmap.
    /// Holding the Arc here means we don't have to plumb config through every
    /// call site once it lands.
    #[allow(dead_code)]
    tdd: Arc<TddEnforcer>,
    skills: Arc<SkillsEngine>,
    session: Arc<RwLock<SessionState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    pub ollama_url: String,
    pub default_model: String,
    pub planning_model: String,
    pub max_parallel_workers: usize,
    pub security_enabled: bool,
    pub tdd_enforced: bool,
    /// User always-rules to inject into the system prompt of every worker.
    /// Loaded from `~/.config/ollama-forge/rules/*.md` by the CLI and
    /// passed through here. Empty string means "no rules configured."
    #[serde(default)]
    pub rules_suffix: String,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        // Must agree with `Config::default` in lib.rs and `STARTER_FORGE_TOML`
        // in main.rs. Keep the first-run model laptop-sized; runtime selection
        // still prefers an installed model over an unavailable configured tag.
        Self {
            ollama_url: crate::providers::ollama::DEFAULT_OLLAMA_ENDPOINT.to_string(),
            default_model: "qwen3.5:4b".to_string(),
            planning_model: "deepseek-r1:8b".to_string(),
            max_parallel_workers: 4,
            security_enabled: true,
            tdd_enforced: false,
            rules_suffix: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub id: String,
    pub project_path: Option<PathBuf>,
    pub active_models: Vec<String>,
    pub context_history: Vec<String>,
    pub start_time: chrono::DateTime<chrono::Utc>,
}

impl Orchestrator {
    pub async fn new(config: OrchestratorConfig) -> Result<Self> {
        validate_ollama_only_build_model("default_model", &config.default_model)?;
        validate_ollama_only_build_model("planning_model", &config.planning_model)?;
        let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));

        if !ollama.health_check().await? {
            warn!("Ollama is not responding at {}", config.ollama_url);
        }

        let router = Arc::new(TaskRouter::new(Default::default()));
        let context = Arc::new(ContextManager::new(32768));
        let executor = Arc::new(ParallelExecutor::new(
            router.clone(),
            ollama.clone(),
            config.max_parallel_workers,
        ));

        let sentinel = Arc::new(VramSentinel::new(2048, true));
        let security = Arc::new(SecurityGuard::new(config.security_enabled));
        let tdd = Arc::new(TddEnforcer::new(config.tdd_enforced));

        let skills_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ollama-forge")
            .join("skills");

        let skills = Arc::new(SkillsEngine::new(skills_dir));
        skills.load_skills().await?;

        let session = SessionState {
            id: uuid::Uuid::new_v4().to_string(),
            project_path: None,
            active_models: Vec::new(),
            context_history: Vec::new(),
            start_time: chrono::Utc::now(),
        };

        info!("Orchestrator initialized with session {}", session.id);

        Ok(Self {
            config,
            ollama,
            router,
            context,
            executor,
            sentinel,
            security,
            tdd,
            skills,
            session: Arc::new(RwLock::new(session)),
        })
    }

    pub async fn execute(&self, request: BuildRequest) -> Result<BuildResult> {
        self.execute_with_progress(request, None).await
    }

    /// Execute with an optional progress channel. The CLI uses this to
    /// stream worker status to stderr while a long parallel build runs.
    pub async fn execute_with_progress(
        &self,
        request: BuildRequest,
        progress: Option<tokio::sync::mpsc::UnboundedSender<crate::executor::ProgressEvent>>,
    ) -> Result<BuildResult> {
        info!("Executing build request: {}", request.task);

        let health = self.sentinel.check_health(None).await;
        info!("Health check: {:?}", health.hardware_profile);

        let available_models: Vec<ModelInfo> = self
            .ollama
            .list_models()
            .await?
            .into_iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect();
        if available_models.is_empty() {
            anyhow::bail!(
                "no models installed in ollama. Pull one first:\n  ollama pull {}",
                self.config.default_model
            );
        }
        let mut complexity = self
            .router
            .analyze_complexity(&request.task, &available_models)
            .await?;
        // The analyzer may pick a model that isn't actually installed
        // (it falls through to a hardcoded default if no available model
        // matches the tier patterns). `route_to_model` walks *available*
        // models in size order and is guaranteed to return one of them.
        // Without this, the executor below tries to preload an
        // uninstalled model and Ollama hangs trying to pull it (5 minute
        // timeout, no useful error).
        complexity.suggested_model = self.router.route_to_model(&complexity, &available_models);
        info!(
            "Complexity score: {} ({:?}) → routed to `{}`",
            complexity.score, complexity.task_type, complexity.suggested_model
        );

        let context_prompt = self.build_system_context(&request).await?;
        self.context.add("system", &context_prompt).await?;

        // Tiered routing: each subtask gets a model override based on its
        // role (architecture → biggest installed model, boilerplate →
        // smallest, default work → analyzer's pick). VRAM-aware: if the
        // sum of selected models wouldn't fit in free VRAM, we collapse
        // back to a single model. When only one model is installed this
        // already falls through to a uniform run.
        let subtasks = self.router.split_into_tiered_subtasks_vram_aware(
            &request.task,
            &available_models,
            &complexity.suggested_model,
            health.hardware_profile.free_vram_mb,
        );

        if self.router.can_parallelize(&complexity) && subtasks.len() > 1 {
            // Log the heterogeneous routing decision so the user can see
            // which model is doing what when they pass --verbose.
            for s in &subtasks {
                info!(
                    "  subtask `{}` → {}",
                    s.name,
                    s.model_override
                        .as_deref()
                        .unwrap_or(&complexity.suggested_model)
                );
            }
            // Pull num_ctx from hardware sentinel so we never overflow VRAM.
            let num_ctx = health.hardware_profile.optimal_context;
            let results = self
                .executor
                .execute_parallel_with_progress(
                    &request.task,
                    subtasks,
                    Some(&context_prompt),
                    &complexity.suggested_model,
                    num_ctx,
                    progress,
                )
                .await?;

            // Sum worker stats *before* merging — `merge_results` consumes
            // the Vec. Previously these were hardcoded to 0 in the
            // returned BuildResult, so callers had no way to see how
            // expensive a build actually was.
            let total_tokens: usize = results.iter().map(|r| r.tokens_generated).sum();
            let total_duration: u64 = results.iter().map(|r| r.duration_ms).sum();
            let failed_workers: Vec<String> = results
                .iter()
                .filter(|r| !r.success)
                .filter_map(|r| r.error.clone())
                .collect();

            let merged = self
                .executor
                .merge_results(results, &complexity.suggested_model, num_ctx)
                .await?;

            if self.config.security_enabled {
                let audit = self.run_security_audit(&merged).await?;
                if !audit.findings.is_empty() {
                    warn!("Security issues found: {}", audit.summary);
                }
            }

            return Ok(BuildResult {
                success: true,
                output: merged,
                model_used: complexity.suggested_model,
                tokens_generated: total_tokens,
                duration_ms: total_duration,
                warnings: failed_workers,
            });
        }

        let response = self
            .executor
            .execute_single(
                &request.task,
                &complexity.suggested_model,
                Some(&context_prompt),
            )
            .await?;

        Ok(BuildResult {
            success: true,
            output: response.content,
            model_used: complexity.suggested_model,
            tokens_generated: response.tokens_generated,
            duration_ms: response.duration_ms,
            warnings: vec![],
        })
    }

    async fn build_system_context(&self, request: &BuildRequest) -> Result<String> {
        let mut context = String::new();

        context.push_str("You are Ollama-Forge, an expert coding assistant.\n\n");

        if let Some(ref path) = request.output_dir {
            context.push_str(&format!("Project directory: {}\n", path.display()));
        }

        if let Some(ref lang) = request.language {
            context.push_str(&format!("Language: {}\n", lang));
        }

        let health = self.sentinel.check_health(None).await;
        context.push_str(&format!(
            "Available VRAM: {} MB\nOptimal context: {}\n",
            health.hardware_profile.free_vram_mb, health.hardware_profile.optimal_context
        ));

        if let Some(skill) = self.skills.match_skill_to_task(&request.task).await {
            context.push_str(&format!("\nSkill context: {}\n", skill.prompts.system));
        }

        // Append user always-rules from ~/.config/ollama-forge/rules/.
        // The CLI loads them and passes them through OrchestratorConfig.
        if !self.config.rules_suffix.is_empty() {
            context.push_str(&self.config.rules_suffix);
        }

        Ok(context)
    }

    async fn run_security_audit(&self, code: &str) -> Result<AuditReport> {
        let findings = self.security.scan_content(code, None).await;
        let summary = format!("Found {} potential issues", findings.len());
        Ok(AuditReport {
            files_scanned: 1,
            findings,
            summary,
        })
    }

    pub async fn self_correct(&self, error: &str, context: &str) -> Result<String> {
        let correction_prompt = format!(
            "The following code produced an error:\n\nError: {}\n\nCode context:\n{}\n\n\
            Generate a corrected version that fixes the error.",
            error, context
        );

        let opts = GenerateOptions {
            model: self.config.default_model.clone(),
            prompt: correction_prompt,
            system: Some(
                "You are a code debugging expert. Fix the error and return corrected code."
                    .to_string(),
            ),
            temperature: Some(0.3),
            num_ctx: Some(16384),
            stream: false,
            ..Default::default()
        };

        self.ollama.generate(opts).await.map(|r| r.content)
    }

    pub async fn model_on_model_audit(&self, code: &str) -> Result<String> {
        let audit_prompt = format!(
            "Review the following code for bugs, security issues, and improvements:\n\n{}\n\n\
            Provide a detailed audit report.",
            code
        );

        let opts = GenerateOptions {
            model: self.config.planning_model.clone(),
            prompt: audit_prompt,
            system: Some(
                "You are a senior code auditor. Review thoroughly and provide actionable feedback."
                    .to_string(),
            ),
            temperature: Some(0.3),
            num_ctx: Some(32768),
            stream: false,
            ..Default::default()
        };

        self.ollama.generate(opts).await.map(|r| r.content)
    }

    pub async fn get_status(&self) -> Result<StatusReport> {
        let health = self.sentinel.check_health(None).await;
        let models = self
            .ollama
            .list_models()
            .await?
            .into_iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect();
        let context_stats = self.context.stats().await;
        let session = self.session.read().await;

        Ok(StatusReport {
            hardware: health.hardware_profile,
            ollama_healthy: self.ollama.health_check().await?,
            available_models: models,
            context_stats,
            session_id: session.id.clone(),
            uptime_seconds: (chrono::Utc::now() - session.start_time).num_seconds(),
        })
    }
}

/// The Build/Orchestrator pipeline deliberately shares one Ollama provider
/// across its router, preload queue, and parallel workers. Agent and Team have
/// provider-aware paths for configured `local:` endpoints; Build does not yet.
/// Rejecting such a selector before starting any preload is safer than sending
/// it to Ollama as though it were a pullable tag.
fn validate_ollama_only_build_model(field: &str, model: &str) -> Result<()> {
    if parse_local_model_selector(model)?.is_some() {
        anyhow::bail!(
            "forge build does not yet support configured local endpoint selector `{model}` in `{field}`. Use `forge agent`, `forge team`, or the desktop Agent/Team workflow for `local:<endpoint>/<model>` routing."
        );
    }
    if is_ollama_cloud_tag(model) {
        anyhow::bail!(
            "forge build cannot use `{model}` in `{field}` because it is a hosted Ollama Cloud tag, not an offline model. Select a pulled local tag instead."
        );
    }

    let catalog = ModelRegistry::seed();
    let exact = |entry: &crate::models::CuratedModel| {
        (!entry.ollama_tag.is_empty() && entry.ollama_tag.eq_ignore_ascii_case(model))
            || (!entry.source_ref.is_empty() && entry.source_ref.eq_ignore_ascii_case(model))
            || entry
                .installed_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(model))
    };
    let compact_model = compact_catalog_identifier(model);
    let Some(entry) = catalog
        .catalog()
        .find(|entry| entry.local_availability != LocalAvailability::OllamaLocal && exact(entry))
        .or_else(|| {
            catalog.catalog().find(|entry| {
                entry.local_availability != LocalAvailability::OllamaLocal
                    && compact_catalog_identifier(&entry.family) == compact_model
            })
        })
    else {
        return Ok(());
    };
    if entry.can_pull_from_ollama() {
        return Ok(());
    }

    match entry.local_availability {
        LocalAvailability::SelfHostedLocal => anyhow::bail!(
            "forge build cannot send `{model}` directly to Ollama: it is a separately self-hosted catalog entry. Configure its loopback server and use a `local:<endpoint>/<model>` selector with Agent or Team instead."
        ),
        LocalAvailability::CloudOnly => anyhow::bail!(
            "forge build cannot use `{model}` because it is cataloged as cloud-only, not an offline Ollama model. {}",
            entry.caveat
        ),
        LocalAvailability::OllamaLocal => Ok(()),
    }
}

fn compact_catalog_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub task: String,
    pub output_dir: Option<PathBuf>,
    pub language: Option<String>,
    pub run_tests: bool,
    pub skip_security: bool,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub success: bool,
    pub output: String,
    pub model_used: String,
    pub tokens_generated: usize,
    pub duration_ms: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub hardware: crate::monitoring::HardwareProfile,
    pub ollama_healthy: bool,
    pub available_models: Vec<ModelInfo>,
    pub context_stats: crate::context::ContextStats,
    pub session_id: String,
    pub uptime_seconds: i64,
}

#[cfg(test)]
mod tests {
    use super::validate_ollama_only_build_model;

    #[test]
    fn build_rejects_configured_endpoint_and_non_ollama_catalog_models() {
        let endpoint = validate_ollama_only_build_model("default_model", "local:lab/writer")
            .expect_err("Build cannot yet route configured endpoint models");
        assert!(format!("{endpoint:#}").contains("does not yet support"));

        let self_hosted = validate_ollama_only_build_model("planning_model", "DeepSeek-V4-Flash")
            .expect_err("Build cannot send self-hosted model identifiers to Ollama");
        assert!(format!("{self_hosted:#}").contains("self-hosted"));

        let cloud = validate_ollama_only_build_model("planning_model", "minimax-m3:cloud")
            .expect_err("Build cannot send cloud-only model identifiers to Ollama");
        assert!(format!("{cloud:#}").contains("hosted"));

        assert!(validate_ollama_only_build_model("planning_model", "gemma4:CLOUD").is_err());

        assert!(validate_ollama_only_build_model("default_model", "qwen3.5:4b").is_ok());
    }
}

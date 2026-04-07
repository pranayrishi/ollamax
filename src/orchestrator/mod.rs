use crate::context::ContextManager;
use crate::executor::{MergingAgent, ParallelExecutor};
use crate::monitoring::{HealthStatus, OptimizationPlan, VramSentinel};
use crate::providers::{GenerateOptions, LlmProvider, LlmResponse, OllamaProvider, ModelInfo};
use crate::router::{ComplexityScore, SubTask, TaskRouter};
use crate::security::{AuditReport, SecurityGuard, TddEnforcer, TddReport};
use crate::skills::{Skill, SkillsEngine};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub struct Orchestrator {
    config: OrchestratorConfig,
    ollama: Arc<OllamaProvider>,
    router: Arc<TaskRouter>,
    context: Arc<ContextManager>,
    executor: Arc<ParallelExecutor>,
    sentinel: Arc<VramSentinel>,
    security: Arc<SecurityGuard>,
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
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            default_model: "llama3.2:3b".to_string(),
            planning_model: "qwen2.5-coder:7b".to_string(),
            max_parallel_workers: 4,
            security_enabled: true,
            tdd_enforced: true,
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
        info!("Executing build request: {}", request.task);
        
        let health = self.sentinel.check_health(None).await;
        info!("Health check: {:?}", health.hardware_profile);
        
        let available_models = self.ollama.list_models().await?;
        let complexity = self.router.analyze_complexity(&request.task, &available_models).await?;
        info!("Complexity score: {} ({:?})", complexity.score, complexity.task_type);
        
        let context_prompt = self.build_system_context(&request).await?;
        self.context.add("system", &context_prompt).await?;
        
        let subtasks = self.router.split_into_subtasks(&request.task);
        
        if self.router.can_parallelize(&complexity) && subtasks.len() > 1 {
            info!("Running {} tasks in parallel", subtasks.len());
            let results = self.executor
                .execute_parallel(&request.task, subtasks, Some(&context_prompt))
                .await?;
            
            let merged = self.executor.merge_results(results).await?;
            
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
                tokens_generated: 0,
                duration_ms: 0,
                warnings: vec![],
            });
        }
        
        let response = self.executor
            .execute_single(&request.task, &complexity.suggested_model, Some(&context_prompt))
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
            health.hardware_profile.free_vram_mb,
            health.hardware_profile.optimal_context
        ));
        
        if let Some(skill) = self.skills.match_skill_to_task(&request.task).await {
            context.push_str(&format!("\nSkill context: {}\n", skill.prompts.system));
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
            model: "deepseek-coder-v2:16b".to_string(),
            prompt: correction_prompt,
            system: Some("You are a code debugging expert. Fix the error and return corrected code.".to_string()),
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
            model: "llama3.3:70b".to_string(),
            prompt: audit_prompt,
            system: Some("You are a senior code auditor. Review thoroughly and provide actionable feedback.".to_string()),
            temperature: Some(0.3),
            num_ctx: Some(32768),
            stream: false,
            ..Default::default()
        };
        
        self.ollama.generate(opts).await.map(|r| r.content)
    }

    pub async fn get_status(&self) -> Result<StatusReport> {
        let health = self.sentinel.check_health(None).await;
        let models = self.ollama.list_models().await?;
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

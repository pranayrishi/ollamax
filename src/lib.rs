pub mod agent;
pub mod cli;
pub mod codeblocks;
pub mod context;
pub mod executor;
pub mod graph;
pub mod hub;
pub mod instincts;
pub mod memory;
pub mod models;
pub mod monitoring;
pub mod orchestrator;
pub mod providers;
pub mod replay;
pub mod router;
pub mod rules;
pub mod security;
pub mod server;
pub mod skills;
pub mod tools;

pub use context::ContextManager;
pub use executor::ParallelExecutor;
pub use monitoring::VramSentinel;
pub use orchestrator::Orchestrator;
pub use providers::ollama::OllamaProvider;
pub use router::TaskRouter;
pub use security::SecurityGuard;
pub use skills::SkillsEngine;

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn init_tracing(log_level: &str) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub ollama_url: String,
    pub default_model: String,
    pub planning_model: String,
    pub execution_models: Vec<String>,
    pub max_context_tokens: usize,
    pub enable_parallel: bool,
    pub max_parallel_workers: usize,
    pub security_enabled: bool,
    pub tdd_enforced: bool,
    pub auto_unload_models: bool,
    pub min_free_vram_mb: usize,
}

impl Config {
    /// Load config from `$XDG_CONFIG_HOME/ollama-forge/config.yaml` (or platform
    /// equivalent). Returns `Config::default()` if the file does not exist —
    /// missing config is not an error, it's the first-run case.
    pub async fn load() -> Result<Self> {
        let Some(dir) = dirs::config_dir() else {
            return Ok(Self::default());
        };
        let path = dir.join("ollama-forge").join("config.yaml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))
    }
}

impl Default for Config {
    fn default() -> Self {
        // These three model defaults must agree with each other AND with
        // STARTER_FORGE_TOML in main.rs AND with OrchestratorConfig::default
        // in orchestrator/mod.rs. The qwen2.5-coder family is the canonical
        // ladder — see monitoring::suggest_model and the rationale there.
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            default_model: "qwen2.5-coder:7b".to_string(),
            planning_model: "qwen2.5-coder:7b".to_string(),
            execution_models: vec![
                "qwen2.5-coder:1.5b".to_string(),
                "qwen2.5-coder:7b".to_string(),
                "qwen2.5-coder:14b".to_string(),
            ],
            max_context_tokens: 16384,
            enable_parallel: true,
            max_parallel_workers: 4,
            security_enabled: true,
            // TDD enforcement isn't wired into the build path yet; defaulting
            // it on would mean enforcing a feature that doesn't exist.
            tdd_enforced: false,
            auto_unload_models: true,
            min_free_vram_mb: 2048,
        }
    }
}

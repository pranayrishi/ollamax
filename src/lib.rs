pub mod cli;
pub mod context;
pub mod executor;
pub mod monitoring;
pub mod orchestrator;
pub mod providers;
pub mod router;
pub mod security;
pub mod skills;

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

pub fn init() -> Result<Config> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("ollama-forge");

    std::fs::create_dir_all(&config_dir)?;

    let config_path = config_dir.join("config.yaml");
    let config = if config_path.exists() {
        serde_yaml::from_str(&std::fs::read_to_string(&config_path)?)?
    } else {
        Config::default()
    };

    Ok(config)
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
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            default_model: "llama3.2:3b".to_string(),
            planning_model: "qwen2.5-coder:7b".to_string(),
            execution_models: vec![
                "llama3.2:3b".to_string(),
                "deepseek-coder-v2:16b".to_string(),
                "llama3.3:70b".to_string(),
            ],
            max_context_tokens: 32768,
            enable_parallel: true,
            max_parallel_workers: 4,
            security_enabled: true,
            tdd_enforced: true,
            auto_unload_models: true,
            min_free_vram_mb: 2048,
        }
    }
}

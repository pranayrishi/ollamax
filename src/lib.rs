pub mod agent;
pub mod cli;
pub mod codeblocks;
pub mod context;
pub mod evals;
pub mod executor;
pub mod graph;
pub mod hub;
pub mod instincts;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod monitoring;
pub mod orchestrator;
pub mod plugins;
pub mod providers;
pub mod replay;
pub mod router;
pub mod rules;
pub mod scheduler;
pub mod security;
pub mod server;
pub mod skills;
pub mod team;
pub mod tools;

pub use context::ContextManager;
pub use executor::ParallelExecutor;
pub use monitoring::VramSentinel;
pub use orchestrator::Orchestrator;
pub use providers::ollama::{resolve_ollama_endpoint, OllamaProvider, DEFAULT_OLLAMA_ENDPOINT};
pub use router::TaskRouter;
pub use security::SecurityGuard;
pub use skills::SkillsEngine;

use anyhow::Result;
use std::path::Path;
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
    /// Load configuration in precedence order:
    ///
    /// 1. built-in defaults;
    /// 2. the existing global YAML file at
    ///    `$XDG_CONFIG_HOME/ollama-forge/config.yaml` (when present);
    /// 3. a project-local `forge.toml` in the current directory (when present).
    ///
    /// `forge init` creates the third form. Its nested TOML sections are
    /// intentionally applied as overrides so a concise project file does not
    /// discard settings a user already has in their global YAML config.
    pub async fn load() -> Result<Self> {
        let mut config = match dirs::config_dir() {
            Some(dir) => {
                let path = dir.join("ollama-forge").join("config.yaml");
                if path.exists() {
                    let content = tokio::fs::read_to_string(&path)
                        .await
                        .map_err(|e| anyhow::anyhow!("reading {}: {e:#}", path.display()))?;
                    Self::from_yaml_str(&content)
                        .map_err(|e| anyhow::anyhow!("parsing {}: {e:#}", path.display()))?
                } else {
                    Self::default()
                }
            }
            None => Self::default(),
        };
        let project_path = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("resolve current directory for forge.toml: {e:#}"))?
            .join("forge.toml");
        if project_path.is_file() {
            let content = tokio::fs::read_to_string(&project_path)
                .await
                .map_err(|e| anyhow::anyhow!("reading {}: {e:#}", project_path.display()))?;
            config
                .apply_forge_toml(&content)
                .map_err(|e| anyhow::anyhow!("parsing {}: {e:#}", project_path.display()))?;
        }
        config.normalize_ollama_url()?;
        Ok(config)
    }

    /// Load one explicit config file for `forge --config <path>`. YAML retains
    /// its original flat `Config` shape; `.toml` files use the documented
    /// project shape written by `forge init`.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e:#}", path.display()))?;
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());
        let mut config = match extension.as_deref() {
            Some("toml") => Self::from_forge_toml_str(&content)
                .map_err(|e| anyhow::anyhow!("parsing {} as TOML: {e:#}", path.display()))?,
            Some("yaml") | Some("yml") => Self::from_yaml_str(&content)
                .map_err(|e| anyhow::anyhow!("parsing {} as YAML: {e:#}", path.display()))?,
            _ => match Self::from_yaml_str(&content) {
                Ok(config) => config,
                Err(yaml_error) => Self::from_forge_toml_str(&content).map_err(|toml_error| {
                    anyhow::anyhow!(
                        "parsing {} as YAML or TOML failed (YAML: {yaml_error:#}; TOML: {toml_error:#})",
                        path.display()
                    )
                })?,
            },
        };
        config.normalize_ollama_url()?;
        Ok(config)
    }

    fn from_yaml_str(content: &str) -> Result<Self> {
        serde_yaml::from_str(content).map_err(Into::into)
    }

    fn from_forge_toml_str(content: &str) -> Result<Self> {
        let mut config = Self::default();
        config.apply_forge_toml(content)?;
        Ok(config)
    }

    fn apply_forge_toml(&mut self, content: &str) -> Result<()> {
        let project: ForgeToml = toml::from_str(content)?;

        if let Some(url) = project.ollama.url {
            self.ollama_url = url;
        }
        if let Some(model) = project.ollama.default_model {
            self.default_model = model;
        }
        if let Some(model) = project.ollama.planning_model {
            self.planning_model = model;
        }
        if let Some(models) = project.ollama.execution_models {
            self.execution_models = models;
        }
        if let Some(enabled) = project.execution.enable_parallel {
            self.enable_parallel = enabled;
        }
        if let Some(workers) = project.execution.parallel_workers {
            self.max_parallel_workers = workers;
        }
        if let Some(tokens) = project.execution.max_context_tokens {
            self.max_context_tokens = tokens;
        }
        if let Some(enabled) = project.security.enabled {
            self.security_enabled = enabled;
        }
        if let Some(enforced) = project.tdd.enforced {
            self.tdd_enforced = enforced;
        }
        if let Some(unload) = project.optimization.auto_unload_models {
            self.auto_unload_models = unload;
        }
        if let Some(vram) = project.optimization.min_free_vram_mb {
            self.min_free_vram_mb = vram;
        }
        Ok(())
    }

    fn normalize_ollama_url(&mut self) -> Result<()> {
        self.ollama_url = resolve_ollama_endpoint(&self.ollama_url)
            .map_err(|e| anyhow::anyhow!("resolving configured Ollama endpoint: {e:#}"))?;
        Ok(())
    }
}

/// Shape of the project-local `forge.toml` written by `forge init`. Fields are
/// optional so a project can override only the settings it owns while inheriting
/// global YAML settings and built-in defaults.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeToml {
    ollama: ForgeTomlOllama,
    execution: ForgeTomlExecution,
    security: ForgeTomlSecurity,
    tdd: ForgeTomlTdd,
    optimization: ForgeTomlOptimization,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeTomlOllama {
    url: Option<String>,
    default_model: Option<String>,
    planning_model: Option<String>,
    execution_models: Option<Vec<String>>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeTomlExecution {
    enable_parallel: Option<bool>,
    parallel_workers: Option<usize>,
    max_context_tokens: Option<usize>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeTomlSecurity {
    enabled: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeTomlTdd {
    enforced: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ForgeTomlOptimization {
    auto_unload_models: Option<bool>,
    min_free_vram_mb: Option<usize>,
}

impl Default for Config {
    fn default() -> Self {
        // These three model defaults must agree with each other AND with
        // STARTER_FORGE_TOML in main.rs AND with OrchestratorConfig::default
        // in orchestrator/mod.rs. The qwen3.5/qwen3.6 pair is the canonical
        // July-2026 ladder — see monitoring::suggest_model and the rationale
        // there (qwen3.5:9b runs on ~8 GB machines; qwen3.6:27b is the
        // planning-grade coder for 24 GB machines, and every selection path
        // prefers an *installed* model over these fallbacks).
        Self {
            ollama_url: DEFAULT_OLLAMA_ENDPOINT.to_string(),
            default_model: "qwen3.5:9b".to_string(),
            planning_model: "qwen3.6:27b".to_string(),
            execution_models: vec![
                "qwen3.5:2b".to_string(),
                "qwen3.5:9b".to_string(),
                "qwen3.6:27b".to_string(),
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

#[cfg(test)]
mod tests {
    use super::{Config, DEFAULT_OLLAMA_ENDPOINT};

    #[test]
    fn config_default_uses_ipv4_loopback_for_ollama() {
        assert_eq!(Config::default().ollama_url, DEFAULT_OLLAMA_ENDPOINT);
    }

    #[test]
    fn documented_forge_toml_loads_through_the_explicit_config_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.toml");
        std::fs::write(
            &path,
            r#"
[forge]
version = "1.0"

[ollama]
url = "http://127.0.0.1:11555"
default_model = "llama3.2:3b"
planning_model = "qwen2.5-coder:14b"
execution_models = ["llama3.2:3b", "qwen2.5-coder:14b"]

[execution]
enable_parallel = false
parallel_workers = 2
max_context_tokens = 32768

[security]
enabled = false

[tdd]
enforced = true

[optimization]
auto_unload_models = false
min_free_vram_mb = 4096
"#,
        )
        .unwrap();

        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config.ollama_url, "http://127.0.0.1:11555");
        assert_eq!(config.default_model, "llama3.2:3b");
        assert_eq!(config.planning_model, "qwen2.5-coder:14b");
        assert_eq!(
            config.execution_models,
            vec!["llama3.2:3b", "qwen2.5-coder:14b"]
        );
        assert!(!config.enable_parallel);
        assert_eq!(config.max_parallel_workers, 2);
        assert_eq!(config.max_context_tokens, 32768);
        assert!(!config.security_enabled);
        assert!(config.tdd_enforced);
        assert!(!config.auto_unload_models);
        assert_eq!(config.min_free_vram_mb, 4096);
    }

    #[test]
    fn project_toml_overrides_only_its_declared_values() {
        let mut config = Config {
            default_model: "global-model".to_string(),
            max_parallel_workers: 9,
            ..Default::default()
        };

        config
            .apply_forge_toml(
                r#"
[ollama]
url = "http://127.0.0.1:11556"

[execution]
max_context_tokens = 24576
"#,
            )
            .unwrap();

        assert_eq!(config.ollama_url, "http://127.0.0.1:11556");
        assert_eq!(config.default_model, "global-model");
        assert_eq!(config.max_parallel_workers, 9);
        assert_eq!(config.max_context_tokens, 24576);
    }

    #[test]
    fn yaml_config_format_remains_supported() {
        let config = Config::from_yaml_str(
            r#"
ollama_url: http://127.0.0.1:11557
default_model: yaml-model
max_parallel_workers: 3
"#,
        )
        .unwrap();

        assert_eq!(config.ollama_url, "http://127.0.0.1:11557");
        assert_eq!(config.default_model, "yaml-model");
        assert_eq!(config.max_parallel_workers, 3);
    }
}

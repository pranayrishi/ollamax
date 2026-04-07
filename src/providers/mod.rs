use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub mod ollama;

pub use ollama::OllamaProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub model: String,
    pub tokens_generated: usize,
    pub context_used: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct GenerateOptions {
    pub model: String,
    pub prompt: String,
    pub system: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub num_ctx: Option<usize>,
    pub num_gpu: Option<i32>,
    pub main_gpu: Option<i32>,
    pub repeat_penalty: Option<f32>,
    pub stop: Option<Vec<String>>,
    pub stream: bool,
    /// e.g. "30m", "1h", "0" — passed straight to Ollama's `keep_alive`.
    /// `None` lets Ollama use its server-side default (5m as of v0.1.x).
    pub keep_alive: Option<String>,
    /// Ollama `format` parameter (v0.5+).
    /// - `Some(json!("json"))` → free-form valid JSON
    /// - `Some(json!({...}))`  → strict JSON Schema (constrained decoding)
    /// - `None`                → unconstrained text
    ///
    /// This is the local-LLM equivalent of OpenAI's `response_format` and
    /// the closest thing the harness has to "guaranteed-valid tool calls"
    /// without dropping into raw GBNF grammars. ECC has nothing equivalent
    /// for hosted Claude.
    pub format: Option<serde_json::Value>,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            model: "llama3.2:3b".to_string(),
            prompt: String::new(),
            system: None,
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            num_ctx: Some(4096),
            num_gpu: None,
            main_gpu: None,
            repeat_penalty: Some(1.1),
            stop: None,
            stream: false,
            keep_alive: Some("30m".to_string()),
            format: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub num_ctx: Option<usize>,
    pub stream: bool,
}

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn generate(&self, options: GenerateOptions) -> Result<LlmResponse>;
    async fn chat(&self, options: ChatOptions) -> Result<LlmResponse>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    /// Best-effort warm-load: tells Ollama to keep this model resident.
    /// Default impl is a no-op so non-Ollama providers don't have to care.
    async fn preload(&self, _model: &str, _keep_alive: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub size: u64,
    pub size_human: String,
    pub modified_at: String,
    pub digest: String,
}

/// One model currently loaded by Ollama into RAM/VRAM. Surfaced by
/// `OllamaProvider::running_models` and rendered by `forge status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningModel {
    pub name: String,
    pub size_vram_bytes: u64,
    pub expires_at: Option<String>,
}

#[derive(Clone)]
pub struct ProviderPool {
    providers: Arc<RwLock<HashMap<String, Arc<dyn LlmProvider>>>>,
}

impl std::fmt::Debug for ProviderPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderPool").finish_non_exhaustive()
    }
}

impl ProviderPool {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, name: String, provider: Arc<dyn LlmProvider>) {
        let mut providers = self.providers.write().await;
        info!("Registered LLM provider: {}", name);
        providers.insert(name, provider);
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn LlmProvider>> {
        let providers = self.providers.read().await;
        providers.get(name).cloned()
    }

    pub async fn default(&self) -> Option<Arc<dyn LlmProvider>> {
        let providers = self.providers.read().await;
        providers.values().next().cloned()
    }

    pub async fn list_providers(&self) -> Vec<String> {
        let providers = self.providers.read().await;
        providers.keys().cloned().collect()
    }
}

impl Default for ProviderPool {
    fn default() -> Self {
        Self::new()
    }
}

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

pub struct ContextManager {
    max_tokens: usize,
    sliding_window: bool,
    history: Arc<RwLock<VecDeque<ContextEntry>>>,
    token_counts: Arc<RwLock<HashMap<String, usize>>>,
}

#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub id: String,
    pub role: String,
    pub content: String,
    pub tokens: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct TruncatedContext {
    pub entries: Vec<ContextEntry>,
    pub total_tokens: usize,
    pub truncated_count: usize,
}

impl ContextManager {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            sliding_window: true,
            history: Arc::new(RwLock::new(VecDeque::new())),
            token_counts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_sliding_window(mut self, enabled: bool) -> Self {
        self.sliding_window = enabled;
        self
    }

    pub async fn add(&self, role: &str, content: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let tokens = self.count_tokens(content);
        
        let entry = ContextEntry {
            id: id.clone(),
            role: role.to_string(),
            content: content.to_string(),
            tokens,
            timestamp: chrono::Utc::now(),
        };

        let mut history = self.history.write().await;
        history.push_back(entry);

        if self.sliding_window {
            self.trim_to_max(&mut history).await;
        }

        let mut counts = self.token_counts.write().await;
        counts.insert(id.clone(), tokens);

        debug!("Added context entry {} ({} tokens)", id, tokens);
        Ok(id)
    }

    async fn trim_to_max(&self, history: &mut VecDeque<ContextEntry>) {
        let mut total: usize = history.iter().map(|e| e.tokens).sum();
        
        while total > self.max_tokens && history.len() > 1 {
            if let Some(removed) = history.pop_front() {
                total = total.saturating_sub(removed.tokens);
                debug!("Trimmed context entry {} (freed {} tokens)", removed.id, removed.tokens);
            }
        }
    }

    pub async fn get_context(&self, system_prompt: Option<&str>) -> Result<String> {
        let history = self.history.read().await;
        let mut context = String::new();

        if let Some(system) = system_prompt {
            context.push_str(&format!("[System]\n{}\n\n", system));
        }

        for entry in history.iter() {
            context.push_str(&format!("[{}]\n{}\n\n", entry.role, entry.content));
        }

        Ok(context)
    }

    pub async fn get_truncated_context(&self, max_tokens: Option<usize>) -> Result<TruncatedContext> {
        let limit = max_tokens.unwrap_or(self.max_tokens);
        let history = self.history.read().await;
        
        let mut entries = Vec::new();
        let mut total = 0;
        let mut truncated_count = 0;

        for entry in history.iter().rev() {
            if total + entry.tokens <= limit {
                entries.insert(0, entry.clone());
                total += entry.tokens;
            } else {
                truncated_count += 1;
            }
        }

        Ok(TruncatedContext {
            entries,
            total_tokens: total,
            truncated_count,
        })
    }

    pub async fn clear(&self) {
        let mut history = self.history.write().await;
        history.clear();
        
        let mut counts = self.token_counts.write().await;
        counts.clear();
        
        debug!("Context cleared");
    }

    pub async fn stats(&self) -> ContextStats {
        let history = self.history.read().await;
        let total_tokens: usize = history.iter().map(|e| e.tokens).sum();
        let entry_count = history.len();

        ContextStats {
            entry_count,
            total_tokens,
            max_tokens: self.max_tokens,
            utilization_percent: if self.max_tokens > 0 {
                (total_tokens as f64 / self.max_tokens as f64 * 100.0).min(100.0)
            } else {
                0.0
            },
        }
    }

    fn count_tokens(&self, text: &str) -> usize {
        text.split_whitespace().count().max(1)
    }
}

#[derive(Debug, Clone)]
pub struct ContextStats {
    pub entry_count: usize,
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub utilization_percent: f64,
}

pub struct ModelfileGenerator;

impl ModelfileGenerator {
    pub fn generate_for_context_size(model: &str, context_size: usize) -> String {
        format!(
            r#"FROM {}
PARAMETER num_ctx {}
PARAMETER temperature 0.7
PARAMETER top_p 0.9
PARAMETER repeat_penalty 1.1
"#,
            model, context_size
        )
    }

    pub fn generate_optimized(model: &str, vram_gb: f32, target_tps: usize) -> String {
        let num_ctx = Self::calculate_optimal_context(vram_gb);
        let num_gpu = Self::calculate_gpu_layers(vram_gb);
        
        let _ = target_tps; // reserved for future tps-aware tuning
        format!(
            r##"FROM {model}
PARAMETER num_ctx {num_ctx}
PARAMETER num_gpu {num_gpu}
PARAMETER temperature 0.7
PARAMETER top_p 0.9
PARAMETER top_k 40
PARAMETER repeat_penalty 1.1
PARAMETER presence_penalty 0.0
PARAMETER frequency_penalty 0.0
SYSTEM """
You are an expert coding assistant optimized for {ctx_name} context.
Provide concise, efficient solutions.
"""
"##,
            model = model,
            num_ctx = num_ctx,
            num_gpu = num_gpu,
            ctx_name = context_size_to_name(num_ctx),
        )
    }

    fn calculate_optimal_context(vram_gb: f32) -> usize {
        match vram_gb {
            v if v >= 48.0 => 131072,
            v if v >= 32.0 => 65536,
            v if v >= 24.0 => 32768,
            v if v >= 16.0 => 16384,
            v if v >= 12.0 => 8192,
            _ => 4096,
        }
    }

    fn calculate_gpu_layers(vram_gb: f32) -> i32 {
        match vram_gb {
            v if v >= 32.0 => -1,
            v if v >= 24.0 => 100,
            v if v >= 16.0 => 75,
            v if v >= 12.0 => 50,
            v if v >= 8.0 => 33,
            _ => 0,
        }
    }
}

fn context_size_to_name(tokens: usize) -> &'static str {
    match tokens {
        131072 => "128k",
        65536 => "64k",
        32768 => "32k",
        16384 => "16k",
        8192 => "8k",
        _ => "4k",
    }
}

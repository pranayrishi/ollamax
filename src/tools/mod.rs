//! Tool layer for the agent loop.
//!
//! A `Tool` is something the model can call by name with a JSON-shaped
//! argument blob. Tools are the bridge between forge's "model talks to
//! itself" world and the outside world (web search, URL fetching, file
//! reads, etc.).
//!
//! ## Free-only constraint
//!
//! Every bundled tool here hits a *free, no-API-key* endpoint:
//! - `web_search` → DuckDuckGo Instant Answer JSON API
//! - `wikipedia`  → Wikipedia REST `summary`/`search` endpoints
//! - `arxiv`      → arXiv Atom API
//! - `fetch_url`  → plain HTTP GET, no service in front
//!
//! No OpenAI, no Google CSE, no SerpAPI, no Brave API, no Tavily, no Bing.
//! The model itself is also free (Ollama). The whole agent path is
//! zero-cost-per-call.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub mod arxiv;
pub mod fetch_url;
pub mod files;
pub mod shell;
pub mod web_search;
pub mod wikipedia;

/// Result of a tool invocation, returned to the model as the next message.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool: String,
    pub ok: bool,
    /// Plain-text content for the model. Already truncated to a reasonable
    /// size by the tool itself — never let a tool dump 10 MB of HTML at the
    /// model.
    pub content: String,
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier the model uses to invoke the tool.
    fn name(&self) -> &str;

    /// One-line description shown to the model in the system prompt. This
    /// is what the model uses to decide *which* tool to call. Be terse and
    /// describe inputs concretely.
    fn description(&self) -> &str;

    /// JSON schema for the tool's `args` object. The agent loop hands this
    /// to Ollama via the `format` parameter so the model is forced to emit
    /// a parseable arg blob.
    fn args_schema(&self) -> Value;

    /// Execute the tool against `args`. Errors are returned as
    /// `ToolResult { ok: false, content: "<error>" }` so the model can
    /// recover instead of crashing the agent loop.
    async fn invoke(&self, args: Value) -> Result<ToolResult>;
}

/// Minimum interval between two invocations of the same tool. Prevents
/// the agent loop from hammering DDG/Wikipedia/arXiv into a temporary IP
/// ban when a model gets stuck calling the same tool 5 times in a row.
/// 250 ms is generous enough to be invisible to humans, tight enough to
/// not actually slow research down on a well-behaved model.
pub const MIN_TOOL_INTERVAL_MS: u64 = 250;

/// Map of name → tool. Cheap to clone (Arc-wrapped).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Per-tool last-invocation timestamp. Behind a tokio Mutex so the
    /// agent loop can be on multiple threads without races.
    last_invoked: Arc<Mutex<HashMap<String, Instant>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the default registry with the four bundled free tools.
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(web_search::WebSearchTool::new()));
        r.register(Arc::new(wikipedia::WikipediaTool::new()));
        r.register(Arc::new(arxiv::ArxivTool::new()));
        r.register(Arc::new(fetch_url::FetchUrlTool::new()));
        r
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Get a tool and enforce the per-tool rate limit. If the same tool was
    /// invoked less than `MIN_TOOL_INTERVAL_MS` ago, this sleeps the
    /// difference before returning. The agent loop calls this instead of
    /// `get()` so polite-but-stuck behavior is automatic.
    pub async fn get_rate_limited(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let tool = self.tools.get(name).cloned()?;
        let mut last = self.last_invoked.lock().await;
        let now = Instant::now();
        let min = Duration::from_millis(MIN_TOOL_INTERVAL_MS);
        if let Some(prev) = last.get(name) {
            let since = now.duration_since(*prev);
            if since < min {
                let wait = min - since;
                drop(last);
                tokio::time::sleep(wait).await;
                last = self.last_invoked.lock().await;
            }
        }
        last.insert(name.to_string(), Instant::now());
        Some(tool)
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Render the tool catalog as a chunk of system-prompt text the model
    /// can read. Each tool gets its name, description, and arg schema.
    pub fn describe_for_model(&self) -> String {
        let mut buf = String::new();
        buf.push_str("You have access to the following tools:\n\n");
        let mut tools: Vec<&Arc<dyn Tool>> = self.tools.values().collect();
        tools.sort_by_key(|t| t.name().to_string());
        for tool in tools {
            buf.push_str(&format!(
                "- `{}`: {}\n  args schema: {}\n",
                tool.name(),
                tool.description(),
                tool.args_schema()
            ));
        }
        buf
    }
}

/// Cap any tool output at this many bytes before handing it to the model.
/// Tools should respect this themselves; this constant is the contract.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 8_000;

/// Truncate `text` to roughly `MAX_TOOL_OUTPUT_BYTES` characters with a
/// trailing marker the model can recognize. Used by every bundled tool.
pub fn truncate_for_model(text: &str) -> String {
    if text.len() <= MAX_TOOL_OUTPUT_BYTES {
        return text.to_string();
    }
    let mut out = text.chars().take(MAX_TOOL_OUTPUT_BYTES).collect::<String>();
    out.push_str("\n\n[... truncated by forge: tool output exceeded budget]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_describes_zero_tools_cleanly() {
        let r = ToolRegistry::new();
        assert!(r.is_empty());
        let s = r.describe_for_model();
        assert!(s.contains("You have access"));
    }

    #[test]
    fn defaults_register_four_tools() {
        let r = ToolRegistry::with_defaults();
        let names = r.names();
        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&"wikipedia".to_string()));
        assert!(names.contains(&"arxiv".to_string()));
        assert!(names.contains(&"fetch_url".to_string()));
    }

    #[test]
    fn truncate_passthroughs_short_inputs() {
        assert_eq!(truncate_for_model("hello"), "hello");
    }

    #[test]
    fn truncate_marks_long_inputs() {
        let big = "a".repeat(MAX_TOOL_OUTPUT_BYTES + 100);
        let out = truncate_for_model(&big);
        assert!(out.contains("[... truncated by forge"));
        assert!(out.len() < big.len() + 200);
    }
}

//! Sub-agent delegation tool (Hermes-class `delegate_task`).
//!
//! `delegate` lets the main agent hand a focused sub-task to an **isolated,
//! short-lived child agent** with its own fresh context and a **restricted
//! toolset**. The child runs its own loop and returns only its final answer —
//! the main agent's context isn't polluted with the child's intermediate steps.
//!
//! Recursion is prevented structurally: the child registry passed in here must
//! NOT contain a `delegate` tool, and the child runs with a smaller iteration
//! budget. The UI shows children in a dedicated "Sub-agents" lane.

use super::{truncate_for_model, Tool, ToolRegistry, ToolResult};
use crate::agent::{Agent, AgentConfig};
use crate::providers::ollama::OllamaProvider;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct DelegateTool {
    provider: Arc<OllamaProvider>,
    model: String,
    num_ctx: usize,
    max_iterations: usize,
    /// The toolset the CHILD gets — must exclude `delegate` to prevent unbounded
    /// recursion. Build it from the read/research tools only.
    child_tools: ToolRegistry,
    /// Optional event sink (the SSE channel) so the child's steps stream to the
    /// UI's dedicated Sub-agents lane instead of being discarded.
    events: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
}

impl DelegateTool {
    pub fn new(
        provider: Arc<OllamaProvider>,
        model: impl Into<String>,
        num_ctx: usize,
        child_tools: ToolRegistry,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            num_ctx,
            max_iterations: 6,
            child_tools,
            events: None,
        }
    }

    /// Stream the child's lifecycle (`subagent_start`/`subagent_step`/
    /// `subagent_end`) to this sink so the UI can render a Sub-agents lane.
    pub fn with_events(mut self, tx: tokio::sync::mpsc::UnboundedSender<Value>) -> Self {
        self.events = Some(tx);
        self
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }
    fn description(&self) -> &str {
        "Delegate a focused sub-task to an isolated sub-agent with its own fresh context and a restricted, read-only toolset. Use for self-contained research/lookups so your own context stays clean. args: {task: string}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"task":{"type":"string"}},"required":["task"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("").trim();
        if task.is_empty() {
            return Ok(ToolResult { tool: "delegate".into(), ok: false, content: "task is empty".into() });
        }
        let mut child = Agent::new(
            self.provider.clone(),
            self.child_tools.clone(),
            AgentConfig {
                model: self.model.clone(),
                num_ctx: self.num_ctx,
                keep_alive: "1h".to_string(),
                max_iterations: self.max_iterations,
                system_suffix:
                    "You are a focused sub-agent. Complete ONLY the given task and report the result concisely. Do not ask follow-up questions."
                        .to_string(),
            },
        );
        // Stream the child's lifecycle to the Sub-agents lane (only its final
        // answer still crosses back into the parent's CONTEXT).
        let label: String = task.chars().take(80).collect();
        if let Some(ev) = &self.events {
            let _ = ev.send(json!({ "type": "subagent_start", "task": label }));
        }
        let events = self.events.clone();
        let result = child
            .run(task, move |step| {
                if let Some(ev) = &events {
                    let preview: String =
                        step.result_preview.replace('\n', " ").chars().take(200).collect();
                    let _ = ev.send(json!({
                        "type": "subagent_step",
                        "iteration": step.iteration,
                        "tool": step.tool,
                        "ok": step.ok,
                        "preview": preview,
                    }));
                }
            })
            .await;
        let out = match result {
            Ok(trace) => ToolResult {
                tool: "delegate".into(),
                ok: true,
                content: truncate_for_model(&trace.answer),
            },
            Err(e) => ToolResult {
                tool: "delegate".into(),
                ok: false,
                content: format!("sub-agent failed: {e}"),
            },
        };
        if let Some(ev) = &self.events {
            let _ = ev.send(json!({ "type": "subagent_end", "ok": out.ok }));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_tool_shape() {
        // Construction is network-free; invoke would need a live model, so we
        // only assert the tool's contract here.
        let provider = Arc::new(OllamaProvider::new("http://localhost:11434"));
        let t = DelegateTool::new(provider, "qwen2.5-coder:7b", 8192, ToolRegistry::with_defaults());
        assert_eq!(t.name(), "delegate");
        assert!(t.description().contains("sub-agent"));
        assert_eq!(t.args_schema()["required"][0], "task");
    }
}

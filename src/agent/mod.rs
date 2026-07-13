//! Tool-calling agent loop.
//!
//! Wraps an `OllamaProvider` and a `ToolRegistry`. Each iteration:
//!
//! 1. Builds a chat-style prompt: system prompt + tool catalog + the
//!    accumulated transcript.
//! 2. Calls Ollama with `format=<json schema>` so the model is *forced* to
//!    emit one of two shapes:
//!    - `{"action": "use_tool", "tool": "<name>", "args": { ... }}`
//!    - `{"action": "answer", "text": "..."}`
//! 3. If `use_tool`, looks the tool up, invokes it, appends the result to
//!    the transcript, and loops.
//! 4. If `answer`, returns it as the final response.
//!
//! Hard cap of `MAX_ITERATIONS` so a confused model can't run forever.
//!
//! Why JSON schema not free-form parsing: a 7B local model will produce
//! malformed `<tool>web_search</tool>` style markup roughly 10% of the
//! time. Schema-constrained decoding (Ollama's `format` parameter) makes
//! the loop reliable on small models, which is the entire point.

use crate::providers::{GenerateOptions, LlmProvider, OllamaProvider};
use crate::replay::{quick_hash, ReplayLog, ReplayRecord};
use crate::tools::{ToolRegistry, ToolResult};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Default cap for a lightweight research-style run. Override at the call site
/// via `AgentConfig::max_iterations`.
pub const DEFAULT_MAX_ITERATIONS: usize = 6;

/// A workspace coding task commonly needs to inventory the repository, search,
/// read one or more files, edit, and validate. Six turns is too short for that
/// sequence, so interactive Agent surfaces use this larger bounded budget.
pub const DEFAULT_CODING_MAX_ITERATIONS: usize = 12;

pub struct Agent {
    provider: Arc<OllamaProvider>,
    tools: ToolRegistry,
    model: String,
    num_ctx: usize,
    keep_alive: String,
    max_iterations: usize,
    system_suffix: String,
    /// Cached `/api/show` digest for `model`. Populated once on first
    /// `run()` so we don't pay the round-trip on every iteration.
    cached_digest: String,
    /// Optional approval gate consulted before a *consequential* tool runs
    /// (the Autonomy Dial). `None` = approve everything (default behavior).
    approval: Option<Arc<dyn ApprovalPolicy>>,
    /// When true (and an approval policy is set), run an Intent-Preview planning
    /// pass before the loop and gate it through `approve_plan`.
    plan: bool,
}

/// Decision returned by an [`ApprovalPolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Deny,
}

/// Consulted before a consequential tool runs so a UI can require human
/// approval (Hermes-class step-level intervention). Implementors decide based on
/// the Autonomy Dial mode.
#[async_trait::async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn approve(&self, tool: &str, args: &Value) -> Approval;
    /// Consulted once, before the loop, with the agent's proposed plan (Intent
    /// Preview). Default allows — only a UI that wants a plan gate overrides it.
    async fn approve_plan(&self, _plan: &str) -> Approval {
        Approval::Allow
    }
    /// Whether this caller requires an intent-preview plan to be generated and
    /// approved before a mutating run begins. Keeping this explicit lets
    /// automatic and read-only callers avoid an unnecessary planning request,
    /// while confirm-mode callers fail closed if the preview cannot be made.
    fn requires_plan_approval(&self) -> bool {
        false
    }
}

/// Explicit opt-in policy for embedding/tests that intentionally allow every
/// action. Team workflows require an [`ApprovalPolicy`] rather than treating a
/// missing policy as implicit permission, so callers must choose this type
/// deliberately when they want unattended execution.
#[derive(Debug, Default)]
pub struct AllowAllApproval;

#[async_trait::async_trait]
impl ApprovalPolicy for AllowAllApproval {
    async fn approve(&self, _tool: &str, _args: &Value) -> Approval {
        Approval::Allow
    }
}

/// Tools that mutate the workspace or execute code — these are gated by the
/// Autonomy Dial in "confirm" mode. Read-only tools (research/graph/fs_read)
/// always stream through.
pub fn is_consequential(tool: &str) -> bool {
    matches!(tool, "fs_write" | "fs_edit" | "shell") || tool.starts_with("mcp__")
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub num_ctx: usize,
    pub keep_alive: String,
    pub max_iterations: usize,
    /// Suffix appended to the agent's system prompt. Used by the CLI to
    /// inject `~/.config/ollama-forge/rules/*.md` so user always-rules
    /// apply to the research agent the same way they apply to chat.
    pub system_suffix: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "qwen2.5-coder:7b".to_string(),
            num_ctx: 16_384,
            keep_alive: "1h".to_string(),
            max_iterations: DEFAULT_CODING_MAX_ITERATIONS,
            system_suffix: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentTrace {
    /// What the user asked.
    pub task: String,
    /// Sequence of (tool_name, args, result) for every tool call made.
    pub steps: Vec<AgentStep>,
    /// Final answer text.
    pub answer: String,
    /// True if the loop hit `MAX_ITERATIONS` before the model gave an answer.
    pub iteration_capped: bool,
    /// True when an approval policy rejected the intent-preview plan before
    /// any tool could be invoked. Callers can distinguish an explicit human
    /// stop from a completed no-op task.
    pub plan_declined: bool,
    /// Successful local generation calls, including an intent-preview plan
    /// when one was requested. This makes team/evaluation telemetry factual
    /// rather than estimating calls from tool steps.
    pub model_calls: u32,
    /// Sum of Ollama-reported generated tokens across successful calls.
    pub tokens_generated: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentStep {
    pub iteration: usize,
    pub tool: String,
    pub args: Value,
    pub ok: bool,
    pub result_preview: String,
}

impl Agent {
    pub fn new(provider: Arc<OllamaProvider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            provider,
            tools,
            model: config.model,
            num_ctx: config.num_ctx,
            keep_alive: config.keep_alive,
            max_iterations: config.max_iterations.max(1),
            system_suffix: config.system_suffix,
            cached_digest: String::new(),
            approval: None,
            plan: false,
        }
    }

    /// Attach an approval gate (the Autonomy Dial). Consulted before each
    /// consequential tool call; on `Deny` the tool is skipped, not executed.
    pub fn with_approval(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval = Some(policy);
        self
    }

    /// Enable the Intent Preview: a quick planning pass before the loop, gated
    /// through the approval policy's `approve_plan`. No-op without an approval.
    pub fn with_planning(mut self, on: bool) -> Self {
        self.plan = on;
        self
    }

    /// One short model call to produce a numbered plan for `task`. When a
    /// caller explicitly requires plan approval, an unavailable or empty plan
    /// is an error: execution must not bypass the promised approval gate.
    async fn generate_plan(&self, task: &str) -> Result<(String, u32, usize)> {
        let opts = GenerateOptions {
            model: self.model.clone(),
            prompt: format!(
                "Task: {task}\n\nList the concrete steps you will take, as a short numbered \
                 list (2-6 steps). Plain text only — no preamble, no code."
            ),
            system: Some("You are planning before acting. Be concise and concrete.".to_string()),
            temperature: Some(0.2),
            num_ctx: Some(self.num_ctx),
            stream: false,
            keep_alive: Some(self.keep_alive.clone()),
            ..Default::default()
        };
        let response = self
            .provider
            .generate(opts)
            .await
            .context("agent intent-preview plan generation")?;
        let plan = response.content.trim().to_string();
        if plan.is_empty() {
            anyhow::bail!("agent intent-preview plan generation returned an empty plan");
        }
        Ok((plan, 1, response.tokens_generated))
    }

    /// Run the agent loop. `on_step` is called for each tool invocation so
    /// the CLI can stream progress to stderr while the loop runs. The
    /// returned `AgentTrace` lets callers inspect what happened — useful
    /// for `forge research --trace` and for replay.
    pub async fn run<F>(&mut self, task: &str, mut on_step: F) -> Result<AgentTrace>
    where
        F: FnMut(&AgentStep),
    {
        // Cache the model digest once per run so the replay log records it
        // without paying an /api/show round-trip on every iteration.
        if std::env::var_os("FORGE_REPLAY_LOG").is_some() && self.cached_digest.is_empty() {
            self.cached_digest = self
                .provider
                .model_digest(&self.model)
                .await
                .unwrap_or_default();
        }
        let mut system_prompt = build_system_prompt(&self.tools);
        if !self.system_suffix.is_empty() {
            system_prompt.push_str(&self.system_suffix);
        }
        // We use `format: "json"` (the simpler form), not a strict schema.
        // Reason: a strict schema with `args: {type: object}` lets the
        // model emit `args: {}` because the empty object satisfies the
        // type. Llama 3.1 8B falls into that local minimum every time.
        // Free-form JSON + a strong system prompt + few-shot examples
        // gives the model enough freedom to actually populate args.
        // We then parse and validate by hand below.
        let format_param = serde_json::json!("json");

        let mut transcript = String::new();
        transcript.push_str(&format!("User task: {task}\n\n"));

        let mut trace = AgentTrace {
            task: task.to_string(),
            steps: Vec::new(),
            answer: String::new(),
            iteration_capped: false,
            plan_declined: false,
            model_calls: 0,
            tokens_generated: 0,
        };

        // Intent Preview: propose a plan up front and let the user gate it.
        if self.plan {
            let policy = self.approval.as_ref().ok_or_else(|| {
                anyhow!("agent intent-preview planning requires an approval policy")
            })?;
            let (plan, calls, tokens) = self.generate_plan(task).await?;
            trace.model_calls = trace.model_calls.saturating_add(calls);
            trace.tokens_generated = trace.tokens_generated.saturating_add(tokens);
            if policy.approve_plan(&plan).await == Approval::Deny {
                info!("agent: plan declined by user before execution");
                trace.plan_declined = true;
                trace.answer = format!(
                    "Plan declined — nothing was executed. The proposed plan was:\n\n{plan}"
                );
                return Ok(trace);
            }
        }

        let max = self.max_iterations;
        for iteration in 1..=max {
            debug!("agent iteration {iteration}/{max}");

            // Tell the model on the *last* turn that it must answer, no
            // more tool calls. Otherwise the loop just dies and the user
            // sees nothing.
            let suffix = if iteration == max {
                "\n\nThis is your final turn. You MUST emit an `answer` action now. \
                 Do not request another tool call."
            } else {
                ""
            };

            let prompt = format!("{transcript}{suffix}\n\nYour turn.");
            let opts = GenerateOptions {
                model: self.model.clone(),
                prompt,
                system: Some(system_prompt.clone()),
                temperature: Some(0.2),
                num_ctx: Some(self.num_ctx),
                stream: false,
                keep_alive: Some(self.keep_alive.clone()),
                format: Some(format_param.clone()),
                ..Default::default()
            };
            let resp = self
                .provider
                .generate(opts.clone())
                .await
                .context("agent: ollama call")?;
            trace.model_calls = trace.model_calls.saturating_add(1);
            trace.tokens_generated = trace.tokens_generated.saturating_add(resp.tokens_generated);
            let raw = resp.content.trim();
            debug!("agent raw response: {raw}");

            // Honor FORGE_REPLAY_LOG: every Ollama call the agent makes
            // gets recorded so the entire research session is replayable.
            // The agent path is exactly where deterministic replay matters
            // most — a regulated user re-running a research session needs
            // bit-identical reasoning.
            if let Ok(log_path) = std::env::var("FORGE_REPLAY_LOG") {
                let log = ReplayLog::new(PathBuf::from(log_path));
                let mut prompt_material = String::new();
                if let Some(s) = &opts.system {
                    prompt_material.push_str(s);
                    prompt_material.push('\n');
                }
                prompt_material.push_str(&opts.prompt);
                if let Some(f) = &opts.format {
                    prompt_material.push('\n');
                    prompt_material.push_str(&f.to_string());
                }
                let record = ReplayRecord {
                    ts: chrono::Utc::now().to_rfc3339(),
                    forge_version: crate::cli::VERSION.to_string(),
                    model: opts.model.clone(),
                    model_digest: self.cached_digest.clone(),
                    temperature: opts.temperature,
                    top_p: opts.top_p,
                    num_ctx: opts.num_ctx,
                    keep_alive: opts.keep_alive.clone(),
                    seed: opts.seed,
                    format: opts.format.clone(),
                    system: opts.system.clone(),
                    prompt: opts.prompt.clone(),
                    prompt_hash: quick_hash(prompt_material.as_bytes()),
                    response_hash: quick_hash(resp.content.as_bytes()),
                    response: resp.content.chars().take(16_384).collect(),
                };
                if let Err(e) = log.append(&record).await {
                    warn!("agent: failed to append replay record: {e}");
                }
            }

            // Recovery: if the model returned malformed JSON despite
            // `format: "json"`, try to extract the first balanced
            // `{...}` from the response. Small models occasionally emit
            // a leading sentence like "Here you go: {...}". Without this
            // the entire research session crashes on a single hiccup.
            let parsed: Value = match serde_json::from_str(raw) {
                Ok(v) => v,
                Err(_) => match extract_first_json_object(raw) {
                    Some(v) => {
                        warn!(
                            "agent: model returned non-pure JSON; recovered the first object from `{}`",
                            raw.chars().take(80).collect::<String>()
                        );
                        v
                    }
                    None => {
                        // Append a "your last response was malformed" hint
                        // and let the next iteration try again instead of
                        // killing the loop.
                        transcript.push_str(
                            "\n[round error] Your last response was not valid JSON. \
                             Re-emit your action as a single JSON object with the schema \
                             you were given. Do not add prose around it.\n",
                        );
                        warn!("agent: round {iteration} produced non-JSON; asking model to retry");
                        continue;
                    }
                },
            };

            let action = parsed
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("agent: response missing `action` field: {raw}"))?;

            match action {
                "answer" => {
                    let text = parsed
                        .get("text")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("agent: answer missing `text`"))?;
                    info!("agent: answered after {iteration} iteration(s)");
                    trace.answer = text.to_string();
                    return Ok(trace);
                }
                "use_tool" => {
                    let tool_name = parsed
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("agent: use_tool missing `tool`"))?
                        .to_string();
                    let args = parsed.get("args").cloned().unwrap_or_else(|| json!({}));

                    let Some(tool) = self.tools.get_rate_limited(&tool_name).await else {
                        warn!("agent: model asked for unknown tool `{tool_name}`");
                        let result = ToolResult {
                            tool: tool_name.clone(),
                            ok: false,
                            content: format!(
                                "ERROR: no tool named `{tool_name}`. Available: {:?}",
                                self.tools.names()
                            ),
                        };
                        record_step(
                            &mut trace,
                            &mut transcript,
                            iteration,
                            &tool_name,
                            &args,
                            &result,
                            &mut on_step,
                        );
                        continue;
                    };

                    // Autonomy Dial: gate consequential tools behind approval.
                    if is_consequential(&tool_name) {
                        if let Some(policy) = &self.approval {
                            if policy.approve(&tool_name, &args).await == Approval::Deny {
                                let result = ToolResult {
                                    tool: tool_name.clone(),
                                    ok: false,
                                    content: "DENIED by user — tool not executed.".to_string(),
                                };
                                record_step(
                                    &mut trace,
                                    &mut transcript,
                                    iteration,
                                    &tool_name,
                                    &args,
                                    &result,
                                    &mut on_step,
                                );
                                continue;
                            }
                        }
                    }
                    let result = match tool.invoke(args.clone()).await {
                        Ok(r) => r,
                        Err(e) => ToolResult {
                            tool: tool_name.clone(),
                            ok: false,
                            content: format!("ERROR: tool `{tool_name}` failed: {e}"),
                        },
                    };
                    record_step(
                        &mut trace,
                        &mut transcript,
                        iteration,
                        &tool_name,
                        &args,
                        &result,
                        &mut on_step,
                    );
                }
                other => {
                    return Err(anyhow!(
                        "agent: model emitted unknown action `{other}`. Schema should have prevented this."
                    ));
                }
            }
        }

        warn!("agent: hit max_iterations={max} without an answer");
        trace.iteration_capped = true;
        trace.answer = format!(
            "(agent loop hit cap of {max} iterations without producing an answer; \
             tools used: {:?})",
            trace
                .steps
                .iter()
                .map(|s| s.tool.as_str())
                .collect::<Vec<_>>()
        );
        Ok(trace)
    }
}

#[allow(clippy::too_many_arguments)]
fn record_step<F>(
    trace: &mut AgentTrace,
    transcript: &mut String,
    iteration: usize,
    tool_name: &str,
    args: &Value,
    result: &ToolResult,
    on_step: &mut F,
) where
    F: FnMut(&AgentStep),
{
    // Cap at 800 chars — generous enough for URLs + a sentence of context.
    // The CLI re-truncates to FORGE_TRACE_WIDTH (default 300) before
    // rendering; this only bounds memory in the trace struct itself.
    let preview: String = result.content.chars().take(800).collect();
    let step = AgentStep {
        iteration,
        tool: tool_name.to_string(),
        args: args.clone(),
        ok: result.ok,
        result_preview: preview,
    };
    on_step(&step);
    trace.steps.push(step);

    transcript.push_str(&format!(
        "\n[round {iteration}] You called tool `{tool_name}` with args:\n{args}\n\n"
    ));
    transcript.push_str(&format!(
        "[tool result, ok={}]\n{}\n",
        result.ok, result.content
    ));
}

fn build_system_prompt(tools: &ToolRegistry) -> String {
    let mut s = String::new();
    s.push_str(
        "You are Ollamax, a local-first coding and research agent. Solve the user's task by \
         inspecting the available tools and using them when they materially help. Make the \
         smallest correct change, verify it when a safe validation tool is available, and report \
         only work that actually completed. For research, cite source URLs in the final answer \
         when you used web tools.\n\n",
    );
    let has_workspace_tools = tools.get("fs_list").is_some();
    if has_workspace_tools {
        s.push_str(
            "For a code task, first orient yourself with fs_list/fs_search, then read the \
             relevant current files before changing them. When the user asks to change files, \
             use the filesystem tools instead of only printing a code sample.\n\n",
        );
    }
    s.push_str(&tools.describe_for_model());
    s.push('\n');
    s.push_str(
        "Each turn, you must emit a JSON object matching the schema you were given. \
         Either request a tool call (`action: use_tool`) or give the final answer \
         (`action: answer`). Do not emit prose outside the JSON object.\n\n",
    );
    // Few-shot examples — small models (7-8B) cannot infer arg shape from a
    // raw JSON Schema reliably. Only show examples for tools that are actually
    // registered: otherwise a local-only coding run may waste turns trying an
    // unavailable web tool.
    if tools.get("web_search").is_some() {
        s.push_str(
            "Examples of valid web-tool calls:\n\n\
             {\"action\":\"use_tool\",\"tool\":\"web_search\",\"args\":{\"query\":\"recent advances in transformer architectures\"}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"wikipedia\",\"args\":{\"title\":\"Quantum entanglement\"}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"wikipedia\",\"args\":{\"search\":\"swallow airspeed\"}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"arxiv\",\"args\":{\"query\":\"attention is all you need\",\"max_results\":3}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"fetch_url\",\"args\":{\"url\":\"https://en.wikipedia.org/wiki/Barn_swallow\"}}\n\n",
        );
    }
    if has_workspace_tools {
        s.push_str(
            "Workspace-tool examples:\n\n\
             {\"action\":\"use_tool\",\"tool\":\"fs_list\",\"args\":{\"path\":\"\",\"depth\":2}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"fs_search\",\"args\":{\"query\":\"handle_login\",\"path\":\"src\"}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"fs_read\",\"args\":{\"path\":\"src/auth.rs\"}}\n\n\
             {\"action\":\"use_tool\",\"tool\":\"fs_edit\",\"args\":{\"path\":\"src/auth.rs\",\"old_string\":\"let enabled = false;\",\"new_string\":\"let enabled = true;\"}}\n\n",
        );
    }
    s.push_str(
        "Example of a final answer:\n\n\
         {\"action\":\"answer\",\"text\":\"Updated src/auth.rs and verified the requested condition.\"}\n\n",
    );
    s.push_str(
        "When you have enough information, stop calling tools and emit `action: answer` \
         with the full answer in the `text` field. Always populate the `args` object \
         with the fields the tool needs — never send an empty `args: {}`.",
    );
    s
}

/// Try to extract the first balanced `{...}` JSON object from `s`.
///
/// Used as a recovery path when a model emits something like
/// `Here is the answer: {"action":"answer","text":"hi"}` despite us asking
/// for pure JSON. We don't try to handle nested arrays or strings
/// containing braces in any sophisticated way — we count `{` and `}`
/// while skipping over double-quoted strings, which is enough for the
/// shapes the agent loop produces.
pub(crate) fn extract_first_json_object(s: &str) -> Option<Value> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &s[start..=i];
                    return serde_json::from_str::<Value>(candidate).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// **Currently unused** but kept for the planned schema-mode flag. The
/// strict-schema path lets the model emit `args: {}` because the empty
/// object satisfies `type: object`. Real fix is per-tool oneOf, which
/// requires a richer schema language than Ollama's `format` parameter
/// supports today. We use `format: "json"` (free-form valid JSON) instead
/// and rely on the few-shot system prompt + manual validation.
#[allow(dead_code)]
fn build_response_schema(tools: &ToolRegistry) -> Value {
    let tool_names: Vec<Value> = tools.names().into_iter().map(Value::String).collect();
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["use_tool", "answer"] },
            "tool":   { "type": "string", "enum": tool_names },
            "args":   { "type": "object" },
            "text":   { "type": "string" }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_object_from_prefix() {
        let s = r#"Sure, here you go: {"action":"answer","text":"42"}"#;
        let v = extract_first_json_object(s).expect("should recover");
        assert_eq!(v["action"], "answer");
        assert_eq!(v["text"], "42");
    }

    #[tokio::test]
    async fn approval_gate_contract() {
        // Only mutating/executing tools are gated; read-only tools stream through.
        assert!(is_consequential("shell"));
        assert!(is_consequential("fs_write"));
        assert!(is_consequential("fs_edit"));
        assert!(!is_consequential("fs_read"));
        assert!(!is_consequential("web_search"));
        assert!(!is_consequential("graph_query"));
        assert!(is_consequential("mcp__github__create_issue"));

        struct DenyAll;
        #[async_trait::async_trait]
        impl ApprovalPolicy for DenyAll {
            async fn approve(&self, _t: &str, _a: &Value) -> Approval {
                Approval::Deny
            }
        }
        assert_eq!(DenyAll.approve("shell", &json!({})).await, Approval::Deny);
    }

    #[test]
    fn recovers_object_with_nested_braces_and_quoted_braces() {
        let s = r#"junk {"a":1,"b":{"c":"a }{ b","d":2}} more"#;
        let v = extract_first_json_object(s).expect("should recover");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"]["c"], "a }{ b");
    }

    #[test]
    fn returns_none_when_no_object_present() {
        assert!(extract_first_json_object("totally not json").is_none());
        assert!(extract_first_json_object("{ unbalanced").is_none());
    }

    #[test]
    fn schema_includes_all_registered_tools() {
        let registry = ToolRegistry::with_defaults();
        let schema = build_response_schema(&registry);
        let names = schema["properties"]["tool"]["enum"].as_array().unwrap();
        let stringified: Vec<String> = names
            .iter()
            .filter_map(|v| v.as_str())
            .map(String::from)
            .collect();
        assert!(stringified.contains(&"web_search".to_string()));
        assert!(stringified.contains(&"wikipedia".to_string()));
        assert!(stringified.contains(&"arxiv".to_string()));
        assert!(stringified.contains(&"fetch_url".to_string()));
    }

    #[test]
    fn system_prompt_advertises_every_tool() {
        let registry = ToolRegistry::with_defaults();
        let s = build_system_prompt(&registry);
        for name in registry.names() {
            assert!(s.contains(&name), "system prompt missing tool `{name}`");
        }
    }

    #[test]
    fn local_workspace_prompt_does_not_advertise_unavailable_web_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(crate::tools::files::FsListTool::new(
            std::env::temp_dir(),
        )));
        let s = build_system_prompt(&registry);
        assert!(s.contains("fs_list"));
        assert!(!s.contains("\"tool\":\"web_search\""));
        assert!(!s.contains("\"tool\":\"fetch_url\""));
    }
}

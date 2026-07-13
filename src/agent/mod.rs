//! Tool-calling agent loop.
//!
//! Wraps a local [`LlmProvider`] and a `ToolRegistry`. Each iteration:
//!
//! 1. Builds a chat-style prompt: system prompt + tool catalog + the
//!    accumulated transcript.
//! 2. Calls the selected local provider with `format=<json schema>` so the model is *forced* to
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

use crate::context::estimate_tokens;
use crate::providers::{GenerateOptions, LlmProvider};
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
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    model: String,
    num_ctx: usize,
    keep_alive: String,
    max_iterations: usize,
    system_suffix: String,
    /// Sensitive local contexts (for example a lasso-selected screen region)
    /// disable optional replay logging for the entire agent run.
    replay_enabled: bool,
    /// Cached provider-supplied artifact fingerprint for `model`. Populated
    /// once on first `run()` so replay records a digest where the runtime can
    /// supply one without pretending every provider exposes Ollama metadata.
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
    /// Whether this run may write prompts and responses to `FORGE_REPLAY_LOG`.
    /// The default keeps existing deterministic-replay behavior; sensitive
    /// visual-context callers explicitly turn it off.
    pub replay_enabled: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "qwen3.5:4b".to_string(),
            num_ctx: 16_384,
            keep_alive: "1h".to_string(),
            max_iterations: DEFAULT_CODING_MAX_ITERATIONS,
            system_suffix: String::new(),
            replay_enabled: true,
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
    /// Sum of provider-reported generated tokens across successful calls.
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
    pub fn new(provider: Arc<dyn LlmProvider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            provider,
            tools,
            model: config.model,
            num_ctx: config.num_ctx,
            keep_alive: config.keep_alive,
            max_iterations: config.max_iterations.max(1),
            system_suffix: config.system_suffix,
            replay_enabled: config.replay_enabled,
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
        let (system, prompt) = budget_model_input(
            Some("You are planning before acting. Be concise and concrete."),
            &format!("Task:\n{task}\n\n"),
            "List the concrete steps you will take, as a short numbered list (2-6 steps). Plain text only — no preamble, no code.",
            self.num_ctx,
            0,
        )?;
        let opts = GenerateOptions {
            model: self.model.clone(),
            prompt,
            system,
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
        // Cache a provider fingerprint once per run so replay records it when
        // available without making each generation pay a metadata round-trip.
        if self.replay_enabled
            && std::env::var_os("FORGE_REPLAY_LOG").is_some()
            && self.cached_digest.is_empty()
        {
            self.cached_digest = self
                .provider
                .model_fingerprint(&self.model)
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
        // OpenAI-compatible servers receive this as response-format metadata
        // rather than prompt text, but it still consumes request/context
        // capacity on several local runtimes. Reserve its BPE estimate before
        // composing every agent turn.
        let format_reserved_tokens = estimate_tokens(&format_param.to_string());

        // Keep the initial task separate from the mutable transcript so the
        // host-side budgeter can retain it while dropping old tool evidence.
        // This matters for OpenAI-compatible local runtimes: their standard
        // wire protocol has no `num_ctx` control, so a provider hint alone
        // cannot keep a long-running agent request inside an endpoint's
        // declared context ceiling.
        let task_prefix = format!("User task: {task}\n\n");
        let mut transcript = String::new();

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

            let prompt_tail = format!("{transcript}{suffix}\n\nYour turn.");
            let (system, prompt) = budget_model_input(
                Some(&system_prompt),
                &task_prefix,
                &prompt_tail,
                self.num_ctx,
                format_reserved_tokens,
            )?;
            let opts = GenerateOptions {
                model: self.model.clone(),
                prompt,
                system,
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
                .context("agent: local provider call")?;
            trace.model_calls = trace.model_calls.saturating_add(1);
            trace.tokens_generated = trace.tokens_generated.saturating_add(resp.tokens_generated);
            let raw = resp.content.trim();
            debug!("agent raw response: {raw}");

            // Honor FORGE_REPLAY_LOG: every local provider call the agent makes
            // gets recorded so the entire research session is replayable.
            // The agent path is exactly where deterministic replay matters
            // most — a regulated user re-running a research session needs
            // bit-identical reasoning.
            if self.replay_enabled {
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

/// Reserve roughly 30% of a model's context for its completion, matching the
/// server chat path. This budget is enforced before every Agent request so a
/// configured OpenAI-compatible endpoint stays within its declared total
/// context even though the standard wire protocol has no `num_ctx` field.
const INPUT_CONTEXT_NUMERATOR: usize = 7;
const INPUT_CONTEXT_DENOMINATOR: usize = 10;
/// OpenAI-compatible chat requests carry role/message/template framing beyond
/// the literal system and user text. Keep a small, deterministic reserve so
/// host-side budgeting does not spend the entire input share on text alone.
pub(crate) const MODEL_REQUEST_FRAMING_TOKENS: usize = 32;
const CONTEXT_OMISSION_MARKER: &str =
    "\n[... earlier context omitted to fit the model budget ...]\n";

/// Token room available for literal system/prompt text after reserving the
/// response share and the protocol/model framing that a local runtime needs.
/// Callers with a grammar or response-format payload pass its measured token
/// cost through `additional_reserved_tokens`.
pub(crate) fn model_input_payload_budget(
    num_ctx: usize,
    additional_reserved_tokens: usize,
) -> Result<usize> {
    let input_budget = num_ctx
        .saturating_mul(INPUT_CONTEXT_NUMERATOR)
        .saturating_div(INPUT_CONTEXT_DENOMINATOR)
        .max(1);
    let reserved = MODEL_REQUEST_FRAMING_TOKENS.saturating_add(additional_reserved_tokens);
    if reserved >= input_budget {
        anyhow::bail!(
            "configured context ceiling ({num_ctx} tokens) is too small for the agent request framing; choose a larger endpoint context window"
        );
    }
    Ok(input_budget - reserved)
}

/// Fit a system message and a structured prompt inside the input share of a
/// model context window. `prompt_prefix` is the durable task/instructions and
/// `prompt_tail` is the newest transcript/evidence; when truncation is needed
/// we retain both ends rather than silently dropping the current task.
///
/// This is `pub(crate)` so Team's direct planner/reviewer calls use exactly
/// the same host-side contract as Agent calls.
pub(crate) fn budget_model_input(
    system: Option<&str>,
    prompt_prefix: &str,
    prompt_tail: &str,
    num_ctx: usize,
    additional_reserved_tokens: usize,
) -> Result<(Option<String>, String)> {
    let input_budget = model_input_payload_budget(num_ctx, additional_reserved_tokens)?;
    let system = system.filter(|text| !text.is_empty()).unwrap_or_default();
    let full_prompt = format!("{prompt_prefix}{prompt_tail}");

    if estimate_tokens(system).saturating_add(estimate_tokens(&full_prompt)) <= input_budget {
        return Ok((
            (!system.is_empty()).then(|| system.to_string()),
            full_prompt,
        ));
    }

    // Preserve room for the user task and most recent evidence even when a
    // tool-rich system prompt is larger than a small endpoint's context. If
    // the system text is naturally small, its unused allocation goes back to
    // the prompt rather than being wasted.
    let prompt_reserve = input_budget.div_ceil(2);
    let system_budget = if system.is_empty() {
        0
    } else {
        input_budget.saturating_sub(prompt_reserve)
    };
    let bounded_system = truncate_to_token_budget(system, system_budget, false);
    let prompt_budget = input_budget.saturating_sub(estimate_tokens(&bounded_system));
    let bounded_prompt = budget_prompt_parts(prompt_prefix, prompt_tail, prompt_budget);

    debug_assert!(
        estimate_tokens(&bounded_system).saturating_add(estimate_tokens(&bounded_prompt))
            <= input_budget,
        "agent request must fit its host-side input budget"
    );
    Ok((
        (!bounded_system.is_empty()).then_some(bounded_system),
        bounded_prompt,
    ))
}

fn budget_prompt_parts(prefix: &str, tail: &str, budget: usize) -> String {
    let full = format!("{prefix}{tail}");
    if estimate_tokens(&full) <= budget {
        return full;
    }
    if budget == 0 {
        return String::new();
    }

    let marker_tokens = estimate_tokens(CONTEXT_OMISSION_MARKER);
    let separator = (marker_tokens < budget).then_some(CONTEXT_OMISSION_MARKER);
    let content_budget = budget.saturating_sub(separator.map_or(0, estimate_tokens));

    // A very long task should not crowd out the newest tool result, but a
    // short task receives all the room it needs. The prefix helper preserves
    // both the beginning and the end of a genuinely oversized task.
    let reserved_for_tail = estimate_tokens(tail).min(content_budget / 2);
    let prefix_budget =
        estimate_tokens(prefix).min(content_budget.saturating_sub(reserved_for_tail));
    let bounded_prefix = truncate_middle_to_token_budget(prefix, prefix_budget);
    let tail_budget = content_budget.saturating_sub(estimate_tokens(&bounded_prefix));
    let bounded_tail = truncate_to_token_budget(tail, tail_budget, true);

    match (
        bounded_prefix.is_empty(),
        bounded_tail.is_empty(),
        separator,
    ) {
        (true, true, _) => String::new(),
        (false, true, _) => bounded_prefix,
        (true, false, _) => bounded_tail,
        (false, false, Some(separator)) => {
            format!("{bounded_prefix}{separator}{bounded_tail}")
        }
        (false, false, None) => format!("{bounded_prefix}{bounded_tail}"),
    }
}

/// Return a prefix or suffix whose BPE estimate stays within `budget`.
fn truncate_to_token_budget(text: &str, budget: usize, keep_tail: bool) -> String {
    if budget == 0 || text.is_empty() {
        return String::new();
    }
    if estimate_tokens(text) <= budget {
        return text.to_string();
    }

    let marker_tokens = estimate_tokens(CONTEXT_OMISSION_MARKER);
    if marker_tokens >= budget {
        return take_text_with_token_budget(text, budget, keep_tail);
    }
    let retained = take_text_with_token_budget(text, budget - marker_tokens, keep_tail);
    if retained.is_empty() {
        return CONTEXT_OMISSION_MARKER.to_string();
    }
    if keep_tail {
        format!("{CONTEXT_OMISSION_MARKER}{retained}")
    } else {
        format!("{retained}{CONTEXT_OMISSION_MARKER}")
    }
}

/// Preserve both the leading task label and its final details if one task is
/// itself larger than the context allocation.
fn truncate_middle_to_token_budget(text: &str, budget: usize) -> String {
    if budget == 0 || text.is_empty() {
        return String::new();
    }
    if estimate_tokens(text) <= budget {
        return text.to_string();
    }

    let marker_tokens = estimate_tokens(CONTEXT_OMISSION_MARKER);
    if marker_tokens >= budget {
        return take_text_with_token_budget(text, budget, false);
    }
    let content_budget = budget - marker_tokens;
    let leading = take_text_with_token_budget(text, content_budget.div_ceil(2), false);
    let trailing_budget = content_budget.saturating_sub(estimate_tokens(&leading));
    let trailing = take_text_with_token_budget(text, trailing_budget, true);
    match (leading.is_empty(), trailing.is_empty()) {
        (true, true) => String::new(),
        (false, true) => leading,
        (true, false) => trailing,
        (false, false) => format!("{leading}{CONTEXT_OMISSION_MARKER}{trailing}"),
    }
}

/// BPE-aware character-boundary slice. The estimator is deterministic, and a
/// binary search avoids repeatedly tokenizing every character of a large tool
/// result while never splitting UTF-8 text.
fn take_text_with_token_budget(text: &str, budget: usize, keep_tail: bool) -> String {
    if budget == 0 || text.is_empty() {
        return String::new();
    }
    if estimate_tokens(text) <= budget {
        return text.to_string();
    }

    let mut boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    boundaries.push(text.len());
    let character_count = boundaries.len() - 1;
    let mut low = 0usize;
    let mut high = character_count;
    while low < high {
        let candidate_count = (low + high).div_ceil(2);
        let candidate = if keep_tail {
            &text[boundaries[character_count - candidate_count]..]
        } else {
            &text[..boundaries[candidate_count]]
        };
        if estimate_tokens(candidate) <= budget {
            low = candidate_count;
        } else {
            high = candidate_count - 1;
        }
    }
    if keep_tail {
        text[boundaries[character_count - low]..].to_string()
    } else {
        text[..boundaries[low]].to_string()
    }
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

    #[test]
    fn model_input_budget_preserves_task_and_latest_evidence() {
        let system = "system instruction ".repeat(800);
        let task = "User task: TASK-MARKER keep this requested outcome.\n\n";
        let tail = format!(
            "STALE-EVIDENCE-MARKER {}\nLATEST-EVIDENCE-MARKER",
            "old tool output ".repeat(2_000),
        );

        let (bounded_system, bounded_prompt) =
            budget_model_input(Some(&system), task, &tail, 512, 0).unwrap();
        let used = bounded_system
            .as_deref()
            .map(estimate_tokens)
            .unwrap_or_default()
            .saturating_add(estimate_tokens(&bounded_prompt));

        assert!(
            used + MODEL_REQUEST_FRAMING_TOKENS
                <= 512 * INPUT_CONTEXT_NUMERATOR / INPUT_CONTEXT_DENOMINATOR
        );
        assert!(bounded_prompt.contains("TASK-MARKER"));
        assert!(bounded_prompt.contains("LATEST-EVIDENCE-MARKER"));
        assert!(!bounded_prompt.contains("STALE-EVIDENCE-MARKER"));
    }

    #[test]
    fn model_input_budget_rejects_a_ceiling_too_small_for_request_framing() {
        let error = budget_model_input(Some("system"), "task", "latest", 32, 8)
            .expect_err("format and protocol framing must not exceed a tiny endpoint ceiling");
        assert!(format!("{error:#}").contains("too small for the agent request framing"));
    }
}

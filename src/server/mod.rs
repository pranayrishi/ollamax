//! `forge serve` — a local-only structured backend for the desktop app and
//! the VSCode extension.
//!
//! ## Why this module exists
//!
//! The `forge` CLI is built for the terminal: run one command, stream tokens
//! to stdout, exit. A chat UI needs the opposite — a persistent, structured,
//! cancellable, multi-turn interface. `forge serve` is that interface. It is
//! **additive**: every existing subcommand keeps working unchanged.
//!
//! ## Design choices (see the report for the full rationale)
//!
//! - **Transport: Server-Sent Events (SSE) over a hand-rolled HTTP/1.1
//!   server.** SSE is one-directional server→client streaming, which is
//!   exactly token streaming. It needs no WebSocket upgrade handshake or
//!   frame masking, so it's small and correct by hand — in keeping with this
//!   codebase's "avoid heavy deps" ethos (no `axum`/`hyper`/`tungstenite`
//!   added; we reuse the `tokio` net stack already pulled in by `full`).
//!   Cancellation is a separate `POST /api/cancel` carrying the request id.
//! - **Local-only.** We bind `127.0.0.1` and *refuse* anything else
//!   ([`sanitize_host`]). The product's privacy claim depends on this.
//! - **Reuse, don't reimplement.** Chat calls
//!   [`OllamaProvider::generate_streaming`], research drives the real
//!   [`crate::agent::Agent`] loop, build drives the real
//!   [`crate::orchestrator::Orchestrator`] and forwards its `ProgressEvent`s.
//!   Rules, skills, the secret scanner, and replay logging behave exactly as
//!   they do in the CLI.
//!
//! ## Protocol (all JSON; streaming endpoints are `text/event-stream`)
//!
//! - `GET  /health`        → `{ ok, version }`
//! - `GET  /api/status`    → hardware profile + Ollama health (reuses [`VramSentinel`])
//! - `GET  /console`       → local, browser-based Agent Console (same server origin)
//! - `GET  /api/models`    → installed models (reuses `list_models`)
//! - `GET  /api/workspace` → approved workspace root for this server process
//! - `POST /api/chat`      → SSE: `meta` → `token`* → (`done` | `error` | `cancelled`)
//! - `POST /api/research`  → SSE: `meta` → `step`* → `answer` → (`done` | …)
//! - `POST /api/team`      → SSE: team role events → verification → `team_result` → `done`
//! - `POST /api/build`     → SSE: `meta` → `progress`* → `result` → `done`
//! - `POST /api/cancel`    → `{ ok }` (cancels the in-flight request with that id)

use crate::agent::{Agent, AgentConfig};
use crate::codeblocks::extract_and_write_code_blocks;
use crate::executor::ProgressEvent;
use crate::monitoring::VramSentinel;
use crate::orchestrator::{BuildRequest, Orchestrator, OrchestratorConfig};
use crate::providers::{GenerateOptions, LlmProvider, ModelInfo, OllamaProvider};
use crate::replay::{quick_hash, ReplayLog, ReplayRecord};
use crate::router::{TaskRouter, TaskType};
use crate::rules::RuleSet;
use crate::security::SecurityGuard;
use crate::team::{TeamConfig, TeamCoordinator, TeamEvent, TeamMode};
use crate::tools::ToolRegistry;
use crate::Config;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{info, warn};
use uuid::Uuid;

/// The local UI must prove it was launched alongside this server before it can
/// access project data or invoke any API. This prevents a random web page from
/// using a browser's loopback access to read local paths or drive an agent.
pub const API_TOKEN_HEADER: &str = "x-ollamax-token";
const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Shared state for the running server.
pub struct ServerState {
    config: Config,
    version: &'static str,
    /// Captured once when the local server starts. Every filesystem-capable
    /// agent tool is confined to this root; clients cannot submit an arbitrary
    /// path in an HTTP request to widen the sandbox.
    workspace_root: PathBuf,
    /// Descriptor-relative capability opened at server startup and shared by
    /// all Agent/Team filesystem tools. Keeping this handle prevents a later
    /// rename or symlink replacement of `workspace_root` from changing the
    /// workspace that a long-running local server edits.
    workspace_fs: crate::tools::files::WorkspaceFs,
    /// Per-server, high-entropy capability required for every `/api/*` route.
    /// Managed hosts pass it in `FORGE_SERVER_TOKEN`; standalone `forge serve`
    /// generates and prints one on its ready line for the local console host.
    api_token: String,
    /// In-flight request id → sticky cancellation signal. The atomic flag
    /// closes the race where a cancel arrives during request setup before a
    /// handler begins waiting for a notification.
    cancels: Mutex<HashMap<String, Arc<CancellationSignal>>>,
    /// At most one pending approval per Agent run. It is keyed by an opaque
    /// nonce so an early or duplicate HTTP decision cannot approve a later
    /// plan/action in the same run.
    approvals: Mutex<HashMap<String, PendingApproval>>,
}

struct PendingApproval {
    approval_id: String,
    tx: mpsc::UnboundedSender<bool>,
}

/// One-way cancellation state shared by a request handler and `/api/cancel`.
/// `Notify` wakes an active stream loop; the atomic makes a pre-start request
/// observable even when nobody was waiting at the exact instant of cancel.
struct CancellationSignal {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationSignal {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // `notify_one` retains a permit when the stream loop has not yet
        // polled its waiter, closing the setup-to-wait race as well as waking
        // an already-running loop.
        self.notify.notify_one();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        // The atomic handles an already-cancelled request; `notify_one` above
        // retains a permit if cancellation races this waiter before it polls.
        let notified = self.notify.notified();
        tokio::pin!(notified);
        if self.is_cancelled() {
            return;
        }
        notified.as_mut().await;
    }
}

impl ServerState {
    fn new(config: Config, workspace_root: PathBuf, api_token: String) -> Self {
        let workspace_fs = crate::tools::files::WorkspaceFs::new(&workspace_root);
        Self {
            config,
            version: crate::cli::VERSION,
            workspace_root,
            workspace_fs,
            api_token,
            cancels: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
        }
    }

    fn authorizes(&self, head: &RequestHead) -> bool {
        let Some(supplied) = head.headers.get(API_TOKEN_HEADER) else {
            return false;
        };
        // Compare every byte instead of short-circuiting. This is mainly a
        // browser/CSRF capability rather than an OS-user boundary, but costs
        // nothing and avoids exposing a prefix through timing.
        supplied.len() == self.api_token.len()
            && supplied
                .as_bytes()
                .iter()
                .zip(self.api_token.as_bytes())
                .fold(0_u8, |different, (a, b)| different | (a ^ b))
                == 0
    }

    async fn register(&self, id: &str) -> Option<Arc<CancellationSignal>> {
        let n = Arc::new(CancellationSignal::new());
        let mut cancels = self.cancels.lock().await;
        if cancels.contains_key(id) {
            None
        } else {
            cancels.insert(id.to_string(), n.clone());
            Some(n)
        }
    }
    async fn unregister(&self, id: &str) {
        self.cancels.lock().await.remove(id);
    }
    async fn cancel(&self, id: &str) -> bool {
        match self.cancels.lock().await.get(id) {
            Some(n) => {
                n.request_cancel();
                true
            }
            None => false,
        }
    }
    /// Register exactly one current approval prompt. This happens immediately
    /// before the SSE event is emitted, not at run start, so client decisions
    /// cannot accumulate and be replayed against a future tool call.
    async fn register_approval(
        &self,
        run_id: &str,
        approval_id: &str,
    ) -> Option<mpsc::UnboundedReceiver<bool>> {
        let (tx, rx) = mpsc::unbounded_channel::<bool>();
        let mut approvals = self.approvals.lock().await;
        if approvals.contains_key(run_id) {
            None
        } else {
            approvals.insert(
                run_id.to_string(),
                PendingApproval {
                    approval_id: approval_id.to_string(),
                    tx,
                },
            );
            Some(rx)
        }
    }
    async fn clear_approval(&self, run_id: &str, approval_id: &str) {
        let mut approvals = self.approvals.lock().await;
        if approvals
            .get(run_id)
            .is_some_and(|pending| pending.approval_id == approval_id)
        {
            approvals.remove(run_id);
        }
    }
    async fn clear_approvals_for_run(&self, run_id: &str) {
        self.approvals.lock().await.remove(run_id);
    }
    async fn send_approval(&self, run_id: &str, approval_id: &str, decision: bool) -> bool {
        let pending = {
            let mut approvals = self.approvals.lock().await;
            match approvals.get(run_id) {
                Some(pending) if pending.approval_id == approval_id => approvals.remove(run_id),
                _ => None,
            }
        };
        match pending {
            Some(pending) => pending.tx.send(decision).is_ok(),
            None => false,
        }
    }
}

fn server_api_token() -> String {
    std::env::var("FORGE_SERVER_TOKEN")
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| valid_api_token(token))
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn valid_api_token(token: &str) -> bool {
    (16..=256).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// CLI entry point for `forge serve`. Binds `host:port` (forced to loopback),
/// prints a machine-readable ready line for the extension to discover the
/// port, then serves forever.
pub async fn run(config: Config, host: String, port: u16) -> Result<()> {
    let bind_host = sanitize_host(&host);
    if bind_host != host {
        eprintln!(
            "forge serve: refusing to bind `{host}`; using {bind_host} (local-only by design)"
        );
    }
    let listener = TcpListener::bind((bind_host.as_str(), port))
        .await
        .with_context(|| format!("bind {bind_host}:{port}"))?;
    let addr = listener.local_addr().context("read local_addr")?;

    let workspace_root = std::env::current_dir()
        .context("resolve current workspace")?
        .canonicalize()
        .context("canonicalize current workspace")?;
    let state = Arc::new(ServerState::new(config, workspace_root, server_api_token()));
    // Machine-readable line the managed desktop and VS Code hosts parse to
    // discover both the port and the private per-server API capability.
    println!(
        "FORGE_SERVE_READY {}",
        json!({
            "host": addr.ip().to_string(),
            "port": addr.port(),
            "version": crate::cli::VERSION,
            "token": state.api_token,
        })
    );
    eprintln!("forge serve listening on http://{addr}  (local-only; Ctrl-C to stop)");
    serve(listener, state).await
}

/// Serve on an already-bound listener. Exposed so tests can bind an ephemeral
/// `127.0.0.1:0` port and drive the protocol without a live Ollama.
pub async fn serve_listener(listener: TcpListener, config: Config) -> Result<()> {
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    serve_listener_in_workspace(listener, config, workspace_root).await
}

/// Variant used by integration tests and embedded hosts that need to pin an
/// explicit, already-approved workspace root without relying on process cwd.
pub async fn serve_listener_in_workspace(
    listener: TcpListener,
    config: Config,
    workspace_root: PathBuf,
) -> Result<()> {
    serve_listener_in_workspace_with_token(listener, config, workspace_root, server_api_token())
        .await
}

/// Test/embedded-host variant with an explicit private API capability. Managed
/// desktop and VS Code hosts normally pass this through `FORGE_SERVER_TOKEN`.
pub async fn serve_listener_in_workspace_with_token(
    listener: TcpListener,
    config: Config,
    workspace_root: PathBuf,
    api_token: impl Into<String>,
) -> Result<()> {
    let workspace_root = workspace_root
        .canonicalize()
        .with_context(|| format!("canonicalize workspace {}", workspace_root.display()))?;
    let api_token = api_token.into();
    if !valid_api_token(&api_token) {
        anyhow::bail!("server API token must be 16-256 URL-safe ASCII characters");
    }
    let state = Arc::new(ServerState::new(config, workspace_root, api_token));
    serve(listener, state).await
}

/// Epoch seconds (used by the scheduler tick + handlers).
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run one non-streaming agent pass for a scheduled task (no client socket).
async fn run_agent_oneshot(state: &Arc<ServerState>, prompt: &str) -> String {
    let provider = Arc::new(OllamaProvider::new(&state.config.ollama_url));
    // Scheduled runs are local-only unless a future scheduler UI explicitly
    // offers an egress toggle. They must not silently turn a background task
    // into web research.
    let registry = ToolRegistry::new();
    let mut agent = Agent::new(
        provider,
        registry,
        AgentConfig {
            model: state.config.default_model.clone(),
            num_ctx: 8192,
            keep_alive: "5m".to_string(),
            max_iterations: crate::agent::DEFAULT_MAX_ITERATIONS,
            system_suffix: String::new(),
        },
    );
    match agent.run(prompt, |_s| {}).await {
        Ok(trace) => trace.answer,
        Err(e) => format!("error: {e}"),
    }
}

/// #1h Background scheduler: every 30s, run any due scheduled tasks on-device.
fn spawn_scheduler_tick(state: Arc<ServerState>) {
    tokio::spawn(async move {
        let sched = crate::scheduler::Scheduler::for_config();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let now = now_ts();
            for task in sched.due(now) {
                info!("scheduler: running task {} ({})", task.id, task.prompt);
                let result = run_agent_oneshot(&state, &task.prompt).await;
                let _ = sched.mark_ran(&task.id, now_ts(), Some(result));
            }
        }
    });
}

async fn serve(listener: TcpListener, state: Arc<ServerState>) -> Result<()> {
    info!("forge serve accepting connections");
    spawn_scheduler_tick(state.clone());
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                warn!("forge serve: accept failed: {e}");
                continue;
            }
        };
        let st = state.clone();
        tokio::spawn(async move {
            handle_conn(stream, &st).await;
        });
    }
}

/// Force loopback. `localhost`/`127.0.0.1`/`::1` are accepted as-is; anything
/// else (including `0.0.0.0`) collapses to `127.0.0.1`.
fn sanitize_host(host: &str) -> String {
    match host {
        "localhost" | "127.0.0.1" | "::1" => host.to_string(),
        _ => "127.0.0.1".to_string(),
    }
}

// =====================================================================
// HTTP plumbing (hand-rolled, minimal — only what this protocol needs)
// =====================================================================

#[derive(Debug)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
}

impl RequestHead {
    fn content_length(&self) -> Option<usize> {
        match self.headers.get("content-length") {
            Some(value) => value.trim().parse().ok(),
            None => Some(0),
        }
    }
}

/// Parse the request line + headers (everything before the blank line).
/// Returns `None` if the request line is malformed. Tolerates `\n` and
/// `\r\n` line endings. Pure function — unit-tested below.
pub fn parse_head(raw: &str) -> Option<RequestHead> {
    let mut it = raw.lines();
    let first = it.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for line in it {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    Some(RequestHead {
        method,
        path,
        headers,
    })
}

/// Format one SSE message frame from a JSON value. Compact JSON has no
/// embedded newlines, so a single `data:` line round-trips cleanly. Pure
/// function — unit-tested below.
pub fn sse_frame(value: &Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
    )
}

/// Extract a query-string parameter from a request path (percent-decoded).
/// `/api/model_info?name=qwen2.5-coder%3A7b` → `Some("qwen2.5-coder:7b")`.
fn query_param(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// Minimal percent-decoder for query values (`%3A` → `:`, `+` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 3 <= bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Pull the context-window size out of an `/api/show` document. Ollama nests it
/// under `model_info` with an architecture-prefixed key (e.g.
/// `qwen2.context_length`), so we scan for any key ending in `.context_length`.
fn extract_context_length(doc: &Value) -> Option<u64> {
    let mi = doc.get("model_info")?.as_object()?;
    mi.iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

async fn write_json(mut w: OwnedWriteHalf, status: u16, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Vary: Origin\r\n\
         Connection: close\r\n\r\n{body}",
        reason = reason(status),
        len = body.len(),
    );
    w.write_all(resp.as_bytes()).await?;
    w.flush().await
}

async fn write_html(mut w: OwnedWriteHalf, status: u16, body: &str) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'self'; base-uri 'none'; frame-ancestors 'none'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'\r\n\
         Connection: close\r\n\r\n{body}",
        reason = reason(status),
        len = body.len(),
    );
    w.write_all(resp.as_bytes()).await?;
    w.flush().await
}

async fn write_cors_preflight(mut w: OwnedWriteHalf) -> std::io::Result<()> {
    let resp = "HTTP/1.1 204 No Content\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, X-Ollamax-Token\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n";
    w.write_all(resp.as_bytes()).await?;
    w.flush().await
}

async fn write_sse_head(w: &mut OwnedWriteHalf) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Vary: Origin\r\n\
         X-Accel-Buffering: no\r\n\r\n";
    w.write_all(head.as_bytes()).await?;
    w.flush().await
}

async fn write_sse(w: &mut OwnedWriteHalf, value: &Value) -> std::io::Result<()> {
    w.write_all(sse_frame(value).as_bytes()).await?;
    w.flush().await
}

async fn handle_conn(stream: TcpStream, state: &Arc<ServerState>) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Read the request head (request line + headers) up to the blank line.
    let mut head_text = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if head_text.len().saturating_add(line.len()) > MAX_REQUEST_HEAD_BYTES {
            let _ = write_json(
                write_half,
                431,
                &json!({"error": "request headers are too large"}),
            )
            .await;
            return;
        }
        head_text.push_str(&line);
    }

    let Some(head) = parse_head(&head_text) else {
        let _ = write_json(write_half, 400, &json!({"error": "malformed request"})).await;
        return;
    };

    // Reject an unauthenticated API call before allocating or waiting for its
    // body. Otherwise a hostile page could tie up local-server memory with a
    // large declared Content-Length even though route() would reject it later.
    let request_path = head.path.split('?').next().unwrap_or("");
    if head.method != "OPTIONS" && request_path.starts_with("/api/") && !state.authorizes(&head) {
        let _ = write_json(
            write_half,
            401,
            &json!({"error": "missing or invalid local Ollamax API token"}),
        )
        .await;
        return;
    }

    // Read the body (Content-Length bytes) from the same buffered reader.
    let Some(content_length) = head.content_length() else {
        let _ = write_json(
            write_half,
            400,
            &json!({"error": "invalid Content-Length header"}),
        )
        .await;
        return;
    };
    if content_length > MAX_REQUEST_BODY_BYTES {
        let _ = write_json(
            write_half,
            413,
            &json!({"error": format!("request body exceeds {MAX_REQUEST_BODY_BYTES} bytes")}),
        )
        .await;
        return;
    }
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    route(head, body, write_half, state).await;
}

async fn route(head: RequestHead, body: String, w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let path = head.path.split('?').next().unwrap_or("").to_string();
    if head.method == "OPTIONS" {
        let _ = write_cors_preflight(w).await;
        return;
    }
    // Every API route carries private project information or can trigger local
    // work. A loopback bind alone is not enough: browsers can reach localhost
    // from arbitrary sites. Managed hosts attach the per-server capability.
    if path.starts_with("/api/") && !state.authorizes(&head) {
        let _ = write_json(
            w,
            401,
            &json!({"error": "missing or invalid local Ollamax API token"}),
        )
        .await;
        return;
    }
    match (head.method.as_str(), path.as_str()) {
        ("GET", "/health") => {
            let _ = write_json(w, 200, &json!({"ok": true, "version": state.version})).await;
        }
        ("GET", "/console") => {
            // The console is same-origin and its static template is never
            // CORS-readable. Injecting the per-server token lets it call the
            // protected API without persisting the capability in localStorage.
            let page =
                include_str!("console.html").replace("__OLLAMAX_API_TOKEN__", &state.api_token);
            let _ = write_html(w, 200, &page).await;
        }
        ("GET", "/api/status") => handle_status(w, state).await,
        ("GET", "/api/workspace") => handle_workspace(w, state).await,
        ("GET", "/api/models") => handle_models(w, state).await,
        ("GET", "/api/models/catalog") => {
            handle_models_catalog(w, state, query_param(&head.path, "verify")).await
        }
        ("GET", "/api/model_info") => {
            handle_model_info(w, state, query_param(&head.path, "name")).await
        }
        ("POST", "/api/agent/approve") => handle_agent_approve(w, state, &body).await,
        ("GET", "/api/voice/locate") => {
            handle_voice_locate(w, state, query_param(&head.path, "q")).await
        }
        ("GET", "/api/schedule") => handle_schedule_list(w).await,
        ("POST", "/api/schedule") => handle_schedule_add(w, &body).await,
        ("POST", "/api/schedule/remove") => handle_schedule_remove(w, &body).await,
        ("GET", "/api/hub/categories") => handle_hub_categories(w).await,
        ("GET", "/api/hub/search") => handle_hub_search(w, query_param(&head.path, "q")).await,
        ("GET", p) if p.starts_with("/api/hub/package/") => {
            handle_hub_package(w, p.trim_start_matches("/api/hub/package/")).await
        }
        ("GET", "/api/memory") => handle_memory_list(w, state).await,
        ("POST", "/api/memory/clear") => handle_memory_clear(w, state).await,
        ("GET", "/api/graph/status") => handle_graph_status(w, state).await,
        ("POST", "/api/graph/build") => handle_graph_build(w, state, &body).await,
        ("POST", "/api/chat") => handle_chat(w, state, &body).await,
        ("POST", "/api/research") => handle_research(w, state, &body).await,
        ("POST", "/api/team") => handle_team(w, state, &body).await,
        ("POST", "/api/build") => handle_build(w, state, &body).await,
        ("POST", "/api/cancel") => handle_cancel(w, state, &body).await,
        _ => {
            let _ = write_json(w, 404, &json!({"error": "not found", "path": path})).await;
        }
    }
}

// =====================================================================
// Request payloads
// =====================================================================

#[derive(Debug, Deserialize)]
struct ChatReq {
    id: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    messages: Vec<ChatMsg>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    context: Vec<ContextItem>,
    #[serde(default)]
    temperature: Option<f32>,
    /// Feature 2: when true, the selected model can call the free web tools
    /// (web_search/wikipedia/arxiv/fetch_url) via the agent loop during normal
    /// chat. Off by default (pure-local). Enabling it sends search queries +
    /// fetched page text to the internet (inference still stays local) — the
    /// server discloses this in the `meta` event.
    #[serde(default)]
    tools: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatMsg {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ContextItem {
    #[serde(default)]
    path: String,
    #[serde(default)]
    content: String,
    /// #7 Vision: base64-encoded image bytes (no data: prefix). When present,
    /// the turn is routed with images and needs a vision-capable model.
    #[serde(default)]
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResearchReq {
    id: String,
    question: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_iterations: Option<usize>,
    #[serde(default)]
    context: Vec<ContextItem>,
    /// Autonomy Dial mode: "auto" | "confirm" | "readonly" (defaults to
    /// confirm, so omitted authority never grants write/shell access).
    #[serde(default)]
    autonomy: Option<String>,
    /// Coding agents are local-only by default. Callers must explicitly opt
    /// into web tools because search queries and fetched pages leave the device.
    #[serde(default)]
    web_tools: bool,
}

/// Controlled local coding-team request. The workspace is deliberately not a
/// request field: `forge serve` captured it at startup and all team roles stay
/// confined to that one root.
#[derive(Debug, Deserialize)]
struct TeamReq {
    id: String,
    task: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    scout_model: Option<String>,
    #[serde(default)]
    planner_model: Option<String>,
    #[serde(default)]
    reviewer_model: Option<String>,
    #[serde(default)]
    max_iterations: Option<usize>,
    #[serde(default)]
    max_repair_rounds: Option<usize>,
    #[serde(default)]
    parallel_scouts: bool,
    #[serde(default)]
    autonomy: Option<String>,
    #[serde(default)]
    context: Vec<ContextItem>,
}

#[derive(Debug, Deserialize)]
struct BuildReq {
    id: String,
    task: String,
    #[serde(default)]
    output_dir: Option<String>,
    #[serde(default)]
    no_security: bool,
}

#[derive(Debug, Deserialize)]
struct CancelReq {
    id: String,
}

// =====================================================================
// Handlers
// =====================================================================

async fn handle_status(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let sentinel = VramSentinel::new(state.config.min_free_vram_mb, false);
    let hw = sentinel.detect_hardware().await;
    let ollama = OllamaProvider::new(&state.config.ollama_url);
    let healthy = ollama.health_check().await.unwrap_or(false);
    let payload = json!({
        "ollamaUrl": state.config.ollama_url,
        "ollamaHealthy": healthy,
        "hardware": serde_json::to_value(&hw).unwrap_or_else(|_| json!({})),
        "version": state.version,
    });
    let _ = write_json(w, 200, &payload).await;
}

/// The local console uses this to make its scope explicit. The root is captured
/// at server startup and is informational only; callers cannot change it.
async fn handle_workspace(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let _ = write_json(
        w,
        200,
        &json!({
            "root": state.workspace_root.to_string_lossy(),
            "name": state.workspace_root.file_name().and_then(|n| n.to_str()).unwrap_or("workspace"),
        }),
    )
    .await;
}

async fn handle_models(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let ollama = OllamaProvider::new(&state.config.ollama_url);
    let payload = match ollama.list_models().await {
        Ok(models) => {
            let arr: Vec<Value> = models
                .iter()
                .map(|m| {
                    json!({
                        "name": m.name,
                        "size": m.size,
                        "sizeHuman": m.size_human,
                        "digest": m.digest,
                    })
                })
                .collect();
            json!({ "models": arr, "default": state.config.default_model })
        }
        Err(e) => {
            json!({ "models": [], "default": state.config.default_model, "error": format!("{e:#}") })
        }
    };
    let _ = write_json(w, 200, &payload).await;
}

/// `GET /api/models/catalog[?verify=true]` — the curated, hardware-tiered
/// registry of FREE open-weight models (Feature 1), reconciled with what's
/// installed locally and filtered to what this machine's VRAM can run. With
/// `?verify=true`, each tag is best-effort checked against the live Ollama
/// library (slower; networked). Drives the model picker's "discover / pull /
/// select" with an honest, hardware-aware default.
async fn handle_models_catalog(
    w: OwnedWriteHalf,
    state: &Arc<ServerState>,
    verify: Option<String>,
) {
    use crate::models::{verify_in_library, HardwareTier, ModelRegistry};

    let sentinel = VramSentinel::new(state.config.min_free_vram_mb, false);
    let hw = sentinel.detect_hardware().await;
    let free_vram = hw.free_vram_mb;

    let ollama = OllamaProvider::new(&state.config.ollama_url);
    let installed: Vec<String> = ollama
        .list_models()
        .await
        .map(|ms| ms.into_iter().map(|m| m.name).collect())
        .unwrap_or_default();

    let mut registry = ModelRegistry::seed();
    registry.mark_installed(&installed);

    // Optional live library verification (off by default — it's networked).
    let do_verify = matches!(verify.as_deref(), Some("true") | Some("1"));
    let mut verified: std::collections::HashMap<String, Option<bool>> = Default::default();
    if do_verify {
        for m in registry.all() {
            verified.insert(m.ollama_tag.clone(), verify_in_library(&m.ollama_tag).await);
        }
    }

    let recommended = registry
        .recommend(free_vram, &installed)
        .map(|m| m.ollama_tag.clone());

    let fits: std::collections::HashSet<String> = registry
        .fits(free_vram)
        .into_iter()
        .map(|m| m.ollama_tag.clone())
        .collect();

    let models: Vec<Value> = registry
        .all()
        .iter()
        .map(|m| {
            json!({
                "family": m.family,
                "ollamaTag": m.ollama_tag,
                "pullCommand": format!("ollama pull {}", m.ollama_tag),
                "params": m.params,
                "tier": m.tier,
                "tierLabel": m.tier.label(),
                "approxVramMb": m.approx_vram_mb,
                "license": m.license.spdx(),
                "commercialFriendly": m.license.commercial_friendly(),
                "strengths": m.strengths,
                "installed": m.installed,
                "fits": fits.contains(&m.ollama_tag),
                "libraryVerified": verified.get(&m.ollama_tag).copied().flatten(),
            })
        })
        .collect();

    let payload = json!({
        "hardware": {
            "freeVramMb": free_vram,
            "gpuKind": hw.gpu_kind,
            "tier": HardwareTier::for_vram(free_vram),
            "tierLabel": HardwareTier::for_vram(free_vram).label(),
        },
        "recommended": recommended,
        "installed": installed,
        "verified": do_verify,
        // Honest disclosure rendered by the picker.
        "note": "Free, open-weight models run locally via Ollama. Cloud models (GPT/Claude/Gemini) are paid, bring-your-own-key, and not listed here.",
        "models": models,
    });
    let _ = write_json(w, 200, &payload).await;
}

/// `GET /api/model_info?name=<model>` — local-only model metadata for the
/// picker: context window + capabilities (`tools`, `thinking`, `vision`) +
/// size/quant details. Reuses `OllamaProvider::show` (Ollama `/api/show`), so
/// no inference and no extra network egress.
async fn handle_model_info(w: OwnedWriteHalf, state: &Arc<ServerState>, name: Option<String>) {
    let Some(name) = name else {
        let _ = write_json(w, 400, &json!({"error": "missing ?name="})).await;
        return;
    };
    let ollama = OllamaProvider::new(&state.config.ollama_url);
    let payload = match ollama.show(&name).await {
        Ok(doc) => json!({
            "name": name,
            "contextLength": extract_context_length(&doc),
            "capabilities": doc.get("capabilities").cloned().unwrap_or_else(|| json!([])),
            "details": doc.get("details").cloned().unwrap_or_else(|| json!({})),
        }),
        Err(e) => json!({ "name": name, "error": e.to_string() }),
    };
    let _ = write_json(w, 200, &payload).await;
}

async fn handle_cancel(w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    match serde_json::from_str::<CancelReq>(body) {
        Ok(req) => {
            let ok = state.cancel(&req.id).await;
            let _ = write_json(w, 200, &json!({"ok": ok})).await;
        }
        Err(e) => {
            let _ = write_json(w, 400, &json!({"error": e.to_string()})).await;
        }
    }
}

// --- Autonomy Dial: the webview POSTs the user's approve/deny decision for a
// paused consequential tool call here; it's routed to the waiting agent run. ----
async fn handle_agent_approve(w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
        #[serde(rename = "approvalId")]
        approval_id: String,
        decision: bool,
    }
    let req: Req = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({ "error": e.to_string() })).await;
            return;
        }
    };
    let delivered = state
        .send_approval(&req.id, &req.approval_id, req.decision)
        .await;
    let _ = write_json(w, 200, &json!({ "delivered": delivered })).await;
}

// --- Phase 2 Voice navigation: resolve a transcribed phrase to a code location
// via the local code graph (intent -> file:line). No audio touches the engine —
// STT happens in the extension; only the transcript text arrives here. ---------
async fn handle_voice_locate(w: OwnedWriteHalf, state: &Arc<ServerState>, q: Option<String>) {
    let q = q.unwrap_or_default();
    let graph_path = state.workspace_root.join("graphify-out").join("graph.json");
    match crate::graph::CodeGraph::from_file(&graph_path) {
        Ok(g) => match g.locate(&q) {
            Some(loc) => {
                let _ =
                    write_json(w, 200, &json!({ "found": true, "query": q, "target": loc })).await;
            }
            None => {
                let _ = write_json(w, 200, &json!({ "found": false, "query": q })).await;
            }
        },
        Err(_) => {
            let _ = write_json(
                w,
                200,
                &json!({ "found": false, "query": q, "error": "no code graph indexed for this workspace" }),
            )
            .await;
        }
    }
}

// --- #1h On-device NL scheduler (Hermes-class cron). Tasks persist locally; the
// background tick in serve() runs the due ones. -------------------------------
async fn handle_schedule_list(w: OwnedWriteHalf) {
    let tasks = crate::scheduler::Scheduler::for_config().list();
    let _ = write_json(w, 200, &json!({ "tasks": tasks })).await;
}

async fn handle_schedule_add(w: OwnedWriteHalf, body: &str) {
    #[derive(serde::Deserialize)]
    struct Req {
        prompt: String,
        schedule: String,
    }
    let req: Req = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({ "error": e.to_string() })).await;
            return;
        }
    };
    match crate::scheduler::Scheduler::for_config().add(&req.prompt, &req.schedule, now_ts()) {
        Ok(task) => {
            let _ = write_json(w, 200, &json!({ "task": task })).await;
        }
        Err(e) => {
            let _ = write_json(w, 400, &json!({ "error": e.to_string() })).await;
        }
    }
}

async fn handle_schedule_remove(w: OwnedWriteHalf, body: &str) {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
    }
    let req: Req = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({ "error": e.to_string() })).await;
            return;
        }
    };
    let removed = crate::scheduler::Scheduler::for_config()
        .remove(&req.id)
        .unwrap_or(false);
    let _ = write_json(w, 200, &json!({ "removed": removed })).await;
}

// --- #7 Central Hub catalog, served LOCALLY (auto-loads with no account server)
// + intent-aware search. The account server is optional enrichment only. --------
async fn handle_hub_categories(w: OwnedWriteHalf) {
    let cats = crate::hub::categories();
    let _ = write_json(
        w,
        200,
        &json!({ "categories": cats, "source": "local-engine" }),
    )
    .await;
}

async fn handle_hub_search(w: OwnedWriteHalf, q: Option<String>) {
    let q = q.unwrap_or_default();
    let results = crate::hub::search(&q, 24);
    let _ = write_json(w, 200, &json!({ "query": q, "categories": results })).await;
}

async fn handle_hub_package(w: OwnedWriteHalf, slug: &str) {
    // Slugs are simple kebab-case ASCII (no escaping needed); just drop any
    // trailing query string before the exact-match lookup.
    let slug = slug.split('?').next().unwrap_or(slug);
    match crate::hub::package(slug) {
        Some(p) => {
            let _ = write_json(
                w,
                200,
                &serde_json::to_value(&p).unwrap_or_else(|_| json!({})),
            )
            .await;
        }
        None => {
            let _ = write_json(w, 404, &json!({ "error": "unknown package", "slug": slug })).await;
        }
    }
}

// --- Part B: on-device conversational memory (view/clear). The store is local
// per-project; nothing here ever reaches the identity backend. ----------------
async fn handle_memory_list(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let store = crate::memory::MemoryStore::for_project(&state.workspace_root);
    let entries = store.all();
    let items: Vec<Value> = entries
        .iter()
        .map(|e| json!({ "ts": e.ts, "kind": e.kind, "text": e.text, "tags": e.tags }))
        .collect();
    let _ = write_json(
        w,
        200,
        &json!({ "memory": items, "path": store.path().to_string_lossy(), "onDevice": true }),
    )
    .await;
}

async fn handle_memory_clear(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let store = crate::memory::MemoryStore::for_project(&state.workspace_root);
    let ok = store.clear().is_ok();
    let _ = write_json(w, 200, &json!({ "ok": ok })).await;
}

// --- Part A: code knowledge graph status + build (graphify managed builder). ---
async fn handle_graph_status(w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let idx = crate::graph::GraphIndex::new(state.workspace_root.clone());
    let exists = idx.exists();
    let node_count = if exists {
        crate::graph::CodeGraph::from_file(&idx.graph_path())
            .map(|g| g.node_count())
            .ok()
    } else {
        None
    };
    let _ = write_json(
        w,
        200,
        &json!({
            "indexed": exists,
            "stale": exists && idx.is_stale(),
            "path": idx.graph_path().to_string_lossy(),
            "nodeCount": node_count,
        }),
    )
    .await;
}

async fn handle_graph_build(w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    // Spawns the managed graphify builder (Python, hidden). `graphify` must be
    // resolvable (bundled with the app / on PATH for dev). Fails gracefully.
    let bin = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("graphify").and_then(|b| b.as_str()).map(String::from))
        .unwrap_or_else(|| "graphify".to_string());
    let update = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("update").and_then(|b| b.as_bool()))
        .unwrap_or(true);
    let idx = crate::graph::GraphIndex::new(state.workspace_root.clone());
    match idx.build(&bin, update) {
        Ok(p) => {
            let _ = write_json(w, 200, &json!({ "ok": true, "path": p.to_string_lossy() })).await;
        }
        Err(e) => {
            let _ = write_json(
                w,
                200,
                &json!({ "ok": false, "error": e.to_string(), "hint": "graphify not installed/bundled yet" }),
            )
            .await;
        }
    }
}

async fn handle_chat(mut w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    let req: ChatReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({"error": e.to_string()})).await;
            return;
        }
    };

    let id = req.id.clone();
    let requested = req
        .model
        .clone()
        .unwrap_or_else(|| state.config.default_model.clone());

    // Auto-routing (Feature 2): when the picker is on "auto", delegate the
    // model choice to the existing `TaskRouter` (complexity → tier). The
    // decision + a one-line "why" are surfaced in the `meta` event for trust.
    // Policy: Auto only ever picks from *installed local* Ollama models — it
    // never escalates to a paid cloud provider (cloud requires an explicit
    // manual pick). A manual selection always overrides Auto.
    let (model, routing) = if requested.eq_ignore_ascii_case("auto") {
        let ollama = OllamaProvider::new(&state.config.ollama_url);
        let installed = ollama.list_models().await.unwrap_or_default();
        let task_text = latest_user_text(&req.messages, req.prompt.as_deref());
        route_auto(&task_text, &installed, &state.config.default_model).await
    } else {
        (requested, Value::Null)
    };

    // Reload rules per request so `forge rules edit` takes effect live.
    let mut rules_suffix = RuleSet::load_default().unwrap_or_default().render();
    // Part B: prepend on-device memory relevant to this turn (token-budgeted) so
    // plain chat isn't a cold start either. Local only — never sent anywhere.
    {
        let task_text = latest_user_text(&req.messages, req.prompt.as_deref());
        let mem = crate::memory::MemoryStore::for_project(&state.workspace_root)
            .render_for_context(&task_text, 400);
        if !mem.is_empty() {
            rules_suffix = format!("{rules_suffix}\n\n{mem}");
        }
    }
    let system = if rules_suffix.is_empty() {
        None
    } else {
        Some(rules_suffix)
    };

    // Secret-scan attached context *before* it reaches the model. We surface
    // findings as a warning event (the UI shows a banner); we do not silently
    // strip content. Hard-blocking can be made a config option later.
    let guard = SecurityGuard::new(true);
    let mut warnings = Vec::new();
    for c in &req.context {
        for f in guard.scan_content(&c.content, None).await {
            warnings.push(json!({
                "severity": format!("{:?}", f.rule.severity).to_lowercase(),
                "rule": f.rule.name,
                "file": c.path,
                "line": f.line_number,
            }));
        }
    }

    // No artificial message/length cap. Instead, degrade gracefully: size the
    // context window to the hardware and trim the *oldest* history that won't
    // fit, reusing the real BPE token estimator. We report how many messages
    // were dropped so the UI can show it (never silent truncation).
    let sentinel = VramSentinel::new(state.config.min_free_vram_mb, false);
    let hw = sentinel.detect_hardware().await;
    let num_ctx = hw.optimal_context;
    let max_input_tokens = (num_ctx * 7) / 10; // reserve ~30% for the response
    let (kept_msgs, trimmed) = budget_messages(&req.messages, &req.context, max_input_tokens);

    // #7 Vision: collect any attached images (base64) UP FRONT — before the
    // web-tools branch. The tools/agent path flattens context to text and cannot
    // carry image bytes, so an image turn must use the plain vision path below;
    // collecting after the tools branch silently dropped images (review #2).
    let images: Vec<String> = req.context.iter().filter_map(|c| c.image.clone()).collect();
    let mut vision_warning = None;
    if !images.is_empty() {
        let probe = OllamaProvider::new(&state.config.ollama_url);
        if !probe.supports_vision(&model).await {
            vision_warning = Some(json!({
                "severity": "warning",
                "rule": "vision_model_required",
                "file": "",
                "message": format!("`{model}` can't read images. Pick a vision model (e.g. one tagged `vision` in the model list) to analyze the attached image."),
            }));
        }
    }
    if let Some(vw) = vision_warning {
        warnings.push(vw);
    }

    // Feature 2: web tools in NORMAL chat (opt-in). Reuse the exact same agent
    // loop as /api/research over the flattened conversation — the model decides
    // when to search/fetch, steps stream to the UI, and the meta event discloses
    // the egress. SKIPPED when an image is attached (the agent path can't see
    // images): the image turn falls through to the plain vision path so the
    // picture is actually analyzed. Off by default = pure-local chat.
    if req.tools == Some(true) && images.is_empty() {
        let question = build_chat_prompt(&kept_msgs, req.prompt.as_deref(), &req.context);
        let rules = RuleSet::load_default().unwrap_or_default().render();
        // Carry the secret-scan `warnings` (this path can send context to the
        // web), the Auto `routing` reasoning, and the `trimmed` count through —
        // same safety/transparency surface as the plain chat path.
        run_agent_streamed(
            w,
            state,
            id,
            model,
            question,
            num_ctx,
            crate::agent::DEFAULT_MAX_ITERATIONS,
            rules,
            true,
            warnings,
            routing,
            trimmed,
            "confirm".to_string(),
        )
        .await;
        return;
    } else if req.tools == Some(true) {
        // tools requested but an image is attached — disclose that we skipped them.
        warnings.push(json!({
            "severity": "info",
            "rule": "web_tools_skipped_for_image",
            "file": "",
            "message": "Web tools were skipped this turn because an image is attached — analyzing the image locally instead.",
        }));
    }

    // #4 Streaming "thinking": enable reasoning output ONLY for thinking-capable
    // models (we never fabricate a transcript for models that don't reason). The
    // UI falls back to the rotating status labels otherwise. Cheap local
    // /api/show capability check.
    let think = {
        let probe = OllamaProvider::new(&state.config.ollama_url);
        if probe.supports_thinking(&model).await {
            Some(true)
        } else {
            None
        }
    };

    let replay_mode = std::env::var_os("FORGE_REPLAY_LOG").is_some();
    let opts = GenerateOptions {
        model: model.clone(),
        prompt: build_chat_prompt(&kept_msgs, req.prompt.as_deref(), &req.context),
        system,
        num_ctx: Some(num_ctx),
        stream: true,
        temperature: if replay_mode {
            Some(0.0)
        } else {
            Some(req.temperature.unwrap_or(0.7))
        },
        seed: if replay_mode { Some(0) } else { None },
        images: if images.is_empty() {
            None
        } else {
            Some(images)
        },
        think,
        ..Default::default()
    };

    if write_sse_head(&mut w).await.is_err() {
        return;
    }
    let _ = write_sse(
        &mut w,
        &json!({
            "type": "meta",
            "id": id,
            "model": model,
            "warnings": warnings,
            "numCtx": num_ctx,
            "trimmed": trimmed,
            "routing": routing,
        }),
    )
    .await;

    let Some(cancel) = state.register(&id).await else {
        let _ = write_sse(
            &mut w,
            &json!({"type": "error", "message": "a request with this id is already running"}),
        )
        .await;
        return;
    };
    let url = state.config.ollama_url.clone();
    // The channel now carries discriminated SSE events so reasoning ("thinking")
    // streams distinctly from the answer ("token").
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let full = Arc::new(std::sync::Mutex::new(String::new()));
    let full_for_task = full.clone();
    let tx_think = tx.clone();
    let gen_opts = opts.clone();
    let gen = tokio::spawn(async move {
        let provider = OllamaProvider::new(&url);
        provider
            .generate_streaming_parts(
                gen_opts,
                move |chunk| {
                    let _ = tx.send(json!({"type": "token", "text": chunk}));
                    if let Ok(mut g) = full_for_task.lock() {
                        g.push_str(chunk);
                    }
                },
                move |thinking| {
                    // Real reasoning tokens — rendered in a collapsible block; the
                    // answer text is NOT polluted with them.
                    let _ = tx_think.send(json!({"type": "thinking", "text": thinking}));
                },
            )
            .await
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                gen.abort();
                cancelled = true;
                let _ = write_sse(&mut w, &json!({"type": "cancelled"})).await;
                break;
            }
            msg = rx.recv() => match msg {
                Some(ev) => {
                    if write_sse(&mut w, &ev).await.is_err() {
                        gen.abort();
                        cancelled = true;
                        break;
                    }
                }
                None => break,
            }
        }
    }
    state.unregister(&id).await;
    if cancelled {
        return;
    }

    match gen.await {
        Ok(Ok(bytes)) => {
            let response_text = full.lock().map(|g| g.clone()).unwrap_or_default();
            maybe_log_replay(&opts, &response_text, &state.config.ollama_url).await;
            let _ = write_sse(&mut w, &json!({"type": "done", "bytes": bytes})).await;
        }
        Ok(Err(e)) => {
            let _ = write_sse(&mut w, &json!({"type": "error", "message": e.to_string()})).await;
        }
        Err(_join) => { /* aborted: client already saw `cancelled` */ }
    }
}

async fn handle_research(w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    let req: ResearchReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({"error": e.to_string()})).await;
            return;
        }
    };

    let id = req.id.clone();
    let rules_suffix = RuleSet::load_default().unwrap_or_default().render();
    let sentinel = VramSentinel::new(state.config.min_free_vram_mb, false);
    let hw = sentinel.detect_hardware().await;
    let num_ctx = hw.optimal_context;

    let pick_provider = OllamaProvider::new(&state.config.ollama_url);
    // The shared picker uses "auto" as its default. Treat it as no manual
    // override here rather than sending the literal model name `auto` to
    // Ollama, which made the Agent tab fail while ordinary Chat worked.
    let model = match req
        .model
        .filter(|m| !m.trim().is_empty() && !m.eq_ignore_ascii_case("auto"))
    {
        Some(m) => m,
        None => pick_model(&state.config, &pick_provider).await,
    };

    let mut question = req.question.clone();
    if !req.context.is_empty() {
        question.push_str("\n\nAttached context:\n");
        for c in &req.context {
            question.push_str(&c.content);
            question.push('\n');
        }
    }

    let max_iterations = req
        .max_iterations
        .unwrap_or(crate::agent::DEFAULT_CODING_MAX_ITERATIONS);
    let autonomy = match req.autonomy.as_deref() {
        Some("auto") => "auto".to_string(),
        Some("readonly") => "readonly".to_string(),
        // Unknown and omitted values fail closed to the interactive behavior.
        _ => "confirm".to_string(),
    };
    // The same endpoint powers the interactive coding Agent. Keep it strictly
    // local by default; a caller may explicitly opt into web research and the
    // meta event will disclose that egress.
    run_agent_streamed(
        w,
        state,
        id,
        model,
        question,
        num_ctx,
        max_iterations,
        rules_suffix,
        req.web_tools,
        Vec::new(),
        Value::Null,
        0,
        autonomy,
    )
    .await;
}

/// Run the bounded workspace-team topology through the local SSE protocol.
/// Unlike the legacy build endpoint, this path has actual filesystem tools: a
/// pair of read-only scouts, one writer, fixed verifier commands, and an
/// advisory reviewer. It intentionally never gives concurrent writers the
/// same checkout.
async fn handle_team(mut w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    let req: TeamReq = match serde_json::from_str(body) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_json(w, 400, &json!({"error": error.to_string()})).await;
            return;
        }
    };
    if req.task.trim().is_empty() {
        let _ = write_json(w, 400, &json!({"error": "team task must not be empty"})).await;
        return;
    }
    if write_sse_head(&mut w).await.is_err() {
        return;
    }

    let id = req.id.clone();
    // Register before model/plugin setup or progress events so an immediate
    // `/api/cancel` is acknowledged and observed before this request can spawn
    // a coordinator with filesystem access.
    let Some(cancel) = state.register(&id).await else {
        let _ = write_sse(
            &mut w,
            &json!({"type":"error", "message":"a request with this id is already running"}),
        )
        .await;
        let _ = write_sse(&mut w, &json!({"type":"done"})).await;
        return;
    };
    let pick_provider = OllamaProvider::new(&state.config.ollama_url);
    let installed = pick_provider.list_models().await.unwrap_or_default();
    let model = match req
        .model
        .filter(|model| !model.trim().is_empty() && !model.eq_ignore_ascii_case("auto"))
    {
        Some(model) if installed.iter().any(|installed| installed.name == model) => model,
        Some(model) => {
            let _ = write_sse(
                &mut w,
                &json!({"type":"error", "id": id, "message": format!("requested writer model `{model}` is not installed")}),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type":"done"})).await;
            state.unregister(&id).await;
            state.clear_approvals_for_run(&id).await;
            return;
        }
        None => pick_model(&state.config, &pick_provider).await,
    };
    let scout_model = match req.scout_model.filter(|model| !model.trim().is_empty()) {
        Some(model) if installed.iter().any(|installed| installed.name == model) => model,
        Some(model) => {
            let _ = write_sse(
                &mut w,
                &json!({"type":"error", "id": id, "message": format!("requested scout model `{model}` is not installed")}),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type":"done"})).await;
            state.unregister(&id).await;
            state.clear_approvals_for_run(&id).await;
            return;
        }
        None => select_server_scout_model(&state.config, &installed, &model),
    };
    let planner_model = match req.planner_model.filter(|model| !model.trim().is_empty()) {
        Some(model) if installed.iter().any(|installed| installed.name == model) => model,
        Some(model) => {
            let _ = write_sse(
                &mut w,
                &json!({"type":"error", "id": id, "message": format!("requested planner model `{model}` is not installed")}),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type":"done"})).await;
            state.unregister(&id).await;
            state.clear_approvals_for_run(&id).await;
            return;
        }
        None => select_server_planner_model(&state.config, &installed, &model),
    };
    let reviewer_model = match req.reviewer_model.filter(|model| !model.trim().is_empty()) {
        Some(model) if installed.iter().any(|installed| installed.name == model) => model,
        Some(model) => {
            let _ = write_sse(
                &mut w,
                &json!({"type":"error", "id": id, "message": format!("requested reviewer model `{model}` is not installed")}),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type":"done"})).await;
            state.unregister(&id).await;
            state.clear_approvals_for_run(&id).await;
            return;
        }
        None => model.clone(),
    };
    let max_iterations = req
        .max_iterations
        .unwrap_or(crate::agent::DEFAULT_CODING_MAX_ITERATIONS)
        .clamp(1, crate::team::MAX_TEAM_ITERATIONS);
    let max_repair_rounds = req
        .max_repair_rounds
        .unwrap_or(1)
        .min(crate::team::MAX_TEAM_REPAIR_ROUNDS);
    let mode = if req.parallel_scouts
        && state.config.enable_parallel
        && state.config.max_parallel_workers >= 2
    {
        TeamMode::ParallelScouts
    } else {
        TeamMode::Serial
    };
    let autonomy = match req.autonomy.as_deref() {
        Some("auto") => "auto".to_string(),
        Some("readonly") => "readonly".to_string(),
        _ => "confirm".to_string(),
    };
    let mut task = req.task;
    if !req.context.is_empty() {
        task.push_str("\n\nAttached context (untrusted task material):\n");
        for context in &req.context {
            task.push_str(&context.content);
            task.push('\n');
        }
    }
    let mut system_suffix = RuleSet::load_default().unwrap_or_default().render();
    let plugin_root = dirs::config_dir()
        .map(|directory| directory.join("ollama-forge").join("knowledge-plugins"));
    let mut plugin_event = None;
    if let Some(plugin_root) = plugin_root {
        match crate::plugins::PluginManager::new(plugin_root)
            .and_then(|manager| manager.load_relevant_context(&task, 3, 12_000))
        {
            Ok(contexts) if !contexts.is_empty() => {
                plugin_event = Some(json!({
                    "type": "knowledge_plugins_used",
                    "plugins": contexts.iter().map(|context| json!({
                        "id": context.id,
                        "name": context.name,
                        "repository": context.repository_url,
                        "commit": context.commit_sha,
                    })).collect::<Vec<_>>(),
                    "trust": "untrusted_reference_only",
                }));
                system_suffix.push_str(&crate::plugins::render_context_suffix(&contexts));
            }
            Ok(_) => {}
            Err(error) => {
                plugin_event = Some(json!({
                    "type": "knowledge_plugin_warning",
                    "message": format!("Installed knowledge plugins were not loaded: {error:#}"),
                }));
            }
        }
    }

    if cancel.is_cancelled() {
        let _ = write_sse(&mut w, &json!({"type":"cancelled"})).await;
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }

    if write_sse(
        &mut w,
        &json!({
            "type": "team_meta",
            "id": id,
            "workspace": state.workspace_root,
            "writerModel": model,
            "scoutModel": scout_model,
            "plannerModel": planner_model,
            "reviewerModel": reviewer_model,
            "mode": match mode { TeamMode::Serial => "serial", TeamMode::ParallelScouts => "parallel_scouts" },
            "maxIterations": max_iterations,
            "maxRepairRounds": max_repair_rounds,
            "autonomy": autonomy,
            "parallelRequestedButDisabled": req.parallel_scouts
                && (!state.config.enable_parallel || state.config.max_parallel_workers < 2),
        }),
    )
    .await
    .is_err()
    {
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }
    if let Some(event) = plugin_event {
        if write_sse(&mut w, &event).await.is_err() {
            state.unregister(&id).await;
            state.clear_approvals_for_run(&id).await;
            return;
        }
    }
    if cancel.is_cancelled() {
        let _ = write_sse(&mut w, &json!({"type":"cancelled"})).await;
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let approval: Arc<dyn crate::agent::ApprovalPolicy> = Arc::new(ChannelApprovalPolicy {
        mode: autonomy.clone(),
        tx: tx.clone(),
        state: state.clone(),
        run_id: id.clone(),
    });
    let provider = Arc::new(OllamaProvider::new(&state.config.ollama_url));
    let workspace = state.workspace_root.clone();
    let workspace_fs = state.workspace_fs.clone();
    let task_for_run = task.clone();
    let run = tokio::spawn(async move {
        let coordinator = TeamCoordinator::with_workspace_fs(
            provider,
            workspace,
            workspace_fs,
            TeamConfig {
                model,
                scout_model: Some(scout_model),
                planner_model: Some(planner_model),
                reviewer_model: Some(reviewer_model),
                num_ctx: VramSentinel::new(0, false)
                    .detect_hardware()
                    .await
                    .optimal_context,
                keep_alive: "1h".to_string(),
                max_iterations,
                max_repair_rounds,
                mode,
                system_suffix,
            },
        )?;
        coordinator
            .run(&task_for_run, approval, |event| {
                let _ = tx.send(team_event_json(event));
            })
            .await
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                run.abort();
                cancelled = true;
                let _ = write_sse(&mut w, &json!({"type":"cancelled"})).await;
                break;
            }
            event = rx.recv() => match event {
                Some(event) => {
                    if write_sse(&mut w, &event).await.is_err() {
                        run.abort();
                        cancelled = true;
                        break;
                    }
                }
                None => break,
            }
        }
    }
    if cancelled {
        // Wait for task-drop cleanup before releasing the run ID. In
        // particular, a cancelled ShellTool's process-group guard runs during
        // this join, so ordinary verifier descendants cannot outlive the
        // request that launched them.
        let _ = run.await;
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }
    state.unregister(&id).await;
    state.clear_approvals_for_run(&id).await;
    match run.await {
        Ok(Ok(result)) => {
            let answer = result.implementation_answers.join("\n\n");
            let _ = write_sse(&mut w, &json!({"type":"answer", "text": answer})).await;
            let _ = write_sse(
                &mut w,
                &json!({
                    "type": "team_result",
                    "status": result.status,
                    "writerMutationSteps": result.writer_mutation_steps,
                    "verification": result.verification,
                    "functionalVerificationPassed": result.functional_verification_passed,
                    "review": result.review,
                    "reviewAvailable": result.review_available,
                    "elapsedMs": result.elapsed_ms,
                    "modelCalls": result.model_calls,
                    "tokens": result.tokens_generated,
                    "toolCalls": result.tool_calls,
                    "plan": result.plan,
                }),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type":"done"})).await;
        }
        Ok(Err(error)) => {
            let _ = write_sse(
                &mut w,
                &json!({"type":"error", "message": error.to_string()}),
            )
            .await;
        }
        Err(_) => {}
    }
}

fn select_server_scout_model(
    config: &Config,
    installed: &[ModelInfo],
    writer_model: &str,
) -> String {
    let writer_size = installed
        .iter()
        .find(|model| model.name == writer_model)
        .map(|model| model.size);
    config
        .execution_models
        .iter()
        .filter(|name| name.as_str() != writer_model)
        .filter_map(|name| installed.iter().find(|model| model.name == *name))
        .filter(|model| writer_size.map_or(true, |writer_size| model.size <= writer_size))
        .max_by_key(|model| model.size)
        .map(|model| model.name.clone())
        .unwrap_or_else(|| writer_model.to_string())
}

fn select_server_planner_model(
    config: &Config,
    installed: &[ModelInfo],
    writer_model: &str,
) -> String {
    if config.planning_model != writer_model
        && installed
            .iter()
            .any(|model| model.name == config.planning_model)
    {
        return config.planning_model.clone();
    }
    writer_model.to_string()
}

fn team_event_json(event: &TeamEvent) -> Value {
    match event {
        TeamEvent::PlanCreated { plan } => json!({"type":"team_plan", "plan": plan}),
        TeamEvent::ScoutStarted { role } => json!({"type":"team_scout_started", "role": role}),
        TeamEvent::ScoutFinished { role, steps } => {
            json!({"type":"team_scout_finished", "role": role, "steps": steps})
        }
        TeamEvent::PlannerStarted => json!({"type":"team_planner_started"}),
        TeamEvent::PlannerFinished { summary } => {
            json!({"type":"team_planner_finished", "summary": summary})
        }
        TeamEvent::ImplementerStarted { repair_round } => {
            json!({"type":"team_writer_started", "repairRound": repair_round})
        }
        TeamEvent::ImplementerStep { repair_round, step } => json!({
            "type":"step",
            "role":"implementer",
            "repairRound": repair_round,
            "iteration": step.iteration,
            "tool": step.tool,
            "ok": step.ok,
            "args": step.args,
            "preview": step.result_preview,
        }),
        TeamEvent::ImplementerFinished {
            repair_round,
            steps,
        } => {
            json!({"type":"team_writer_finished", "repairRound": repair_round, "steps": steps})
        }
        TeamEvent::VerificationStarted { command } => {
            json!({"type":"team_verification_started", "command": command})
        }
        TeamEvent::VerificationFinished { result } => {
            json!({"type":"team_verification_finished", "result": result})
        }
        TeamEvent::ReviewerFinished { available } => {
            json!({"type":"team_reviewer_finished", "available": available})
        }
    }
}

/// Shared agent-loop streamer used by BOTH `/api/research` and `/api/chat` (when
/// its `tools` toggle is on) — the web tools are the *same* system surfaced in
/// two places, not a fork. Streams `step` events as tools are called, then a
/// final `answer` + `done`. When `web_disclosure` is set, the `meta` event
/// states that queries/fetched pages leave the machine even though inference
/// stays local.
/// Approval gate (Autonomy Dial) backed by the SSE channel + a decision channel.
/// "auto" allows all; "readonly" denies all consequential tools; "confirm" emits
/// an `approval_request` event and awaits the user's decision (timeout → deny).
struct ChannelApprovalPolicy {
    mode: String,
    tx: mpsc::UnboundedSender<Value>,
    state: Arc<ServerState>,
    run_id: String,
}

impl ChannelApprovalPolicy {
    async fn request_decision(
        &self,
        mut event: Value,
        timeout: std::time::Duration,
    ) -> crate::agent::Approval {
        use crate::agent::Approval;
        let approval_id = Uuid::new_v4().to_string();
        let Some(mut rx) = self
            .state
            .register_approval(&self.run_id, &approval_id)
            .await
        else {
            return Approval::Deny;
        };
        event["approvalId"] = json!(approval_id);
        if self.tx.send(event).is_err() {
            self.state.clear_approval(&self.run_id, &approval_id).await;
            return Approval::Deny;
        }
        let decision = match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(true)) => Approval::Allow,
            // Timeout, closed channel, or an explicit denial all fail closed.
            _ => Approval::Deny,
        };
        self.state.clear_approval(&self.run_id, &approval_id).await;
        decision
    }
}

#[async_trait::async_trait]
impl crate::agent::ApprovalPolicy for ChannelApprovalPolicy {
    fn requires_plan_approval(&self) -> bool {
        self.mode == "confirm"
    }

    async fn approve_plan(&self, plan: &str) -> crate::agent::Approval {
        use crate::agent::Approval;
        // Always surface the plan (Intent Preview). Only "confirm" pauses for the
        // user's Run/Cancel; auto/readonly show it and proceed.
        if self.mode != "confirm" {
            let _ = self.tx.send(json!({ "type": "plan", "text": plan }));
            return Approval::Allow;
        }
        self.request_decision(
            json!({ "type": "plan", "text": plan }),
            std::time::Duration::from_secs(300),
        )
        .await
    }

    async fn approve(&self, tool: &str, args: &Value) -> crate::agent::Approval {
        use crate::agent::Approval;
        match self.mode.as_str() {
            "auto" => Approval::Allow,
            "readonly" => Approval::Deny,
            _ => {
                self.request_decision(
                    json!({ "type": "approval_request", "tool": tool, "args": args }),
                    std::time::Duration::from_secs(120),
                )
                .await
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_streamed(
    mut w: OwnedWriteHalf,
    state: &Arc<ServerState>,
    id: String,
    model: String,
    question: String,
    num_ctx: usize,
    max_iterations: usize,
    rules_suffix: String,
    web_disclosure: bool,
    // Carried through so the egressing path keeps the SAME safety surface as
    // plain chat: secret-scan `warnings`, the Auto `routing` reasoning, and the
    // `trimmed`-history count are all surfaced in the meta event.
    warnings: Vec<Value>,
    routing: Value,
    trimmed: usize,
    // Autonomy Dial: "auto" (run freely), "confirm" (pause for each consequential
    // tool and await user approval), or "readonly" (deny all consequential tools).
    autonomy: String,
) {
    if write_sse_head(&mut w).await.is_err() {
        return;
    }
    let mut meta = json!({
        "type": "meta", "id": id, "model": model, "numCtx": num_ctx,
        "warnings": warnings, "routing": routing, "trimmed": trimmed,
    });
    if web_disclosure {
        meta["toolsEnabled"] = json!(true);
        meta["disclosure"] = json!(
            "Web tools are ON. Inference stays local, but your search queries and the text of \
             fetched pages leave your machine (to DuckDuckGo, Wikipedia, arXiv, and any URL \
             fetched). Turn tools off for pure-local chat."
        );
    }
    let _ = write_sse(&mut w, &meta).await;

    let Some(cancel) = state.register(&id).await else {
        let _ = write_sse(
            &mut w,
            &json!({"type": "error", "message": "a request with this id is already running"}),
        )
        .await;
        return;
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let url = state.config.ollama_url.clone();
    // Intent Preview only PAUSES in confirm mode (still shown in others).
    let plan_enabled = autonomy == "confirm";
    let approval: Arc<dyn crate::agent::ApprovalPolicy> = Arc::new(ChannelApprovalPolicy {
        mode: autonomy,
        tx: tx.clone(),
        state: state.clone(),
        run_id: id.clone(),
    });
    let workspace_root = state.workspace_root.clone();
    let workspace_fs = state.workspace_fs.clone();
    let run = tokio::spawn(async move {
        let provider = Arc::new(OllamaProvider::new(&url));
        let mut registry = if web_disclosure {
            ToolRegistry::with_defaults()
        } else {
            ToolRegistry::new()
        };
        // Part A: if this project is indexed (graphify graph present), give the
        // agent the graph tools so it queries the graph instead of reading whole
        // files — the token win.
        let cwd = workspace_root;
        crate::graph::register_graph_tools(
            &mut registry,
            &cwd.join("graphify-out").join("graph.json"),
        );
        // Filesystem tools are sandboxed to the workspace. The host shell starts
        // there but is not OS-sandboxed; it carries a deny-list + timeout + audit
        // and interactive per-call consent in the Agent UI. Shell honors the
        // FORGE_SHELL_DISABLED kill-switch.
        registry.register(Arc::new(crate::tools::files::FsListTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(crate::tools::files::FsSearchTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(crate::tools::files::FsReadTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(crate::tools::files::FsWriteTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(crate::tools::files::FsEditTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(crate::tools::shell::ShellTool::from_workspace(
            &cwd,
            workspace_fs.clone(),
            crate::tools::shell::ShellPolicy::default(),
        )));
        // #1f Sub-agent delegation: the child gets a read-only research toolset
        // (no shell/write/delegate) so it can't recurse or mutate.
        let mut child_registry = if web_disclosure {
            ToolRegistry::with_defaults()
        } else {
            ToolRegistry::new()
        };
        crate::graph::register_graph_tools(
            &mut child_registry,
            &cwd.join("graphify-out").join("graph.json"),
        );
        child_registry.register(Arc::new(crate::tools::files::FsListTool::from_workspace(
            workspace_fs.clone(),
        )));
        child_registry.register(Arc::new(crate::tools::files::FsSearchTool::from_workspace(
            workspace_fs.clone(),
        )));
        child_registry.register(Arc::new(crate::tools::files::FsReadTool::from_workspace(
            workspace_fs.clone(),
        )));
        registry.register(Arc::new(
            crate::tools::delegate::DelegateTool::new(
                provider.clone(),
                model.clone(),
                num_ctx,
                child_registry,
            )
            .with_events(tx.clone()),
        ));
        // #1d MCP tools: connect any allowlisted MCP servers and register their
        // remote tools (the open protocol Hermes is built on). Failures are
        // logged + skipped — a broken MCP server never breaks the agent.
        if web_disclosure {
            if let Some(mcp_cfg) =
                dirs::config_dir().map(|d| d.join("ollama-forge").join("mcp_servers.json"))
            {
                crate::mcp::register_mcp_tools(&mut registry, &mcp_cfg).await;
            }
        }
        // #1 Agentic file editing: tell the agent to actually WRITE code into the
        // workspace via fs_write/fs_edit (not just print it in chat). Each such
        // call is gated by the Autonomy Dial + shown as a diff/preview before it
        // touches a file (the extension's previewEdit flow).
        let mut suffix = format!(
            "{rules_suffix}\n\n## Editing files\nWhen the user asks you to create or \
             change code/files, USE the `fs_write` (new/overwrite) and `fs_edit` \
             (precise edit) tools to write them into the workspace — give a relative \
             path and the full file content. Do NOT just print code in your answer \
             when the intent is to build something. The user reviews each change as a \
             diff before it is applied."
        );
        let mem = crate::memory::MemoryStore::for_project(&cwd).render_for_context(&question, 400);
        if !mem.is_empty() {
            // Surface recalled memory to the Agent UI (Memory drawer) before
            // appending it to the prompt.
            let _ = tx.send(json!({
                "type": "memory_used",
                "preview": mem.chars().take(400).collect::<String>(),
            }));
            suffix = format!("{suffix}\n\n{mem}");
        }
        // #1c Skills-in-the-loop: auto-apply the single most relevant skill's
        // guidance to this task (Hermes-class self-improving skills). The UI
        // shows which skill was applied via the `skill_applied` event.
        if let Some(sd) = dirs::config_dir().map(|d| d.join("ollama-forge").join("skills")) {
            let eng = crate::skills::SkillsEngine::new(sd);
            if eng.load_skills().await.is_ok() {
                if let Some(skill) = eng.best_match(&question).await {
                    suffix = format!(
                        "{suffix}\n\n## Applied skill: {}\n{}",
                        skill.name, skill.prompts.system
                    );
                    let _ = tx.send(json!({"type": "skill_applied", "name": skill.name}));
                }
            }
        }
        // Curated GitHub knowledge plugins are deliberately documentation-only:
        // load bounded, integrity-checked reference text only and make the
        // untrusted-data boundary explicit in the system suffix. A corrupt or
        // tampered cache is skipped rather than injected into the model.
        if let Some(plugin_root) =
            dirs::config_dir().map(|d| d.join("ollama-forge").join("knowledge-plugins"))
        {
            match crate::plugins::PluginManager::new(plugin_root)
                .and_then(|manager| manager.load_relevant_context(&question, 3, 12_000))
            {
                Ok(contexts) if !contexts.is_empty() => {
                    let applied = contexts
                        .iter()
                        .map(|context| {
                            json!({
                                "id": context.id,
                                "name": context.name,
                                "repository": context.repository_url,
                                "commit": context.commit_sha,
                            })
                        })
                        .collect::<Vec<_>>();
                    suffix.push_str(&crate::plugins::render_context_suffix(&contexts));
                    let _ = tx.send(json!({
                        "type": "knowledge_plugins_used",
                        "plugins": applied,
                        "trust": "untrusted_reference_only",
                    }));
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = tx.send(json!({
                        "type": "knowledge_plugin_warning",
                        "message": format!("Installed knowledge plugins were not loaded: {error:#}"),
                    }));
                }
            }
        }
        let mut agent = Agent::new(
            provider,
            registry,
            AgentConfig {
                model,
                num_ctx,
                keep_alive: "1h".to_string(),
                max_iterations,
                system_suffix: suffix,
            },
        )
        .with_approval(approval)
        .with_planning(plan_enabled);
        let result = agent
            .run(&question, |step| {
                let preview: String = step
                    .result_preview
                    .replace('\n', " ")
                    .chars()
                    .take(300)
                    .collect();
                let _ = tx.send(json!({
                    "type": "step",
                    "iteration": step.iteration,
                    "tool": step.tool,
                    "ok": step.ok,
                    "args": step.args,
                    "preview": preview,
                }));
            })
            .await;
        // #1b Memory write-back: persist a session summary at stream end so the
        // next session isn't a cold start (Hermes-class persistent memory).
        // SecurityGuard-gated — never persist anything that scans as a secret.
        // Stays entirely on the device (per-project JSONL).
        if let Ok(trace) = &result {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let pair = [
                ("user".to_string(), question.clone()),
                ("assistant".to_string(), trace.answer.clone()),
            ];
            if let Some(entry) = crate::memory::summarize_session(&pair, ts) {
                let guard = crate::security::SecurityGuard::new(true);
                if guard.scan_content(&entry.text, None).await.is_empty() {
                    let _ = crate::memory::MemoryStore::for_project(&cwd).remember(&entry);
                }
            }
        }
        result
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                run.abort();
                cancelled = true;
                let _ = write_sse(&mut w, &json!({"type": "cancelled"})).await;
                break;
            }
            ev = rx.recv() => match ev {
                Some(v) => {
                    if write_sse(&mut w, &v).await.is_err() {
                        run.abort();
                        cancelled = true;
                        break;
                    }
                }
                None => break,
            }
        }
    }
    if cancelled {
        let _ = run.await;
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }
    state.unregister(&id).await;
    state.clear_approvals_for_run(&id).await;

    match run.await {
        Ok(Ok(trace)) => {
            let _ = write_sse(
                &mut w,
                &json!({"type": "answer", "text": trace.answer, "capped": trace.iteration_capped}),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type": "done", "steps": trace.steps.len()})).await;
        }
        Ok(Err(e)) => {
            let _ = write_sse(&mut w, &json!({"type": "error", "message": e.to_string()})).await;
        }
        Err(_join) => {}
    }
}

async fn handle_build(mut w: OwnedWriteHalf, state: &Arc<ServerState>, body: &str) {
    let req: BuildReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(w, 400, &json!({"error": e.to_string()})).await;
            return;
        }
    };

    let id = req.id.clone();
    // `output_dir` comes from an HTTP client, so treat it exactly like a
    // model-supplied filesystem path. Resolve it while the server's approved
    // workspace root is still available, before spending any model work.
    let output_dir =
        match resolve_build_output_dir(&state.workspace_root, req.output_dir.as_deref()) {
            Ok(dir) => dir,
            Err(error) => {
                if write_sse_head(&mut w).await.is_ok() {
                    let _ = write_sse(
                        &mut w,
                        &json!({
                            "type": "error",
                            "id": id,
                            "message": format!("invalid build output_dir: {error:#}"),
                        }),
                    )
                    .await;
                    let _ = write_sse(&mut w, &json!({"type": "done"})).await;
                }
                return;
            }
        };
    let rules_suffix = RuleSet::load_default().unwrap_or_default().render();
    let cfg = OrchestratorConfig {
        ollama_url: state.config.ollama_url.clone(),
        default_model: state.config.default_model.clone(),
        planning_model: state.config.planning_model.clone(),
        max_parallel_workers: state.config.max_parallel_workers,
        security_enabled: state.config.security_enabled && !req.no_security,
        tdd_enforced: state.config.tdd_enforced,
        rules_suffix,
    };

    if write_sse_head(&mut w).await.is_err() {
        return;
    }
    let _ = write_sse(&mut w, &json!({"type": "meta", "id": id})).await;

    let Some(cancel) = state.register(&id).await else {
        let _ = write_sse(
            &mut w,
            &json!({"type": "error", "message": "a request with this id is already running"}),
        )
        .await;
        return;
    };
    let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<ProgressEvent>();
    let task_text = req.task.clone();
    let no_security = req.no_security;
    let output_dir_for_build = output_dir.clone();
    let run = tokio::spawn(async move {
        let orchestrator = Orchestrator::new(cfg).await?;
        let request = BuildRequest {
            task: task_text,
            output_dir: output_dir_for_build,
            language: None,
            run_tests: false,
            skip_security: no_security,
        };
        orchestrator
            .execute_with_progress(request, Some(prog_tx))
            .await
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                run.abort();
                cancelled = true;
                let _ = write_sse(&mut w, &json!({"type": "cancelled"})).await;
                break;
            }
            ev = prog_rx.recv() => match ev {
                Some(pe) => {
                    if write_sse(&mut w, &progress_event_json(&pe)).await.is_err() {
                        run.abort();
                        cancelled = true;
                        break;
                    }
                }
                None => break,
            }
        }
    }
    if cancelled {
        let _ = run.await;
        state.unregister(&id).await;
        state.clear_approvals_for_run(&id).await;
        return;
    }
    state.unregister(&id).await;
    state.clear_approvals_for_run(&id).await;

    match run.await {
        Ok(Ok(result)) => {
            // Optional: extract labeled code blocks to disk, reusing the exact
            // same path-safety guard the CLI's `--output` uses.
            let mut files: Vec<String> = Vec::new();
            let mut warnings = result.warnings;
            if let Some(dir) = &output_dir {
                match extract_and_write_code_blocks(dir, &result.output) {
                    Ok(paths) => {
                        files = paths.iter().map(|p| p.display().to_string()).collect();
                    }
                    Err(error) => warnings.push(format!(
                        "could not write build output under approved workspace directory {}: {error:#}",
                        dir.display()
                    )),
                }
            }
            let _ = write_sse(
                &mut w,
                &json!({
                    "type": "result",
                    "output": result.output,
                    "model": result.model_used,
                    "tokens": result.tokens_generated,
                    "durationMs": result.duration_ms,
                    "warnings": warnings,
                    "files": files,
                }),
            )
            .await;
            let _ = write_sse(&mut w, &json!({"type": "done"})).await;
        }
        Ok(Err(e)) => {
            let _ = write_sse(&mut w, &json!({"type": "error", "message": e.to_string()})).await;
        }
        Err(_join) => {}
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Resolve the optional **legacy text-build** output directory under the fixed
/// server workspace. This retains lexical/current symlink validation for the
/// compatibility path; Agent and Team workspace tools use the pinned
/// descriptor capability instead.
fn resolve_build_output_dir(
    workspace_root: &Path,
    requested: Option<&str>,
) -> Result<Option<PathBuf>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let requested = requested.trim();
    if requested.is_empty() {
        anyhow::bail!("output_dir must be a non-empty path relative to the workspace");
    }

    let candidate = crate::tools::files::resolve_within(workspace_root, requested)
        .with_context(|| format!("resolve output_dir `{requested}`"))?;
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("output_dir `{requested}` is a symlink and is not allowed");
        }
        Ok(metadata) if !metadata.is_dir() => {
            anyhow::bail!("output_dir `{requested}` is not a directory");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&candidate)
                .with_context(|| format!("create output_dir `{requested}`"))?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspect output_dir `{requested}`"));
        }
    }

    let metadata = std::fs::symlink_metadata(&candidate)
        .with_context(|| format!("inspect output_dir `{requested}`"))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("output_dir `{requested}` is a symlink and is not allowed");
    }
    if !metadata.is_dir() {
        anyhow::bail!("output_dir `{requested}` is not a directory");
    }

    let canonical_workspace = workspace_root
        .canonicalize()
        .context("canonicalize approved workspace root")?;
    let canonical_output = candidate
        .canonicalize()
        .with_context(|| format!("canonicalize output_dir `{requested}`"))?;
    if !canonical_output.starts_with(&canonical_workspace) {
        anyhow::bail!("output_dir `{requested}` resolves outside the approved workspace");
    }
    Ok(Some(canonical_output))
}

fn progress_event_json(ev: &ProgressEvent) -> Value {
    match ev {
        ProgressEvent::PreloadStarted { model } => {
            json!({"type": "progress", "kind": "preload_started", "model": model})
        }
        ProgressEvent::PreloadFinished {
            model,
            ok,
            elapsed_ms,
        } => json!({
            "type": "progress", "kind": "preload_finished",
            "model": model, "ok": ok, "elapsedMs": elapsed_ms,
        }),
        ProgressEvent::WorkerStarted {
            subtask_id,
            subtask_name,
            model,
        } => json!({
            "type": "progress", "kind": "worker_started",
            "subtaskId": subtask_id, "subtask": subtask_name, "model": model,
        }),
        ProgressEvent::WorkerFinished {
            subtask_id,
            subtask_name,
            ok,
            elapsed_ms,
            tokens,
        } => json!({
            "type": "progress", "kind": "worker_finished",
            "subtaskId": subtask_id, "subtask": subtask_name,
            "ok": ok, "elapsedMs": elapsed_ms, "tokens": tokens,
        }),
    }
}

/// Flatten a conversation (+ attached context) into a single `/api/generate`
/// prompt. This reuses [`OllamaProvider::generate_streaming`] verbatim
/// instead of adding a streaming `/api/chat` path. Multi-turn works fine for
/// instruct models; switching to native chat-message streaming is a clean
/// follow-up.
fn build_chat_prompt(
    messages: &[ChatMsg],
    prompt: Option<&str>,
    context: &[ContextItem],
) -> String {
    let mut s = String::new();
    if !context.is_empty() {
        s.push_str("## Attached context\n\n");
        for c in context {
            // Image-only items carry no text — emit a marker rather than an empty
            // code fence (review #21). The bytes travel via opts.images, not text.
            if c.content.is_empty() && c.image.is_some() {
                let name = if c.path.is_empty() {
                    "image"
                } else {
                    c.path.as_str()
                };
                s.push_str(&format!("[image attached: {name}]\n\n"));
                continue;
            }
            if c.path.is_empty() {
                s.push_str("```\n");
            } else {
                s.push_str(&format!("File: {}\n```\n", c.path));
            }
            s.push_str(&c.content);
            s.push_str("\n```\n\n");
        }
    }
    for m in messages {
        let who = match m.role.as_str() {
            "assistant" => "Assistant",
            "system" => "System",
            _ => "User",
        };
        s.push_str(&format!("{who}: {}\n\n", m.content));
    }
    if let Some(p) = prompt {
        s.push_str(&format!("User: {p}\n\n"));
    }
    s.push_str("Assistant:");
    s
}

/// The text used to classify task complexity for Auto routing: the most recent
/// user message, falling back to the one-shot prompt.
fn latest_user_text(messages: &[ChatMsg], prompt: Option<&str>) -> String {
    if let Some(m) = messages.iter().rev().find(|m| m.role == "user") {
        return m.content.clone();
    }
    prompt.unwrap_or("").to_string()
}

/// Auto-routing: classify the task with the existing `TaskRouter` and pick a
/// model from the *installed* set (local-only by construction — `route_to_model`
/// only ever returns an installed Ollama model). Returns the chosen model plus a
/// JSON object describing the decision for the UI. Never escalates to cloud.
async fn route_auto(task: &str, installed: &[ModelInfo], fallback: &str) -> (String, Value) {
    if installed.is_empty() {
        return (
            fallback.to_string(),
            json!({"auto": true, "available": false, "model": fallback,
                   "reasoning": format!("Auto: no installed models; using default {fallback}")}),
        );
    }
    let router = TaskRouter::new(Default::default());
    let complexity = match router.analyze_complexity(task, installed).await {
        Ok(c) => c,
        Err(_) => {
            return (
                fallback.to_string(),
                json!({"auto": true, "available": true, "model": fallback,
                       "reasoning": "Auto: complexity analysis failed; using default"}),
            );
        }
    };
    // Decisive tier mapping: classify with the router, then pick from the
    // *installed* models sorted by size so the choice is monotonic — simple →
    // smallest, architect → largest. This is the heterogeneous-routing intent
    // made explicit (the raw `select_model_for_task` can grab a small "coder"
    // model for a complex task via substring matching, which isn't decisive).
    let mut by_size: Vec<&ModelInfo> = installed.iter().collect();
    by_size.sort_by_key(|m| m.size);
    let n = by_size.len();
    let idx = match complexity.task_type {
        TaskType::Simple => 0,
        TaskType::Medium => n / 3,
        TaskType::Complex => (2 * n) / 3,
        TaskType::Architect => n - 1,
    };
    let model = by_size[idx.min(n - 1)].name.clone();
    let task_type = format!("{:?}", complexity.task_type);
    let reasoning = format!(
        "Auto: {task_type} task (score {:.2}) → {model}",
        complexity.score
    );
    (
        model.clone(),
        json!({"auto": true, "available": true, "model": model,
               "taskType": task_type, "score": complexity.score, "reasoning": reasoning}),
    )
}

/// Trim conversation history to fit a token budget, dropping the *oldest*
/// messages first (a sliding window). Attached `context` is reserved first
/// because it's an explicit user action. Returns the kept messages (in
/// original order) and how many were dropped, so the caller can tell the user
/// instead of letting Ollama silently truncate. Reuses the real BPE estimator
/// in [`crate::context::estimate_tokens`].
fn budget_messages(
    messages: &[ChatMsg],
    context: &[ContextItem],
    max_tokens: usize,
) -> (Vec<ChatMsg>, usize) {
    use crate::context::estimate_tokens;
    let ctx_tokens: usize = context.iter().map(|c| estimate_tokens(&c.content)).sum();
    let mut budget = max_tokens.saturating_sub(ctx_tokens);
    let mut kept_rev: Vec<ChatMsg> = Vec::new();
    // Walk newest → oldest, keeping while they fit. Always keep at least the
    // most recent message even if it alone exceeds the budget (Ollama will
    // truncate it, but dropping the user's current turn would be worse).
    for m in messages.iter().rev() {
        let cost = estimate_tokens(&m.content) + 8; // small per-message overhead
        if kept_rev.is_empty() || cost <= budget {
            budget = budget.saturating_sub(cost);
            kept_rev.push(m.clone());
        } else {
            break;
        }
    }
    let dropped = messages.len() - kept_rev.len();
    kept_rev.reverse();
    (kept_rev, dropped)
}

/// Pick the model to run when the request didn't specify one: prefer the
/// configured default if installed, else the largest installed model, else
/// fall back to the configured default name.
async fn pick_model(config: &Config, ollama: &OllamaProvider) -> String {
    match ollama.list_models().await {
        Ok(models) => {
            if models.iter().any(|m| m.name == config.default_model) {
                return config.default_model.clone();
            }
            models
                .iter()
                .max_by_key(|m| m.size)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| config.default_model.clone())
        }
        Err(_) => config.default_model.clone(),
    }
}

/// Append a replay record if `FORGE_REPLAY_LOG` is set. Mirrors the CLI's
/// `maybe_log_replay` in `main.rs` so chat through the server is replayable
/// exactly like chat through the terminal.
async fn maybe_log_replay(opts: &GenerateOptions, response: &str, ollama_url: &str) {
    let Ok(path) = std::env::var("FORGE_REPLAY_LOG") else {
        return;
    };
    let provider = OllamaProvider::new(ollama_url);
    let digest = provider.model_digest(&opts.model).await.unwrap_or_default();
    let log = ReplayLog::new(PathBuf::from(path));
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
        model_digest: digest,
        temperature: opts.temperature,
        top_p: opts.top_p,
        num_ctx: opts.num_ctx,
        keep_alive: opts.keep_alive.clone(),
        seed: opts.seed,
        format: opts.format.clone(),
        system: opts.system.clone(),
        prompt: opts.prompt.clone(),
        prompt_hash: quick_hash(prompt_material.as_bytes()),
        response_hash: quick_hash(response.as_bytes()),
        response: response.chars().take(16_384).collect(),
    };
    if let Err(e) = log.append(&record).await {
        warn!("forge serve: replay append failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_output_dir_is_confined_to_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let output = workspace.path().join("generated");
        std::fs::create_dir(&output).unwrap();

        let resolved = resolve_build_output_dir(workspace.path(), Some("generated"))
            .unwrap()
            .unwrap();
        assert_eq!(resolved, output.canonicalize().unwrap());

        let created = resolve_build_output_dir(workspace.path(), Some("new-generated"))
            .unwrap()
            .unwrap();
        assert!(created.is_dir());
        assert!(resolve_build_output_dir(workspace.path(), Some("../outside")).is_err());
        assert!(resolve_build_output_dir(workspace.path(), Some("")).is_err());

        let outside = tempfile::tempdir().unwrap();
        assert!(
            resolve_build_output_dir(workspace.path(), Some(outside.path().to_str().unwrap()))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_output_dir_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), workspace.path().join("linked-output")).unwrap();

        assert!(resolve_build_output_dir(workspace.path(), Some("linked-output")).is_err());
    }

    #[test]
    fn parses_get_request_head() {
        let raw = "GET /api/models?x=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n";
        let head = parse_head(raw).expect("should parse");
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/api/models?x=1");
        assert_eq!(
            head.headers.get("host").map(String::as_str),
            Some("localhost")
        );
        assert_eq!(head.content_length(), Some(0));
    }

    #[test]
    fn parses_content_length_for_post() {
        let raw = "POST /api/chat HTTP/1.1\nContent-Length: 42\n";
        let head = parse_head(raw).expect("should parse");
        assert_eq!(head.method, "POST");
        assert_eq!(head.content_length(), Some(42));
    }

    #[test]
    fn rejects_empty_head() {
        assert!(parse_head("").is_none());
    }

    #[test]
    fn sse_frame_is_single_data_line() {
        let frame = sse_frame(&json!({"type": "token", "text": "hi"}));
        assert!(frame.starts_with("data: "));
        assert!(frame.ends_with("\n\n"));
        assert!(frame.contains("\"text\":\"hi\""));
        // Exactly one data line (compact JSON has no embedded newlines).
        assert_eq!(frame.matches("data:").count(), 1);
    }

    #[test]
    fn query_param_decodes_model_names() {
        assert_eq!(
            query_param("/api/model_info?name=qwen2.5-coder%3A7b", "name").as_deref(),
            Some("qwen2.5-coder:7b")
        );
        assert_eq!(
            query_param("/api/model_info?foo=1&name=llama3.2", "name").as_deref(),
            Some("llama3.2")
        );
        assert_eq!(query_param("/api/model_info", "name"), None);
    }

    #[test]
    fn extract_context_length_finds_arch_prefixed_key() {
        let doc = json!({
            "model_info": { "general.architecture": "qwen2", "qwen2.context_length": 32768 }
        });
        assert_eq!(extract_context_length(&doc), Some(32768));
        assert_eq!(extract_context_length(&json!({"model_info": {}})), None);
        assert_eq!(extract_context_length(&json!({})), None);
    }

    #[test]
    fn sanitize_host_forces_loopback() {
        assert_eq!(sanitize_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(sanitize_host("localhost"), "localhost");
        assert_eq!(sanitize_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(sanitize_host("8.8.8.8"), "127.0.0.1");
    }

    fn fake_model(name: &str, size: u64) -> ModelInfo {
        ModelInfo {
            name: name.to_string(),
            size,
            size_human: String::new(),
            modified_at: String::new(),
            digest: String::new(),
        }
    }

    #[tokio::test]
    async fn auto_routes_simple_to_small_complex_to_large() {
        let models = vec![
            fake_model("qwen2.5-coder:1.5b", 1_000_000_000),
            fake_model("qwen2.5-coder:7b", 5_000_000_000),
            fake_model("llama3.3:70b", 40_000_000_000),
        ];
        let (simple_model, simple_meta) =
            route_auto("rename all .txt files to .md", &models, "qwen2.5-coder:7b").await;
        let (complex_model, complex_meta) = route_auto(
            "design a distributed microservices architecture with an API gateway and auth",
            &models,
            "qwen2.5-coder:7b",
        )
        .await;
        // Simple should land on a smaller model than the complex/architect task.
        assert_ne!(
            simple_model, complex_model,
            "routing should differ by complexity"
        );
        assert!(simple_meta["auto"].as_bool().unwrap());
        assert!(complex_meta["reasoning"]
            .as_str()
            .unwrap()
            .starts_with("Auto:"));
        // The architect task should route to the largest installed model.
        assert_eq!(complex_model, "llama3.3:70b");
    }

    #[tokio::test]
    async fn auto_falls_back_when_no_models_installed() {
        let (model, meta) = route_auto("anything", &[], "qwen2.5-coder:7b").await;
        assert_eq!(model, "qwen2.5-coder:7b");
        assert_eq!(meta["available"].as_bool(), Some(false));
    }

    #[test]
    fn latest_user_text_prefers_last_user_message() {
        let msgs = vec![
            ChatMsg {
                role: "user".into(),
                content: "first".into(),
            },
            ChatMsg {
                role: "assistant".into(),
                content: "reply".into(),
            },
            ChatMsg {
                role: "user".into(),
                content: "second".into(),
            },
        ];
        assert_eq!(latest_user_text(&msgs, None), "second");
        assert_eq!(latest_user_text(&[], Some("p")), "p");
    }

    #[test]
    fn budget_keeps_recent_drops_oldest() {
        let msgs: Vec<ChatMsg> = (0..12)
            .map(|i| ChatMsg {
                role: "user".into(),
                content: format!("message number {i} with several words of content here"),
            })
            .collect();
        // Tiny budget forces dropping older messages.
        let (kept, dropped) = budget_messages(&msgs, &[], 30);
        assert!(kept.len() < msgs.len(), "should have trimmed something");
        assert_eq!(dropped, msgs.len() - kept.len());
        assert!(!kept.is_empty(), "must always keep the most recent message");
        // The most recent message must survive; the oldest must be gone.
        assert!(kept.last().unwrap().content.contains("number 11"));
        assert!(!kept.iter().any(|m| m.content.contains("number 0 ")));
    }

    #[test]
    fn budget_keeps_everything_when_it_fits() {
        let msgs = vec![
            ChatMsg {
                role: "user".into(),
                content: "hi".into(),
            },
            ChatMsg {
                role: "assistant".into(),
                content: "hello".into(),
            },
        ];
        let (kept, dropped) = budget_messages(&msgs, &[], 100_000);
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn chat_prompt_includes_context_and_turns() {
        let msgs = vec![ChatMsg {
            role: "user".into(),
            content: "hello".into(),
        }];
        let ctx = vec![ContextItem {
            path: "src/x.rs".into(),
            content: "fn x() {}".into(),
            image: None,
        }];
        let p = build_chat_prompt(&msgs, None, &ctx);
        assert!(p.contains("Attached context"));
        assert!(p.contains("src/x.rs"));
        assert!(p.contains("User: hello"));
        assert!(p.trim_end().ends_with("Assistant:"));
    }
}

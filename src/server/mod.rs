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
//! - `GET  /api/models`    → installed models (reuses `list_models`)
//! - `POST /api/chat`      → SSE: `meta` → `token`* → (`done` | `error` | `cancelled`)
//! - `POST /api/research`  → SSE: `meta` → `step`* → `answer` → (`done` | …)
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
use crate::tools::ToolRegistry;
use crate::Config;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{info, warn};

/// Shared state for the running server.
pub struct ServerState {
    config: Config,
    version: &'static str,
    /// In-flight request id → cancel signal. `Notify::notify_one` stores a
    /// permit, so a cancel that arrives a hair before the handler starts
    /// awaiting is not lost.
    cancels: Mutex<HashMap<String, Arc<Notify>>>,
}

impl ServerState {
    async fn register(&self, id: &str) -> Arc<Notify> {
        let n = Arc::new(Notify::new());
        self.cancels.lock().await.insert(id.to_string(), n.clone());
        n
    }
    async fn unregister(&self, id: &str) {
        self.cancels.lock().await.remove(id);
    }
    async fn cancel(&self, id: &str) -> bool {
        match self.cancels.lock().await.get(id) {
            Some(n) => {
                n.notify_one();
                true
            }
            None => false,
        }
    }
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

    // Machine-readable line the VSCode extension parses to discover the port
    // (it launches `forge serve --port 0` and reads this from stdout).
    println!(
        "FORGE_SERVE_READY {}",
        json!({ "host": addr.ip().to_string(), "port": addr.port(), "version": crate::cli::VERSION })
    );
    eprintln!("forge serve listening on http://{addr}  (local-only; Ctrl-C to stop)");

    let state = Arc::new(ServerState {
        config,
        version: crate::cli::VERSION,
        cancels: Mutex::new(HashMap::new()),
    });
    serve(listener, state).await
}

/// Serve on an already-bound listener. Exposed so tests can bind an ephemeral
/// `127.0.0.1:0` port and drive the protocol without a live Ollama.
pub async fn serve_listener(listener: TcpListener, config: Config) -> Result<()> {
    let state = Arc::new(ServerState {
        config,
        version: crate::cli::VERSION,
        cancels: Mutex::new(HashMap::new()),
    });
    serve(listener, state).await
}

async fn serve(listener: TcpListener, state: Arc<ServerState>) -> Result<()> {
    info!("forge serve accepting connections");
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
    fn content_length(&self) -> usize {
        self.headers
            .get("content-length")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
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
        404 => "Not Found",
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
         Access-Control-Allow-Headers: Content-Type\r\n\
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
        head_text.push_str(&line);
    }

    let Some(head) = parse_head(&head_text) else {
        let _ = write_json(write_half, 400, &json!({"error": "malformed request"})).await;
        return;
    };

    // Read the body (Content-Length bytes) from the same buffered reader.
    let content_length = head.content_length();
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    route(head, body, write_half, state).await;
}

async fn route(head: RequestHead, body: String, w: OwnedWriteHalf, state: &Arc<ServerState>) {
    let path = head.path.split('?').next().unwrap_or("").to_string();
    match (head.method.as_str(), path.as_str()) {
        ("OPTIONS", _) => {
            let _ = write_cors_preflight(w).await;
        }
        ("GET", "/health") => {
            let _ = write_json(w, 200, &json!({"ok": true, "version": state.version})).await;
        }
        ("GET", "/api/status") => handle_status(w, state).await,
        ("GET", "/api/models") => handle_models(w, state).await,
        ("GET", "/api/models/catalog") => {
            handle_models_catalog(w, state, query_param(&head.path, "verify")).await
        }
        ("GET", "/api/model_info") => {
            handle_model_info(w, state, query_param(&head.path, "name")).await
        }
        ("GET", "/api/memory") => handle_memory_list(w).await,
        ("POST", "/api/memory/clear") => handle_memory_clear(w).await,
        ("GET", "/api/graph/status") => handle_graph_status(w).await,
        ("POST", "/api/graph/build") => handle_graph_build(w, &body).await,
        ("POST", "/api/chat") => handle_chat(w, state, &body).await,
        ("POST", "/api/research") => handle_research(w, state, &body).await,
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
            json!({ "models": [], "default": state.config.default_model, "error": e.to_string() })
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

// --- Part B: on-device conversational memory (view/clear). The store is local
// per-project; nothing here ever reaches the identity backend. ----------------
fn project_cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

async fn handle_memory_list(w: OwnedWriteHalf) {
    let store = crate::memory::MemoryStore::for_project(&project_cwd());
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

async fn handle_memory_clear(w: OwnedWriteHalf) {
    let store = crate::memory::MemoryStore::for_project(&project_cwd());
    let ok = store.clear().is_ok();
    let _ = write_json(w, 200, &json!({ "ok": ok })).await;
}

// --- Part A: code knowledge graph status + build (graphify managed builder). ---
async fn handle_graph_status(w: OwnedWriteHalf) {
    let idx = crate::graph::GraphIndex::new(project_cwd());
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

async fn handle_graph_build(w: OwnedWriteHalf, body: &str) {
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
    let idx = crate::graph::GraphIndex::new(project_cwd());
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
        let mem = crate::memory::MemoryStore::for_project(&project_cwd()).render_for_context(&task_text, 400);
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

    // Feature 2: web tools in NORMAL chat (opt-in). Reuse the exact same agent
    // loop as /api/research over the flattened conversation — the model decides
    // when to search/fetch, steps stream to the UI, and the meta event discloses
    // the egress. Off by default = pure-local chat (the plain path below).
    if req.tools == Some(true) {
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
        )
        .await;
        return;
    }

    // #7 Vision: collect any attached images (base64) and, if present, ensure
    // the model can actually see — otherwise warn the user to switch.
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
        images: if images.is_empty() { None } else { Some(images) },
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

    let cancel = state.register(&id).await;
    let url = state.config.ollama_url.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let full = Arc::new(std::sync::Mutex::new(String::new()));
    let full_for_task = full.clone();
    let gen_opts = opts.clone();
    let gen = tokio::spawn(async move {
        let provider = OllamaProvider::new(&url);
        provider
            .generate_streaming(gen_opts, move |chunk| {
                let _ = tx.send(chunk.to_string());
                if let Ok(mut g) = full_for_task.lock() {
                    g.push_str(chunk);
                }
            })
            .await
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.notified() => {
                gen.abort();
                cancelled = true;
                let _ = write_sse(&mut w, &json!({"type": "cancelled"})).await;
                break;
            }
            msg = rx.recv() => match msg {
                Some(chunk) => {
                    if write_sse(&mut w, &json!({"type": "token", "text": chunk})).await.is_err() {
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
    let model = match req.model {
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
        .unwrap_or(crate::agent::DEFAULT_MAX_ITERATIONS);
    // The dedicated research mode is web-enabled by definition; the meta event
    // discloses the egress just like the chat tools toggle does.
    run_agent_streamed(
        w, state, id, model, question, num_ctx, max_iterations, rules_suffix, true,
        Vec::new(), Value::Null, 0,
    )
    .await;
}

/// Shared agent-loop streamer used by BOTH `/api/research` and `/api/chat` (when
/// its `tools` toggle is on) — the web tools are the *same* system surfaced in
/// two places, not a fork. Streams `step` events as tools are called, then a
/// final `answer` + `done`. When `web_disclosure` is set, the `meta` event
/// states that queries/fetched pages leave the machine even though inference
/// stays local.
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

    let cancel = state.register(&id).await;
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let url = state.config.ollama_url.clone();
    let run = tokio::spawn(async move {
        let provider = Arc::new(OllamaProvider::new(&url));
        let mut registry = ToolRegistry::with_defaults();
        // Part A: if this project is indexed (graphify graph present), give the
        // agent the graph tools so it queries the graph instead of reading whole
        // files — the token win.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        crate::graph::register_graph_tools(&mut registry, &cwd.join("graphify-out").join("graph.json"));
        // Part B: prepend on-device memory relevant to this question (token-
        // budgeted) so the session isn't a cold start. Stays on the device.
        let mut suffix = rules_suffix;
        let mem = crate::memory::MemoryStore::for_project(&cwd).render_for_context(&question, 400);
        if !mem.is_empty() {
            suffix = format!("{suffix}\n\n{mem}");
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
        );
        agent
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
            .await
    });

    let mut cancelled = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.notified() => {
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
    state.unregister(&id).await;
    if cancelled {
        return;
    }

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

    let cancel = state.register(&id).await;
    let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<ProgressEvent>();
    let task_text = req.task.clone();
    let no_security = req.no_security;
    let run = tokio::spawn(async move {
        let orchestrator = Orchestrator::new(cfg).await?;
        let request = BuildRequest {
            task: task_text,
            output_dir: None,
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
            _ = cancel.notified() => {
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
    state.unregister(&id).await;
    if cancelled {
        return;
    }

    match run.await {
        Ok(Ok(result)) => {
            // Optional: extract labeled code blocks to disk, reusing the exact
            // same path-safety guard the CLI's `--output` uses.
            let mut files: Vec<String> = Vec::new();
            if let Some(dir) = &req.output_dir {
                if let Ok(paths) =
                    extract_and_write_code_blocks(std::path::Path::new(dir), &result.output)
                {
                    files = paths.iter().map(|p| p.display().to_string()).collect();
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
                    "warnings": result.warnings,
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
    fn parses_get_request_head() {
        let raw = "GET /api/models?x=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n";
        let head = parse_head(raw).expect("should parse");
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/api/models?x=1");
        assert_eq!(
            head.headers.get("host").map(String::as_str),
            Some("localhost")
        );
        assert_eq!(head.content_length(), 0);
    }

    #[test]
    fn parses_content_length_for_post() {
        let raw = "POST /api/chat HTTP/1.1\nContent-Length: 42\n";
        let head = parse_head(raw).expect("should parse");
        assert_eq!(head.method, "POST");
        assert_eq!(head.content_length(), 42);
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

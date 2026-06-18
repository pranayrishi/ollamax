//! Minimal MCP (Model Context Protocol) client — stdio transport.
//!
//! MCP is the open protocol Hermes is built on; speaking it natively is how forge
//! gains remote tools (GitHub, filesystem, databases, browsers, …) WITHOUT
//! bundling a Python agent. This is a small, dependency-light JSON-RPC 2.0 client
//! over newline-delimited stdio (the MCP stdio framing): it spawns a configured
//! server, performs the `initialize` handshake, lists the server's tools, and
//! wraps each as a forge [`Tool`] whose `invoke` issues `tools/call`.
//!
//! Servers are declared in `mcp_servers.json` under the config dir and gated by
//! an explicit `allowlist` — only listed servers are ever launched.
//!
//! HONEST SCOPE: the config parsing, JSON-RPC framing, and tool mapping are unit
//! tested; the live process handshake is not validated in CI (no MCP server is
//! available here). Connection failures are logged and skipped — they never break
//! the agent.

use super::tools::{truncate_for_model, Tool, ToolResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
    /// Only servers whose key appears here are launched. Empty = none.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

impl McpConfig {
    pub fn load(path: &Path) -> McpConfig {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// (name, config) pairs that are both defined AND allowlisted.
    pub fn allowed(&self) -> Vec<(String, McpServerConfig)> {
        self.allowlist
            .iter()
            .filter_map(|n| self.servers.get(n).map(|c| (n.clone(), c.clone())))
            .collect()
    }
}

/// Build a JSON-RPC 2.0 request envelope.
pub fn jsonrpc_request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Extract the `result` from a JSON-RPC response, or an error string.
pub fn jsonrpc_result(resp: &Value) -> Result<Value> {
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("mcp error: {err}"));
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| anyhow!("mcp response missing result"))
}

/// Flatten an MCP `tools/call` result's `content` array into plain text.
pub fn flatten_content(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(|c| c.as_array()) else {
        return result.to_string();
    };
    let mut out = String::new();
    for it in items {
        if let Some(t) = it.get("text").and_then(|t| t.as_str()) {
            out.push_str(t);
            out.push('\n');
        } else {
            out.push_str(&it.to_string());
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

struct Conn {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Best-effort: kill the server process when the client goes away.
        let _ = self.child.start_kill();
    }
}

impl Conn {
    /// Send a request and read newline-delimited responses until the one with a
    /// matching id arrives (skipping notifications / mismatched ids).
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&jsonrpc_request(id, method, params))?;
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self.stdout.read_line(&mut buf).await?;
            if n == 0 {
                return Err(anyhow!("mcp server closed the connection"));
            }
            let Ok(v) = serde_json::from_str::<Value>(buf.trim()) else {
                continue;
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                return jsonrpc_result(&v);
            }
            // else: a notification or another id — keep reading.
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = serde_json::to_string(&json!({"jsonrpc":"2.0","method":method,"params":params}))?;
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

pub struct McpClient {
    conn: Mutex<Conn>,
}

impl McpClient {
    /// Spawn the server and perform the MCP initialize handshake.
    pub async fn connect(cfg: &McpServerConfig) -> Result<Self> {
        let mut cmd = tokio::process::Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| anyhow!("spawn {}: {e}", cfg.command))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let mut conn = Conn { child, stdin, stdout: BufReader::new(stdout), next_id: 1 };
        // initialize handshake
        conn.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "forge", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        conn.notify("notifications/initialized", json!({})).await?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// `tools/list` → (name, description, input_schema) triples.
    pub async fn list_tools(&self) -> Result<Vec<(String, String, Value)>> {
        let res = self.conn.lock().await.request("tools/list", json!({})).await?;
        let tools = res.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
                let schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"}));
                Some((name, desc, schema))
            })
            .collect())
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<String> {
        let res = self
            .conn
            .lock()
            .await
            .request("tools/call", json!({ "name": name, "arguments": args }))
            .await?;
        Ok(flatten_content(&res))
    }
}

/// A forge Tool backed by a remote MCP tool. The name is prefixed with the
/// server key (`mcp__github__create_issue`) to avoid collisions.
pub struct McpTool {
    full_name: String,
    remote_name: String,
    description: String,
    schema: Value,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn args_schema(&self) -> Value {
        self.schema.clone()
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        match self.client.call_tool(&self.remote_name, args).await {
            Ok(text) => Ok(ToolResult {
                tool: self.full_name.clone(),
                ok: true,
                content: truncate_for_model(&text),
            }),
            Err(e) => Ok(ToolResult { tool: self.full_name.clone(), ok: false, content: e.to_string() }),
        }
    }
}

/// Connect every allowlisted MCP server and register its tools into `registry`.
/// Failures are logged and skipped — a broken MCP server never breaks the agent.
pub async fn register_mcp_tools(registry: &mut super::tools::ToolRegistry, config_path: &Path) {
    let cfg = McpConfig::load(config_path);
    for (server, sc) in cfg.allowed() {
        match McpClient::connect(&sc).await {
            Ok(client) => {
                let client = Arc::new(client);
                match client.list_tools().await {
                    Ok(tools) => {
                        for (name, desc, schema) in tools {
                            registry.register(Arc::new(McpTool {
                                full_name: format!("mcp__{server}__{name}"),
                                remote_name: name,
                                description: desc,
                                schema,
                                client: client.clone(),
                            }));
                        }
                    }
                    Err(e) => tracing::warn!("mcp {server}: tools/list failed: {e}"),
                }
            }
            Err(e) => tracing::warn!("mcp {server}: connect failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_allowlist_filters() {
        let cfg: McpConfig = serde_json::from_str(
            r#"{"servers":{"github":{"command":"npx","args":["-y","x"]},"db":{"command":"y"}},"allowlist":["github"]}"#,
        )
        .unwrap();
        let allowed = cfg.allowed();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].0, "github");
        assert_eq!(allowed[0].1.command, "npx");
    }

    #[test]
    fn missing_config_is_empty_not_error() {
        let cfg = McpConfig::load(Path::new("/nonexistent/mcp_servers.json"));
        assert!(cfg.allowed().is_empty());
    }

    #[test]
    fn jsonrpc_roundtrip_framing() {
        let req = jsonrpc_request(7, "tools/list", json!({}));
        assert_eq!(req["id"], 7);
        assert_eq!(req["method"], "tools/list");
        assert_eq!(req["jsonrpc"], "2.0");
        let ok = json!({"jsonrpc":"2.0","id":7,"result":{"tools":[]}});
        assert!(jsonrpc_result(&ok).is_ok());
        let errd = json!({"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"nope"}});
        assert!(jsonrpc_result(&errd).is_err());
    }

    #[test]
    fn flatten_content_joins_text_blocks() {
        let r = json!({"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]});
        assert_eq!(flatten_content(&r), "hello\nworld");
    }
}

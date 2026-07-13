//! Sandboxed shell tool for the autonomous Agent.
//!
//! Power comes with the widest trust surface, so this tool ships three hard
//! guardrails independent of the UI's per-command consent (the Autonomy Dial):
//!   1. **policy** — a deny-list of catastrophic patterns (rm -rf /, sudo, mkfs,
//!      fork bombs, pipe-to-shell installers, …) + an `enabled` kill-switch
//!      (also honored via the `FORGE_SHELL_DISABLED` env var).
//!   2. **timeout** — the child is killed if it runs past the budget.
//!   3. **audit** — every command + exit status is appended to an on-device
//!      audit log under the config dir; nothing leaves the machine.
//!
//! Commands run in the workspace root via the platform shell.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct ShellPolicy {
    pub enabled: bool,
    pub timeout: Duration,
}
impl Default for ShellPolicy {
    fn default() -> Self {
        Self { enabled: true, timeout: Duration::from_secs(30) }
    }
}

/// Catastrophic patterns refused before execution. Not a complete security
/// boundary (a determined model can obfuscate) — it's a guardrail against the
/// obvious footguns; the real boundary is the UI consent gate + this being
/// opt-in. Conservative + readable beats clever.
const DENY: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    ":(){",      // fork bomb
    "mkfs",
    "dd if=",
    "> /dev/sd",
    "shutdown",
    "reboot",
    "sudo ",
    "doas ",
    "chmod -r 777 /",
    "curl ",     // block network-fetch-then-run patterns; fetch_url tool is the sanctioned path
    "wget ",
];

fn denied(cmd: &str) -> Option<&'static str> {
    let lc = cmd.to_lowercase();
    DENY.iter().copied().find(|p| lc.contains(p))
}

pub struct ShellTool {
    root: PathBuf,
    policy: ShellPolicy,
}
impl ShellTool {
    pub fn new(root: impl Into<PathBuf>, policy: ShellPolicy) -> Self {
        Self { root: root.into(), policy }
    }

    fn audit(&self, cmd: &str, status: &str) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let Some(dir) = dirs::config_dir() else { return };
        let dir = dir.join("ollama-forge");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("shell-audit.log"))
        {
            let _ = writeln!(f, "{ts}\t{status}\t{}", cmd.replace('\n', " "));
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn description(&self) -> &str {
        "Run a shell command in the workspace and return its output. Dangerous commands are refused; runs are time-limited and audited. args: {command: string}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let mk = |ok: bool, content: String| ToolResult { tool: "shell".to_string(), ok, content };

        if !self.policy.enabled || std::env::var_os("FORGE_SHELL_DISABLED").is_some() {
            return Ok(mk(false, "shell tool is disabled".into()));
        }
        if cmd.is_empty() {
            return Ok(mk(false, "command is empty".into()));
        }
        if let Some(p) = denied(&cmd) {
            self.audit(&cmd, "DENIED");
            return Ok(mk(false, format!("refused: command matches a blocked pattern ('{p}')")));
        }

        #[cfg(windows)]
        let mut command = {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&cmd);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&cmd);
            c
        };
        command.current_dir(&self.root).kill_on_drop(true);

        // kill_on_drop means a timeout drops the future and kills the child.
        let result = tokio::time::timeout(self.policy.timeout, command.output()).await;
        match result {
            Err(_) => {
                self.audit(&cmd, "TIMEOUT");
                Ok(mk(false, format!("command timed out after {}s (killed)", self.policy.timeout.as_secs())))
            }
            Ok(Err(e)) => {
                self.audit(&cmd, "ERROR");
                Ok(mk(false, format!("could not run command: {e}")))
            }
            Ok(Ok(out)) => {
                let code = out.status.code().unwrap_or(-1);
                self.audit(&cmd, &format!("exit={code}"));
                let mut body = String::new();
                if !out.stdout.is_empty() {
                    body.push_str(&String::from_utf8_lossy(&out.stdout));
                }
                if !out.stderr.is_empty() {
                    body.push_str("\n[stderr]\n");
                    body.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                if body.trim().is_empty() {
                    body = format!("(no output; exit {code})");
                }
                Ok(mk(out.status.success(), truncate_for_model(&body)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_a_safe_command() {
        let t = ShellTool::new(std::env::temp_dir(), ShellPolicy::default());
        let r = t.invoke(json!({"command":"echo forge-ok"})).await.unwrap();
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("forge-ok"));
    }

    #[tokio::test]
    async fn refuses_dangerous_command() {
        let t = ShellTool::new(std::env::temp_dir(), ShellPolicy::default());
        let r = t.invoke(json!({"command":"rm -rf / --no-preserve-root"})).await.unwrap();
        assert!(!r.ok && r.content.contains("refused"));
    }

    #[tokio::test]
    async fn disabled_policy_blocks() {
        let pol = ShellPolicy { enabled: false, ..Default::default() };
        let t = ShellTool::new(std::env::temp_dir(), pol);
        let r = t.invoke(json!({"command":"echo hi"})).await.unwrap();
        assert!(!r.ok && r.content.contains("disabled"));
    }

    #[tokio::test]
    async fn timeout_kills_long_command() {
        let pol = ShellPolicy { enabled: true, timeout: Duration::from_millis(200) };
        let t = ShellTool::new(std::env::temp_dir(), pol);
        let command = if cfg!(windows) {
            // cmd.exe has no `sleep`; ping's interval makes a portable long
            // command without depending on PowerShell availability.
            "ping 127.0.0.1 -n 6 >NUL"
        } else {
            "sleep 5"
        };
        let r = t.invoke(json!({"command":command})).await.unwrap();
        assert!(!r.ok && r.content.contains("timed out"));
    }
}

//! Guardrailed host-shell tool for the autonomous Agent.
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
//! Commands start in the workspace root via the platform shell. This is **not**
//! an OS/container sandbox: a command can still reference paths outside that
//! directory. On macOS/Linux a timeout or cancelled task kills the shell's
//! isolated session/process group (including ordinary descendants); on Windows it can
//! only terminate the direct child without a Job Object. The deny-list,
//! timeout, audit log, and approval layer are guardrails, not containment.

use super::{files::WorkspaceFs, truncate_for_model, Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct ShellPolicy {
    pub enabled: bool,
    pub timeout: Duration,
}
impl Default for ShellPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: Duration::from_secs(30),
        }
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
    ":(){", // fork bomb
    "mkfs",
    "dd if=",
    "> /dev/sd",
    "shutdown",
    "reboot",
    "sudo ",
    "doas ",
    "chmod -r 777 /",
    "curl ", // block network-fetch-then-run patterns; fetch_url tool is the sanctioned path
    "wget ",
];

fn denied(cmd: &str) -> Option<&'static str> {
    let lc = cmd.to_lowercase();
    DENY.iter().copied().find(|p| lc.contains(p))
}

pub struct ShellTool {
    root: PathBuf,
    workspace: WorkspaceFs,
    policy: ShellPolicy,
}
impl ShellTool {
    pub fn new(root: impl Into<PathBuf>, policy: ShellPolicy) -> Self {
        let root = root.into();
        let workspace = WorkspaceFs::new(&root);
        Self::from_workspace(root, workspace, policy)
    }

    /// Reuse the exact workspace descriptor held by file tools in an Agent or
    /// Team. On Unix, the child process changes directory by that descriptor,
    /// rather than by a mutable pathname that could be swapped by a workspace
    /// process between approval and spawn.
    pub fn from_workspace(
        root: impl Into<PathBuf>,
        workspace: WorkspaceFs,
        policy: ShellPolicy,
    ) -> Self {
        Self {
            root: root.into(),
            workspace,
            policy,
        }
    }

    fn audit(&self, cmd: &str, status: &str) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let Some(dir) = dirs::config_dir() else {
            return;
        };
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
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let mk = |ok: bool, content: String| ToolResult {
            tool: "shell".to_string(),
            ok,
            content,
        };

        if !self.policy.enabled || std::env::var_os("FORGE_SHELL_DISABLED").is_some() {
            return Ok(mk(false, "shell tool is disabled".into()));
        }
        if cmd.is_empty() {
            return Ok(mk(false, "command is empty".into()));
        }
        if let Some(p) = denied(&cmd) {
            self.audit(&cmd, "DENIED");
            return Ok(mk(
                false,
                format!("refused: command matches a blocked pattern ('{p}')"),
            ));
        }
        match self.workspace.matches_root_path(&self.root) {
            Ok(true) => {}
            Ok(false) => {
                self.audit(&cmd, "WORKSPACE_CHANGED");
                return Ok(mk(
                    false,
                    "refused: workspace root changed since this run started".into(),
                ));
            }
            Err(error) => {
                self.audit(&cmd, "WORKSPACE_UNAVAILABLE");
                return Ok(mk(
                    false,
                    format!("refused: could not verify workspace root: {error:#}"),
                ));
            }
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
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            let fd = match self.workspace.unix_dir_fd() {
                Ok(fd) => fd,
                Err(error) => {
                    self.audit(&cmd, "WORKSPACE_UNAVAILABLE");
                    return Ok(mk(
                        false,
                        format!("refused: could not pin workspace root: {error:#}"),
                    ));
                }
            };
            // SAFETY: `fd` belongs to the immutable directory capability held
            // by `self.workspace`, which outlives spawn. `setsid` and `fchdir`
            // are async-signal-safe and run in the child immediately before
            // exec. A new session also makes the shell the leader of its own
            // process group, which lets cancellation reach ordinary descendants
            // without signalling the server's terminal group.
            unsafe {
                command.pre_exec(move || {
                    // Do not use `CommandExt::process_group(0)` here. On
                    // macOS, signal delivery to a just-spawned process group
                    // has historically been racy; making the child a session
                    // leader in this same fork/exec hook gives us a stable
                    // PGID equal to its PID. `setsid` must happen before
                    // `fchdir`, and cannot be combined with process_group(0)
                    // because that would make the child a group leader first.
                    if libc::setsid() == -1 {
                        Err(std::io::Error::last_os_error())
                    } else if libc::fchdir(fd) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }
        #[cfg(not(unix))]
        command.current_dir(&self.root);

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.audit(&cmd, "ERROR");
                return Ok(mk(false, format!("could not run command: {error}")));
            }
        };
        // `kill_on_drop` handles the direct child. Keep a process-group guard
        // on the async stack as well: Tokio cancellation drops this future, so
        // the guard also closes ordinary background descendants on an explicit
        // Agent/Team cancellation, not only on a timeout.
        #[cfg(unix)]
        let mut process_group = ProcessGroupGuard::new(child.id());
        let result = tokio::time::timeout(self.policy.timeout, child.wait_with_output()).await;
        match result {
            Err(_) => {
                #[cfg(unix)]
                process_group.terminate_now();
                self.audit(&cmd, "TIMEOUT");
                Ok(mk(
                    false,
                    format!(
                        "command timed out after {}s (killed)",
                        self.policy.timeout.as_secs()
                    ),
                ))
            }
            Ok(Err(e)) => {
                #[cfg(unix)]
                process_group.terminate_now();
                self.audit(&cmd, "ERROR");
                Ok(mk(false, format!("could not run command: {e}")))
            }
            Ok(Ok(out)) => {
                #[cfg(unix)]
                process_group.disarm();
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

/// Owns the Unix process group spawned for one shell invocation. Its `Drop`
/// implementation is deliberate: an outer Tokio task can be aborted while the
/// command future is pending, which bypasses ordinary timeout/error branches.
/// In that case the guard still terminates the group before unwinding returns
/// control to the server.
#[cfg(unix)]
struct ProcessGroupGuard {
    pid: Option<u32>,
    armed: bool,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn terminate_now(&mut self) {
        if self.armed {
            terminate_process_group(self.pid);
            self.disarm();
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.terminate_now();
    }
}

/// Best-effort process-tree cleanup for the Unix shell verifier. `kill` with a
/// negative PID targets the process group created by `setsid` before exec.
/// The child may already have exited, which is harmless (ESRCH is ignored).
#[cfg(unix)]
fn terminate_process_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: `kill` receives a validated process-group identifier owned by a
    // child we spawned specifically for this command. Errors are intentionally
    // ignored because a completed group has no remaining members to kill.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
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
        let r = t
            .invoke(json!({"command":"rm -rf / --no-preserve-root"}))
            .await
            .unwrap();
        assert!(!r.ok && r.content.contains("refused"));
    }

    #[tokio::test]
    async fn disabled_policy_blocks() {
        let pol = ShellPolicy {
            enabled: false,
            ..Default::default()
        };
        let t = ShellTool::new(std::env::temp_dir(), pol);
        let r = t.invoke(json!({"command":"echo hi"})).await.unwrap();
        assert!(!r.ok && r.content.contains("disabled"));
    }

    #[tokio::test]
    async fn timeout_kills_long_command() {
        let pol = ShellPolicy {
            enabled: true,
            timeout: Duration::from_millis(200),
        };
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

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_shell_descendants_on_unix() {
        let marker = std::env::temp_dir().join(format!(
            "ollamax-shell-descendant-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let marker_text = marker.to_string_lossy().replace('\'', "'\\''");
        let command = format!("(sleep 1; touch '{marker_text}') & wait");
        let policy = ShellPolicy {
            enabled: true,
            timeout: Duration::from_millis(100),
        };
        let tool = ShellTool::new(std::env::temp_dir(), policy);
        let result = tool.invoke(json!({"command": command})).await.unwrap();
        assert!(
            !result.ok && result.content.contains("timed out"),
            "{}",
            result.content
        );
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(
            !marker.exists(),
            "timed-out shell descendant survived and created {}",
            marker.display()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn task_abort_kills_shell_descendants_on_unix() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let escaped = root.path().join("escaped");
        let ready_text = ready.to_string_lossy().replace('\'', "'\\''");
        let escaped_text = escaped.to_string_lossy().replace('\'', "'\\''");
        // Have the background child signal readiness itself. That guarantees
        // the cancellation below races a live descendant, not merely the
        // parent shell before it has forked the background job.
        let command = format!("(touch '{ready_text}'; sleep 1; touch '{escaped_text}') & wait");
        let tool = ShellTool::new(
            root.path(),
            ShellPolicy {
                enabled: true,
                timeout: Duration::from_secs(10),
            },
        );
        let run = tokio::spawn(async move { tool.invoke(json!({"command": command})).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("shell command did not start");
        run.abort();
        let _ = run.await;

        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(
            !escaped.exists(),
            "cancelled shell descendant survived and created {}",
            escaped.display()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_workspace_path_replacement_before_shell_spawn() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let moved = parent.path().join("workspace-moved");
        std::fs::create_dir(&workspace).unwrap();
        let capability = WorkspaceFs::new(&workspace);
        let outside = tempfile::tempdir().unwrap();
        std::fs::rename(&workspace, &moved).unwrap();
        symlink(outside.path(), &workspace).unwrap();

        let tool = ShellTool::from_workspace(
            &workspace,
            capability,
            ShellPolicy {
                enabled: true,
                timeout: Duration::from_secs(2),
            },
        );
        let result = tool
            .invoke(json!({"command":"touch should-not-run"}))
            .await
            .unwrap();
        assert!(!result.ok, "{}", result.content);
        assert!(result.content.contains("workspace root changed"));
        assert!(!outside.path().join("should-not-run").exists());
        assert!(!moved.join("should-not-run").exists());
    }
}

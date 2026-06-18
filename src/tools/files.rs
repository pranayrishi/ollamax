//! Filesystem tools for the autonomous Agent: read / write / edit, each
//! **sandboxed** to a workspace root. A model-supplied path can never escape the
//! sandbox (absolute paths and `..` traversal are rejected lexically against the
//! canonicalized root). These are the "act on files" half of Hermes-class tools;
//! the consent/Autonomy-Dial gating lives in the UI layer — these tools enforce
//! the hard sandbox + size caps regardless.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// Resolve a relative path against `root`, rejecting anything that escapes the
/// sandbox. Canonicalizes `root` (must exist), joins `rel`, resolves `.`/`..`
/// lexically, and confirms the result stays under root. Absolute `rel` is
/// rejected because joining an absolute path would replace the root.
pub fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.trim().is_empty() {
        return Err(anyhow!("path is empty"));
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| anyhow!("workspace root invalid: {e}"))?;
    let joined = canon_root.join(rel);
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    return Err(anyhow!("path '{rel}' escapes the workspace sandbox"));
                }
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    if !out.starts_with(&canon_root) {
        return Err(anyhow!("path '{rel}' escapes the workspace sandbox"));
    }
    Ok(out)
}

fn err(tool: &str, msg: impl Into<String>) -> ToolResult {
    ToolResult { tool: tool.to_string(), ok: false, content: msg.into() }
}

/// Read a UTF-8 file from the workspace.
pub struct FsReadTool {
    root: PathBuf,
}
impl FsReadTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for FsReadTool {
    fn name(&self) -> &str {
        "fs_read"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 text file from the workspace. args: {path: string (relative to the workspace)}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        match tokio::fs::read_to_string(&abs).await {
            Ok(s) => Ok(ToolResult {
                tool: self.name().to_string(),
                ok: true,
                content: truncate_for_model(&s),
            }),
            Err(e) => Ok(err(self.name(), format!("could not read {path}: {e}"))),
        }
    }
}

/// Write (create or overwrite) a file in the workspace.
pub struct FsWriteTool {
    root: PathBuf,
}
impl FsWriteTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for FsWriteTool {
    fn name(&self) -> &str {
        "fs_write"
    }
    fn description(&self) -> &str {
        "Create or overwrite a workspace file. args: {path: string, content: string}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        if let Some(parent) = abs.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return Ok(err(self.name(), format!("mkdir failed: {e}")));
            }
        }
        match tokio::fs::write(&abs, content).await {
            Ok(()) => Ok(ToolResult {
                tool: self.name().to_string(),
                ok: true,
                content: format!("wrote {} bytes to {path}", content.len()),
            }),
            Err(e) => Ok(err(self.name(), format!("could not write {path}: {e}"))),
        }
    }
}

/// Replace an exact, unique substring in a workspace file (like a precise patch).
pub struct FsEditTool {
    root: PathBuf,
}
impl FsEditTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for FsEditTool {
    fn name(&self) -> &str {
        "fs_edit"
    }
    fn description(&self) -> &str {
        "Replace an exact, UNIQUE substring in a workspace file. Fails if old_string is missing or appears more than once. args: {path, old_string, new_string}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"}},"required":["path","old_string","new_string"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let old = args.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
        let new = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        if old.is_empty() {
            return Ok(err(self.name(), "old_string is empty"));
        }
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let cur = match tokio::fs::read_to_string(&abs).await {
            Ok(s) => s,
            Err(e) => return Ok(err(self.name(), format!("could not read {path}: {e}"))),
        };
        let count = cur.matches(old).count();
        if count == 0 {
            return Ok(err(self.name(), "old_string not found"));
        }
        if count > 1 {
            return Ok(err(self.name(), format!("old_string is not unique ({count} matches)")));
        }
        let updated = cur.replacen(old, new, 1);
        match tokio::fs::write(&abs, &updated).await {
            Ok(()) => Ok(ToolResult {
                tool: self.name().to_string(),
                ok: true,
                content: format!("edited {path}"),
            }),
            Err(e) => Ok(err(self.name(), format!("could not write {path}: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-files-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        let root = tmp();
        assert!(resolve_within(&root, "../etc/passwd").is_err());
        assert!(resolve_within(&root, "/etc/passwd").is_err());
        assert!(resolve_within(&root, "a/../../b").is_err());
        assert!(resolve_within(&root, "ok/inside.txt").is_ok());
    }

    #[tokio::test]
    async fn write_then_read_then_edit() {
        let root = tmp();
        let w = FsWriteTool::new(&root);
        let r = w.invoke(json!({"path":"sub/f.txt","content":"hello world"})).await.unwrap();
        assert!(r.ok, "{}", r.content);

        let rd = FsReadTool::new(&root);
        let got = rd.invoke(json!({"path":"sub/f.txt"})).await.unwrap();
        assert!(got.ok && got.content.contains("hello world"));

        let e = FsEditTool::new(&root);
        let ed = e.invoke(json!({"path":"sub/f.txt","old_string":"world","new_string":"forge"})).await.unwrap();
        assert!(ed.ok, "{}", ed.content);
        let after = rd.invoke(json!({"path":"sub/f.txt"})).await.unwrap();
        assert!(after.content.contains("hello forge"));
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let root = tmp();
        FsWriteTool::new(&root).invoke(json!({"path":"d.txt","content":"x x"})).await.unwrap();
        let e = FsEditTool::new(&root);
        let dup = e.invoke(json!({"path":"d.txt","old_string":"x","new_string":"y"})).await.unwrap();
        assert!(!dup.ok && dup.content.contains("not unique"));
        let missing = e.invoke(json!({"path":"d.txt","old_string":"zzz","new_string":"y"})).await.unwrap();
        assert!(!missing.ok && missing.content.contains("not found"));
    }

    #[tokio::test]
    async fn read_outside_sandbox_is_blocked() {
        let root = tmp();
        let rd = FsReadTool::new(&root);
        let r = rd.invoke(json!({"path":"../../../../etc/hosts"})).await.unwrap();
        assert!(!r.ok && r.content.contains("escapes"));
    }
}

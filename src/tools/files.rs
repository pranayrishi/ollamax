//! Filesystem tools for the autonomous Agent: list / search / read / write / edit, each
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

/// A tool call should never make the engine read an arbitrarily large source or
/// generated file into memory. The model only receives an 8 KiB excerpt anyway,
/// so a 1 MiB hard cap is deliberately generous for normal source files.
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
const MAX_SEARCH_RESULTS: usize = 200;

/// Directories that are expensive/noisy and should not become agent context by
/// accident. This matches the IDE's explorer policy and keeps a workspace scan
/// useful on real projects.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    ".venv",
    "venv",
    "vendor",
];

/// Resolve a relative path against `root`, rejecting anything that escapes the
/// sandbox. In addition to lexical traversal, reject symlink components: a
/// workspace symlink can otherwise point outside the root and make a seemingly
/// safe `fs_write` modify an unrelated file. This is a hard boundary independent
/// of the UI approval layer.
pub fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.trim().is_empty() {
        return Err(anyhow!("path is empty"));
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| anyhow!("workspace root invalid: {e}"))?;
    let supplied = Path::new(rel);
    if supplied.is_absolute() {
        return Err(anyhow!("path '{rel}' must be relative to the workspace"));
    }

    let mut out = canon_root.clone();
    for comp in supplied.components() {
        match comp {
            Component::ParentDir => {
                return Err(anyhow!("path '{rel}' escapes the workspace sandbox"));
            }
            Component::CurDir => {}
            Component::Normal(segment) => out.push(segment),
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!("path '{rel}' must be relative to the workspace"));
            }
        }
    }
    if !out.starts_with(&canon_root) {
        return Err(anyhow!("path '{rel}' escapes the workspace sandbox"));
    }
    reject_symlink_components(&canon_root, &out, rel)?;
    Ok(out)
}

/// Reject an existing symlink anywhere below `root`. `canonicalize()` alone is
/// insufficient because it only canonicalizes the root, not a new target path.
/// We intentionally stop at the first non-existent component; it will be
/// created beneath an already verified parent by the write tool.
fn reject_symlink_components(root: &Path, target: &Path, rel: &str) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| anyhow!("path '{rel}' escapes the workspace sandbox"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!(
                    "path '{rel}' contains a symlink (`{}`), which is blocked by the workspace sandbox",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(anyhow!(
                    "inspect workspace path '{}': {e}",
                    current.display()
                ))
            }
        }
    }
    Ok(())
}

fn workspace_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .map_err(|e| anyhow!("workspace root invalid: {e}"))
}

fn resolve_optional_dir(root: &Path, path: Option<&str>) -> Result<PathBuf> {
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(rel) => resolve_within(root, rel),
        None => workspace_root(root),
    }
}

fn ignored_dir(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    IGNORED_DIRS.iter().any(|ignored| name == *ignored)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn err(tool: &str, msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool: tool.to_string(),
        ok: false,
        content: msg.into(),
    }
}

/// List the useful portion of a workspace tree. This gives a coding model a
/// deterministic first step before it opens files, rather than forcing it to
/// guess paths or request a shell command just to see a repository.
pub struct FsListTool {
    root: PathBuf,
}
impl FsListTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for FsListTool {
    fn name(&self) -> &str {
        "fs_list"
    }
    fn description(&self) -> &str {
        "List files and folders inside the workspace. args: {path?: string (relative directory, default workspace root), depth?: integer 0-6, max_entries?: integer}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"depth":{"type":"integer"},"max_entries":{"type":"integer"}}})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let path = args.get("path").and_then(|v| v.as_str());
        let base = match resolve_optional_dir(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let metadata = match std::fs::metadata(&base) {
            Ok(m) if m.is_dir() => m,
            Ok(_) => return Ok(err(self.name(), "path is not a directory")),
            Err(e) => {
                return Ok(err(
                    self.name(),
                    format!("could not list {}: {e}", path.unwrap_or(".")),
                ))
            }
        };
        let _ = metadata;
        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .clamp(0, 6) as usize;
        let max_entries = args
            .get("max_entries")
            .and_then(|v| v.as_u64())
            .unwrap_or(200)
            .clamp(1, MAX_LIST_ENTRIES as u64) as usize;
        let root = workspace_root(&self.root)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .min_depth(1)
            .max_depth(depth)
            .into_iter()
            .filter_entry(|e| {
                !(e.depth() > 0 && e.file_type().is_dir() && ignored_dir(e.file_name()))
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().is_symlink() {
                continue;
            }
            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
            let mut label = display_relative(&root, entry.path());
            if entry.file_type().is_dir() {
                label.push('/');
            }
            entries.push(label);
        }
        let mut content = if entries.is_empty() {
            "(no visible entries)".to_string()
        } else {
            entries.join("\n")
        };
        if truncated {
            content.push_str("\n[... listing truncated; narrow path or raise max_entries]");
        }
        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&content),
        })
    }
}

/// Search workspace text without granting a model a shell just to locate a
/// symbol or phrase. It skips generated/vendor directories, binary files, and
/// huge files, and returns short file:line excerpts suitable for a model turn.
pub struct FsSearchTool {
    root: PathBuf,
}
impl FsSearchTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for FsSearchTool {
    fn name(&self) -> &str {
        "fs_search"
    }
    fn description(&self) -> &str {
        "Search UTF-8 workspace files for an exact text fragment. args: {query: string, path?: string (relative directory), max_results?: integer}"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"max_results":{"type":"integer"}},"required":["query"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.trim().is_empty() {
            return Ok(err(self.name(), "query is empty"));
        }
        if query.len() > 512 {
            return Ok(err(self.name(), "query is too long (max 512 bytes)"));
        }
        let path = args.get("path").and_then(|v| v.as_str());
        let base = match resolve_optional_dir(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(80)
            .clamp(1, MAX_SEARCH_RESULTS as u64) as usize;
        let root = workspace_root(&self.root)?;
        let mut matches = Vec::new();
        let mut truncated = false;
        'files: for entry in walkdir::WalkDir::new(&base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                !(e.depth() > 0 && e.file_type().is_dir() && ignored_dir(e.file_name()))
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() || entry.file_type().is_symlink() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_READ_BYTES {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if content.contains('\0') {
                continue;
            }
            for (line_no, line) in content.lines().enumerate() {
                if !line.contains(query) {
                    continue;
                }
                if matches.len() >= max_results {
                    truncated = true;
                    break 'files;
                }
                let excerpt: String = line.chars().take(300).collect();
                matches.push(format!(
                    "{}:{}: {}",
                    display_relative(&root, entry.path()),
                    line_no + 1,
                    excerpt
                ));
            }
        }
        let mut content = if matches.is_empty() {
            format!("no matches for `{query}`")
        } else {
            matches.join("\n")
        };
        if truncated {
            content.push_str("\n[... search results truncated; narrow query/path]");
        }
        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&content),
        })
    }
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
        let metadata = match tokio::fs::metadata(&abs).await {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(err(self.name(), format!("{path} is not a regular file"))),
            Err(e) => return Ok(err(self.name(), format!("could not read {path}: {e}"))),
        };
        if metadata.len() > MAX_READ_BYTES {
            return Ok(err(
                self.name(),
                format!(
                    "{path} is too large to read ({} bytes; max {MAX_READ_BYTES})",
                    metadata.len()
                ),
            ));
        }
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
        if content.len() > MAX_WRITE_BYTES {
            return Ok(err(
                self.name(),
                format!(
                    "content is too large to write ({} bytes; max {MAX_WRITE_BYTES})",
                    content.len()
                ),
            ));
        }
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        if let Some(parent) = abs.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return Ok(err(self.name(), format!("mkdir failed: {e}")));
            }
        }
        // Re-resolve after mkdir so a concurrently-created symlink cannot turn
        // the previously checked target into an escape path.
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
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
        let old = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if old.is_empty() {
            return Ok(err(self.name(), "old_string is empty"));
        }
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let metadata = match tokio::fs::metadata(&abs).await {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(err(self.name(), format!("{path} is not a regular file"))),
            Err(e) => return Ok(err(self.name(), format!("could not read {path}: {e}"))),
        };
        if metadata.len() > MAX_READ_BYTES {
            return Ok(err(
                self.name(),
                format!(
                    "{path} is too large to edit ({} bytes; max {MAX_READ_BYTES})",
                    metadata.len()
                ),
            ));
        }
        let cur = match tokio::fs::read_to_string(&abs).await {
            Ok(s) => s,
            Err(e) => return Ok(err(self.name(), format!("could not read {path}: {e}"))),
        };
        let count = cur.matches(old).count();
        if count == 0 {
            return Ok(err(self.name(), "old_string not found"));
        }
        if count > 1 {
            return Ok(err(
                self.name(),
                format!("old_string is not unique ({count} matches)"),
            ));
        }
        let updated = cur.replacen(old, new, 1);
        if updated.len() > MAX_WRITE_BYTES {
            return Ok(err(
                self.name(),
                format!(
                    "edited content is too large ({} bytes; max {MAX_WRITE_BYTES})",
                    updated.len()
                ),
            ));
        }
        let abs = match resolve_within(&self.root, path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        match tokio::fs::metadata(&abs).await {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(err(self.name(), format!("{path} is not a regular file"))),
            Err(e) => return Ok(err(self.name(), format!("could not recheck {path}: {e}"))),
        }
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
        let r = w
            .invoke(json!({"path":"sub/f.txt","content":"hello world"}))
            .await
            .unwrap();
        assert!(r.ok, "{}", r.content);

        let rd = FsReadTool::new(&root);
        let got = rd.invoke(json!({"path":"sub/f.txt"})).await.unwrap();
        assert!(got.ok && got.content.contains("hello world"));

        let e = FsEditTool::new(&root);
        let ed = e
            .invoke(json!({"path":"sub/f.txt","old_string":"world","new_string":"forge"}))
            .await
            .unwrap();
        assert!(ed.ok, "{}", ed.content);
        let after = rd.invoke(json!({"path":"sub/f.txt"})).await.unwrap();
        assert!(after.content.contains("hello forge"));
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let root = tmp();
        FsWriteTool::new(&root)
            .invoke(json!({"path":"d.txt","content":"x x"}))
            .await
            .unwrap();
        let e = FsEditTool::new(&root);
        let dup = e
            .invoke(json!({"path":"d.txt","old_string":"x","new_string":"y"}))
            .await
            .unwrap();
        assert!(!dup.ok && dup.content.contains("not unique"));
        let missing = e
            .invoke(json!({"path":"d.txt","old_string":"zzz","new_string":"y"}))
            .await
            .unwrap();
        assert!(!missing.ok && missing.content.contains("not found"));
    }

    #[tokio::test]
    async fn edit_refuses_non_regular_and_oversized_targets() {
        let root = tmp();
        std::fs::create_dir_all(root.join("directory")).unwrap();
        let edit = FsEditTool::new(&root);
        let directory = edit
            .invoke(json!({"path":"directory","old_string":"x","new_string":"y"}))
            .await
            .unwrap();
        assert!(!directory.ok && directory.content.contains("regular file"));

        let large = root.join("large.txt");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_READ_BYTES + 1)
            .unwrap();
        let oversized = edit
            .invoke(json!({"path":"large.txt","old_string":"x","new_string":"y"}))
            .await
            .unwrap();
        assert!(!oversized.ok && oversized.content.contains("too large to edit"));
    }

    #[tokio::test]
    async fn read_outside_sandbox_is_blocked() {
        let root = tmp();
        let rd = FsReadTool::new(&root);
        let r = rd
            .invoke(json!({"path":"../../../../etc/hosts"}))
            .await
            .unwrap();
        assert!(!r.ok && r.content.contains("escapes"));
    }

    #[tokio::test]
    async fn list_and_search_make_workspace_discoverable() {
        let root = tmp();
        FsWriteTool::new(&root)
            .invoke(json!({"path":"src/lib.rs","content":"pub fn useful_symbol() {}\n"}))
            .await
            .unwrap();
        FsWriteTool::new(&root)
            .invoke(json!({"path":"node_modules/ignored.js","content":"useful_symbol"}))
            .await
            .unwrap();
        let listed = FsListTool::new(&root)
            .invoke(json!({"path":"","depth":3}))
            .await
            .unwrap();
        assert!(listed.ok && listed.content.contains("src/lib.rs"));
        assert!(!listed.content.contains("node_modules"));
        let found = FsSearchTool::new(&root)
            .invoke(json!({"query":"useful_symbol"}))
            .await
            .unwrap();
        assert!(found.ok && found.content.contains("src/lib.rs:1"));
        assert!(!found.content.contains("ignored.js"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tmp();
        let outside = std::env::temp_dir().join(format!("forge-outside-{}", std::process::id()));
        std::fs::write(&outside, "outside").unwrap();
        symlink(&outside, root.join("escape.txt")).unwrap();
        let err = resolve_within(&root, "escape.txt").unwrap_err();
        assert!(err.to_string().contains("symlink"));
        let _ = std::fs::remove_file(outside);
    }
}

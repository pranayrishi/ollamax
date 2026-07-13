//! Filesystem tools for the autonomous Agent: list / search / read / write / edit, each
//! **sandboxed** to a workspace root. A model-supplied path can never escape the
//! sandbox (absolute paths and `..` traversal are rejected lexically against the
//! canonicalized root). These are the "act on files" half of Hermes-class tools;
//! the consent/Autonomy-Dial gating lives in the UI layer — these tools enforce
//! the hard sandbox + size caps regardless.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use cap_fs_ext::OpenOptionsSyncExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

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

/// Validate a user-provided workspace-relative path without touching the
/// filesystem. This is intentionally only a fast input check: callers that
/// perform I/O must use `workspace_dir` below, whose directory capability
/// makes the authorization boundary race-safe even if a workspace process
/// swaps a symlink after this check.
fn workspace_relative_path(rel: &str) -> Result<PathBuf> {
    if rel.trim().is_empty() {
        return Err(anyhow!("path is empty"));
    }
    let supplied = Path::new(rel);
    if supplied.is_absolute() {
        return Err(anyhow!("path '{rel}' must be relative to the workspace"));
    }

    let mut out = PathBuf::new();
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
    Ok(if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    })
}

/// Resolve a relative path against `root` for callers that require a display
/// or build-output path. This performs lexical and current symlink checks but
/// is not itself an atomic authorization primitive; agent filesystem tools use
/// `workspace_dir` for their actual reads and writes.
pub fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| anyhow!("workspace root invalid: {e}"))?;
    let relative = workspace_relative_path(rel)?;
    let out = canon_root.join(relative);
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

fn optional_workspace_relative(path: Option<&str>) -> Result<PathBuf> {
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(rel) => workspace_relative_path(rel),
        None => Ok(PathBuf::from(".")),
    }
}

fn ignored_dir(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    IGNORED_DIRS.iter().any(|ignored| name == *ignored)
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A directory capability acquired when an agent's workspace tools are set up.
/// Holding the descriptor, rather than reopening a pathname for every call,
/// means a concurrent rename of the workspace root cannot redirect a later
/// model-supplied operation either.
#[derive(Clone)]
pub struct WorkspaceFs {
    handle: std::result::Result<Arc<WorkspaceHandle>, String>,
}

struct WorkspaceHandle {
    dir: Arc<Dir>,
    identity: Arc<same_file::Handle>,
}

impl WorkspaceFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let handle = (|| -> Result<Arc<WorkspaceHandle>> {
            let root = workspace_root(&root)?;
            let dir = Dir::open_ambient_dir(root, ambient_authority())
                .map_err(|error| anyhow!("open workspace capability: {error}"))?;
            // Derive the portable identity from a duplicate of the exact
            // descriptor retained for capability-relative file operations.
            // A later pathname lookup can therefore prove whether it still
            // refers to the user-selected directory before a host subprocess
            // is allowed to use it as its working directory.
            let identity_dir = dir
                .try_clone()
                .map_err(|error| anyhow!("clone workspace capability: {error}"))?;
            let identity = same_file::Handle::from_file(identity_dir.into_std_file())
                .map_err(|error| anyhow!("identify workspace capability: {error}"))?;
            Ok(Arc::new(WorkspaceHandle {
                dir: Arc::new(dir),
                identity: Arc::new(identity),
            }))
        })()
        .map_err(|error| error.to_string());
        Self { handle }
    }

    /// Compare a current path lookup with the directory descriptor captured
    /// above. Host subprocesses use this as a fail-closed preflight on Windows;
    /// Unix additionally changes directory by descriptor in the child.
    pub fn matches_root_path(&self, path: &Path) -> Result<bool> {
        let handle = self
            .handle
            .as_ref()
            .map_err(|error| anyhow!("workspace capability unavailable: {error}"))?;
        let current = same_file::Handle::from_path(path).map_err(|error| {
            anyhow!("inspect current workspace path {}: {error}", path.display())
        })?;
        Ok(handle.identity.as_ref() == &current)
    }

    #[cfg(unix)]
    pub fn unix_dir_fd(&self) -> Result<RawFd> {
        let handle = self
            .handle
            .as_ref()
            .map_err(|error| anyhow!("workspace capability unavailable: {error}"))?;
        Ok(handle.dir.as_raw_fd())
    }
}

async fn in_workspace<T: Send + 'static>(
    workspace: WorkspaceFs,
    operation: impl FnOnce(Arc<Dir>) -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        let handle = workspace
            .handle
            .map_err(|error| anyhow!("workspace capability unavailable: {error}"))?;
        operation(handle.dir.clone())
    })
    .await
    .map_err(|error| anyhow!("workspace I/O task failed: {error}"))?
}

fn read_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).nonblock(true);
    options
}

fn write_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(true)
        .nonblock(true);
    options
}

fn read_write_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).nonblock(true);
    options
}

/// Read a bounded UTF-8 text file from a capability-backed file handle. The
/// limit remains enforced if another process grows the file after metadata was
/// checked, so the agent never buffers an arbitrarily large replacement.
fn read_open_file_limited(file: &mut File, max_bytes: u64) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file exceeds the {max_bytes}-byte workspace limit"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file is not valid UTF-8 text",
        )
    })
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
    workspace: WorkspaceFs,
}
impl FsListTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_workspace(WorkspaceFs::new(root))
    }

    pub fn from_workspace(workspace: WorkspaceFs) -> Self {
        Self { workspace }
    }
}

fn child_relative(parent: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut child = if parent == Path::new(".") {
        PathBuf::new()
    } else {
        parent.to_path_buf()
    };
    child.push(name);
    child
}

fn list_capability_dir(
    dir: &Dir,
    relative: &Path,
    remaining_depth: usize,
    max_entries: usize,
    entries: &mut Vec<String>,
    truncated: &mut bool,
) {
    if remaining_depth == 0 || *truncated {
        return;
    }
    let Ok(read_dir) = dir.entries() else {
        return;
    };
    for entry in read_dir.flatten() {
        if entries.len() >= max_entries {
            *truncated = true;
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // We do not make symlinks visible to models. If a link is swapped in
        // after this check, subsequent capability-relative opens still cannot
        // escape the workspace.
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        if file_type.is_dir() && ignored_dir(&name) {
            continue;
        }
        let child = child_relative(relative, &name);
        let mut label = display_relative(&child);
        if file_type.is_dir() {
            label.push('/');
        }
        entries.push(label);
        if file_type.is_dir() && remaining_depth > 1 {
            if let Ok(child_dir) = entry.open_dir() {
                list_capability_dir(
                    &child_dir,
                    &child,
                    remaining_depth - 1,
                    max_entries,
                    entries,
                    truncated,
                );
            }
        }
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
        let relative = match optional_workspace_relative(path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
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
        let workspace = self.workspace.clone();
        let tool = self.name().to_string();
        let requested = path.unwrap_or(".").to_string();
        match in_workspace(workspace, move |dir| {
            let base = match dir.open_dir(&relative) {
                Ok(base) => base,
                Err(error) => {
                    return Ok(err(&tool, format!("could not list {requested}: {error}")))
                }
            };
            let mut entries = Vec::new();
            let mut truncated = false;
            list_capability_dir(
                &base,
                &relative,
                depth,
                max_entries,
                &mut entries,
                &mut truncated,
            );
            let mut content = if entries.is_empty() {
                "(no visible entries)".to_string()
            } else {
                entries.join("\n")
            };
            if truncated {
                content.push_str("\n[... listing truncated; narrow path or raise max_entries]");
            }
            Ok(ToolResult {
                tool,
                ok: true,
                content: truncate_for_model(&content),
            })
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(err(self.name(), error.to_string())),
        }
    }
}

/// Search workspace text without granting a model a shell just to locate a
/// symbol or phrase. It skips generated/vendor directories, binary files, and
/// huge files, and returns short file:line excerpts suitable for a model turn.
pub struct FsSearchTool {
    workspace: WorkspaceFs,
}
impl FsSearchTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_workspace(WorkspaceFs::new(root))
    }

    pub fn from_workspace(workspace: WorkspaceFs) -> Self {
        Self { workspace }
    }
}

fn search_capability_dir(
    dir: &Dir,
    relative: &Path,
    query: &str,
    max_results: usize,
    matches: &mut Vec<String>,
    truncated: &mut bool,
) {
    if *truncated {
        return;
    }
    let Ok(read_dir) = dir.entries() else {
        return;
    };
    for entry in read_dir.flatten() {
        if *truncated {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let child = child_relative(relative, &name);
        if file_type.is_dir() {
            if ignored_dir(&name) {
                continue;
            }
            if let Ok(child_dir) = entry.open_dir() {
                search_capability_dir(&child_dir, &child, query, max_results, matches, truncated);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(mut file) = entry.open() else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_READ_BYTES {
            continue;
        }
        let Ok(content) = read_open_file_limited(&mut file, MAX_READ_BYTES) else {
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
                *truncated = true;
                return;
            }
            let excerpt: String = line.chars().take(300).collect();
            matches.push(format!(
                "{}:{}: {}",
                display_relative(&child),
                line_no + 1,
                excerpt
            ));
        }
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
        let relative = match optional_workspace_relative(path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(80)
            .clamp(1, MAX_SEARCH_RESULTS as u64) as usize;
        let workspace = self.workspace.clone();
        let tool = self.name().to_string();
        let requested = path.unwrap_or(".").to_string();
        let query = query.to_string();
        match in_workspace(workspace, move |dir| {
            let base = match dir.open_dir(&relative) {
                Ok(base) => base,
                Err(error) => {
                    return Ok(err(&tool, format!("could not search {requested}: {error}")))
                }
            };
            let mut matches = Vec::new();
            let mut truncated = false;
            search_capability_dir(
                &base,
                &relative,
                &query,
                max_results,
                &mut matches,
                &mut truncated,
            );
            let mut content = if matches.is_empty() {
                format!("no matches for `{query}`")
            } else {
                matches.join("\n")
            };
            if truncated {
                content.push_str("\n[... search results truncated; narrow query/path]");
            }
            Ok(ToolResult {
                tool,
                ok: true,
                content: truncate_for_model(&content),
            })
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(err(self.name(), error.to_string())),
        }
    }
}

/// Read a UTF-8 file from the workspace.
pub struct FsReadTool {
    workspace: WorkspaceFs,
}
impl FsReadTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_workspace(WorkspaceFs::new(root))
    }

    pub fn from_workspace(workspace: WorkspaceFs) -> Self {
        Self { workspace }
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
        let relative = match workspace_relative_path(path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let workspace = self.workspace.clone();
        let tool = self.name().to_string();
        let requested = path.to_string();
        match in_workspace(workspace, move |dir| {
            let expected = match dir.metadata(&relative) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => return Ok(err(&tool, format!("{requested} is not a regular file"))),
                Err(error) => {
                    return Ok(err(
                        &tool,
                        format!("could not inspect {requested}: {error}"),
                    ))
                }
            };
            if expected.len() > MAX_READ_BYTES {
                return Ok(err(
                    &tool,
                    format!(
                        "{requested} is too large to read ({} bytes; max {MAX_READ_BYTES})",
                        expected.len()
                    ),
                ));
            }
            let mut file = match dir.open_with(&relative, &read_open_options()) {
                Ok(file) => file,
                Err(error) => {
                    return Ok(err(&tool, format!("could not read {requested}: {error}")))
                }
            };
            let metadata = match file.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => return Ok(err(&tool, format!("{requested} is not a regular file"))),
                Err(error) => {
                    return Ok(err(
                        &tool,
                        format!("could not inspect {requested}: {error}"),
                    ))
                }
            };
            if metadata.len() > MAX_READ_BYTES {
                return Ok(err(
                    &tool,
                    format!(
                        "{requested} is too large to read ({} bytes; max {MAX_READ_BYTES})",
                        metadata.len()
                    ),
                ));
            }
            match read_open_file_limited(&mut file, MAX_READ_BYTES) {
                Ok(content) => Ok(ToolResult {
                    tool,
                    ok: true,
                    content: truncate_for_model(&content),
                }),
                Err(error) => Ok(err(&tool, format!("could not read {requested}: {error}"))),
            }
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(err(self.name(), error.to_string())),
        }
    }
}

/// Write (create or overwrite) a file in the workspace.
pub struct FsWriteTool {
    workspace: WorkspaceFs,
}
impl FsWriteTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_workspace(WorkspaceFs::new(root))
    }

    pub fn from_workspace(workspace: WorkspaceFs) -> Self {
        Self { workspace }
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
        let relative = match workspace_relative_path(path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let workspace = self.workspace.clone();
        let tool = self.name().to_string();
        let requested = path.to_string();
        let content = content.to_string();
        match in_workspace(workspace, move |dir| {
            if let Some(parent) = relative
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty() && *parent != Path::new("."))
            {
                if let Err(error) = dir.create_dir_all(parent) {
                    return Ok(err(&tool, format!("mkdir failed: {error}")));
                }
            }

            // Reject special files (notably FIFOs) before opening them. The
            // subsequent nonblocking open below also protects the small race
            // where an attacker replaces an ordinary path after this check.
            match dir.metadata(&relative) {
                Ok(metadata) if !metadata.is_file() => {
                    return Ok(err(&tool, format!("{requested} is not a regular file")))
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Ok(err(
                        &tool,
                        format!("could not inspect {requested}: {error}"),
                    ))
                }
            }

            // Do not report a timestamp-only rewrite as an implementation
            // change. The read and the eventual write both remain relative to
            // the same directory capability, so a symlink swap cannot turn a
            // no-op check into an outside-workspace write.
            if let Ok(mut existing_file) = dir.open_with(&relative, &read_open_options()) {
                if existing_file.metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.len() <= MAX_WRITE_BYTES as u64
                }) {
                    if let Ok(existing) =
                        read_open_file_limited(&mut existing_file, MAX_WRITE_BYTES as u64)
                    {
                        if existing == content {
                            return Ok(ToolResult {
                                tool,
                                ok: true,
                                content: format!("unchanged {requested}; content already matches"),
                            });
                        }
                    }
                }
            }

            let mut file = match dir.open_with(&relative, &write_open_options()) {
                Ok(file) => file,
                Err(error) => {
                    return Ok(err(&tool, format!("could not write {requested}: {error}")))
                }
            };
            match file.write_all(content.as_bytes()) {
                Ok(()) => Ok(ToolResult {
                    tool,
                    ok: true,
                    content: format!("wrote {} bytes to {requested}", content.len()),
                }),
                Err(error) => Ok(err(&tool, format!("could not write {requested}: {error}"))),
            }
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(err(self.name(), error.to_string())),
        }
    }
}

/// Replace an exact, unique substring in a workspace file (like a precise patch).
pub struct FsEditTool {
    workspace: WorkspaceFs,
}
impl FsEditTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_workspace(WorkspaceFs::new(root))
    }

    pub fn from_workspace(workspace: WorkspaceFs) -> Self {
        Self { workspace }
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
        let relative = match workspace_relative_path(path) {
            Ok(p) => p,
            Err(e) => return Ok(err(self.name(), e.to_string())),
        };
        let workspace = self.workspace.clone();
        let tool = self.name().to_string();
        let requested = path.to_string();
        let old = old.to_string();
        let new = new.to_string();
        match in_workspace(workspace, move |dir| {
            // Hold a single read/write handle for the full edit. A concurrent
            // rename then cannot redirect the write to a different pathname,
            // and the directory capability prevents any link traversal from
            // leaving the selected workspace.
            let expected = match dir.metadata(&relative) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => return Ok(err(&tool, format!("{requested} is not a regular file"))),
                Err(error) => {
                    return Ok(err(
                        &tool,
                        format!("could not inspect {requested}: {error}"),
                    ))
                }
            };
            if expected.len() > MAX_READ_BYTES {
                return Ok(err(
                    &tool,
                    format!(
                        "{requested} is too large to edit ({} bytes; max {MAX_READ_BYTES})",
                        expected.len()
                    ),
                ));
            }
            let mut file = match dir.open_with(&relative, &read_write_open_options()) {
                Ok(file) => file,
                Err(error) => {
                    return Ok(err(&tool, format!("could not read {requested}: {error}")))
                }
            };
            let metadata = match file.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => return Ok(err(&tool, format!("{requested} is not a regular file"))),
                Err(error) => {
                    return Ok(err(
                        &tool,
                        format!("could not inspect {requested}: {error}"),
                    ))
                }
            };
            if metadata.len() > MAX_READ_BYTES {
                return Ok(err(
                    &tool,
                    format!(
                        "{requested} is too large to edit ({} bytes; max {MAX_READ_BYTES})",
                        metadata.len()
                    ),
                ));
            }
            let current = match read_open_file_limited(&mut file, MAX_READ_BYTES) {
                Ok(current) => current,
                Err(error) => {
                    return Ok(err(&tool, format!("could not read {requested}: {error}")))
                }
            };
            let count = current.matches(&old).count();
            if count == 0 {
                return Ok(err(&tool, "old_string not found"));
            }
            if count > 1 {
                return Ok(err(
                    &tool,
                    format!("old_string is not unique ({count} matches)"),
                ));
            }
            let updated = current.replacen(&old, &new, 1);
            if updated.len() > MAX_WRITE_BYTES {
                return Ok(err(
                    &tool,
                    format!(
                        "edited content is too large ({} bytes; max {MAX_WRITE_BYTES})",
                        updated.len()
                    ),
                ));
            }
            if updated == current {
                return Ok(ToolResult {
                    tool,
                    ok: true,
                    content: format!("unchanged {requested}; replacement already matches"),
                });
            }
            if let Err(error) = file.seek(SeekFrom::Start(0)) {
                return Ok(err(&tool, format!("could not seek {requested}: {error}")));
            }
            if let Err(error) = file.set_len(0) {
                return Ok(err(
                    &tool,
                    format!("could not truncate {requested}: {error}"),
                ));
            }
            match file.write_all(updated.as_bytes()) {
                Ok(()) => Ok(ToolResult {
                    tool,
                    ok: true,
                    content: format!("edited {requested}"),
                }),
                Err(error) => Ok(err(&tool, format!("could not write {requested}: {error}"))),
            }
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Ok(err(self.name(), error.to_string())),
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
    async fn writing_identical_content_reports_no_mutation() {
        let root = tempfile::tempdir().unwrap();
        let write = FsWriteTool::new(root.path());
        let first = write
            .invoke(json!({"path":"same.txt","content":"already here"}))
            .await
            .unwrap();
        assert!(first.ok, "{}", first.content);

        let second = write
            .invoke(json!({"path":"same.txt","content":"already here"}))
            .await
            .unwrap();
        assert!(second.ok, "{}", second.content);
        assert_eq!(
            second.content,
            "unchanged same.txt; content already matches"
        );
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
        assert!(!directory.ok, "{}", directory.content);

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

    #[cfg(unix)]
    #[tokio::test]
    async fn capability_backed_tools_cannot_follow_workspace_symlinks_outside() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "do-not-exfiltrate-or-change").unwrap();
        symlink(&outside_file, root.path().join("escape.txt")).unwrap();
        symlink(outside.path(), root.path().join("escape-dir")).unwrap();

        let read = FsReadTool::new(root.path());
        let result = read.invoke(json!({"path":"escape.txt"})).await.unwrap();
        assert!(!result.ok, "{}", result.content);

        let write = FsWriteTool::new(root.path());
        let result = write
            .invoke(json!({"path":"escape.txt","content":"changed"}))
            .await
            .unwrap();
        assert!(!result.ok, "{}", result.content);
        let result = write
            .invoke(json!({"path":"escape-dir/new.txt","content":"changed"}))
            .await
            .unwrap();
        assert!(!result.ok, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "do-not-exfiltrate-or-change"
        );
        assert!(!outside.path().join("new.txt").exists());

        let edit = FsEditTool::new(root.path());
        let result = edit
            .invoke(json!({
                "path":"escape.txt",
                "old_string":"do-not-exfiltrate-or-change",
                "new_string":"changed"
            }))
            .await
            .unwrap();
        assert!(!result.ok, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "do-not-exfiltrate-or-change"
        );

        let search = FsSearchTool::new(root.path());
        let result = search
            .invoke(json!({"query":"do-not-exfiltrate-or-change"}))
            .await
            .unwrap();
        assert!(result.ok, "{}", result.content);
        assert!(!result.content.contains("escape.txt:"));

        let list = FsListTool::new(root.path());
        let result = list.invoke(json!({"depth":2})).await.unwrap();
        assert!(result.ok, "{}", result.content);
        assert!(!result.content.contains("escape.txt"));
        assert!(!result.content.contains("escape-dir"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_workspace_capability_stays_pinned_after_root_path_swap() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let moved_workspace = parent.path().join("workspace-moved");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("inside.txt"), "original workspace").unwrap();

        let capability = WorkspaceFs::new(&workspace);
        let outside = tempfile::tempdir().unwrap();
        std::fs::rename(&workspace, &moved_workspace).unwrap();
        symlink(outside.path(), &workspace).unwrap();

        let read = FsReadTool::from_workspace(capability.clone());
        let result = read.invoke(json!({"path":"inside.txt"})).await.unwrap();
        assert!(result.ok, "{}", result.content);
        assert_eq!(result.content, "original workspace");

        let write = FsWriteTool::from_workspace(capability);
        let result = write
            .invoke(json!({"path":"new.txt","content":"pinned"}))
            .await
            .unwrap();
        assert!(result.ok, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(moved_workspace.join("new.txt")).unwrap(),
            "pinned"
        );
        assert!(!outside.path().join("new.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_paths_are_rejected_without_blocking_workspace_io() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("blocked.fifo");
        let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_name` is a NUL-free path inside a test-only temp dir.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

        let read = FsReadTool::new(root.path());
        let write = FsWriteTool::new(root.path());
        let edit = FsEditTool::new(root.path());
        for result in [
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                read.invoke(json!({"path":"blocked.fifo"})),
            )
            .await
            .expect("FIFO read must not block")
            .unwrap(),
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                write.invoke(json!({"path":"blocked.fifo","content":"x"})),
            )
            .await
            .expect("FIFO write must not block")
            .unwrap(),
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                edit.invoke(json!({
                    "path":"blocked.fifo",
                    "old_string":"x",
                    "new_string":"y"
                })),
            )
            .await
            .expect("FIFO edit must not block")
            .unwrap(),
        ] {
            assert!(!result.ok, "{}", result.content);
            assert!(
                result.content.contains("regular file"),
                "{}",
                result.content
            );
        }
    }
}

//! Labeled fenced-code-block extraction.
//!
//! A model asked to "build" something emits Markdown with labeled fenced
//! code blocks (e.g. ` ```rust src/main.rs `). This module turns those
//! blocks into real files on disk. It is shared by:
//!
//! - `forge build --output dir/` (the CLI), and
//! - the `forge serve` build endpoint (the desktop/extension UI),
//!
//! so both honor the exact same path-safety rules. It used to live in
//! `src/main.rs`; it was moved here so the server could reuse it without
//! duplicating the parser or the traversal guard.

use anyhow::{anyhow, Context, Result};
use std::path::{Component, Path, PathBuf};

/// Walk a model's response, find labeled fenced code blocks, and write each
/// one as a real file under `out_dir`. Returns the list of paths written.
///
/// **Recognized label syntaxes** (everything after the language tag is the
/// path):
/// - ` ```rust src/main.rs `
/// - ` ```python tests/test_foo.py `
/// - ` ```ts // src/index.ts ` (the leading `//` is tolerated)
/// - ` ```yaml file=.github/workflows/ci.yml ` (the `file=` prefix is tolerated)
///
/// Blocks without a path label are skipped — there's no safe place to put
/// them. The caller can surface the raw response instead, or edit the
/// prompt to ask the model to label its blocks.
///
/// **Path safety**: this legacy text-build path rejects traversal and observed
/// symlink components before writing. It is deliberately kept separate from
/// the descriptor-pinned filesystem capability used by `forge agent` and
/// `forge team`; use those controlled workspace-editing paths for untrusted
/// repositories or autonomous code changes.
pub fn extract_and_write_code_blocks(out_dir: &Path, response: &str) -> Result<Vec<PathBuf>> {
    use std::io::Write as _;
    let output_root = canonical_output_root(out_dir)?;
    let mut written = Vec::new();
    let mut lines = response.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            continue;
        }
        let info = trimmed.trim_start_matches('`').trim();

        // Path extraction: try the fence line first; if no path there,
        // peek at the first line *inside* the block. Small models
        // (qwen3-vl:2b, observed in session 6 smoke test) put the path
        // on its own line right after the opening fence instead of on
        // the fence itself.
        let mut parts = info.splitn(2, char::is_whitespace);
        let _lang = parts.next().unwrap_or("");
        let mut raw_path = parts.next().unwrap_or("").trim().to_string();

        if raw_path.is_empty() {
            if let Some(peeked) = lines.peek() {
                let candidate = peeked.trim();
                if looks_like_path(candidate) {
                    raw_path = candidate.to_string();
                    let _ = lines.next();
                }
            }
        }

        if raw_path.is_empty() {
            consume_until_fence_close(&mut lines);
            continue;
        }

        // Tolerate leading `//`, `#`, or `file=` prefixes so models can use
        // whichever convention they prefer.
        let cleaned = raw_path
            .trim_start_matches("//")
            .trim_start_matches('#')
            .trim_start_matches("file=")
            .trim()
            .to_string();

        let p = PathBuf::from(&cleaned);
        if !is_safe_relative_path(&p) {
            eprintln!("⚠️  refusing unsafe path in code block: {cleaned}");
            consume_until_fence_close(&mut lines);
            continue;
        }

        let mut body = String::new();
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                break;
            }
            body.push_str(inner);
            body.push('\n');
        }

        let target = match safe_output_target(&output_root, &p) {
            Ok(target) => target,
            Err(error) => {
                eprintln!("⚠️  refusing unsafe path in code block: {cleaned} ({error:#})");
                continue;
            }
        };
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&target)
            .with_context(|| format!("open output file {}", target.display()))?;
        f.write_all(body.as_bytes())?;
        written.push(target);
    }
    Ok(written)
}

/// Canonicalize the root once before extraction. A missing root is created
/// only because the caller explicitly selected it; once it exists, its direct
/// metadata and canonical location are checked before model output can use it.
fn canonical_output_root(out_dir: &Path) -> Result<PathBuf> {
    match std::fs::symlink_metadata(out_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(anyhow!(
                "output directory {} is a symlink, which is not allowed",
                out_dir.display()
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(anyhow!(
                "output path {} is not a directory",
                out_dir.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(out_dir)
                .with_context(|| format!("create output directory {}", out_dir.display()))?;
        }
        Err(error) => {
            return Err(anyhow!(
                "inspect output directory {}: {error}",
                out_dir.display()
            ));
        }
    }

    // Inspect again after creation so a concurrent replacement cannot turn the
    // selected root itself into a symlink between the first check and use.
    let metadata = std::fs::symlink_metadata(out_dir)
        .with_context(|| format!("inspect output directory {}", out_dir.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "output directory {} is a symlink, which is not allowed",
            out_dir.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(anyhow!(
            "output path {} is not a directory",
            out_dir.display()
        ));
    }
    let canonical = out_dir
        .canonicalize()
        .with_context(|| format!("canonicalize output directory {}", out_dir.display()))?;
    if !canonical.is_dir() {
        return Err(anyhow!(
            "canonical output path {} is not a directory",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Build a target only after proving that all existing components below the
/// canonical root are ordinary directories/files. Newly created directories
/// are canonicalized again before the file is opened, so a symlink cannot move
/// the eventual target outside the chosen root.
fn safe_output_target(output_root: &Path, relative: &Path) -> Result<PathBuf> {
    if !is_safe_relative_path(relative) {
        return Err(anyhow!("output path must be a non-empty relative path"));
    }

    let lexical_target = output_root.join(relative);
    reject_symlink_components(output_root, &lexical_target)?;
    let parent = lexical_target
        .parent()
        .ok_or_else(|| anyhow!("output path has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create output directory {}", parent.display()))?;

    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("canonicalize output directory {}", parent.display()))?;
    if !canonical_parent.starts_with(output_root) {
        return Err(anyhow!(
            "output path resolves outside the output directory: {}",
            canonical_parent.display()
        ));
    }
    let filename = relative
        .file_name()
        .ok_or_else(|| anyhow!("output path has no filename"))?;
    let target = canonical_parent.join(filename);
    reject_symlink_components(output_root, &target)?;
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(anyhow!("output file {} is a symlink", target.display()));
        }
        Ok(metadata) if metadata.is_dir() => {
            return Err(anyhow!("output file {} is a directory", target.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow!("inspect output file {}: {error}", target.display()));
        }
    }
    Ok(target)
}

fn reject_symlink_components(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| anyhow!("output path escapes the output directory"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!(
                    "output path contains symlink {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(anyhow!(
                    "inspect output path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

/// Heuristic: does this line look like a relative file path the model is
/// using to label the block? Used as a fallback when the path isn't on
/// the fence line.
pub fn looks_like_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    if s.chars()
        .any(|c| !c.is_alphanumeric() && !"._-/+".contains(c))
    {
        return false;
    }
    let stem = s.rsplit('/').next().unwrap_or(s);
    if stem.contains('.') {
        return true;
    }
    // Common extensionless filenames the user might want written.
    matches!(
        stem,
        "Dockerfile" | "Makefile" | "Rakefile" | "Gemfile" | "Procfile" | "LICENSE"
    )
}

fn consume_until_fence_close<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) {
    for inner in lines.by_ref() {
        if inner.trim_start().starts_with("```") {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_labeled_blocks_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = "```rust src/main.rs\nfn main() {}\n```\n";
        let written = extract_and_write_code_blocks(tmp.path(), resp).unwrap();
        assert_eq!(written.len(), 1);
        let body = std::fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(body.contains("fn main()"));
    }

    #[test]
    fn refuses_traversal_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = "```rust ../escape.rs\npwned\n```\n";
        let written = extract_and_write_code_blocks(tmp.path(), resp).unwrap();
        assert!(written.is_empty());
    }

    #[test]
    fn creates_a_missing_output_directory_before_canonicalizing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        let response = "```rust src/main.rs\nfn main() {}\n```\n";
        let written = extract_and_write_code_blocks(&missing, response).unwrap();
        assert_eq!(
            written,
            vec![missing.canonicalize().unwrap().join("src/main.rs")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_output_root() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let real_root = tmp.path().join("real-root");
        std::fs::create_dir(&real_root).unwrap();
        let linked_root = tmp.path().join("linked-root");
        symlink(&real_root, &linked_root).unwrap();

        let response = "```rust src/main.rs\nfn main() {}\n```\n";
        assert!(extract_and_write_code_blocks(&linked_root, response).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_component_escapes() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("out");
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let response = "```rust linked/escape.rs\npwned\n```\n";
        let written = extract_and_write_code_blocks(&root, response).unwrap();
        assert!(written.is_empty());
        assert!(!outside.join("escape.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_output_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("out");
        let outside = tmp.path().join("outside.rs");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&outside, "safe").unwrap();
        symlink(&outside, root.join("main.rs")).unwrap();

        let response = "```rust main.rs\npwned\n```\n";
        let written = extract_and_write_code_blocks(&root, response).unwrap();
        assert!(written.is_empty());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "safe");
    }
}

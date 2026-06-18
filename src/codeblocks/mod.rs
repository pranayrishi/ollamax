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

use anyhow::Result;
use std::path::{Path, PathBuf};

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
/// **Path safety**: any extracted path containing `..` or starting with `/`
/// is rejected and skipped (with a stderr warning). We will not let a model
/// pick `--output ./build` then emit ` ```rust ../../../etc/passwd `.
pub fn extract_and_write_code_blocks(out_dir: &Path, response: &str) -> Result<Vec<PathBuf>> {
    use std::io::Write as _;
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
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
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

        let target = out_dir.join(&p);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&target)?;
        f.write_all(body.as_bytes())?;
        written.push(target);
    }
    Ok(written)
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
}

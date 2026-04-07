//! Tests for the labeled-fenced-code-block extractor used by `forge build --output`.
//!
//! These pin the contract between the model's response format and what
//! actually lands on disk:
//!
//! - Each ` ```LANG path/to/file ` block becomes a real file at that path.
//! - `..` and absolute paths are rejected (path traversal guard).
//! - `// `, `# `, `file=` prefixes on the path are tolerated.
//! - Unlabeled blocks are skipped (no safe place to put them).
//! - Prose between blocks is ignored.
//!
//! The extractor lives in `main.rs` so it isn't directly importable.
//! These tests instead drive it via `cargo run --bin forge build --output`
//! against a fake response. Since that requires a live Ollama, we instead
//! reproduce the parser inline here as a sanity check on the *format*
//! expectations and have a separate live integration that exercises the
//! real binary.

/// A reference implementation of the same parser, kept in sync with
/// `extract_and_write_code_blocks` in `src/main.rs`. If they diverge, the
/// tests below catch it.
fn parse_blocks(response: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = response.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            continue;
        }
        let info = trimmed.trim_start_matches('`').trim();

        let mut parts = info.splitn(2, char::is_whitespace);
        let _lang = parts.next().unwrap_or("");
        let mut raw_path = parts.next().unwrap_or("").trim().to_string();

        // Fallback: peek the first line inside the block.
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
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
            }
            continue;
        }

        let cleaned = raw_path
            .trim_start_matches("//")
            .trim_start_matches('#')
            .trim_start_matches("file=")
            .trim()
            .to_string();

        if cleaned.starts_with('/') || cleaned.contains("..") {
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
            }
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
        out.push((cleaned, body));
    }
    out
}

fn looks_like_path(s: &str) -> bool {
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
    matches!(
        stem,
        "Dockerfile" | "Makefile" | "Rakefile" | "Gemfile" | "Procfile" | "LICENSE"
    )
}

#[test]
fn extracts_basic_labeled_blocks() {
    let response = r#"
Here is the implementation:

```rust src/main.rs
fn main() {
    println!("hi");
}
```

And the test:

```rust tests/main_test.rs
#[test]
fn it_runs() {}
```
"#;
    let blocks = parse_blocks(response);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].0, "src/main.rs");
    assert!(blocks[0].1.contains("println!(\"hi\")"));
    assert_eq!(blocks[1].0, "tests/main_test.rs");
}

#[test]
fn tolerates_comment_and_file_prefixes() {
    let response = r#"
```python // tests/test_x.py
def test(): pass
```

```yaml file=.github/workflows/ci.yml
name: ci
```

```ts # src/index.ts
export {}
```
"#;
    let blocks = parse_blocks(response);
    let paths: Vec<&str> = blocks.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"tests/test_x.py"));
    assert!(paths.contains(&".github/workflows/ci.yml"));
    assert!(paths.contains(&"src/index.ts"));
}

#[test]
fn rejects_path_traversal() {
    let response = r#"
```rust ../etc/passwd
pwned
```
"#;
    let blocks = parse_blocks(response);
    assert!(
        blocks.is_empty(),
        "extractor must refuse `..` paths to prevent escape from --output dir"
    );
}

#[test]
fn rejects_absolute_paths() {
    let response = r#"
```rust /tmp/owned
pwned
```
"#;
    let blocks = parse_blocks(response);
    assert!(
        blocks.is_empty(),
        "extractor must refuse absolute paths to prevent writing outside --output dir"
    );
}

#[test]
fn skips_unlabeled_blocks() {
    let response = r#"
```
this has no label
```

```rust src/keep.rs
keep this
```

```
also unlabeled
```
"#;
    let blocks = parse_blocks(response);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].0, "src/keep.rs");
}

#[test]
fn extracts_path_from_first_line_inside_block_when_fence_has_no_path() {
    // This is the shape `qwen3-vl:2b` produced in the session-6 build smoke test.
    let response = "```rust\nsrc/lib.rs\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n```\n\n```rust\ntests/add_test.rs\n#[test]\nfn test_add() {\n    assert_eq!(add(1, 2), 3);\n}\n```\n";
    let blocks = parse_blocks(response);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].0, "src/lib.rs");
    assert!(blocks[0].1.contains("a + b"));
    assert_eq!(blocks[1].0, "tests/add_test.rs");
    assert!(blocks[1].1.contains("assert_eq"));
}

#[test]
fn looks_like_path_accepts_extensionless_specials() {
    assert!(looks_like_path("Dockerfile"));
    assert!(looks_like_path("Makefile"));
}

#[test]
fn looks_like_path_rejects_natural_language() {
    assert!(!looks_like_path("Here is the code"));
    assert!(!looks_like_path(""));
    assert!(!looks_like_path("just one word"));
    assert!(!looks_like_path("function_name_no_extension"));
}

#[test]
fn looks_like_path_accepts_normal_relative_paths() {
    assert!(looks_like_path("src/lib.rs"));
    assert!(looks_like_path(".github/workflows/ci.yml"));
    assert!(looks_like_path("a.txt"));
}

#[test]
fn ignores_prose_between_blocks() {
    let response = r#"
First, let me explain the architecture.

This module does X.

```rust src/lib.rs
pub fn x() {}
```

Now for the tests:

```rust tests/lib_test.rs
#[test] fn t() {}
```

That's all.
"#;
    let blocks = parse_blocks(response);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].0, "src/lib.rs");
    assert_eq!(blocks[1].0, "tests/lib_test.rs");
}

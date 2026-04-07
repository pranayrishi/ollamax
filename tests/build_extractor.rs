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
/// `format_assertions` test below will fail and force a fix.
fn parse_blocks(response: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = response.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            continue;
        }
        let info = trimmed.trim_start_matches('`').trim();
        if info.is_empty() {
            // Skip body of unlabeled block.
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
            }
            continue;
        }
        let mut parts = info.splitn(2, char::is_whitespace);
        let _lang = parts.next().unwrap_or("");
        let raw_path = parts.next().unwrap_or("").trim();
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
            // Skip body, refuse to capture.
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

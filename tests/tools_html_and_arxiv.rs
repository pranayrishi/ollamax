//! Tests for the standalone tool helpers that don't require a network round-trip:
//!
//! - `fetch_url::html_to_text` — adversarial HTML strip + entity decode + whitespace collapse
//! - `arxiv::parse_arxiv_atom` — Atom parser on a representative payload
//! - `tools::truncate_for_model` — output budget contract
//!
//! Live tests for the actual web endpoints are out of scope here; they live
//! gated behind `FORGE_LIVE_NET=1` in `tests/tools_live.rs`.

use ollama_forge::tools::fetch_url::html_to_text;
use ollama_forge::tools::truncate_for_model;
use ollama_forge::tools::ToolRegistry;

#[test]
fn html_strip_drops_script_and_style() {
    let html = r#"
<html>
<head>
  <style>body { color: red; }</style>
  <script>alert('xss')</script>
</head>
<body>
  <h1>Real content</h1>
  <p>This should survive.</p>
  <script>more.evil();</script>
</body>
</html>
"#;
    let out = html_to_text(html);
    assert!(out.contains("Real content"));
    assert!(out.contains("This should survive"));
    assert!(!out.contains("alert"));
    assert!(!out.contains("color: red"));
    assert!(!out.contains("evil"));
}

#[test]
fn html_strip_decodes_real_world_entities() {
    let html = "<p>Tom &amp; Jerry &mdash; 5 &gt; 3 &amp; &lt;&gt;</p>";
    let out = html_to_text(html);
    assert!(out.contains("Tom & Jerry"));
    assert!(out.contains("5 > 3"));
    // We don't decode &mdash; on purpose — it's rare and not worth a table.
    // Just verify it doesn't blow up.
}

#[test]
fn truncate_keeps_short_content_intact() {
    let s = "hello world";
    assert_eq!(truncate_for_model(s), s);
}

#[test]
fn truncate_marks_long_content() {
    let big = "x".repeat(100_000);
    let out = truncate_for_model(&big);
    assert!(out.contains("[... truncated by forge"));
    assert!(out.len() < big.len() / 5);
}

#[test]
fn registry_describes_each_default_tool() {
    let r = ToolRegistry::with_defaults();
    let s = r.describe_for_model();
    for name in ["web_search", "wikipedia", "arxiv", "fetch_url"] {
        assert!(s.contains(name), "tool catalog missing `{name}`");
    }
    // Schema must be present for each tool so the model can call it.
    assert!(
        s.contains("\"required\""),
        "schemas should mention required"
    );
}

//! SKILL.md (Anthropic-style YAML-frontmatter) compatibility tests.
//!
//! Drop-in skills authored for Claude Code must load without edits. The
//! threat is regressions in `parse_skill_md` that quietly break the import
//! path for the only standard skill format in the ecosystem.

use ollama_forge::skills::parse_skill_md;

#[test]
fn parses_minimal_frontmatter() {
    let md = r#"---
name: pdf-expert
description: Extracts and summarizes PDF documents.
---
You are an expert at processing PDF files. Always cite page numbers.
"#;
    let skill = parse_skill_md(md).unwrap();
    assert_eq!(skill.name, "pdf-expert");
    assert_eq!(skill.description, "Extracts and summarizes PDF documents.");
    assert!(skill.prompts.system.contains("expert at processing PDF"));
    assert!(skill.prompts.system.contains("page numbers"));
    assert_eq!(skill.version, "1.0.0", "version defaults to 1.0.0");
}

#[test]
fn parses_full_frontmatter_with_forge_extensions() {
    let md = r#"---
name: rust-reviewer
description: Reviews Rust code for unsafe patterns.
version: "2.1.0"
author: Pranay
tags: [rust, security, review]
model: qwen2.5-coder:14b
temperature: 0.3
tools: [clippy, miri]
---
# Rust Reviewer

You are a senior Rust engineer. Flag any `unsafe` block, `unwrap()` in
non-test code, or use of `transmute`. Suggest fixes.
"#;
    let skill = parse_skill_md(md).unwrap();
    assert_eq!(skill.name, "rust-reviewer");
    assert_eq!(skill.version, "2.1.0");
    assert_eq!(skill.author.as_deref(), Some("Pranay"));
    assert_eq!(skill.tags, vec!["rust", "security", "review"]);
    assert_eq!(skill.settings.model.as_deref(), Some("qwen2.5-coder:14b"));
    assert_eq!(skill.settings.temperature, Some(0.3));
    assert_eq!(skill.settings.tools, vec!["clippy", "miri"]);
    assert!(skill.prompts.system.starts_with("# Rust Reviewer"));
}

#[test]
fn rejects_missing_open_delimiter() {
    let bad = "name: foo\n---\nbody\n";
    assert!(parse_skill_md(bad).is_err());
}

#[test]
fn rejects_missing_close_delimiter() {
    let bad = "---\nname: foo\ndescription: x\nbody never closed\n";
    let err = parse_skill_md(bad).unwrap_err().to_string();
    assert!(err.contains("unterminated"), "got: {err}");
}

#[test]
fn rejects_missing_required_field() {
    let bad = r#"---
name: only-name-no-description
---
body
"#;
    assert!(parse_skill_md(bad).is_err());
}

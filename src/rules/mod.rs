//! Persistent user rules — the "things I always want the model to follow"
//! layer.
//!
//! Inspired by ECC's `rules/` directory and similar features in Cursor and
//! Continue.dev. The user drops Markdown files into
//! `~/.config/ollama-forge/rules/` (or `$XDG_CONFIG_HOME/ollama-forge/rules/`)
//! and every forge command that talks to a model — `chat`, `research`,
//! `run-skill`, `analyze`, `test`, the orchestrator workers — automatically
//! prepends them to the system prompt.
//!
//! ## Why files, not config keys
//!
//! Rules are long, prose-heavy, and frequently edited. Stuffing them into a
//! TOML key would force the user to escape newlines and lose syntax
//! highlighting. Markdown files in a directory let the user version-control
//! them, sync them across machines via dotfiles, and edit them in their
//! normal editor.
//!
//! ## Format
//!
//! Two flavors are supported:
//!
//! 1. **Plain Markdown** — the entire file is treated as one rule. The
//!    filename (sans `.md`) becomes the rule name.
//! 2. **YAML-frontmatter** — same shape as `SKILL.md`. Optional
//!    `name`/`description`/`scope` fields. The Markdown body is the rule
//!    text. This lets the user add metadata without bloating the rule
//!    itself.
//!
//! Rules are concatenated alphabetically by filename so the user controls
//! ordering with a `00-`, `10-`, `20-` prefix convention.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One always-applied user rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    /// Optional one-line description for `forge rules list`.
    pub description: Option<String>,
    /// The actual instruction text injected into the system prompt.
    pub body: String,
    /// Source file path, for `forge rules list`/edit messages.
    pub source: PathBuf,
}

#[derive(Debug, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    pub dir: PathBuf,
}

impl RuleSet {
    /// Default rules directory: `$XDG_CONFIG_HOME/ollama-forge/rules`.
    pub fn default_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ollama-forge")
            .join("rules")
    }

    /// Load every `*.md` file under `dir`. Returns an empty `RuleSet` (no
    /// error) if the directory doesn't exist — that's the first-run case
    /// and forge should keep working.
    pub fn load_from(dir: PathBuf) -> Result<Self> {
        let mut set = Self {
            rules: Vec::new(),
            dir: dir.clone(),
        };
        if !dir.exists() {
            return Ok(set);
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("read rules dir {}", dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
            .collect();
        // Alphabetical sort so the user controls ordering with prefixes.
        entries.sort();
        for path in entries {
            match Self::parse_file(&path) {
                Ok(rule) => set.rules.push(rule),
                Err(e) => tracing::warn!("rules: skipping {}: {e}", path.display()),
            }
        }
        Ok(set)
    }

    /// Convenience: load from the default dir.
    pub fn load_default() -> Result<Self> {
        Self::load_from(Self::default_dir())
    }

    fn parse_file(path: &Path) -> Result<Rule> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("rule")
            .to_string();

        // Try YAML frontmatter first; fall back to plain markdown.
        if raw.trim_start().starts_with("---") {
            if let Some((name, desc, body)) = parse_frontmatter(&raw) {
                return Ok(Rule {
                    name: name.unwrap_or_else(|| stem.clone()),
                    description: desc,
                    body,
                    source: path.to_path_buf(),
                });
            }
        }
        Ok(Rule {
            name: stem,
            description: None,
            body: raw.trim().to_string(),
            source: path.to_path_buf(),
        })
    }

    /// Render the rules as a system-prompt suffix. Empty string when no
    /// rules are configured — callers can unconditionally append.
    pub fn render(&self) -> String {
        if self.rules.is_empty() {
            return String::new();
        }
        let mut s = String::new();
        s.push_str("\n\n## User-defined always-rules\n");
        s.push_str("(Configured in ");
        s.push_str(&self.dir.display().to_string());
        s.push_str(". Follow these without exception.)\n\n");
        for r in &self.rules {
            s.push_str(&format!("### {}\n", r.name));
            s.push_str(r.body.trim());
            s.push_str("\n\n");
        }
        s
    }

    /// True when the user has at least one rule configured.
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_frontmatter(raw: &str) -> Option<(Option<String>, Option<String>, String)> {
    let body = raw.trim_start();
    if !body.starts_with("---") {
        return None;
    }
    let after = &body[3..];
    let close = after.find("\n---")?;
    let yaml = &after[..close];
    let md = after[close + 4..].trim_start_matches('\n').to_string();
    let fm: Frontmatter = serde_yaml::from_str(yaml).ok()?;
    Some((fm.name, fm.description, md))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_dir_yields_empty_set() {
        let tmp = tempfile::tempdir().unwrap();
        let set = RuleSet::load_from(tmp.path().join("rules")).unwrap();
        assert!(set.is_empty());
        assert_eq!(set.render(), "");
    }

    #[test]
    fn missing_dir_is_not_an_error() {
        let set = RuleSet::load_from("/definitely/does/not/exist".into()).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn loads_plain_markdown_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(dir.join("00-style.md"), "Always use 4-space indentation.").unwrap();
        fs::write(
            dir.join("10-tests.md"),
            "Every public function needs a test.",
        )
        .unwrap();
        let set = RuleSet::load_from(dir).unwrap();
        assert_eq!(set.len(), 2);
        // Alphabetical ordering by filename.
        assert_eq!(set.rules[0].name, "00-style");
        assert_eq!(set.rules[1].name, "10-tests");
        assert!(set.rules[0].body.contains("4-space"));
    }

    #[test]
    fn parses_yaml_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("rust.md"),
            "---\nname: rust-style\ndescription: Rust-specific style rules\n---\nNo `unwrap()` outside of tests.\n",
        )
        .unwrap();
        let set = RuleSet::load_from(dir).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.rules[0].name, "rust-style");
        assert_eq!(
            set.rules[0].description.as_deref(),
            Some("Rust-specific style rules")
        );
        assert!(set.rules[0].body.contains("unwrap()"));
    }

    #[test]
    fn render_concatenates_alphabetically() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(dir.join("zz-last.md"), "Last rule.").unwrap();
        fs::write(dir.join("aa-first.md"), "First rule.").unwrap();
        let set = RuleSet::load_from(dir).unwrap();
        let rendered = set.render();
        let pos_first = rendered.find("First rule").unwrap();
        let pos_last = rendered.find("Last rule").unwrap();
        assert!(pos_first < pos_last);
    }

    #[test]
    fn render_includes_section_header_when_nonempty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(dir.join("a.md"), "test rule").unwrap();
        let set = RuleSet::load_from(dir).unwrap();
        let r = set.render();
        assert!(r.contains("User-defined always-rules"));
        assert!(r.contains("test rule"));
    }
}

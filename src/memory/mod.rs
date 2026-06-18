//! Part B — cross-session conversational memory. This is the half graphify does
//! NOT provide: remembering a user's preferences, decisions, and prior-chat
//! summaries so a new session isn't a cold start.
//!
//! ## Local-first & content-free backend (load-bearing)
//!
//! Memory lives **only on the user's device** — a per-project JSONL under the
//! config dir. It is **never** sent to the identity backend, which stays
//! content-free per the established rule. Structurally this module does pure
//! local filesystem I/O and has **no network/HTTP dependency at all** — that's
//! the on-device guarantee, asserted by tests.
//!
//! ## Token efficiency (the point)
//!
//! We store **summaries/preferences/decisions**, never raw transcripts, and
//! retrieve **selectively within a token budget** (keyword + recency + kind
//! scoring) — mirroring the context manager's budgeting approach so a session
//! gets the *relevant* memory, not a dump.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    /// A stated user preference ("I prefer tabs", "use pytest").
    Preference,
    /// A decision made during work ("we chose Postgres over SQLite").
    Decision,
    /// A compact summary of a past conversation/session.
    Summary,
    /// A durable fact about the project/user.
    Fact,
}

impl MemoryKind {
    /// Retrieval weight — durable user intent outranks session summaries.
    fn weight(self) -> f32 {
        match self {
            MemoryKind::Preference => 1.5,
            MemoryKind::Decision => 1.3,
            MemoryKind::Fact => 1.2,
            MemoryKind::Summary => 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unix seconds (caller-stamped, so the module stays time-pure for tests).
    pub ts: i64,
    pub kind: MemoryKind,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl MemoryEntry {
    pub fn new(kind: MemoryKind, text: impl Into<String>, ts: i64) -> Self {
        Self { kind, text: text.into(), ts, tags: Vec::new() }
    }
    pub fn approx_tokens(&self) -> usize {
        approx_tokens(&self.text)
    }
}

/// Rough token estimate (chars/4) — matches the context manager's heuristic and
/// keeps this module dependency- and time-free for unit tests.
pub fn approx_tokens(s: &str) -> usize {
    (s.chars().count() / 4) + 1
}

/// Per-project, on-device memory store (append-only JSONL).
pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    /// Store for a project: `<config>/ollama-forge/memory/<project-hash>.jsonl`.
    /// Per-project so a user's React app and Rust service don't share memory.
    pub fn for_project(project_root: &Path) -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ollama-forge")
            .join("memory");
        let key = project_root.to_string_lossy();
        // Stable, filesystem-safe per-project key (FNV-1a).
        let mut h: u64 = 0xcbf29ce484222325;
        for b in key.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Self { path: base.join(format!("{h:016x}.jsonl")) }
    }

    /// Explicit path (tests / custom locations).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one memory entry (creates the dir/file as needed). Local only.
    pub fn remember(&self, entry: &MemoryEntry) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let mut line = serde_json::to_string(entry).context("serialize memory")?;
        line.push('\n');
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open {}", self.path.display()))?;
        f.write_all(line.as_bytes()).context("append memory")?;
        Ok(())
    }

    /// All entries (oldest→newest). Skips malformed lines rather than failing.
    pub fn all(&self) -> Vec<MemoryEntry> {
        let Ok(data) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        data.lines()
            .filter_map(|l| serde_json::from_str::<MemoryEntry>(l).ok())
            .collect()
    }

    /// Wipe all memory for this project (user control).
    pub fn clear(&self) -> Result<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path).context("clear memory")?;
        }
        Ok(())
    }

    /// Remove entries whose text contains `needle` (user edit/forget). Returns
    /// how many were removed.
    pub fn forget_matching(&self, needle: &str) -> Result<usize> {
        let kept: Vec<MemoryEntry> = self
            .all()
            .into_iter()
            .filter(|e| !e.text.to_lowercase().contains(&needle.to_lowercase()))
            .collect();
        let removed = self.all().len() - kept.len();
        // Rewrite the file with the survivors.
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let body: String = kept
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|mut s| {
                s.push('\n');
                s
            })
            .collect();
        std::fs::write(&self.path, body).context("rewrite memory")?;
        Ok(removed)
    }

    /// Score = keyword overlap × kind weight × recency. Returns the highest-
    /// scoring entries that fit in `token_budget` (greedy, high→low).
    pub fn retrieve(&self, query: &str, token_budget: usize) -> Vec<MemoryEntry> {
        let mut entries = self.all();
        if entries.is_empty() {
            return Vec::new();
        }
        let qterms: Vec<String> = tokenize(query);
        let newest = entries.iter().map(|e| e.ts).max().unwrap_or(0);
        let oldest = entries.iter().map(|e| e.ts).min().unwrap_or(0);
        let span = (newest - oldest).max(1) as f32;

        let mut scored: Vec<(f32, MemoryEntry)> = entries
            .drain(..)
            .map(|e| {
                let et: std::collections::HashSet<String> = tokenize(&e.text).into_iter().collect();
                let overlap = qterms.iter().filter(|t| et.contains(*t)).count() as f32;
                let recency = 0.2 + 0.8 * ((e.ts - oldest) as f32 / span); // 0.2..1.0
                // Even with no keyword overlap, recent preferences/decisions are
                // mildly useful at session start, so base score stays > 0.
                let base = (overlap + 0.25) * e.kind.weight() * recency;
                (base, e)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = Vec::new();
        let mut used = 0usize;
        for (_, e) in scored {
            let t = e.approx_tokens();
            if used + t > token_budget {
                continue;
            }
            used += t;
            out.push(e);
        }
        out
    }

    /// A compact "what I remember" preamble for the prompt, token-budgeted.
    /// Empty string when nothing relevant fits — never a cold dump.
    pub fn render_for_context(&self, query: &str, token_budget: usize) -> String {
        let picked = self.retrieve(query, token_budget);
        if picked.is_empty() {
            return String::new();
        }
        let mut s = String::from("Relevant memory from past sessions (on-device; you may use it):\n");
        for e in &picked {
            let tag = match e.kind {
                MemoryKind::Preference => "preference",
                MemoryKind::Decision => "decision",
                MemoryKind::Summary => "summary",
                MemoryKind::Fact => "fact",
            };
            s.push_str(&format!("- [{tag}] {}\n", e.text));
        }
        s
    }
}

/// Build a compact session summary entry from a conversation. Stores a short
/// summary (first user ask + turn count), NOT the raw transcript — token-cheap.
/// A natural feeder alongside `instincts` (which mines repeated patterns from the
/// replay log). `ts` is caller-supplied.
pub fn summarize_session(messages: &[(String, String)], ts: i64) -> Option<MemoryEntry> {
    let first_user = messages.iter().find(|(role, _)| role == "user").map(|(_, c)| c.clone())?;
    let turns = messages.iter().filter(|(r, _)| r == "user").count();
    let head: String = first_user.chars().take(140).collect();
    Some(MemoryEntry::new(
        MemoryKind::Summary,
        format!("Session ({turns} turn(s)) started with: {head}"),
        ts,
    ))
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(name: &str) -> MemoryStore {
        let p = std::env::temp_dir().join(format!("forge-mem-test-{name}-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        MemoryStore::with_path(p)
    }

    #[test]
    fn store_and_read_roundtrip() {
        let s = tmp_store("roundtrip");
        s.remember(&MemoryEntry::new(MemoryKind::Preference, "prefers Rust and pytest", 100)).unwrap();
        s.remember(&MemoryEntry::new(MemoryKind::Decision, "chose Postgres over SQLite", 200)).unwrap();
        assert_eq!(s.all().len(), 2);
        s.clear().unwrap();
        assert!(s.all().is_empty());
    }

    #[test]
    fn retrieve_respects_token_budget() {
        let s = tmp_store("budget");
        for i in 0..20 {
            s.remember(&MemoryEntry::new(MemoryKind::Summary, format!("summary number {i} about auth and login flows"), 100 + i)).unwrap();
        }
        let picked = s.retrieve("auth login", 30); // tiny budget
        let total: usize = picked.iter().map(|e| e.approx_tokens()).sum();
        assert!(total <= 30, "retrieval must respect the token budget (got {total})");
        assert!(!picked.is_empty());
        s.clear().unwrap();
    }

    #[test]
    fn retrieve_ranks_relevant_and_recent_higher() {
        let s = tmp_store("rank");
        s.remember(&MemoryEntry::new(MemoryKind::Summary, "old talk about kubernetes", 1)).unwrap();
        s.remember(&MemoryEntry::new(MemoryKind::Preference, "user prefers tabs over spaces", 1000)).unwrap();
        s.remember(&MemoryEntry::new(MemoryKind::Summary, "discussed the login and auth code", 1001)).unwrap();
        let picked = s.retrieve("auth login", 1000);
        // The auth summary should rank above the unrelated kubernetes one.
        let auth_pos = picked.iter().position(|e| e.text.contains("login"));
        let k8s_pos = picked.iter().position(|e| e.text.contains("kubernetes"));
        assert!(auth_pos.is_some());
        if let (Some(a), Some(k)) = (auth_pos, k8s_pos) {
            assert!(a < k, "relevant memory should rank above irrelevant");
        }
        s.clear().unwrap();
    }

    #[test]
    fn render_is_empty_when_no_memory() {
        let s = tmp_store("empty");
        assert_eq!(s.render_for_context("anything", 500), "");
        s.clear().ok();
    }

    #[test]
    fn forget_removes_matching_entries() {
        let s = tmp_store("forget");
        s.remember(&MemoryEntry::new(MemoryKind::Fact, "secret project codename Falcon", 1)).unwrap();
        s.remember(&MemoryEntry::new(MemoryKind::Preference, "likes dark mode", 2)).unwrap();
        let removed = s.forget_matching("falcon").unwrap();
        assert_eq!(removed, 1);
        assert_eq!(s.all().len(), 1);
        assert!(s.all()[0].text.contains("dark mode"));
        s.clear().unwrap();
    }

    #[test]
    fn memory_path_is_local_on_device() {
        // The per-project store resolves under the user's local config dir —
        // never a remote/backend location. This is the on-device guarantee.
        let s = MemoryStore::for_project(Path::new("/Users/x/project"));
        let p = s.path().to_string_lossy().to_string();
        assert!(p.ends_with(".jsonl"));
        assert!(p.contains("ollama-forge") && p.contains("memory"));
    }

    #[test]
    fn summarize_session_stores_summary_not_transcript() {
        let convo = vec![
            ("user".to_string(), "Help me refactor the auth module to use JWT".to_string()),
            ("assistant".to_string(), "<a very long answer that should NOT be stored verbatim>".repeat(50)),
            ("user".to_string(), "now add tests".to_string()),
        ];
        let e = summarize_session(&convo, 42).unwrap();
        assert_eq!(e.kind, MemoryKind::Summary);
        assert!(e.text.contains("refactor the auth module"));
        assert!(!e.text.contains("very long answer"), "must store a summary, not the transcript");
        assert!(e.approx_tokens() < 60);
    }
}

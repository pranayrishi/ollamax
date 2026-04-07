//! Deterministic replay log.
//!
//! Every Ollama call the binary makes can be logged to a JSON-Lines file
//! along with everything needed to reproduce it later: the model digest
//! (so we know what weights were running), the seed/temperature/top_p
//! (so the sampler is reproducible), the prompt + system + format params
//! (so the input is identical), and a hash of the response (so we can
//! detect drift).
//!
//! ## Why this matters
//!
//! Hosted models silently rotate weights, so reproducibility is impossible
//! on Claude/GPT/Gemini. Ollama lets you pin a model digest forever — `ollama
//! pull qwen2.5-coder:7b` today and a year from now is the same SHA. That
//! means an "AI-assisted change" in a regulated codebase can have a
//! cryptographic audit trail. This is the wedge against hosted tools for
//! finance, healthcare, defense, legal — exactly the audiences who can't
//! ship code through a third party.
//!
//! ## On-disk format
//!
//! JSON Lines, one record per Ollama call:
//!
//! ```json
//! {"ts":"2026-04-07T...","forge_version":"0.1.0 (abc123)","model":"qwen2.5-coder:7b","model_digest":"sha256:...","seed":42,"temperature":0.2,"top_p":0.9,"num_ctx":16384,"system":"...","prompt":"...","format":"json","prompt_hash":"sha256:...","response_hash":"sha256:...","response":"..."}
//! ```
//!
//! Replay reads this file, re-issues each call against the same model
//! (which must still be installed locally), and compares the new response
//! hash against the recorded one. Any mismatch is surfaced.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub ts: String,
    pub forge_version: String,
    pub model: String,
    /// Ollama model digest. Empty when unavailable (we don't crash on a
    /// missing digest because the rest of the record is still useful).
    #[serde(default)]
    pub model_digest: String,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub num_ctx: Option<usize>,
    pub keep_alive: Option<String>,
    /// PRNG seed. Must be present for a deterministic replay; without it,
    /// even temperature=0 can drift on some samplers.
    #[serde(default)]
    pub seed: Option<i64>,
    /// Optional `format` parameter (Ollama v0.5+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<serde_json::Value>,
    pub system: Option<String>,
    pub prompt: String,
    /// SHA-256 of `prompt` (system + format included). Pinned for replay.
    pub prompt_hash: String,
    /// SHA-256 of `response`.
    pub response_hash: String,
    /// Response text. Truncated to 16 KB so the log doesn't balloon, but
    /// the hash covers the *full* response.
    pub response: String,
}

/// Append-only writer for replay records. Thread-safe.
pub struct ReplayLog {
    path: PathBuf,
    // tokio::sync::Mutex so the critical section can hold across the async
    // file writes. Concurrent appends serialize through this so we don't
    // interleave half-written JSON lines into the file.
    _lock: Mutex<()>,
}

impl ReplayLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            _lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn append(&self, record: &ReplayRecord) -> Result<()> {
        let line = serde_json::to_string(record).context("serialize replay record")?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        // Tokio Mutex so the critical section can hold across the async
        // file writes without tripping clippy::await_holding_lock.
        let _g = self._lock.lock().await;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("open replay log {}", self.path.display()))?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        Ok(())
    }
}

/// Real hex SHA-256 of a byte slice. Stable across Rust versions and across
/// machines, which is the whole point of the replay log.
///
/// **This used to be a `DefaultHasher`-based shim** because we were trying
/// to avoid the `sha2` dep. That was wrong: `DefaultHasher` is documented
/// as "may change between Rust releases" and uses SipHash-1-3, neither of
/// which gives us a stable cross-version hash. Replay logs written with
/// the old shim would silently drift on a future stdlib bump. `sha2` is
/// small, audited, and pure Rust — the right call.
pub fn quick_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

/// Read a replay log into memory. Returns one `ReplayRecord` per non-empty
/// line; lines that fail to parse are skipped with a warning so a
/// truncated log doesn't kill the whole replay.
pub async fn read_log(path: &Path) -> Result<Vec<ReplayRecord>> {
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read replay log {}", path.display()))?;
    let mut out = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ReplayRecord>(line) {
            Ok(r) => out.push(r),
            Err(e) => tracing::warn!("replay: skipping malformed line {}: {e}", line_num + 1),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_hash_is_stable_and_uses_sha256() {
        assert_eq!(quick_hash(b"hello"), quick_hash(b"hello"));
        assert_ne!(quick_hash(b"hello"), quick_hash(b"goodbye"));
        assert!(quick_hash(b"hello").starts_with("sha256:"));
        // Pin a known SHA-256 of "hello" so any bump that breaks the hash
        // function will trip this test (and prevent silent replay drift).
        assert_eq!(
            quick_hash(b"hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[tokio::test]
    async fn append_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("replay.jsonl");
        let log = ReplayLog::new(&path);

        let r1 = ReplayRecord {
            ts: "2026-04-07T00:00:00Z".to_string(),
            forge_version: "0.1.0 (test)".to_string(),
            model: "qwen2.5-coder:7b".to_string(),
            model_digest: "sha256:deadbeef".to_string(),
            temperature: Some(0.2),
            top_p: Some(0.9),
            num_ctx: Some(16384),
            keep_alive: Some("1h".to_string()),
            seed: Some(42),
            format: None,
            system: Some("you are a coder".to_string()),
            prompt: "hello".to_string(),
            prompt_hash: quick_hash(b"hello"),
            response_hash: quick_hash(b"hi there"),
            response: "hi there".to_string(),
        };
        log.append(&r1).await.unwrap();

        let mut r2 = r1.clone();
        r2.prompt = "second".to_string();
        log.append(&r2).await.unwrap();

        let read = read_log(&path).await.unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].prompt, "hello");
        assert_eq!(read[1].prompt, "second");
    }

    #[tokio::test]
    async fn read_skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("replay.jsonl");
        tokio::fs::write(
            &path,
            "not json\n{\"ts\":\"x\",\"forge_version\":\"v\",\"model\":\"m\",\"prompt\":\"p\",\"prompt_hash\":\"h\",\"response_hash\":\"h\",\"response\":\"r\"}\nalso not json\n",
        )
        .await
        .unwrap();
        let read = read_log(&path).await.unwrap();
        assert_eq!(read.len(), 1, "should have skipped 2 garbage lines");
    }
}

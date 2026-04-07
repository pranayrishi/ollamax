//! Continuous-learning loop — surfaces patterns from the replay log as
//! candidate skills/rules.
//!
//! ## What this does
//!
//! Reads `FORGE_REPLAY_LOG` (or a path passed in), groups records by what
//! the user asked, and surfaces patterns the user repeats. The output is a
//! set of *candidate* skills and rules — the user reviews, then promotes
//! the ones they want via `forge skills add` or by saving a `rules/*.md`.
//!
//! ## Design (intentionally not auto-promoting)
//!
//! ECC's "instincts" feature auto-promotes patterns to skills with a
//! confidence score. We deliberately don't do that here because:
//!
//! 1. The replay log is the user's full prompt history including private
//!    code. Auto-extracting that into a skill that gets shared across
//!    sessions is a privacy footgun.
//! 2. Small local models hallucinate enough that an auto-promoted pattern
//!    could be wrong. A human-in-the-loop review step is the safer default.
//!
//! So this module is read-only against the log. It surfaces, never writes.
//!
//! ## Patterns we surface
//!
//! - **Repeated tasks**: prompts that look similar (normalized first 60
//!   chars) appearing 3+ times. Suggests the user could turn the workflow
//!   into a skill.
//! - **Repeated tools**: in agent records, the same tool sequence appearing
//!   3+ times. Suggests the user could promote the chain to a recipe.
//! - **Common system prompts**: if the user manually re-types the same
//!   system prompt, surface it as a candidate rule.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use crate::replay::{stream_log, ReplayRecord};

/// Min number of times a pattern must repeat before we surface it.
pub const MIN_OCCURRENCES: usize = 3;

#[derive(Debug, Clone)]
pub struct InstinctsReport {
    pub total_records: usize,
    pub repeated_tasks: Vec<RepeatedPattern>,
    pub repeated_systems: Vec<RepeatedPattern>,
    /// Tool-call sequences observed inside agent records (e.g., the
    /// chain `web_search → fetch_url → wikipedia`). Surfaced when the
    /// same sequence appears in `threshold+` distinct sessions.
    /// Promote these to recipes inside skills.
    pub repeated_tool_chains: Vec<RepeatedPattern>,
}

#[derive(Debug, Clone)]
pub struct RepeatedPattern {
    /// Normalized text the records had in common.
    pub canonical: String,
    /// How many records contained this pattern.
    pub count: usize,
    /// Distinct models the pattern was used against.
    pub models: Vec<String>,
}

/// Build an `InstinctsReport` from a replay log file. Returns an empty
/// report (not an error) when the file doesn't exist — this is the
/// first-run case and `forge instincts` should print "no log yet" cleanly.
pub async fn from_log(path: &Path, threshold: usize) -> Result<InstinctsReport> {
    if !path.exists() {
        return Ok(InstinctsReport {
            total_records: 0,
            repeated_tasks: Vec::new(),
            repeated_systems: Vec::new(),
            repeated_tool_chains: Vec::new(),
        });
    }
    // Streaming pass: we accumulate enough state to do the analysis
    // without holding every record in memory at once. This matters once
    // a long-lived user's log crosses ~50 MB.
    let mut records = Vec::new();
    stream_log(path, |r| records.push(r)).await?;
    Ok(analyze_with_threshold(&records, threshold))
}

/// Convenience analyzer using the default `MIN_OCCURRENCES` threshold.
pub fn analyze(records: &[ReplayRecord]) -> InstinctsReport {
    analyze_with_threshold(records, MIN_OCCURRENCES)
}

/// Pure-function analyzer with a configurable repetition threshold.
/// Caller-supplied threshold is clamped to `>= 2` (a threshold of 1 would
/// surface every record as its own pattern).
pub fn analyze_with_threshold(records: &[ReplayRecord], threshold: usize) -> InstinctsReport {
    let threshold = threshold.max(2);
    let total_records = records.len();

    // Repeated tasks: normalize the first 80 chars of each prompt
    // (lowercase + collapse whitespace), count, surface duplicates.
    let mut by_task: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for r in records {
        let canon = normalize(&r.prompt, 80);
        if canon.is_empty() {
            continue;
        }
        let entry = by_task.entry(canon).or_insert_with(|| (0, Vec::new()));
        entry.0 += 1;
        if !entry.1.contains(&r.model) {
            entry.1.push(r.model.clone());
        }
    }

    let mut repeated_tasks: Vec<RepeatedPattern> = by_task
        .into_iter()
        .filter(|(_, (count, _))| *count >= threshold)
        .map(|(canonical, (count, models))| RepeatedPattern {
            canonical,
            count,
            models,
        })
        .collect();
    repeated_tasks.sort_by(|a, b| b.count.cmp(&a.count));

    // Repeated system prompts: same idea but on the (optional) system field.
    let mut by_system: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for r in records {
        let Some(sys) = &r.system else {
            continue;
        };
        let canon = normalize(sys, 120);
        if canon.is_empty() {
            continue;
        }
        let entry = by_system.entry(canon).or_insert_with(|| (0, Vec::new()));
        entry.0 += 1;
        if !entry.1.contains(&r.model) {
            entry.1.push(r.model.clone());
        }
    }

    let mut repeated_systems: Vec<RepeatedPattern> = by_system
        .into_iter()
        .filter(|(_, (count, _))| *count >= threshold)
        .map(|(canonical, (count, models))| RepeatedPattern {
            canonical,
            count,
            models,
        })
        .collect();
    repeated_systems.sort_by(|a, b| b.count.cmp(&a.count));

    // Tool-call chain extraction: every agent record's prompt is the
    // running transcript, which contains lines like:
    //   [round N] You called tool `web_search` with args:
    // We grep them out, normalize to a `→`-separated chain, and count
    // how many records produced each chain. A chain seen 3+ times is a
    // candidate recipe.
    let mut by_chain: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for r in records {
        let chain = extract_tool_chain(&r.prompt);
        if chain.is_empty() {
            continue;
        }
        let canon = chain.join(" → ");
        let entry = by_chain.entry(canon).or_insert_with(|| (0, Vec::new()));
        entry.0 += 1;
        if !entry.1.contains(&r.model) {
            entry.1.push(r.model.clone());
        }
    }
    let mut repeated_tool_chains: Vec<RepeatedPattern> = by_chain
        .into_iter()
        .filter(|(_, (count, _))| *count >= threshold)
        .map(|(canonical, (count, models))| RepeatedPattern {
            canonical,
            count,
            models,
        })
        .collect();
    repeated_tool_chains.sort_by(|a, b| b.count.cmp(&a.count));

    InstinctsReport {
        total_records,
        repeated_tasks,
        repeated_systems,
        repeated_tool_chains,
    }
}

/// Extract the ordered list of tool names called inside an agent record's
/// prompt. Returns an empty Vec for non-agent records (chat, run-skill,
/// etc.) which never contain `[round N] You called tool` markers.
///
/// Format we look for, emitted by `agent::record_step`:
///   `[round 1] You called tool ` + backtick + `<name>` + backtick + ` with args:`
fn extract_tool_chain(prompt: &str) -> Vec<String> {
    let mut chain = Vec::new();
    for line in prompt.lines() {
        let Some(idx) = line.find("You called tool `") else {
            continue;
        };
        let after = &line[idx + "You called tool `".len()..];
        if let Some(end) = after.find('`') {
            chain.push(after[..end].to_string());
        }
    }
    chain
}

fn normalize(s: &str, max_chars: usize) -> String {
    let collapsed = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(model: &str, prompt: &str, system: Option<&str>) -> ReplayRecord {
        ReplayRecord {
            ts: "2026-04-07T00:00:00Z".into(),
            forge_version: "test".into(),
            model: model.into(),
            model_digest: "sha256:test".into(),
            temperature: Some(0.0),
            top_p: None,
            num_ctx: Some(4096),
            keep_alive: None,
            seed: Some(0),
            format: None,
            system: system.map(String::from),
            prompt: prompt.into(),
            prompt_hash: "h".into(),
            response_hash: "h".into(),
            response: String::new(),
        }
    }

    #[test]
    fn empty_log_yields_empty_report() {
        let r = analyze(&[]);
        assert_eq!(r.total_records, 0);
        assert!(r.repeated_tasks.is_empty());
    }

    #[test]
    fn surfaces_a_task_repeated_three_times() {
        let records = vec![
            rec("m", "translate this rust to python", None),
            rec("m", "translate this rust to python", None),
            rec("m", "translate this rust to python", None),
        ];
        let r = analyze(&records);
        assert_eq!(r.repeated_tasks.len(), 1);
        assert_eq!(r.repeated_tasks[0].count, 3);
    }

    #[test]
    fn does_not_surface_a_task_seen_twice() {
        let records = vec![
            rec("m", "explain this regex", None),
            rec("m", "explain this regex", None),
        ];
        let r = analyze(&records);
        assert!(
            r.repeated_tasks.is_empty(),
            "two occurrences should not promote"
        );
    }

    #[test]
    fn collapses_whitespace_and_case() {
        let records = vec![
            rec("m", "  TRANSLATE   this rust to python\n", None),
            rec("m", "translate this rust to PYTHON", None),
            rec("m", "Translate This Rust to Python", None),
        ];
        let r = analyze(&records);
        assert_eq!(
            r.repeated_tasks.len(),
            1,
            "case + whitespace should not split"
        );
        assert_eq!(r.repeated_tasks[0].count, 3);
    }

    #[test]
    fn surfaces_repeated_system_prompts() {
        let sys = "You are a senior Rust reviewer. Output a numbered list.";
        let records = vec![
            rec("m1", "review this", Some(sys)),
            rec("m2", "and this", Some(sys)),
            rec("m1", "and this too", Some(sys)),
        ];
        let r = analyze(&records);
        assert_eq!(r.repeated_systems.len(), 1);
        assert_eq!(r.repeated_systems[0].count, 3);
        // Models should be deduped.
        assert_eq!(r.repeated_systems[0].models.len(), 2);
    }

    #[test]
    fn extracts_tool_chain_from_agent_transcript() {
        let prompt = "User task: research X\n\n[round 1] You called tool `web_search` with args:\n{...}\n\n[tool result, ok=true]\n...\n\n[round 2] You called tool `fetch_url` with args:\n{...}\n\n[tool result, ok=true]\n...\n";
        let chain = extract_tool_chain(prompt);
        assert_eq!(chain, vec!["web_search", "fetch_url"]);
    }

    #[test]
    fn extract_tool_chain_returns_empty_for_chat_records() {
        // A plain chat prompt has no `[round N] You called tool` markers.
        let chain = extract_tool_chain("explain quantum entanglement in 5 words");
        assert!(chain.is_empty());
    }

    #[test]
    fn surfaces_repeated_tool_chains() {
        let agent_prompt = "User task: research thing\n\n[round 1] You called tool `web_search` with args:\n{}\n\n[round 2] You called tool `wikipedia` with args:\n{}\n\n[round 3] You called tool `fetch_url` with args:\n{}\n";
        let records = vec![
            rec("m", agent_prompt, None),
            rec("m", agent_prompt, None),
            rec("m", agent_prompt, None),
        ];
        let r = analyze(&records);
        assert_eq!(r.repeated_tool_chains.len(), 1);
        assert_eq!(r.repeated_tool_chains[0].count, 3);
        assert!(r.repeated_tool_chains[0].canonical.contains("web_search"));
        assert!(r.repeated_tool_chains[0].canonical.contains("→"));
    }

    #[test]
    fn sorts_patterns_by_count_descending() {
        let mut records = Vec::new();
        for _ in 0..3 {
            records.push(rec("m", "task A", None));
        }
        for _ in 0..5 {
            records.push(rec("m", "task B much more often", None));
        }
        let r = analyze(&records);
        assert_eq!(r.repeated_tasks.len(), 2);
        assert_eq!(r.repeated_tasks[0].count, 5);
        assert_eq!(r.repeated_tasks[1].count, 3);
    }
}

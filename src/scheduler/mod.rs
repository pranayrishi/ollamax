//! On-device natural-language scheduler (Hermes-class cron).
//!
//! Stores scheduled agent tasks as JSONL under the config dir; a background tick
//! in `forge serve` runs the due ones through a non-streaming agent pass and
//! records the result. Fully local — nothing leaves the device. Natural-language
//! phrases like "every day at 9:30", "in 2 hours", "every 30 minutes" are parsed
//! into a [`Schedule`]; pure functions take `now` so they're deterministically
//! testable.

use anyhow::Result;
use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Fire once at an absolute epoch second.
    Once { at: i64 },
    /// Fire repeatedly every N seconds.
    EverySecs { secs: i64 },
    /// Fire daily at a local wall-clock time.
    DailyAt { hour: u32, min: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub prompt: String,
    pub schedule: Schedule,
    pub next_run: i64,
    pub enabled: bool,
    #[serde(default)]
    pub last_run: Option<i64>,
    #[serde(default)]
    pub last_result: Option<String>,
}

fn unit_secs(word: &str) -> Option<i64> {
    match word.trim_end_matches('s') {
        "second" | "sec" => Some(1),
        "minute" | "min" => Some(60),
        "hour" | "hr" => Some(3600),
        "day" => Some(86400),
        _ => None,
    }
}

/// Parse "HH", "HH:MM", with optional am/pm, into 24h (hour, min).
fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let (body, ampm) = if let Some(b) = s.strip_suffix("am") {
        (b.trim(), Some(false))
    } else if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else {
        (s, None)
    };
    let mut parts = body.split(':');
    let mut hour: u32 = parts.next()?.trim().parse().ok()?;
    let min: u32 = match parts.next() {
        Some(m) => m.trim().parse().ok()?,
        None => 0,
    };
    if let Some(pm) = ampm {
        if hour == 12 {
            hour = 0;
        }
        if pm {
            hour += 12;
        }
    }
    if hour > 23 || min > 59 {
        return None;
    }
    Some((hour, min))
}

fn first_number(words: &[&str]) -> Option<(i64, usize)> {
    words
        .iter()
        .enumerate()
        .find_map(|(i, w)| w.parse::<i64>().ok().map(|n| (n, i)))
}

/// Parse a natural-language schedule. Returns None if unrecognized.
pub fn parse_schedule(text: &str, now: i64) -> Option<Schedule> {
    let lc = text.to_lowercase();
    let lc = lc.trim();

    // Time-of-day forms first: "every day at HH", "daily at HH", "at HH".
    for marker in [" at ", "at "] {
        if let Some(idx) = lc.find(marker) {
            let is_daily =
                lc.contains("every day") || lc.contains("daily") || lc.starts_with("at ");
            if is_daily {
                let after = &lc[idx + marker.len()..];
                if let Some((h, m)) = parse_hhmm(after) {
                    return Some(Schedule::DailyAt { hour: h, min: m });
                }
            }
        }
    }

    let words: Vec<&str> = lc.split_whitespace().collect();

    // "in N unit" -> Once
    if words.first() == Some(&"in") {
        if let Some((n, i)) = first_number(&words) {
            if let Some(u) = words.get(i + 1).and_then(|w| unit_secs(w)) {
                return Some(Schedule::Once {
                    at: now + n.max(0) * u,
                });
            }
        }
    }

    // "every N unit" -> EverySecs
    if words.first() == Some(&"every") {
        if let Some((n, i)) = first_number(&words) {
            if let Some(u) = words.get(i + 1).and_then(|w| unit_secs(w)) {
                return Some(Schedule::EverySecs {
                    secs: (n.max(1)) * u,
                });
            }
        }
    }

    None
}

/// Next absolute epoch second the schedule should fire, given `now`.
pub fn compute_next(s: &Schedule, now: i64) -> i64 {
    match s {
        Schedule::Once { at } => *at,
        Schedule::EverySecs { secs } => now + (*secs).max(1),
        Schedule::DailyAt { hour, min } => {
            let now_dt = Local
                .timestamp_opt(now, 0)
                .single()
                .unwrap_or_else(Local::now);
            let today: NaiveDate = now_dt.date_naive();
            let at = |d: NaiveDate| -> Option<i64> {
                let ndt = d.and_hms_opt(*hour, *min, 0)?;
                Local
                    .from_local_datetime(&ndt)
                    .single()
                    .map(|dt| dt.timestamp())
            };
            match at(today) {
                Some(ts) if ts > now => ts,
                _ => at(today.succ_opt().unwrap_or(today)).unwrap_or(now + 86400),
            }
        }
    }
}

/// Persistent JSONL store of scheduled tasks (one JSON object per line).
pub struct Scheduler {
    path: PathBuf,
}

impl Scheduler {
    pub fn for_config() -> Self {
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ollama-forge")
            .join("schedule.jsonl");
        Self { path }
    }
    pub fn with_path(p: impl Into<PathBuf>) -> Self {
        Self { path: p.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Vec<ScheduledTask> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<ScheduledTask>(l).ok())
            .collect()
    }

    fn write_all(&self, tasks: &[ScheduledTask]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&self.path)?;
        for t in tasks {
            writeln!(f, "{}", serde_json::to_string(t)?)?;
        }
        Ok(())
    }

    /// Parse `schedule_text`, create the task, persist it. Returns the task or an
    /// error if the schedule text is unrecognized.
    pub fn add(&self, prompt: &str, schedule_text: &str, now: i64) -> Result<ScheduledTask> {
        let schedule = parse_schedule(schedule_text, now)
            .ok_or_else(|| anyhow::anyhow!("could not understand schedule: '{schedule_text}'"))?;
        let task = ScheduledTask {
            id: uuid::Uuid::new_v4().to_string(),
            prompt: prompt.to_string(),
            next_run: compute_next(&schedule, now),
            schedule,
            enabled: true,
            last_run: None,
            last_result: None,
        };
        let mut tasks = self.list();
        tasks.push(task.clone());
        self.write_all(&tasks)?;
        Ok(task)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let mut tasks = self.list();
        let before = tasks.len();
        tasks.retain(|t| t.id != id);
        let removed = tasks.len() != before;
        if removed {
            self.write_all(&tasks)?;
        }
        Ok(removed)
    }

    /// Tasks that are enabled and due (next_run <= now).
    pub fn due(&self, now: i64) -> Vec<ScheduledTask> {
        self.list()
            .into_iter()
            .filter(|t| t.enabled && t.next_run <= now)
            .collect()
    }

    /// Record a run: advance `next_run` (or disable a one-shot), store result.
    pub fn mark_ran(&self, id: &str, now: i64, result: Option<String>) -> Result<()> {
        let mut tasks = self.list();
        for t in tasks.iter_mut() {
            if t.id == id {
                t.last_run = Some(now);
                t.last_result = result.clone();
                match t.schedule {
                    Schedule::Once { .. } => t.enabled = false,
                    _ => t.next_run = compute_next(&t.schedule, now),
                }
            }
        }
        self.write_all(&tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_and_recurring() {
        let now = 1_000_000;
        assert_eq!(
            parse_schedule("in 5 minutes", now),
            Some(Schedule::Once { at: now + 300 })
        );
        assert_eq!(
            parse_schedule("in 2 hours", now),
            Some(Schedule::Once { at: now + 7200 })
        );
        assert_eq!(
            parse_schedule("every 30 seconds", now),
            Some(Schedule::EverySecs { secs: 30 })
        );
        assert_eq!(
            parse_schedule("every 15 minutes", now),
            Some(Schedule::EverySecs { secs: 900 })
        );
        assert_eq!(parse_schedule("flibbertigibbet", now), None);
    }

    #[test]
    fn parses_daily_times() {
        let now = 0;
        assert_eq!(
            parse_schedule("every day at 9:30", now),
            Some(Schedule::DailyAt { hour: 9, min: 30 })
        );
        assert_eq!(
            parse_schedule("daily at 14:00", now),
            Some(Schedule::DailyAt { hour: 14, min: 0 })
        );
        assert_eq!(
            parse_schedule("at 9am", now),
            Some(Schedule::DailyAt { hour: 9, min: 0 })
        );
        assert_eq!(
            parse_schedule("at 9pm", now),
            Some(Schedule::DailyAt { hour: 21, min: 0 })
        );
        assert_eq!(
            parse_schedule("at 12am", now),
            Some(Schedule::DailyAt { hour: 0, min: 0 })
        );
    }

    #[test]
    fn compute_next_is_in_the_future_for_daily() {
        let now = 1_700_000_000;
        let next = compute_next(&Schedule::DailyAt { hour: 9, min: 0 }, now);
        assert!(next > now);
        assert!(next <= now + 86400);
    }

    #[test]
    fn store_add_due_remove_roundtrip() {
        let p = std::env::temp_dir().join(format!("forge-sched-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let s = Scheduler::with_path(&p);
        let now = 1_000_000;
        let t = s.add("summarize my day", "in 1 minutes", now).unwrap();
        assert_eq!(s.list().len(), 1);
        assert!(s.due(now).is_empty(), "not due yet");
        assert_eq!(s.due(now + 120).len(), 1, "due after the minute");
        // mark_ran disables a one-shot
        s.mark_ran(&t.id, now + 120, Some("done".into())).unwrap();
        assert!(s.due(now + 1000).is_empty());
        assert!(s.remove(&t.id).unwrap());
        assert!(s.list().is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn recurring_advances_next_run() {
        let p = std::env::temp_dir().join(format!("forge-sched-rec-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let s = Scheduler::with_path(&p);
        let now = 1_000_000;
        let t = s.add("ping", "every 60 seconds", now).unwrap();
        s.mark_ran(&t.id, now + 60, None).unwrap();
        let after = s.list()[0].clone();
        assert!(after.enabled, "recurring stays enabled");
        assert!(after.next_run > now + 60);
        let _ = std::fs::remove_file(&p);
    }
}

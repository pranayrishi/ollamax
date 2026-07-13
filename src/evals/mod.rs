//! Deterministic, local evaluation metadata and scoring primitives.
//!
//! This module deliberately does **not** invoke an agent, execute a verifier
//! command, download a benchmark, or inspect the host machine. Callers provide
//! immutable run metadata and verifier evidence, then use the helpers here to
//! persist and compare results reproducibly. Keeping execution out of this
//! layer makes it useful both for the future local evaluation runner and for
//! unit tests driven by a fake Ollama server.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};

/// An exact local model identity. The digest should come from Ollama's model
/// listing when it is available, because a tag alone can point to different
/// weights after a pull.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl ModelIdentity {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            anyhow::bail!("model name must not be empty");
        }
        if self
            .digest
            .as_deref()
            .is_some_and(|digest| digest.trim().is_empty())
        {
            anyhow::bail!("model digest must not be blank when supplied");
        }
        Ok(())
    }
}

/// Caller-supplied hardware facts associated with an evaluation run. This is
/// intentionally a fingerprint, not live hardware detection: a report should
/// preserve the machine on which it actually ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareFingerprint {
    pub os: String,
    pub arch: String,
    pub cpu_cores: usize,
    pub total_ram_mb: usize,
    pub total_vram_mb: usize,
    #[serde(default)]
    pub gpu_kind: String,
}

impl HardwareFingerprint {
    pub fn validate(&self) -> Result<()> {
        if self.os.trim().is_empty() {
            anyhow::bail!("hardware os must not be empty");
        }
        if self.arch.trim().is_empty() {
            anyhow::bail!("hardware architecture must not be empty");
        }
        if self.cpu_cores == 0 {
            anyhow::bail!("hardware cpu_cores must be at least one");
        }
        Ok(())
    }
}

/// Hard bounds for one evaluation attempt. These are metadata only for now;
/// the future runner must enforce them before it may claim a run respected a
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBudget {
    pub max_wall_time_ms: u64,
    pub max_model_calls: u32,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_tool_calls: u32,
    pub max_test_runs: u32,
}

impl RunBudget {
    pub fn validate(&self) -> Result<()> {
        if self.max_wall_time_ms == 0 {
            anyhow::bail!("run budget max_wall_time_ms must be greater than zero");
        }
        if self.max_model_calls == 0 {
            anyhow::bail!("run budget max_model_calls must be greater than zero");
        }
        if self.max_input_tokens == 0 || self.max_output_tokens == 0 {
            anyhow::bail!("run budget token limits must be greater than zero");
        }
        if self.max_tool_calls == 0 {
            anyhow::bail!("run budget max_tool_calls must be greater than zero");
        }
        if self.max_test_runs == 0 {
            anyhow::bail!("run budget max_test_runs must be greater than zero");
        }
        Ok(())
    }
}

/// Configuration that must stay fixed when comparing orchestration strategies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunConfiguration {
    pub seed: i64,
    pub temperature: f32,
    pub context_tokens: usize,
    pub budget: RunBudget,
    /// A human-readable, bounded description of the orchestration topology
    /// (for example serial team vs. parallel scouts and the role models). It
    /// is part of comparison evidence, not a free-form model instruction.
    #[serde(default = "default_orchestration")]
    pub orchestration: String,
}

impl RunConfiguration {
    pub fn validate(&self) -> Result<()> {
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            anyhow::bail!("temperature must be a finite value between 0.0 and 2.0");
        }
        if self.context_tokens == 0 {
            anyhow::bail!("context_tokens must be greater than zero");
        }
        if self.orchestration.trim().is_empty()
            || self.orchestration.chars().count() > 512
            || self.orchestration.contains(['\0', '\n', '\r'])
        {
            anyhow::bail!("orchestration must be a 1-512 character single-line description");
        }
        self.budget.validate()
    }
}

fn default_orchestration() -> String {
    "unspecified".to_string()
}

/// Immutable task/repository identity. `base_sha` is deliberately required so
/// a result cannot silently mix runs performed against different code states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub base_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

impl TaskSnapshot {
    pub fn validate(&self) -> Result<()> {
        validate_safe_id(&self.task_id, "task_id")?;
        validate_git_sha(&self.base_sha)
    }
}

/// Result of one machine-verifiable check. `NotRun` is deliberately distinct
/// from failure so a JavaScript-free scenario does not lower a lint rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEvidence {
    pub status: CheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub summary: String,
}

impl CheckEvidence {
    pub fn passed() -> Self {
        Self {
            status: CheckStatus::Passed,
            command: None,
            exit_code: Some(0),
            summary: String::new(),
        }
    }

    pub fn failed() -> Self {
        Self {
            status: CheckStatus::Failed,
            command: None,
            exit_code: Some(1),
            summary: String::new(),
        }
    }

    pub fn not_run() -> Self {
        Self {
            status: CheckStatus::NotRun,
            command: None,
            exit_code: None,
            summary: String::new(),
        }
    }
}

/// A changed path outside the scenario's approved scope. It is evidence, not
/// merely a boolean, so dashboards and future policy code can explain why a
/// candidate was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeViolation {
    pub path: String,
    pub reason: String,
}

/// Verifier results supplied by an execution layer. `verified` is intentionally
/// explicit: only the runner may decide the full acceptance contract passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierEvidence {
    pub verified: bool,
    pub build: CheckEvidence,
    pub lint: CheckEvidence,
    pub tests: CheckEvidence,
    #[serde(default)]
    pub regression_detected: bool,
    #[serde(default)]
    pub scope_violations: Vec<ScopeViolation>,
}

/// Observable resource use and verification evidence for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationOutcome {
    pub duration_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub verifier: VerifierEvidence,
}

impl EvaluationOutcome {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// One append-only local evaluation record. `recorded_at` is caller-supplied
/// rather than generated in this module so tests and imported runs remain
/// deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    pub recorded_at: DateTime<Utc>,
    pub task: TaskSnapshot,
    pub model: ModelIdentity,
    pub hardware: HardwareFingerprint,
    pub config: RunConfiguration,
    pub outcome: EvaluationOutcome,
}

impl EvaluationRecord {
    pub fn validate(&self) -> Result<()> {
        self.task.validate()?;
        self.model.validate()?;
        self.hardware.validate()?;
        self.config.validate()
    }
}

/// One benchmark scenario. It is a declarative input only: the verifier command
/// is never executed by this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    pub verify_command: String,
}

impl Scenario {
    pub fn validate(&self) -> Result<()> {
        validate_safe_id(&self.id, "scenario id")?;
        if self.name.trim().is_empty() {
            anyhow::bail!("scenario name must not be empty");
        }
        if self.name.chars().count() > 160 || self.name.contains(['\n', '\r', '\0']) {
            anyhow::bail!("scenario name contains an unsupported length or control character");
        }
        if self.prompt.trim().is_empty() {
            anyhow::bail!("scenario prompt must not be empty");
        }
        if self.prompt.contains('\0') {
            anyhow::bail!("scenario prompt must not contain a NUL byte");
        }
        validate_verify_command(&self.verify_command)?;

        let mut seen = BTreeSet::new();
        for path in &self.allowed_paths {
            validate_allowed_path(path)?;
            if !seen.insert(path) {
                anyhow::bail!("scenario allowed_paths contains duplicate path `{path}`");
            }
        }
        Ok(())
    }
}

/// Selects the scenario parser. Scenarios are intentionally small enough to
/// support both project-friendly TOML and portable JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioFormat {
    Json,
    Toml,
}

impl ScenarioFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref()
        {
            Some("json") => Ok(Self::Json),
            Some("toml") => Ok(Self::Toml),
            _ => Err(anyhow!(
                "scenario {} must use a .json or .toml extension",
                path.display()
            )),
        }
    }
}

/// Parse and validate a scenario from one known format.
pub fn parse_scenario(input: &str, format: ScenarioFormat) -> Result<Scenario> {
    let scenario: Scenario = match format {
        ScenarioFormat::Json => serde_json::from_str(input).context("parse JSON scenario")?,
        ScenarioFormat::Toml => toml::from_str(input).context("parse TOML scenario")?,
    };
    scenario.validate()?;
    Ok(scenario)
}

/// Load a scenario from a caller-selected local file path.
pub fn load_scenario(path: impl AsRef<Path>) -> Result<Scenario> {
    let path = path.as_ref();
    let format = ScenarioFormat::from_path(path)?;
    let input =
        fs::read_to_string(path).with_context(|| format!("read scenario {}", path.display()))?;
    parse_scenario(&input, format).with_context(|| format!("validate scenario {}", path.display()))
}

/// Append-only JSONL storage for evaluation records. The caller owns the path;
/// this type never chooses a global directory or sends records anywhere.
#[derive(Debug, Clone)]
pub struct JsonlEvaluationStore {
    path: PathBuf,
}

impl JsonlEvaluationStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, record: &EvaluationRecord) -> Result<()> {
        record.validate()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create evaluation directory {}", parent.display()))?;
        }
        let mut line = serde_json::to_string(record).context("serialize evaluation record")?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open evaluation log {}", self.path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("append evaluation log {}", self.path.display()))?;
        file.flush()
            .with_context(|| format!("flush evaluation log {}", self.path.display()))?;
        Ok(())
    }

    /// Read every non-blank JSONL record. Invalid lines are errors rather than
    /// being skipped, because silently dropping an evaluation result makes a
    /// comparison untrustworthy.
    pub fn load(&self) -> Result<Vec<EvaluationRecord>> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open evaluation log {}", self.path.display()))
            }
        };
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.with_context(|| {
                format!(
                    "read line {} from evaluation log {}",
                    index + 1,
                    self.path.display()
                )
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let record: EvaluationRecord = serde_json::from_str(&line).with_context(|| {
                format!(
                    "parse JSONL record on line {} in {}",
                    index + 1,
                    self.path.display()
                )
            })?;
            record.validate().with_context(|| {
                format!(
                    "validate evaluation record on line {} in {}",
                    index + 1,
                    self.path.display()
                )
            })?;
            records.push(record);
        }
        Ok(records)
    }
}

/// Aggregate metrics for a comparable set of runs. Build/lint/test pass rates
/// use only checks that actually ran; `NotRun` checks are excluded from their
/// denominators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreReport {
    pub total_runs: usize,
    pub verified_completions: usize,
    pub verified_completion_rate: Option<f64>,
    pub build_checks_run: usize,
    pub build_passes: usize,
    pub build_pass_rate: Option<f64>,
    pub lint_checks_run: usize,
    pub lint_passes: usize,
    pub lint_pass_rate: Option<f64>,
    pub test_checks_run: usize,
    pub test_passes: usize,
    pub test_pass_rate: Option<f64>,
    pub median_duration_ms: Option<u64>,
    pub median_input_tokens: Option<u64>,
    pub median_output_tokens: Option<u64>,
    pub median_total_tokens: Option<u64>,
    pub median_model_calls: Option<u64>,
    pub median_tool_calls: Option<u64>,
    pub regression_runs: usize,
    pub regression_rate: Option<f64>,
    pub scope_violation_runs: usize,
    pub scope_violation_count: usize,
    pub scope_violation_rate: Option<f64>,
}

#[derive(Default)]
struct CheckTotals {
    run: usize,
    passed: usize,
}

impl CheckTotals {
    fn add(&mut self, status: CheckStatus) {
        match status {
            CheckStatus::Passed => {
                self.run += 1;
                self.passed += 1;
            }
            CheckStatus::Failed => self.run += 1,
            CheckStatus::NotRun => {}
        }
    }
}

/// Score a single group of records. The median of an even number of integer
/// samples is rounded down, which avoids floating-point noise in stored reports.
pub fn score_records(records: &[EvaluationRecord]) -> ScoreReport {
    let total_runs = records.len();
    let mut verified_completions = 0usize;
    let mut build = CheckTotals::default();
    let mut lint = CheckTotals::default();
    let mut tests = CheckTotals::default();
    let mut durations = Vec::with_capacity(total_runs);
    let mut input_tokens = Vec::with_capacity(total_runs);
    let mut output_tokens = Vec::with_capacity(total_runs);
    let mut total_tokens = Vec::with_capacity(total_runs);
    let mut model_calls = Vec::with_capacity(total_runs);
    let mut tool_calls = Vec::with_capacity(total_runs);
    let mut regression_runs = 0usize;
    let mut scope_violation_runs = 0usize;
    let mut scope_violation_count = 0usize;

    for record in records {
        let verifier = &record.outcome.verifier;
        if verifier.verified {
            verified_completions += 1;
        }
        build.add(verifier.build.status);
        lint.add(verifier.lint.status);
        tests.add(verifier.tests.status);
        durations.push(record.outcome.duration_ms);
        input_tokens.push(record.outcome.input_tokens);
        output_tokens.push(record.outcome.output_tokens);
        total_tokens.push(record.outcome.total_tokens());
        model_calls.push(record.outcome.model_calls as u64);
        tool_calls.push(record.outcome.tool_calls as u64);
        if verifier.regression_detected {
            regression_runs += 1;
        }
        if !verifier.scope_violations.is_empty() {
            scope_violation_runs += 1;
            scope_violation_count += verifier.scope_violations.len();
        }
    }

    ScoreReport {
        total_runs,
        verified_completions,
        verified_completion_rate: ratio(verified_completions, total_runs),
        build_checks_run: build.run,
        build_passes: build.passed,
        build_pass_rate: ratio(build.passed, build.run),
        lint_checks_run: lint.run,
        lint_passes: lint.passed,
        lint_pass_rate: ratio(lint.passed, lint.run),
        test_checks_run: tests.run,
        test_passes: tests.passed,
        test_pass_rate: ratio(tests.passed, tests.run),
        median_duration_ms: median(&mut durations),
        median_input_tokens: median(&mut input_tokens),
        median_output_tokens: median(&mut output_tokens),
        median_total_tokens: median(&mut total_tokens),
        median_model_calls: median(&mut model_calls),
        median_tool_calls: median(&mut tool_calls),
        regression_runs,
        regression_rate: ratio(regression_runs, total_runs),
        scope_violation_runs,
        scope_violation_count,
        scope_violation_rate: ratio(scope_violation_runs, total_runs),
    }
}

/// A candidate-minus-baseline comparison. Negative duration/token deltas mean
/// the candidate used less of that resource; positive pass-rate deltas mean it
/// passed more often.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreComparison {
    pub baseline: ScoreReport,
    pub candidate: ScoreReport,
    pub delta: ScoreDelta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreDelta {
    pub verified_completion_rate: Option<f64>,
    pub build_pass_rate: Option<f64>,
    pub lint_pass_rate: Option<f64>,
    pub test_pass_rate: Option<f64>,
    pub median_duration_ms: Option<i64>,
    pub median_total_tokens: Option<i64>,
    pub median_model_calls: Option<i64>,
    pub median_tool_calls: Option<i64>,
    pub regression_rate: Option<f64>,
    pub scope_violation_rate: Option<f64>,
    pub scope_violation_count: i64,
}

/// Compare two record groups while preserving their full reports.
pub fn compare_records(
    baseline: &[EvaluationRecord],
    candidate: &[EvaluationRecord],
) -> ScoreComparison {
    let baseline = score_records(baseline);
    let candidate = score_records(candidate);
    let delta = ScoreDelta {
        verified_completion_rate: option_delta(
            candidate.verified_completion_rate,
            baseline.verified_completion_rate,
        ),
        build_pass_rate: option_delta(candidate.build_pass_rate, baseline.build_pass_rate),
        lint_pass_rate: option_delta(candidate.lint_pass_rate, baseline.lint_pass_rate),
        test_pass_rate: option_delta(candidate.test_pass_rate, baseline.test_pass_rate),
        median_duration_ms: option_signed_delta(
            candidate.median_duration_ms,
            baseline.median_duration_ms,
        ),
        median_total_tokens: option_signed_delta(
            candidate.median_total_tokens,
            baseline.median_total_tokens,
        ),
        median_model_calls: option_signed_delta(
            candidate.median_model_calls,
            baseline.median_model_calls,
        ),
        median_tool_calls: option_signed_delta(
            candidate.median_tool_calls,
            baseline.median_tool_calls,
        ),
        regression_rate: option_delta(candidate.regression_rate, baseline.regression_rate),
        scope_violation_rate: option_delta(
            candidate.scope_violation_rate,
            baseline.scope_violation_rate,
        ),
        scope_violation_count: signed_delta(
            candidate.scope_violation_count as u64,
            baseline.scope_violation_count as u64,
        ),
    };
    ScoreComparison {
        baseline,
        candidate,
        delta,
    }
}

/// Validate an identifier used in filenames, scenario manifests, and task
/// snapshots. Safe IDs are ASCII, begin with an alphanumeric character, and do
/// not contain path separators or traversal-like `..` sequences.
pub fn validate_safe_id(value: &str, label: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 80 {
        anyhow::bail!("{label} must be 1-80 characters");
    }
    if !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-' | b'.'))
        || value.contains("..")
    {
        anyhow::bail!(
            "{label} must begin with an ASCII alphanumeric character and contain only ASCII letters, digits, '.', '_' or '-' without '..'"
        );
    }
    Ok(())
}

/// Ensure a scenario-owned path is a portable relative path. Forward slashes
/// and glob characters are allowed; backslashes are rejected so a POSIX-safe
/// scenario cannot become a Windows traversal path.
pub fn validate_allowed_path(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.trim() != value {
        anyhow::bail!("allowed path must be non-empty and have no surrounding whitespace");
    }
    if value.contains('\0') || value.contains('\\') {
        anyhow::bail!("allowed path must not contain NUL bytes or backslashes");
    }
    let bytes = value.as_bytes();
    if value.starts_with('/')
        || value.starts_with("//")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        anyhow::bail!("allowed path `{value}` must be relative");
    }

    let path = Path::new(value);
    if path.is_absolute() {
        anyhow::bail!("allowed path `{value}` must be relative");
    }
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                anyhow::bail!(
                    "allowed path `{value}` must not contain traversal or root components"
                )
            }
        }
    }
    if !has_normal_component
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "..")
    {
        anyhow::bail!("allowed path `{value}` is not a normalized relative path");
    }
    Ok(())
}

fn validate_git_sha(value: &str) -> Result<()> {
    let valid =
        (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid {
        anyhow::bail!("base_sha must be a 7-64 character hexadecimal Git revision");
    }
    Ok(())
}

fn validate_verify_command(value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value {
        anyhow::bail!("verify_command must be non-empty and have no surrounding whitespace");
    }
    if value.contains(['\0', '\n', '\r']) {
        anyhow::bail!("verify_command must be a single line without control characters");
    }
    let lower = value.to_ascii_lowercase();
    const OBVIOUSLY_UNSAFE: &[&str] = &[
        "rm -rf /", "rm -rf ~", "mkfs", "dd if=", "shutdown", "reboot", "sudo ", "doas ",
    ];
    if let Some(fragment) = OBVIOUSLY_UNSAFE
        .iter()
        .find(|fragment| lower.contains(**fragment))
    {
        anyhow::bail!("verify_command contains blocked unsafe fragment `{fragment}`");
    }
    Ok(())
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

fn median(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        let low = values[middle - 1];
        let high = values[middle];
        Some(low.saturating_add(high.saturating_sub(low) / 2))
    }
}

fn option_delta(candidate: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    candidate
        .zip(baseline)
        .map(|(candidate, baseline)| candidate - baseline)
}

fn option_signed_delta(candidate: Option<u64>, baseline: Option<u64>) -> Option<i64> {
    candidate
        .zip(baseline)
        .map(|(candidate, baseline)| signed_delta(candidate, baseline))
}

fn signed_delta(candidate: u64, baseline: u64) -> i64 {
    if candidate >= baseline {
        candidate.saturating_sub(baseline).min(i64::MAX as u64) as i64
    } else {
        let magnitude = baseline.saturating_sub(candidate).min(i64::MAX as u64) as i64;
        -magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn config() -> RunConfiguration {
        RunConfiguration {
            seed: 42,
            temperature: 0.0,
            context_tokens: 8_192,
            orchestration: "serial-team;writer=test-coder:7b".to_string(),
            budget: RunBudget {
                max_wall_time_ms: 60_000,
                max_model_calls: 12,
                max_input_tokens: 16_000,
                max_output_tokens: 8_000,
                max_tool_calls: 40,
                max_test_runs: 4,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        duration_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        model_calls: u32,
        tool_calls: u32,
        verified: bool,
        build: CheckStatus,
        lint: CheckStatus,
        tests: CheckStatus,
        regression_detected: bool,
        scope_violations: usize,
    ) -> EvaluationRecord {
        EvaluationRecord {
            recorded_at: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
            task: TaskSnapshot {
                task_id: "fixture-fix-01".to_string(),
                base_sha: "0123456789abcdef".to_string(),
                repository: Some("fixture-repo".to_string()),
            },
            model: ModelIdentity {
                name: "test-coder:7b".to_string(),
                digest: Some("sha256:fixture".to_string()),
            },
            hardware: HardwareFingerprint {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                cpu_cores: 8,
                total_ram_mb: 32_768,
                total_vram_mb: 12_288,
                gpu_kind: "nvidia".to_string(),
            },
            config: config(),
            outcome: EvaluationOutcome {
                duration_ms,
                input_tokens,
                output_tokens,
                model_calls,
                tool_calls,
                verifier: VerifierEvidence {
                    verified,
                    build: CheckEvidence {
                        status: build,
                        command: Some("cargo test".to_string()),
                        exit_code: None,
                        summary: String::new(),
                    },
                    lint: CheckEvidence {
                        status: lint,
                        command: Some("cargo clippy".to_string()),
                        exit_code: None,
                        summary: String::new(),
                    },
                    tests: CheckEvidence {
                        status: tests,
                        command: Some("cargo test".to_string()),
                        exit_code: None,
                        summary: String::new(),
                    },
                    regression_detected,
                    scope_violations: (0..scope_violations)
                        .map(|number| ScopeViolation {
                            path: format!("outside/{number}.txt"),
                            reason: "outside scenario ownership".to_string(),
                        })
                        .collect(),
                },
            },
        }
    }

    #[test]
    fn parses_and_validates_json_and_toml_scenarios() {
        let json = r#"{
            "id":"greeting-fix-01",
            "name":"Fix greeting",
            "prompt":"Update the greeting and keep tests passing.",
            "allowed_paths":["src/**","tests/**"],
            "verify_command":"cargo test"
        }"#;
        let parsed_json = parse_scenario(json, ScenarioFormat::Json).unwrap();
        assert_eq!(parsed_json.id, "greeting-fix-01");

        let toml = r#"
            id = "greeting-fix-02"
            name = "Fix another greeting"
            prompt = "Update the greeting and keep tests passing."
            allowed_paths = ["src/**", "tests/**"]
            verify_command = "cargo test"
        "#;
        let parsed_toml = parse_scenario(toml, ScenarioFormat::Toml).unwrap();
        assert_eq!(parsed_toml.allowed_paths, vec!["src/**", "tests/**"]);
    }

    #[test]
    fn scenario_validation_rejects_empty_prompt_unsafe_id_paths_and_command() {
        let base = Scenario {
            id: "safe-id-1".to_string(),
            name: "Safe scenario".to_string(),
            prompt: "Do useful work".to_string(),
            allowed_paths: vec!["src/**".to_string()],
            verify_command: "cargo test".to_string(),
        };

        let mut empty_prompt = base.clone();
        empty_prompt.prompt = " \n ".to_string();
        assert!(empty_prompt.validate().is_err());

        for bad_id in ["", "../escape", "not safe", ".hidden", "slash/name"] {
            let mut scenario = base.clone();
            scenario.id = bad_id.to_string();
            assert!(
                scenario.validate().is_err(),
                "{bad_id:?} should be rejected"
            );
        }

        for bad_path in [
            "/etc/passwd",
            "../src",
            "src/../secret",
            r"C:\\temp",
            "src\\file.rs",
        ] {
            let mut scenario = base.clone();
            scenario.allowed_paths = vec![bad_path.to_string()];
            assert!(
                scenario.validate().is_err(),
                "{bad_path:?} should be rejected"
            );
        }

        let mut unsafe_command = base;
        unsafe_command.verify_command = "rm -rf /".to_string();
        assert!(unsafe_command.validate().is_err());
    }

    #[test]
    fn scenario_parser_rejects_unknown_fields_and_unknown_extensions() {
        let unknown_field = r#"{
            "id":"safe-id",
            "name":"Safe",
            "prompt":"Do work",
            "allowed_paths":["src/**"],
            "verify_command":"cargo test",
            "surprise":true
        }"#;
        assert!(parse_scenario(unknown_field, ScenarioFormat::Json).is_err());
        assert!(ScenarioFormat::from_path(Path::new("scenario.yaml")).is_err());
    }

    #[test]
    fn jsonl_store_round_trips_and_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/results.jsonl");
        let store = JsonlEvaluationStore::new(&path);
        assert!(store.load().unwrap().is_empty());

        let first = record(
            100,
            10,
            20,
            1,
            2,
            true,
            CheckStatus::Passed,
            CheckStatus::Passed,
            CheckStatus::Passed,
            false,
            0,
        );
        let second = record(
            200,
            20,
            30,
            2,
            3,
            false,
            CheckStatus::Passed,
            CheckStatus::Failed,
            CheckStatus::Failed,
            true,
            1,
        );
        store.append(&first).unwrap();
        store.append(&second).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded, vec![first, second]);

        fs::write(&path, "not-json\n").unwrap();
        assert!(store.load().is_err(), "corrupt JSONL must not be ignored");
    }

    #[test]
    fn record_validation_rejects_unbounded_or_invalid_configuration() {
        let mut invalid = record(
            1,
            1,
            1,
            1,
            1,
            true,
            CheckStatus::Passed,
            CheckStatus::Passed,
            CheckStatus::Passed,
            false,
            0,
        );
        invalid.config.temperature = f32::NAN;
        assert!(invalid.validate().is_err());

        invalid.config.temperature = 0.0;
        invalid.config.budget.max_model_calls = 0;
        assert!(invalid.validate().is_err());

        invalid.config.budget.max_model_calls = 1;
        invalid.task.base_sha = "not-a-sha".to_string();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn score_reports_verification_checks_medians_and_violation_rates() {
        let records = vec![
            record(
                100,
                10,
                20,
                1,
                3,
                true,
                CheckStatus::Passed,
                CheckStatus::Passed,
                CheckStatus::Passed,
                false,
                0,
            ),
            record(
                300,
                30,
                40,
                2,
                4,
                false,
                CheckStatus::Passed,
                CheckStatus::Failed,
                CheckStatus::Failed,
                true,
                1,
            ),
            record(
                200,
                20,
                30,
                1,
                2,
                true,
                CheckStatus::NotRun,
                CheckStatus::NotRun,
                CheckStatus::Passed,
                false,
                0,
            ),
        ];

        let report = score_records(&records);
        assert_eq!(report.total_runs, 3);
        assert_eq!(report.verified_completions, 2);
        assert_eq!(report.verified_completion_rate, Some(2.0 / 3.0));
        assert_eq!(report.build_checks_run, 2);
        assert_eq!(report.build_pass_rate, Some(1.0));
        assert_eq!(report.lint_checks_run, 2);
        assert_eq!(report.lint_pass_rate, Some(0.5));
        assert_eq!(report.test_checks_run, 3);
        assert_eq!(report.test_pass_rate, Some(2.0 / 3.0));
        assert_eq!(report.median_duration_ms, Some(200));
        assert_eq!(report.median_input_tokens, Some(20));
        assert_eq!(report.median_output_tokens, Some(30));
        assert_eq!(report.median_total_tokens, Some(50));
        assert_eq!(report.median_model_calls, Some(1));
        assert_eq!(report.median_tool_calls, Some(3));
        assert_eq!(report.regression_runs, 1);
        assert_eq!(report.regression_rate, Some(1.0 / 3.0));
        assert_eq!(report.scope_violation_runs, 1);
        assert_eq!(report.scope_violation_count, 1);
        assert_eq!(report.scope_violation_rate, Some(1.0 / 3.0));
    }

    #[test]
    fn comparison_is_candidate_minus_baseline() {
        let baseline = vec![record(
            300,
            50,
            50,
            2,
            4,
            false,
            CheckStatus::Passed,
            CheckStatus::Failed,
            CheckStatus::Failed,
            true,
            1,
        )];
        let candidate = vec![record(
            100,
            20,
            30,
            1,
            2,
            true,
            CheckStatus::Passed,
            CheckStatus::Passed,
            CheckStatus::Passed,
            false,
            0,
        )];

        let comparison = compare_records(&baseline, &candidate);
        assert_eq!(comparison.delta.verified_completion_rate, Some(1.0));
        assert_eq!(comparison.delta.lint_pass_rate, Some(1.0));
        assert_eq!(comparison.delta.test_pass_rate, Some(1.0));
        assert_eq!(comparison.delta.median_duration_ms, Some(-200));
        assert_eq!(comparison.delta.median_total_tokens, Some(-50));
        assert_eq!(comparison.delta.median_model_calls, Some(-1));
        assert_eq!(comparison.delta.median_tool_calls, Some(-2));
        assert_eq!(comparison.delta.regression_rate, Some(-1.0));
        assert_eq!(comparison.delta.scope_violation_rate, Some(-1.0));
        assert_eq!(comparison.delta.scope_violation_count, -1);
    }

    #[test]
    fn empty_scores_have_no_rates_or_medians() {
        let report = score_records(&[]);
        assert_eq!(report.total_runs, 0);
        assert_eq!(report.verified_completion_rate, None);
        assert_eq!(report.build_pass_rate, None);
        assert_eq!(report.median_duration_ms, None);
        assert_eq!(report.scope_violation_rate, None);
    }
}

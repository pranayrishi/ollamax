use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub struct SecurityGuard {
    enabled: bool,
    rules: Arc<RwLock<Vec<SecurityRule>>>,
    audit_log: Arc<RwLock<Vec<AuditEntry>>>,
}

#[derive(Debug, Clone)]
pub struct SecurityRule {
    pub name: String,
    pub pattern: String,
    pub severity: Severity,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event: SecurityEvent,
    pub file: Option<String>,
    pub severity: Severity,
    pub details: String,
}

#[derive(Debug, Clone)]
pub enum SecurityEvent {
    FileAccess,
    FileWrite,
    CommandExecution,
    SecretDetected,
    VulnerabilityFound,
    PolicyViolation,
}

impl SecurityGuard {
    pub fn new(enabled: bool) -> Self {
        let mut guard = Self {
            enabled,
            rules: Arc::new(RwLock::new(Vec::new())),
            audit_log: Arc::new(RwLock::new(Vec::new())),
        };

        guard.load_default_rules();
        guard
    }

    fn load_default_rules(&mut self) {
        let rules = vec![
            SecurityRule {
                name: "AWS Keys".to_string(),
                pattern: r"(?i)(aws_access_key|aws_secret_key|AMAZON|S3_BUCKET)".to_string(), // forge:allow
                severity: Severity::Critical,
                description: "AWS credentials detected".to_string(),
            },
            SecurityRule {
                name: "Private Keys".to_string(),
                pattern: r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----".to_string(),
                severity: Severity::Critical,
                description: "Private key detected".to_string(),
            },
            SecurityRule {
                name: "API Keys".to_string(),
                pattern: r#"(?i)(api[_-]?key|apikey|API_KEY)[=\s]*['"]?[a-zA-Z0-9]{20,}['"]?"#
                    .to_string(),
                severity: Severity::High,
                description: "API key detected".to_string(),
            },
            SecurityRule {
                name: "Database URLs".to_string(),
                pattern: r"(?i)(mysql|postgres|mongodb|redis)://[^\s]+".to_string(),
                severity: Severity::High,
                description: "Database connection string detected".to_string(),
            },
            SecurityRule {
                name: "JWT Tokens".to_string(),
                pattern: r"eyJ[a-zA-Z0-9]{10,}\.eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}"
                    .to_string(),
                severity: Severity::Medium,
                description: "JWT token detected".to_string(),
            },
            SecurityRule {
                name: "GitHub Tokens".to_string(),
                pattern: r"gh[pousr]_[a-zA-Z0-9]{36,}".to_string(),
                severity: Severity::Critical,
                description: "GitHub token detected".to_string(),
            },
            SecurityRule {
                name: "Hardcoded Passwords".to_string(),
                pattern: r#"(?i)(password|passwd|pwd)[=\s]*['"][^'"]{8,}['"]"#.to_string(),
                severity: Severity::High,
                description: "Hardcoded password detected".to_string(),
            },
            SecurityRule {
                name: "Dangerous Shell Commands".to_string(),
                pattern: r"(rm\s+-rf\s+/|mkfs|dd\s+if=)".to_string(), // forge:allow
                severity: Severity::Critical,
                description: "Dangerous command detected".to_string(),
            },
        ];

        self.rules = Arc::new(RwLock::new(rules));
    }

    pub async fn scan_file(&self, path: &Path) -> Result<Vec<SecurityFinding>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(path).await?;
        let findings = self.scan_content(&content, Some(path)).await;

        for finding in &findings {
            self.log_event(
                SecurityEvent::SecretDetected,
                Some(path.to_string_lossy().to_string()),
                finding.rule.severity,
                &finding.rule.description,
            )
            .await;
        }

        Ok(findings)
    }

    pub async fn scan_content(&self, content: &str, file: Option<&Path>) -> Vec<SecurityFinding> {
        // The lowest-level scan entry point also has to honor `enabled` —
        // otherwise callers that bypass `scan_file`/`audit_directory` end
        // up running the scanner even when the user explicitly disabled it.
        if !self.enabled {
            return Vec::new();
        }
        let mut findings = Vec::new();
        let rules = self.rules.read().await;

        for rule in rules.iter() {
            if let Ok(re) = regex::Regex::new(&rule.pattern) {
                for (line_num, line) in content.lines().enumerate() {
                    // Inline suppression: a `// forge:allow` (or `# forge:allow`)
                    // marker on the same line silences the finding. This is the
                    // standard mechanism for legitimate matches like regex
                    // *definitions* in this very file, or test fixtures.
                    if line.contains("forge:allow") {
                        continue;
                    }
                    if re.is_match(line) {
                        findings.push(SecurityFinding {
                            rule: rule.clone(),
                            line_number: line_num + 1,
                            line_content: line.trim().to_string(),
                            file: file.map(|p| p.to_string_lossy().to_string()),
                        });
                    }
                }
            }
        }

        findings
    }

    pub async fn audit_directory(&self, path: &Path) -> Result<AuditReport> {
        if !self.enabled {
            return Ok(AuditReport {
                files_scanned: 0,
                findings: Vec::new(),
                summary: "Security scanning disabled".to_string(),
            });
        }

        let mut findings = Vec::new();
        let mut files_scanned = 0;

        // Skip directories that are almost certainly noise: build artifacts,
        // VCS internals, dependency mirrors, virtualenvs, and dotdirs in
        // general. Cuts a multi-minute scan of a Rust repo down to seconds
        // and prevents `target/` from drowning real findings.
        let is_skipped_dir = |name: &str| -> bool {
            matches!(
                name,
                "target"
                    | "node_modules"
                    | ".git"
                    | "dist"
                    | "build"
                    | "vendor"
                    | "venv"
                    | ".venv"
                    | "__pycache__"
                    | ".cargo"
            ) || (name.starts_with('.') && name.len() > 1)
        };

        let walker = walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && is_skipped_dir(&name))
            });

        for entry in walker.filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let ext = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");

                let scannable = matches!(
                    ext,
                    "rs" | "js"
                        | "ts"
                        | "py"
                        | "go"
                        | "java"
                        | "rb"
                        | "php"
                        | "c"
                        | "cpp"
                        | "h"
                        | "cs"
                        | "json"
                        | "yaml"
                        | "yml"
                        | "toml"
                        | "env"
                        | "config"
                );

                if scannable {
                    if let Ok(file_findings) = self.scan_file(entry.path()).await {
                        findings.extend(file_findings);
                    }
                    files_scanned += 1;
                }
            }
        }

        let critical_count = findings
            .iter()
            .filter(|f| f.rule.severity == Severity::Critical)
            .count();
        let high_count = findings
            .iter()
            .filter(|f| f.rule.severity == Severity::High)
            .count();

        let summary = format!(
            "Scanned {} files, found {} critical and {} high severity issues",
            files_scanned, critical_count, high_count
        );

        info!("{}", summary);

        Ok(AuditReport {
            files_scanned,
            findings,
            summary,
        })
    }

    pub async fn validate_command(&self, command: &str) -> CommandValidation {
        let dangerous_patterns = [
            (
                r"rm\s+-rf\s+/(?:\*|var|etc|usr|home)",
                "Deleting system directories",
            ),
            (r"chmod\s+-R\s+777", "Setting world-writable permissions"),
            (r">\s*/dev/sd[a-z]", "Writing to disk device"),
            (r"wget\|sh", "Pipe to shell execution"),
            (r"curl\|sh", "Pipe to shell execution"),
            // Classic bash fork bomb: `:(){ :|:& };:`. The previous pattern
            // ended with `\$` (literal `$`), which the fork bomb does not
            // contain — so the rule never fired. Match the function-def
            // shape instead.
            (r":\(\)\s*\{[^}]*:\|:[^}]*\}\s*;\s*:", "Fork bomb pattern"),
            (r"sudo\s+su\s+-", "Escalating to root"),
        ];

        let mut issues = Vec::new();

        for (pattern, description) in &dangerous_patterns {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(command) {
                    issues.push(description.to_string());
                }
            }
        }

        CommandValidation {
            command: command.to_string(),
            safe: issues.is_empty(),
            issues,
        }
    }

    async fn log_event(
        &self,
        event: SecurityEvent,
        file: Option<String>,
        severity: Severity,
        details: &str,
    ) {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now(),
            event,
            file,
            severity,
            details: details.to_string(),
        };

        let mut log = self.audit_log.write().await;
        log.push(entry);
    }

    pub async fn get_audit_log(&self) -> Vec<AuditEntry> {
        let log = self.audit_log.read().await;
        log.clone()
    }
}

#[derive(Debug, Clone)]
pub struct SecurityFinding {
    pub rule: SecurityRule,
    pub line_number: usize,
    pub line_content: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditReport {
    pub files_scanned: usize,
    pub findings: Vec<SecurityFinding>,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct CommandValidation {
    pub command: String,
    pub safe: bool,
    pub issues: Vec<String>,
}

pub struct TddEnforcer {
    enabled: bool,
    test_frameworks: HashMap<String, TestFramework>,
}

#[derive(Debug, Clone)]
pub struct TestFramework {
    pub name: String,
    pub test_file_pattern: String,
    pub test_command: String,
    pub run_command: String,
}

impl TddEnforcer {
    pub fn new(enabled: bool) -> Self {
        let mut frameworks = HashMap::new();

        frameworks.insert(
            "rust".to_string(),
            TestFramework {
                name: "Cargo Test".to_string(),
                test_file_pattern: "**/*test*.rs".to_string(),
                test_command: "cargo test --lib".to_string(),
                run_command: "cargo test".to_string(),
            },
        );

        frameworks.insert(
            "javascript".to_string(),
            TestFramework {
                name: "Jest".to_string(),
                test_file_pattern: "**/*.test.js".to_string(),
                test_command: "npm test".to_string(),
                run_command: "npm test".to_string(),
            },
        );

        frameworks.insert(
            "typescript".to_string(),
            TestFramework {
                name: "Jest/Vitest".to_string(),
                test_file_pattern: "**/*.test.ts".to_string(),
                test_command: "npm test".to_string(),
                run_command: "npm test".to_string(),
            },
        );

        frameworks.insert(
            "python".to_string(),
            TestFramework {
                name: "Pytest".to_string(),
                test_file_pattern: "**/test_*.py".to_string(),
                test_command: "pytest".to_string(),
                run_command: "pytest -v".to_string(),
            },
        );

        Self {
            enabled,
            test_frameworks: frameworks,
        }
    }

    pub async fn enforce(
        &self,
        project_path: &Path,
        source_files: Vec<PathBuf>,
    ) -> Result<TddReport> {
        if !self.enabled {
            return Ok(TddReport {
                tests_generated: 0,
                tests_passed: 0,
                status: TddStatus::Disabled,
                details: "TDD enforcement disabled".to_string(),
            });
        }

        let lang = self.detect_language(project_path);
        let framework = self.test_frameworks.get(&lang);

        let report = TddReport {
            tests_generated: source_files.len(),
            tests_passed: 0,
            status: TddStatus::TestsRequired,
            details: format!(
                "Detected {} project with {} framework",
                lang,
                framework.map(|f| &f.name[..]).unwrap_or("unknown")
            ),
        };

        Ok(report)
    }

    fn detect_language(&self, path: &Path) -> String {
        if path.join("Cargo.toml").exists() {
            return "rust".to_string();
        }
        if path.join("package.json").exists() {
            return "javascript".to_string();
        }
        if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() {
            return "python".to_string();
        }
        "unknown".to_string()
    }
}

#[derive(Debug, Clone)]
pub struct TddReport {
    pub tests_generated: usize,
    pub tests_passed: usize,
    pub status: TddStatus,
    pub details: String,
}

#[derive(Debug, Clone)]
pub enum TddStatus {
    TestsRequired,
    TestsGenerated,
    TestsPassed,
    TestsFailed,
    Disabled,
}

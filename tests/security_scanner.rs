//! Behavioral tests for the secret scanner.
//!
//! These document the contract: what *must* trip the scanner, what *must
//! not*, and how `forge:allow` works. The threat is that a future regex
//! tweak silently stops detecting AWS keys (or starts flagging every line
//! containing the word "key"). These tests catch both.

use ollama_forge::security::{SecurityGuard, Severity};
use std::path::Path;

async fn scan(content: &str) -> Vec<ollama_forge::security::SecurityFinding> {
    let guard = SecurityGuard::new(true);
    guard.scan_content(content, None).await
}

#[tokio::test]
async fn scanner_disabled_returns_empty() {
    let guard = SecurityGuard::new(false);
    // Even with an obvious AWS-looking string, disabled means disabled.
    let findings = guard
        .scan_content("AWS_SECRET_KEY=AKIAEXAMPLEKEYDONOTUSE12345", None)
        .await;
    assert!(findings.is_empty());
}

#[tokio::test]
async fn detects_private_key_block() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...";
    let findings = scan(pem).await;
    assert!(
        findings
            .iter()
            .any(|f| f.rule.severity == Severity::Critical),
        "private key block must be Critical, got: {findings:#?}"
    );
}

#[tokio::test]
async fn detects_github_token() {
    let line = "GH_TOKEN=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let findings = scan(line).await;
    assert!(
        findings.iter().any(|f| f.rule.name == "GitHub Tokens"),
        "expected GitHub Tokens rule to fire, got: {findings:#?}"
    );
}

#[tokio::test]
async fn detects_database_url() {
    let line = "DB=postgres://user:pass@host:5432/db";
    let findings = scan(line).await;
    assert!(
        findings.iter().any(|f| f.rule.name == "Database URLs"),
        "expected Database URLs rule to fire"
    );
}

#[tokio::test]
async fn forge_allow_suppresses_finding() {
    let line = "GH_TOKEN=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA // forge:allow";
    let findings = scan(line).await;
    assert!(
        findings.is_empty(),
        "forge:allow should suppress findings on the same line, got: {findings:#?}"
    );
}

#[tokio::test]
async fn dangerous_command_is_critical() {
    let findings = scan("os.system('rm -rf /')").await;
    assert!(findings.iter().any(
        |f| f.rule.severity == Severity::Critical && f.rule.name == "Dangerous Shell Commands"
    ));
}

#[tokio::test]
async fn validate_command_flags_fork_bomb() {
    let guard = SecurityGuard::new(true);
    let v = guard.validate_command(":(){ :|:& };:").await;
    assert!(!v.safe, "fork bomb must not be marked safe");
    assert!(!v.issues.is_empty());
}

#[tokio::test]
async fn validate_command_passes_normal() {
    let guard = SecurityGuard::new(true);
    let v = guard.validate_command("ls -la").await;
    assert!(v.safe);
    assert!(v.issues.is_empty());
}

#[tokio::test]
async fn audit_directory_skips_target_dir() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    // Create a file in target/ that would otherwise trip the scanner.
    let target = tmp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("leaked.rs"),
        "let key = \"-----BEGIN RSA PRIVATE KEY-----\";\n",
    )
    .unwrap();
    // And a clean file at the top level.
    fs::write(tmp.path().join("ok.rs"), "fn main() {}\n").unwrap();

    let guard = SecurityGuard::new(true);
    let report = guard.audit_directory(tmp.path()).await.unwrap();
    assert_eq!(
        report.files_scanned, 1,
        "should have scanned only ok.rs, not target/leaked.rs"
    );
    assert!(
        report.findings.is_empty(),
        "target/ should be skipped entirely"
    );
}

#[tokio::test]
async fn scan_file_attaches_filename_to_findings() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("creds.txt");
    fs::write(&p, "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n").unwrap();
    let guard = SecurityGuard::new(true);
    let findings = guard.scan_file(&p).await.unwrap();
    assert!(!findings.is_empty());
    let f = &findings[0];
    assert_eq!(
        f.file.as_deref(),
        Some(p.to_string_lossy().as_ref()),
        "scan_file must attach the path so the user can find the issue"
    );
}

// Compile-time hint that we depend on these types being public.
const _: fn(&Path) = |_| {};

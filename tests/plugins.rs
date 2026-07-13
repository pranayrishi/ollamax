//! Contract tests for the curated GitHub knowledge-plugin foundation.
//!
//! Every HTTP response comes from a loopback fake GitHub API. The tests never
//! contact github.com and exercise the same configurable API-base path used by
//! production installs.

use ollama_forge::plugins::{
    InstalledPluginManifest, PluginManager, MAX_RELEVANT_CONTEXT_CHARS, MAX_SAVED_DOCUMENT_BYTES,
    UNTRUSTED_DOCUMENT_MARKER,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct FakeGithubConfig {
    supervision_stars: u64,
    supervision_license: Option<String>,
    supervision_readme: String,
    redirect_supervision_readme: bool,
}

impl Default for FakeGithubConfig {
    fn default() -> Self {
        Self {
            supervision_stars: 40_000,
            supervision_license: Some("MIT".to_string()),
            supervision_readme: "# Supervision\n\nPython computer vision detection, tracking, and annotation reference.\n"
                .to_string(),
            redirect_supervision_readme: false,
        }
    }
}

async fn fake_github(
    config: FakeGithubConfig,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Arc::new(config);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let config = config.clone();
            tokio::spawn(async move {
                let _ = handle_fake_github(stream, &config).await;
            });
        }
    });
    (addr, task)
}

async fn handle_fake_github(stream: TcpStream, config: &FakeGithubConfig) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    if path == "/repos/roboflow/supervision/readme" && config.redirect_supervision_readme {
        let response = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        write.write_all(response.as_bytes()).await?;
        return write.flush().await;
    }
    let (status, content_type, body) = match path {
        "/repos/roboflow/supervision" => {
            let license = config
                .supervision_license
                .as_deref()
                .map(|spdx_id| json!({"spdx_id":spdx_id}))
                .unwrap_or(serde_json::Value::Null);
            (
                "200 OK",
                "application/json",
                json!({
                    "full_name":"roboflow/supervision",
                    "html_url":"https://github.com/roboflow/supervision",
                    "default_branch":"main",
                    "stargazers_count":config.supervision_stars,
                    "license":license
                })
                .to_string(),
            )
        }
        "/repos/roboflow/supervision/commits/main" => (
            "200 OK",
            "application/json",
            json!({"sha":"0123456789abcdef0123456789abcdef01234567"}).to_string(),
        ),
        "/repos/roboflow/supervision/readme" => (
            "200 OK",
            "text/markdown; charset=utf-8",
            config.supervision_readme.clone(),
        ),
        "/repos/fastapi/fastapi" => (
            "200 OK",
            "application/json",
            json!({
                "full_name":"fastapi/fastapi",
                "html_url":"https://github.com/fastapi/fastapi",
                "default_branch":"main",
                "stargazers_count":90_000,
                "license":{"spdx_id":"MIT"}
            })
            .to_string(),
        ),
        "/repos/fastapi/fastapi/commits/main" => (
            "200 OK",
            "application/json",
            json!({"sha":"abcdef0123456789abcdef0123456789abcdef01"}).to_string(),
        ),
        "/repos/fastapi/fastapi/readme" => (
            "200 OK",
            "text/markdown; charset=utf-8",
            "# FastAPI\n\nPython API and backend reference documentation.\n".to_string(),
        ),
        _ => (
            "404 Not Found",
            "application/json",
            json!({"message":"not found"}).to_string(),
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write.write_all(response.as_bytes()).await?;
    write.flush().await
}

fn manager_for(root: &std::path::Path, address: std::net::SocketAddr) -> PluginManager {
    PluginManager::with_api_base(root.join("knowledge-plugins"), format!("http://{address}"))
        .unwrap()
}

#[tokio::test]
async fn install_records_fetched_provenance_and_only_saves_untrusted_documentation() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig::default()).await;
    let manager = manager_for(temp.path(), address);

    let manifest = manager.install("roboflow-supervision").await.unwrap();
    fake_task.abort();

    assert_eq!(manifest.repository.full_name, "roboflow/supervision");
    assert_eq!(
        manifest.repository.url,
        "https://github.com/roboflow/supervision"
    );
    assert_eq!(manifest.repository.stars, 40_000);
    assert_eq!(manifest.repository.license, "MIT");
    assert_eq!(
        manifest.repository.default_branch_commit_sha.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(manifest.provenance.kind, "curated-github-knowledge");
    assert!(manifest
        .provenance
        .readme_url
        .contains("/repos/roboflow/supervision/readme"));
    assert_eq!(manifest.document.file_name, "README.md");
    assert_eq!(manifest.document.trust, "untrusted");
    assert!(!manifest.document.executable);
    assert!(manifest.document.bytes <= MAX_SAVED_DOCUMENT_BYTES);

    let install = manager.install_root().join("roboflow-supervision");
    assert!(!install.join(".git").exists());
    assert!(!install.join("scripts").exists());
    let document = std::fs::read_to_string(install.join("README.md")).unwrap();
    assert!(document.starts_with(UNTRUSTED_DOCUMENT_MARKER));
    assert!(document.contains("never a plugin executable"));
    let mut hasher = Sha256::new();
    hasher.update(document.as_bytes());
    assert_eq!(format!("{:x}", hasher.finalize()), manifest.document.sha256);

    let saved: InstalledPluginManifest =
        serde_json::from_slice(&std::fs::read(install.join("plugin.json")).unwrap()).unwrap();
    assert_eq!(saved.installed_at, manifest.installed_at);
    assert_eq!(saved.document.sha256, manifest.document.sha256);

    let contexts = manager
        .load_relevant_context("python computer vision", 3, 8_000)
        .unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].id, "roboflow-supervision");
    assert!(contexts[0].content.contains(UNTRUSTED_DOCUMENT_MARKER));
}

#[tokio::test]
async fn context_matches_multiple_plugins_by_tokens_and_remove_is_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig::default()).await;
    let manager = manager_for(temp.path(), address);
    manager.install("roboflow-supervision").await.unwrap();
    manager.install("fastapi-fastapi").await.unwrap();
    fake_task.abort();

    let contexts = manager
        .load_relevant_context("python", 99, MAX_RELEVANT_CONTEXT_CHARS * 2)
        .unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].id, "fastapi-fastapi");
    assert_eq!(contexts[1].id, "roboflow-supervision");
    assert!(contexts
        .iter()
        .all(|context| context.content.contains(UNTRUSTED_DOCUMENT_MARKER)));
    assert!(
        contexts
            .iter()
            .map(|context| context.content.chars().count())
            .sum::<usize>()
            <= MAX_RELEVANT_CONTEXT_CHARS
    );

    assert!(manager.remove("fastapi-fastapi").unwrap());
    assert!(!manager.remove("fastapi-fastapi").unwrap());
    let listed = manager.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "roboflow-supervision");
}

#[tokio::test]
async fn fetched_star_and_license_policy_fail_before_any_install_is_written() {
    let temp = tempfile::tempdir().unwrap();
    let (low_star_address, low_star_task) = fake_github(FakeGithubConfig {
        supervision_stars: 4,
        ..FakeGithubConfig::default()
    })
    .await;
    let low_star_manager = manager_for(temp.path(), low_star_address);
    let error = low_star_manager
        .install("roboflow-supervision")
        .await
        .unwrap_err();
    low_star_task.abort();
    assert!(error.to_string().contains("requires at least"));
    assert!(!low_star_manager
        .install_root()
        .join("roboflow-supervision")
        .exists());

    let (license_address, license_task) = fake_github(FakeGithubConfig {
        supervision_license: Some("GPL-3.0-only".to_string()),
        ..FakeGithubConfig::default()
    })
    .await;
    let license_manager = manager_for(temp.path(), license_address);
    let error = license_manager
        .install("roboflow-supervision")
        .await
        .unwrap_err();
    license_task.abort();
    assert!(error.to_string().contains("not allowed"));
    assert!(!license_manager
        .install_root()
        .join("roboflow-supervision")
        .exists());
}

#[tokio::test]
async fn redirecting_readme_requests_are_rejected_without_a_cross_origin_fetch() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig {
        redirect_supervision_readme: true,
        ..FakeGithubConfig::default()
    })
    .await;
    let manager = manager_for(temp.path(), address);
    let error = manager.install("roboflow-supervision").await.unwrap_err();
    fake_task.abort();

    assert!(error.to_string().contains("returned HTTP 302"), "{error:#}");
    assert!(!manager
        .install_root()
        .join("roboflow-supervision")
        .exists());
}

#[tokio::test]
async fn oversized_readmes_are_truncated_and_still_integrity_recorded() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig {
        supervision_readme: "x".repeat(MAX_SAVED_DOCUMENT_BYTES * 2),
        ..FakeGithubConfig::default()
    })
    .await;
    let manager = manager_for(temp.path(), address);
    let manifest = manager.install("roboflow-supervision").await.unwrap();
    fake_task.abort();

    assert!(manifest.document.truncated);
    assert!(manifest.document.bytes <= MAX_SAVED_DOCUMENT_BYTES);
    let document = std::fs::read(
        manager
            .install_root()
            .join("roboflow-supervision/README.md"),
    )
    .unwrap();
    assert_eq!(document.len(), manifest.document.bytes);
}

#[tokio::test]
async fn traversal_ids_are_rejected_before_any_network_or_filesystem_escape() {
    let temp = tempfile::tempdir().unwrap();
    let manager = PluginManager::new(temp.path().join("knowledge-plugins")).unwrap();
    let error = manager.install("../outside").await.unwrap_err();
    assert!(error.to_string().contains("invalid knowledge-plugin id"));
    assert!(!temp.path().join("outside").exists());
}

#[tokio::test]
async fn cached_manifest_must_match_registry_and_configured_api_origin() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig::default()).await;
    let manager = manager_for(temp.path(), address);
    let original = manager.install("roboflow-supervision").await.unwrap();
    fake_task.abort();
    let manifest_path = manager
        .install_root()
        .join("roboflow-supervision")
        .join("plugin.json");

    let mut metadata_tamper = original.clone();
    metadata_tamper.name = "Planted prompt source".to_string();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&metadata_tamper).unwrap(),
    )
    .unwrap();
    let error = manager
        .load_relevant_context("python computer vision", 1, 8_000)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not match its embedded curated registry"));

    let mut origin_tamper = original;
    origin_tamper.provenance.github_api_base = "https://attacker.example".to_string();
    origin_tamper.provenance.repository_metadata_url =
        "https://attacker.example/repos/roboflow/supervision".to_string();
    origin_tamper.provenance.readme_url =
        "https://attacker.example/repos/roboflow/supervision/readme".to_string();
    origin_tamper.provenance.commit_url =
        Some("https://attacker.example/repos/roboflow/supervision/commits/main".to_string());
    origin_tamper.document.source_url = origin_tamper.provenance.readme_url.clone();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&origin_tamper).unwrap(),
    )
    .unwrap();
    let error = manager
        .load_relevant_context("python computer vision", 1, 8_000)
        .unwrap_err();
    assert!(error.to_string().contains("provenance URLs"));
}

#[tokio::test]
async fn incomplete_staging_or_legacy_partial_directories_do_not_block_healthy_plugins() {
    let temp = tempfile::tempdir().unwrap();
    let (address, fake_task) = fake_github(FakeGithubConfig::default()).await;
    let manager = manager_for(temp.path(), address);
    manager.install("roboflow-supervision").await.unwrap();
    fake_task.abort();

    std::fs::create_dir(manager.install_root().join(".staging-interrupted-install")).unwrap();
    std::fs::create_dir(manager.install_root().join("fastapi-fastapi")).unwrap();
    std::fs::write(
        manager
            .install_root()
            .join("fastapi-fastapi")
            .join("README.md"),
        "partial legacy install",
    )
    .unwrap();

    let listed = manager.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "roboflow-supervision");
}

#[cfg(unix)]
#[test]
fn symlinked_plugin_directory_is_rejected_not_followed() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let manager = PluginManager::new(temp.path().join("knowledge-plugins")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(
        outside.path(),
        manager.install_root().join("roboflow-supervision"),
    )
    .unwrap();

    let error = manager.list().unwrap_err();
    assert!(error.to_string().contains("refusing symlink"));
    assert!(outside.path().exists());
}

//! End-to-end contract for the bounded local coding-team path.
//!
//! The fake Ollama endpoint scripts the two read-only reconnaissance agents,
//! the single workspace writer, and the advisory reviewer.  The real
//! coordinator then runs its fixed `cargo test --workspace` verifier against
//! a tiny throwaway Rust workspace.  This protects the important distinction
//! between several chat completions and a coordinated team that actually
//! changes files, proves the change, and keeps exactly one writer lane.

use ollama_forge::agent::AllowAllApproval;
use ollama_forge::providers::OllamaProvider;
use ollama_forge::team::{TeamConfig, TeamCoordinator, TeamEvent, TeamMode, TeamRole, TeamStatus};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn fake_ollama() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = handle_fake_ollama(stream).await;
            });
        }
    });
    (address, task)
}

async fn handle_fake_ollama(stream: TcpStream) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await?;

    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let request: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let response = scripted_response(&request);
    let payload = json!({
        "response": response,
        "model": "team-test-coder",
        "done": true,
        "eval_count": 1,
        "prompt_eval_count": 1,
    });
    let body = serde_json::to_string(&payload).unwrap();
    let status = if path == "/api/generate" {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let http = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write.write_all(http.as_bytes()).await?;
    write.flush().await
}

/// Keep the fake model deterministic from the role-specific system prompt and
/// whether the agent has already received a tool result.  This deliberately
/// does not rely on a race-prone global counter, so the test remains useful if
/// the coordinator later changes the scheduling of read-only scouts.
fn scripted_response(request: &Value) -> String {
    let system = request
        .get("system")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prompt = request
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let saw_tool_result = prompt.contains("[tool result, ok=");

    let action = if system.contains("Map the relevant architecture") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "The workspace is a single Rust package; app/src/lib.rs owns the greeting."
            })
        } else {
            json!({"action":"use_tool","tool":"fs_list","args":{"path":"","depth":3}})
        }
    } else if system.contains("Find the existing verification conventions") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "Cargo.toml selects cargo test --workspace; the greeting unit test is in app/src/lib.rs."
            })
        } else {
            json!({"action":"use_tool","tool":"fs_read","args":{"path":"Cargo.toml"}})
        }
    } else if system.contains("Controlled workspace implementer") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "Changed app/src/lib.rs through the sole writer lane."
            })
        } else {
            json!({
                "action": "use_tool",
                "tool": "fs_edit",
                "args": {
                    "path": "app/src/lib.rs",
                    "old_string": "\"hello world\"",
                    "new_string": "\"hello Ollamax\""
                }
            })
        }
    } else if system.contains("read-only local team planner") {
        return "Read app/src/lib.rs, update only the greeting literal, then rely on cargo test --workspace for verification."
            .to_string();
    } else if system.contains("read-only code reviewer") {
        // The reviewer is not an agent loop, so it expects normal text rather
        // than the action JSON used by the scouts and implementer.
        return "Verifier evidence is passing; no additional risk identified in this small change."
            .to_string();
    } else {
        json!({"action":"answer","text":"Unexpected test role."})
    };
    action.to_string()
}

fn create_rust_workspace(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"team-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/src/lib.rs"),
        r#"pub fn greeting() -> &'static str {
    "hello world"
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_is_updated() {
        assert_eq!(greeting(), "hello Ollamax");
    }
}
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn serial_team_edits_workspace_then_records_passing_verifier_evidence() {
    let workspace = tempfile::tempdir().unwrap();
    create_rust_workspace(workspace.path());
    let (ollama_addr, fake_ollama_task) = fake_ollama().await;

    let coordinator = TeamCoordinator::new(
        Arc::new(OllamaProvider::new(format!("http://{ollama_addr}"))),
        workspace.path(),
        TeamConfig {
            model: "team-test-coder".to_string(),
            max_iterations: 4,
            max_repair_rounds: 0,
            mode: TeamMode::Serial,
            ..TeamConfig::default()
        },
    )
    .unwrap();

    let mut events = Vec::new();
    let run = tokio::time::timeout(
        Duration::from_secs(30),
        coordinator.run(
            "Update the greeting in app/src/lib.rs so the existing test passes.",
            Arc::new(AllowAllApproval),
            |event| events.push(event.clone()),
        ),
    )
    .await
    .expect("team run timed out")
    .expect("team run should succeed");
    fake_ollama_task.abort();

    assert!(matches!(run.status, TeamStatus::Verified));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("app/src/lib.rs")).unwrap(),
        r#"pub fn greeting() -> &'static str {
    "hello Ollamax"
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_is_updated() {
        assert_eq!(greeting(), "hello Ollamax");
    }
}
"#
    );

    assert_eq!(run.scouts.len(), 2);
    assert!(run
        .scouts
        .iter()
        .any(|report| matches!(report.role, TeamRole::ArchitectureScout)));
    assert!(run
        .scouts
        .iter()
        .any(|report| matches!(report.role, TeamRole::TestScout)));
    assert!(run.planner_summary.contains("app/src/lib.rs"));
    assert_eq!(
        run.plan
            .roles
            .iter()
            .filter(|role| matches!(role, TeamRole::Implementer))
            .count(),
        1,
        "the plan must expose exactly one workspace writer"
    );
    assert_eq!(run.verification.len(), 1);
    assert_eq!(run.verification[0].command, "cargo test --workspace");
    assert!(run.verification[0].passed, "{:?}", run.verification[0]);
    assert!(run.verification[0].output.contains("test result: ok"));

    let writer_starts = events
        .iter()
        .filter(|event| matches!(event, TeamEvent::ImplementerStarted { .. }))
        .count();
    let writer_edits = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                TeamEvent::ImplementerStep { step, .. }
                    if step.tool == "fs_edit" || step.tool == "fs_write"
            )
        })
        .count();
    assert_eq!(writer_starts, 1, "no second writer lane may be started");
    assert_eq!(writer_edits, 1, "the one writer owns the only edit");

    let first_writer = events
        .iter()
        .position(|event| matches!(event, TeamEvent::ImplementerStarted { .. }))
        .unwrap();
    let scouts_finished_before_writer = events[..first_writer]
        .iter()
        .filter(|event| matches!(event, TeamEvent::ScoutFinished { .. }))
        .count();
    assert_eq!(
        scouts_finished_before_writer, 2,
        "both read-only handoffs must complete before the single writer begins"
    );
}

//! End-to-end contract for the `POST /api/team` SSE endpoint.
//!
//! This keeps the browser/extension-facing team surface honest: it must reject
//! an unauthenticated request, then drive the real bounded coordinator against
//! the server's fixed workspace root.  A deterministic fake Ollama scripts the
//! scouts, single writer, and reviewer; the verifier is the real `cargo test`
//! command run in a disposable workspace.

use ollama_forge::server::{serve_listener_in_workspace_with_token, API_TOKEN_HEADER};
use ollama_forge::Config;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const TEST_TOKEN: &str = "server-team-test-token-0123456789";

async fn fake_ollama() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    fake_ollama_with_tag_delay(None, None).await
}

async fn fake_ollama_with_tag_delay(
    tag_delay: Option<Duration>,
    tag_started: Option<tokio::sync::mpsc::UnboundedSender<()>>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let tag_started = tag_started.clone();
            tokio::spawn(async move {
                let _ = handle_fake_ollama(stream, tag_delay, tag_started).await;
            });
        }
    });
    (address, task)
}

async fn handle_fake_ollama(
    stream: TcpStream,
    tag_delay: Option<Duration>,
    tag_started: Option<tokio::sync::mpsc::UnboundedSender<()>>,
) -> std::io::Result<()> {
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
    let (status, payload) = match path {
        // `/api/team` checks the installed model list before it starts the
        // role lanes, so make that real selection path part of the contract.
        "/api/tags" => {
            if let Some(tag_started) = tag_started {
                let _ = tag_started.send(());
            }
            if let Some(delay) = tag_delay {
                tokio::time::sleep(delay).await;
            }
            (
                "200 OK",
                json!({
                    "models": [{
                        "name": "team-test-coder",
                        "size": 1,
                        "modified_at": "now",
                        "digest": "team-test-digest"
                    }]
                }),
            )
        }
        "/api/generate" => {
            let request: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            json_response(scripted_response(&request))
        }
        _ => (
            "404 Not Found",
            json!({"error": "unexpected fake Ollama path"}),
        ),
    };
    let body = serde_json::to_string(&payload).unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write.write_all(response.as_bytes()).await?;
    write.flush().await
}

fn json_response(response: String) -> (&'static str, Value) {
    (
        "200 OK",
        json!({
            "response": response,
            "model": "team-test-coder",
            "done": true,
            "eval_count": 1,
            "prompt_eval_count": 1,
        }),
    )
}

/// Role prompts, rather than a global request counter, make this fake stable
/// even if the server later schedules the two read-only scouts concurrently.
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

    if system.contains("You are planning before acting") {
        return "1. Inspect the workspace.\n2. Update the greeting.\n3. Run the fixed verifier."
            .to_string();
    }
    if system.contains("read-only local team planner") {
        return "Edit the greeting in app/src/lib.rs, then use the fixed cargo test verifier."
            .to_string();
    }
    if system.contains("read-only code reviewer") {
        return "The verifier passed and the focused greeting change has no additional risk."
            .to_string();
    }

    let action = if system.contains("Map the relevant architecture") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "The greeting lives in app/src/lib.rs."
            })
        } else {
            json!({"action":"use_tool","tool":"fs_list","args":{"path":"","depth":3}})
        }
    } else if system.contains("Find the existing verification conventions") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "The workspace uses cargo test --workspace."
            })
        } else {
            json!({"action":"use_tool","tool":"fs_read","args":{"path":"Cargo.toml"}})
        }
    } else if system.contains("Controlled workspace implementer") {
        if saw_tool_result {
            json!({
                "action": "answer",
                "text": "Updated app/src/lib.rs through the single writer lane."
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
        "[package]\nname = \"server-team-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
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

async fn raw_request(address: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

async fn post_team(address: std::net::SocketAddr, token: Option<&str>, body: &Value) -> String {
    let body = serde_json::to_string(body).unwrap();
    let auth = token
        .map(|token| format!("{API_TOKEN_HEADER}: {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /api/team HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    raw_request(address, &request).await
}

async fn post_cancel(address: std::net::SocketAddr, id: &str) -> String {
    let body = json!({"id": id}).to_string();
    let request = format!(
        "POST /api/cancel HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    raw_request(address, &request).await
}

fn sse_events(response: &str) -> Vec<Value> {
    response
        .split("\n\n")
        .filter_map(|frame| {
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data:"))?
                .trim();
            serde_json::from_str(data).ok()
        })
        .collect()
}

#[tokio::test]
async fn team_api_authenticates_then_edits_and_verifies_the_server_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    create_rust_workspace(workspace.path());
    let (ollama_address, fake_ollama_task) = fake_ollama().await;
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_listener.local_addr().unwrap();
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        server_listener,
        Config {
            ollama_url: format!("http://{ollama_address}"),
            default_model: "team-test-coder".to_string(),
            ..Config::default()
        },
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));

    let request = json!({
        "id": "server-team-e2e",
        "task": "Update app/src/lib.rs so the existing greeting test passes.",
        "model": "team-test-coder",
        "autonomy": "auto",
        "max_iterations": 4,
        "max_repair_rounds": 0
    });
    let rejected = post_team(server_address, None, &request).await;
    assert!(
        rejected.contains("401 Unauthorized"),
        "response: {rejected}"
    );
    assert!(
        rejected.contains("invalid local Ollamax API token"),
        "response: {rejected}"
    );

    let response = tokio::time::timeout(
        Duration::from_secs(45),
        post_team(server_address, Some(TEST_TOKEN), &request),
    )
    .await
    .expect("team SSE run timed out");

    server_task.abort();
    fake_ollama_task.abort();

    assert!(response.contains("200 OK"), "response: {response}");
    assert!(
        response.contains("text/event-stream"),
        "response: {response}"
    );
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

    let events = sse_events(&response);
    let event_types = events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"team_meta"), "events: {events:#?}");
    assert!(event_types.contains(&"team_plan"), "events: {events:#?}");
    assert!(
        event_types.contains(&"team_scout_finished"),
        "events: {events:#?}"
    );
    assert!(
        event_types.contains(&"team_writer_started"),
        "events: {events:#?}"
    );
    assert!(
        event_types.contains(&"team_planner_finished"),
        "events: {events:#?}"
    );
    assert!(
        event_types.contains(&"team_verification_finished"),
        "events: {events:#?}"
    );
    assert!(event_types.contains(&"team_result"), "events: {events:#?}");
    assert_eq!(event_types.last(), Some(&"done"), "events: {events:#?}");

    let result = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("team_result"))
        .expect("team result event");
    assert_eq!(result.get("status"), Some(&json!("Verified")));
    let verification = result
        .get("verification")
        .and_then(Value::as_array)
        .expect("verification evidence");
    assert_eq!(verification.len(), 1);
    assert_eq!(
        verification[0].get("command"),
        Some(&json!("cargo test --workspace"))
    );
    assert_eq!(verification[0].get("passed"), Some(&json!(true)));
}

#[tokio::test]
async fn team_cancel_is_registered_before_model_setup_and_prevents_writer_start() {
    let workspace = tempfile::tempdir().unwrap();
    create_rust_workspace(workspace.path());
    let (tag_started_tx, mut tag_started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ollama_address, fake_ollama_task) =
        fake_ollama_with_tag_delay(Some(Duration::from_secs(2)), Some(tag_started_tx)).await;
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_listener.local_addr().unwrap();
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        server_listener,
        Config {
            ollama_url: format!("http://{ollama_address}"),
            default_model: "team-test-coder".to_string(),
            ..Config::default()
        },
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));
    let request = json!({
        "id": "cancel-before-team-start",
        "task": "Update the greeting.",
        "model": "team-test-coder",
        "autonomy": "auto"
    });
    let request_for_task = request.clone();
    let response_task = tokio::spawn(async move {
        post_team(server_address, Some(TEST_TOKEN), &request_for_task).await
    });
    tokio::time::timeout(Duration::from_secs(5), tag_started_rx.recv())
        .await
        .expect("team request never began model setup")
        .expect("tag-start notification channel closed");

    let cancel_response = post_cancel(server_address, "cancel-before-team-start").await;
    assert!(
        cancel_response.contains("\"ok\":true"),
        "cancel response: {cancel_response}"
    );
    let response = tokio::time::timeout(Duration::from_secs(8), response_task)
        .await
        .expect("cancelled team request did not finish")
        .expect("team request task panicked");

    server_task.abort();
    fake_ollama_task.abort();

    assert!(
        response.contains("\"type\":\"cancelled\""),
        "response: {response}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("app/src/lib.rs")).unwrap(),
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
        "a cancellation during setup must prevent any writer edit"
    );
}

#[tokio::test]
async fn team_rejects_an_uninstalled_reviewer_before_any_workspace_run() {
    let workspace = tempfile::tempdir().unwrap();
    create_rust_workspace(workspace.path());
    let (ollama_address, fake_ollama_task) = fake_ollama().await;
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_listener.local_addr().unwrap();
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        server_listener,
        Config {
            ollama_url: format!("http://{ollama_address}"),
            default_model: "team-test-coder".to_string(),
            ..Config::default()
        },
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));
    let response = post_team(
        server_address,
        Some(TEST_TOKEN),
        &json!({
            "id": "invalid-team-reviewer",
            "task": "Update the greeting.",
            "model": "team-test-coder",
            "reviewer_model": "not-installed"
        }),
    )
    .await;

    server_task.abort();
    fake_ollama_task.abort();

    assert!(
        response.contains("requested reviewer model `not-installed` is not installed"),
        "response: {response}"
    );
    assert!(
        response.contains("\"type\":\"done\""),
        "response: {response}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("app/src/lib.rs")).unwrap(),
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
"#
    );
}

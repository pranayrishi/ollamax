//! End-to-end contract for the local coding-agent path.
//!
//! A deterministic fake Ollama drives the same HTTP/SSE server that the IDEs
//! use. The scripted model inventories a temporary workspace, reads a file,
//! edits it, validates it with the sandboxed shell, and returns an answer. This
//! catches regressions where the UI/server merely displays code instead of
//! making the requested filesystem change.

use ollama_forge::server::{serve_listener_in_workspace_with_token, API_TOKEN_HEADER};
use ollama_forge::Config;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const TEST_TOKEN: &str = "agent-workspace-test-token-0123456789";

#[cfg(windows)]
const VALIDATE_EDIT_COMMAND: &str = r#"findstr /C:"hello Ollamax" src\example.txt >NUL"#;

#[cfg(not(windows))]
const VALIDATE_EDIT_COMMAND: &str =
    "test -f src/example.txt && grep -q 'hello Ollamax' src/example.txt";

async fn fake_ollama() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let calls = calls.clone();
            tokio::spawn(async move {
                let _ = handle_fake_ollama(stream, calls).await;
            });
        }
    });
    (addr, task)
}

async fn handle_fake_ollama(stream: TcpStream, calls: Arc<AtomicUsize>) -> std::io::Result<()> {
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
    if content_length > 0 {
        reader.read_exact(&mut body).await?;
    }

    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let payload = if path == "/api/generate" {
        let step = calls.fetch_add(1, Ordering::SeqCst);
        let action = match step {
            0 => json!({"action":"use_tool","tool":"fs_list","args":{"path":"","depth":2}}),
            1 => json!({"action":"use_tool","tool":"fs_read","args":{"path":"src/example.txt"}}),
            2 => json!({
                "action":"use_tool",
                "tool":"fs_edit",
                "args":{
                    "path":"src/example.txt",
                    "old_string":"hello world",
                    "new_string":"hello Ollamax"
                }
            }),
            3 => json!({
                "action":"use_tool",
                "tool":"shell",
                "args":{"command":VALIDATE_EDIT_COMMAND}
            }),
            _ => {
                json!({"action":"answer","text":"Updated src/example.txt and validated the change."})
            }
        };
        json!({
            "response": action.to_string(),
            "model": "test-coder",
            "done": true,
            "eval_count": 1,
            "prompt_eval_count": 1,
        })
    } else if path == "/api/tags" {
        json!({"models":[{"name":"test-coder","size":1,"modified_at":"now","digest":"test"}]})
    } else {
        json!({"error":"unexpected fake Ollama path"})
    };
    let body = serde_json::to_string(&payload).unwrap();
    let status = if path == "/api/generate" || path == "/api/tags" {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write.write_all(response.as_bytes()).await?;
    write.flush().await
}

async fn confirm_fake_ollama() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    confirm_fake_ollama_with_empty_plan(false).await
}

async fn confirm_fake_ollama_with_empty_plan(
    empty_plan: bool,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let calls = calls.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut reader = BufReader::new(read);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).await.is_err() {
                    return;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.is_err() || line == "\r\n" || line == "\n"
                    {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0; content_length];
                if reader.read_exact(&mut body).await.is_err() {
                    return;
                }
                let request: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                let path = request_line.split_whitespace().nth(1).unwrap_or("");
                let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
                let payload = if path == "/api/generate" {
                    let response_text = if prompt.contains("List the concrete steps") {
                        if empty_plan {
                            String::new()
                        } else {
                            "1. Update the greeting.\n2. Verify the result.".to_string()
                        }
                    } else if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        json!({
                            "action":"use_tool",
                            "tool":"fs_edit",
                            "args":{"path":"src/example.txt","old_string":"hello world","new_string":"hello confirmed"}
                        })
                        .to_string()
                    } else {
                        json!({"action":"answer","text":"Applied the confirmed change."})
                            .to_string()
                    };
                    json!({"response": response_text, "model":"test-coder", "done":true})
                } else if path == "/api/tags" {
                    json!({"models":[{"name":"test-coder","size":1,"modified_at":"now","digest":"test"}]})
                } else {
                    // Capability probes are deliberately side-effect free in the
                    // fake: they must not consume a scripted generation action.
                    json!({"capabilities":[]})
                };
                let response_body = serde_json::to_string(&payload).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                let _ = write.write_all(response.as_bytes()).await;
            });
        }
    });
    (addr, task)
}

async fn post_and_read(addr: std::net::SocketAddr, path: &str, body: &Value) -> String {
    let data = serde_json::to_string(body).unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{data}",
        data.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Drive an SSE run and approve only the nonce carried by each real pending
/// event. This proves early/guessed decisions cannot queue up for a later edit.
async fn post_sse_approving_real_prompts(
    addr: std::net::SocketAddr,
    path: &str,
    body: &Value,
) -> String {
    let data = serde_json::to_string(body).unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{data}",
        data.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut all = String::new();
    let mut frames = String::new();
    let mut approved = 0usize;
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        if count == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&chunk[..count]);
        all.push_str(&text);
        frames.push_str(&text);
        while let Some(index) = frames.find("\n\n") {
            let frame = frames[..index].to_string();
            frames.drain(..index + 2);
            let Some(data_line) = frame.lines().find(|line| line.starts_with("data:")) else {
                continue;
            };
            let event: Value =
                match serde_json::from_str(data_line.trim_start_matches("data:").trim()) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
            if !matches!(
                event.get("type").and_then(Value::as_str),
                Some("plan") | Some("approval_request")
            ) {
                continue;
            }
            let approval_id = event
                .get("approvalId")
                .and_then(Value::as_str)
                .expect("pending event must contain an approval nonce");
            let id = body.get("id").and_then(Value::as_str).unwrap();
            let response = post_and_read(
                addr,
                "/api/agent/approve",
                &json!({"id":id,"approvalId":approval_id,"decision":true}),
            )
            .await;
            assert!(
                response.contains("\"delivered\":true"),
                "approval response: {response}"
            );
            approved += 1;
        }
    }
    assert_eq!(approved, 2, "expected plan + edit approvals: {all}");
    all
}

#[tokio::test]
async fn agent_edits_and_validates_only_the_approved_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/example.txt"), "hello world\n").unwrap();

    let (ollama_addr, fake_ollama_task) = fake_ollama().await;
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server_listener.local_addr().unwrap();
    let config = Config {
        ollama_url: format!("http://{ollama_addr}"),
        default_model: "test-coder".to_string(),
        ..Config::default()
    };
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        server_listener,
        config,
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        post_and_read(
            server_addr,
            "/api/research",
            &json!({
                "id":"agent-workspace-e2e",
                "question":"Update the greeting in src/example.txt and validate it.",
                "model":"test-coder",
                "autonomy":"auto",
                "max_iterations":8
            }),
        ),
    )
    .await
    .expect("agent SSE run timed out");

    server_task.abort();
    fake_ollama_task.abort();

    let updated = std::fs::read_to_string(workspace.path().join("src/example.txt")).unwrap();
    assert_eq!(updated, "hello Ollamax\n");
    assert!(
        response.contains("\"tool\":\"fs_list\""),
        "response: {response}"
    );
    assert!(
        response.contains("\"tool\":\"fs_edit\""),
        "response: {response}"
    );
    assert!(
        response.contains("\"tool\":\"shell\""),
        "response: {response}"
    );
    assert!(
        response.contains("Updated src/example.txt"),
        "response: {response}"
    );
}

#[tokio::test]
async fn confirm_mode_requires_the_server_approval_channel_before_editing() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/example.txt"), "hello world\n").unwrap();

    let (ollama_addr, fake_ollama_task) = confirm_fake_ollama().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        ollama_url: format!("http://{ollama_addr}"),
        default_model: "test-coder".to_string(),
        ..Config::default()
    };
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        listener,
        config,
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));

    let id = "agent-confirm-e2e";
    let request_body = json!({
        "id":id,
        "question":"Change the greeting only after approval.",
        "model":"test-coder",
        "autonomy":"confirm",
        "max_iterations":5
    });
    // An early guessed decision must not queue up or release the future plan.
    let early = post_and_read(
        addr,
        "/api/agent/approve",
        &json!({"id":id,"approvalId":"guessed-nonce","decision":true}),
    )
    .await;
    assert!(
        early.contains("\"delivered\":false"),
        "early response: {early}"
    );
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        post_sse_approving_real_prompts(addr, "/api/research", &request_body),
    )
    .await
    .expect("confirmed agent SSE run timed out");

    server_task.abort();
    fake_ollama_task.abort();

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/example.txt")).unwrap(),
        "hello confirmed\n"
    );
    assert!(
        response.contains("\"type\":\"plan\""),
        "response: {response}"
    );
    assert!(
        response.contains("\"type\":\"approval_request\""),
        "response: {response}"
    );
    assert!(
        response.contains("Applied the confirmed change"),
        "response: {response}"
    );
}

#[tokio::test]
async fn confirm_mode_fails_closed_when_intent_preview_is_empty() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/example.txt"), "hello world\n").unwrap();

    let (ollama_addr, fake_ollama_task) = confirm_fake_ollama_with_empty_plan(true).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        listener,
        Config {
            ollama_url: format!("http://{ollama_addr}"),
            default_model: "test-coder".to_string(),
            ..Config::default()
        },
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        post_and_read(
            addr,
            "/api/research",
            &json!({
                "id":"empty-confirm-plan",
                "question":"Change the greeting only after an approved plan.",
                "model":"test-coder",
                "autonomy":"confirm",
                "max_iterations":5
            }),
        ),
    )
    .await
    .expect("empty-plan request timed out");

    server_task.abort();
    fake_ollama_task.abort();

    assert!(
        response.contains("intent-preview plan generation returned an empty plan"),
        "response: {response}"
    );
    assert!(
        !response.contains("\"type\":\"approval_request\""),
        "an unavailable plan must not fall through to an edit approval: {response}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/example.txt")).unwrap(),
        "hello world\n",
        "no file action may run when confirm-mode planning fails"
    );
}

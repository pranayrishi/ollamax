//! End-to-end contract for an explicitly configured, loopback-only
//! OpenAI-compatible endpoint.
//!
//! This intentionally does not rely on Ollama: one fake listener accepts the
//! benign `/api/tags` probe caused by `/api/models` and the actual
//! `/v1/chat/completions` request. It proves the server exposes only the
//! declared `local:` selector while translating the model at the local
//! provider boundary to the endpoint's declared served name.

use ollama_forge::server::{serve_listener_in_workspace_with_token, API_TOKEN_HEADER};
use ollama_forge::{Config, LocalEndpointConfig, LocalEndpointModelConfig};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const TEST_TOKEN: &str = "server-local-endpoint-test-token-0123456789";
const SELECTOR: &str = "local:lab/serving";
const SERVED_MODEL: &str = "Declared-Server-Model";

/// A fake server bound to a *literal* IPv4 loopback listener. Configuration
/// below intentionally spells both Ollama and this endpoint as `localhost`;
/// `ServerState::new` must normalize those embedded settings before use.
async fn fake_local_endpoint() -> (
    std::net::SocketAddr,
    mpsc::UnboundedReceiver<Value>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let requests_tx = requests_tx.clone();
            tokio::spawn(async move {
                let _ = handle_fake_local_endpoint(stream, requests_tx).await;
            });
        }
    });
    (address, requests_rx, task)
}

async fn handle_fake_local_endpoint(
    stream: TcpStream,
    requests: mpsc::UnboundedSender<Value>,
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
    if content_length > 0 {
        reader.read_exact(&mut body).await?;
    }

    let method = request_line.split_whitespace().next().unwrap_or("");
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let (status, payload) = match (method, path) {
        // `/api/models` makes an Ollama tags request first. Answer it locally
        // so the configured endpoint stays visible alongside an empty Ollama
        // installation instead of relying on a connection error path.
        ("GET", "/api/tags") => ("200 OK", json!({"models": []})),
        ("POST", "/v1/chat/completions") => {
            let request: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
            let _ = requests.send(request);
            (
                "200 OK",
                json!({
                    "model": SERVED_MODEL,
                    "choices": [{"message": {"role": "assistant", "content": "local endpoint answer"}}],
                    "usage": {"prompt_tokens": 9, "completion_tokens": 3}
                }),
            )
        }
        _ => (
            "404 Not Found",
            json!({"error": format!("unexpected {method} {path}")}),
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

async fn raw_request(address: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

fn api_request(method: &str, path: &str, body: Option<&Value>) -> String {
    let serialized = body.map(serde_json::to_string).transpose().unwrap();
    let content_length = serialized.as_ref().map_or(0, String::len);
    format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {content_length}\r\n\r\n{}",
        serialized.unwrap_or_default(),
    )
}

fn json_body(response: &str) -> Value {
    let (_, body) = response
        .split_once("\r\n\r\n")
        .expect("HTTP response must include a body separator");
    serde_json::from_str(body).expect("response body must be JSON")
}

#[tokio::test]
async fn configured_loopback_endpoint_is_listed_and_receives_its_served_model() {
    let (endpoint_address, mut endpoint_requests, endpoint_task) = fake_local_endpoint().await;
    let workspace = tempfile::tempdir().unwrap();
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_listener.local_addr().unwrap();
    let port = endpoint_address.port();
    let config = Config {
        // Exercise ServerState's normalization for embedded Config callers:
        // both raw hosts are `localhost`, but the listener is IPv4 literal.
        ollama_url: format!("http://localhost:{port}"),
        default_model: SELECTOR.to_string(),
        local_endpoints: vec![LocalEndpointConfig {
            id: "lab".to_string(),
            url: format!("http://localhost:{port}"),
            api_key_env: None,
            max_parallel_requests: 1,
            models: vec![LocalEndpointModelConfig {
                id: "serving".to_string(),
                served_model: SERVED_MODEL.to_string(),
                label: Some("Test local endpoint".to_string()),
                vision: false,
                thinking: true,
                context_window_tokens: Some(16_384),
            }],
        }],
        ..Config::default()
    };
    let server_task = tokio::spawn(serve_listener_in_workspace_with_token(
        server_listener,
        config,
        workspace.path().to_path_buf(),
        TEST_TOKEN,
    ));

    let models_response =
        raw_request(server_address, &api_request("GET", "/api/models", None)).await;
    assert!(
        models_response.contains("200 OK"),
        "response: {models_response}"
    );
    let models = json_body(&models_response);
    assert_eq!(models["default"], SELECTOR);
    let exposed = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["name"] == SELECTOR)
        .expect("configured local selector must appear in /api/models");
    assert_eq!(exposed["runtime"], "openai-compatible-local");
    assert_eq!(exposed["local"], true);
    assert_eq!(exposed["servedModel"], SERVED_MODEL);
    assert_eq!(exposed["endpoint"], "lab");

    let chat = json!({
        "id": "configured-local-chat",
        "model": SELECTOR,
        "prompt": "Reply from the configured local endpoint."
    });
    let chat_response = tokio::time::timeout(
        Duration::from_secs(10),
        raw_request(
            server_address,
            &api_request("POST", "/api/chat", Some(&chat)),
        ),
    )
    .await
    .expect("configured local chat timed out");

    server_task.abort();
    endpoint_task.abort();

    assert!(
        chat_response.contains("200 OK"),
        "response: {chat_response}"
    );
    assert!(
        chat_response.contains("text/event-stream"),
        "response: {chat_response}"
    );
    assert!(
        chat_response.contains("\"type\":\"meta\"") && chat_response.contains(SELECTOR),
        "response: {chat_response}"
    );
    assert!(
        chat_response.contains("\"type\":\"token\"")
            && chat_response.contains("local endpoint answer"),
        "response: {chat_response}"
    );
    assert!(
        chat_response.contains("\"type\":\"done\"") && chat_response.contains("\"buffered\":true"),
        "response: {chat_response}"
    );

    let endpoint_request = tokio::time::timeout(Duration::from_secs(2), endpoint_requests.recv())
        .await
        .expect("configured endpoint did not receive chat completion")
        .expect("endpoint request channel closed");
    assert_eq!(endpoint_request["model"], SERVED_MODEL);
    assert_eq!(endpoint_request["stream"], false);
    assert!(endpoint_request["messages"].is_array());
}

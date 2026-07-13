//! Integration tests for the `forge serve` backend.
//!
//! These drive the real server over a loopback TCP socket on an
//! OS-assigned port, so they exercise the actual HTTP plumbing
//! (request parsing, routing, JSON responses, CORS) without needing a
//! running Ollama daemon. Endpoints that *would* call Ollama
//! (`/api/chat`, `/api/models`) are not exercised here — those need a
//! live model and are covered by manual/`FORGE_LIVE_OLLAMA` testing.
//!
//! Pure-function coverage (head parsing, SSE framing, host sanitization,
//! prompt building) lives in the `#[cfg(test)]` module inside
//! `src/server/mod.rs`.

use ollama_forge::server::{serve_listener_in_workspace_with_token, API_TOKEN_HEADER};
use ollama_forge::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Bind an ephemeral loopback port, spawn the server on it, and return the
/// address the test client should connect to.
const TEST_TOKEN: &str = "server-protocol-test-token-0123456789";

async fn spawn_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let workspace = std::env::current_dir().unwrap();
    tokio::spawn(async move {
        let _ = serve_listener_in_workspace_with_token(
            listener,
            Config::default(),
            workspace,
            TEST_TOKEN,
        )
        .await;
    });
    addr
}

/// Send a raw HTTP request and read the full response (the server sends
/// `Connection: close` on non-streaming endpoints, so `read_to_end` returns).
async fn raw_request(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn health_endpoint_returns_ok_json() {
    let addr = spawn_server().await;
    let resp = raw_request(addr, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.contains("200 OK"), "got: {resp}");
    assert!(resp.contains("application/json"));
    assert!(resp.contains("\"ok\":true"));
    // Capability-authenticated API clients may use CORS from the packaged
    // desktop renderer; project APIs themselves reject missing tokens.
    assert!(resp.contains("Access-Control-Allow-Origin: *"));
}

#[tokio::test]
async fn unknown_route_is_404() {
    let addr = spawn_server().await;
    let resp = raw_request(addr, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.contains("404 Not Found"), "got: {resp}");
}

#[tokio::test]
async fn options_preflight_returns_204_with_cors() {
    let addr = spawn_server().await;
    let resp = raw_request(addr, "OPTIONS /api/chat HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.contains("204 No Content"), "got: {resp}");
    assert!(resp.contains("Access-Control-Allow-Methods"));
}

#[tokio::test]
async fn cancel_unknown_id_returns_ok_false() {
    let addr = spawn_server().await;
    let body = "{\"id\":\"does-not-exist\"}";
    let req = format!(
        "POST /api/cancel HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let resp = raw_request(addr, &req).await;
    assert!(resp.contains("200 OK"), "got: {resp}");
    assert!(resp.contains("\"ok\":false"));
}

#[tokio::test]
async fn malformed_chat_body_is_400() {
    let addr = spawn_server().await;
    let body = "{ not json";
    let req = format!(
        "POST /api/chat HTTP/1.1\r\nHost: x\r\n{API_TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let resp = raw_request(addr, &req).await;
    assert!(resp.contains("400 Bad Request"), "got: {resp}");
}

#[tokio::test]
async fn api_rejects_requests_without_the_private_capability() {
    let addr = spawn_server().await;
    let body = r#"{"id":"no-token"}"#;
    let req = format!(
        "POST /api/cancel HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let resp = raw_request(addr, &req).await;
    assert!(resp.contains("401 Unauthorized"), "got: {resp}");
    assert!(resp.contains("invalid local Ollamax API token"), "got: {resp}");
}

#[tokio::test]
async fn console_is_same_origin_and_receives_a_private_api_capability() {
    let addr = spawn_server().await;
    let resp = raw_request(addr, "GET /console HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.contains("200 OK"), "got: {resp}");
    assert!(resp.contains("Ollamax Agent Console"), "got: {resp}");
    assert!(!resp.contains("__OLLAMAX_API_TOKEN__"), "got: {resp}");
    assert!(!resp.contains("Access-Control-Allow-Origin"), "got: {resp}");
}

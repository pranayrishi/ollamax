//! A strict, local-only OpenAI-compatible provider.
//!
//! Self-hosted inference servers such as vLLM and SGLang commonly expose the
//! OpenAI Chat Completions wire protocol.  This module deliberately implements
//! only that *wire protocol*: model-specific chat templates and deployment
//! flags remain the responsibility of the local server operator.  In
//! particular, it does not turn an arbitrary internet URL into a provider.
//!
//! The endpoint is normalized to a loopback `/v1` base URL, proxies are
//! bypassed, and an optional bearer token can be read only from a named
//! environment variable.  This keeps a configured self-hosted model local and
//! avoids putting a token in project configuration or logs.

use super::{ChatOptions, GenerateOptions, LlmProvider, LlmResponse, ModelInfo};
use anyhow::{Context, Result};
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, Ipv6Addr},
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ERROR_BODY_CHARS: usize = 8_000;
const MAX_IMAGE_BASE64_CHARS: usize = 6 * 1024 * 1024;

/// Normalize a user-configured, OpenAI-compatible inference endpoint.
///
/// Only loopback URLs are accepted. `localhost` is rewritten to the literal
/// IPv4 loopback address so a modified hosts file cannot make a supposedly
/// local provider resolve somewhere else. The only accepted base paths are
/// `/` and `/v1`; the returned URL always ends in `/v1` without a trailing
/// slash.
pub fn normalize_openai_compatible_endpoint(raw_endpoint: &str) -> Result<String> {
    let raw_endpoint = raw_endpoint.trim();
    if raw_endpoint.is_empty() {
        anyhow::bail!("OpenAI-compatible endpoint is empty");
    }

    let has_explicit_scheme = raw_endpoint.contains("://");
    let candidate = if has_explicit_scheme {
        raw_endpoint.to_owned()
    } else {
        // A bare IPv6 loopback needs brackets when it becomes a URL host.
        let host = if raw_endpoint.parse::<Ipv6Addr>().is_ok() {
            format!("[{raw_endpoint}]")
        } else {
            raw_endpoint.to_owned()
        };
        format!("http://{host}")
    };

    let mut endpoint = Url::parse(&candidate).map_err(|error| {
        anyhow::anyhow!("invalid OpenAI-compatible endpoint `{raw_endpoint}`: {error:#}")
    })?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        anyhow::bail!(
            "invalid OpenAI-compatible endpoint `{raw_endpoint}`: expected an http:// or https:// URL"
        );
    }
    if endpoint.username() != "" || endpoint.password().is_some() {
        anyhow::bail!(
            "invalid OpenAI-compatible endpoint `{raw_endpoint}`: userinfo is not allowed"
        );
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        anyhow::bail!(
            "invalid OpenAI-compatible endpoint `{raw_endpoint}`: query strings and fragments are not allowed"
        );
    }

    let raw_host = endpoint.host_str().ok_or_else(|| {
        anyhow::anyhow!("invalid OpenAI-compatible endpoint `{raw_endpoint}`: missing host")
    })?;
    let host_for_validation = raw_host.trim_matches(|ch| ch == '[' || ch == ']');
    let normalized_host = normalize_loopback_host(host_for_validation).map_err(|error| {
        anyhow::anyhow!("invalid OpenAI-compatible endpoint `{raw_endpoint}`: {error:#}")
    })?;
    if normalized_host != host_for_validation || raw_host != normalized_host {
        let host_for_url = if normalized_host.parse::<Ipv6Addr>().is_ok() {
            format!("[{normalized_host}]")
        } else {
            normalized_host
        };
        endpoint.set_host(Some(&host_for_url)).map_err(|error| {
            anyhow::anyhow!(
                "invalid OpenAI-compatible endpoint `{raw_endpoint}`: could not set loopback host: {error:#}"
            )
        })?;
    }

    let path = endpoint.path().trim_end_matches('/');
    if !matches!(path, "" | "/" | "/v1") {
        anyhow::bail!(
            "invalid OpenAI-compatible endpoint `{raw_endpoint}`: base path must be `/` or `/v1`"
        );
    }
    endpoint.set_path("/v1");
    Ok(endpoint.as_str().trim_end_matches('/').to_owned())
}

fn normalize_loopback_host(host: &str) -> Result<String> {
    if host.eq_ignore_ascii_case("localhost") {
        // Do not delegate loopback enforcement to name resolution.
        return Ok("127.0.0.1".to_string());
    }
    let address = host
        .parse::<IpAddr>()
        .map_err(|_| anyhow::anyhow!("host `{host}` is not a literal loopback address"))?;
    if !address.is_loopback() {
        anyhow::bail!("host `{host}` is not loopback; remote endpoints are not allowed");
    }
    Ok(address.to_string())
}

fn valid_environment_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn load_api_key_from_environment(name: Option<&str>) -> Result<Option<String>> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    if !valid_environment_variable_name(name) {
        anyhow::bail!(
            "OpenAI-compatible API-key environment variable `{name}` is not a valid environment variable name"
        );
    }
    let key = std::env::var(name).with_context(|| {
        format!("OpenAI-compatible API-key environment variable `{name}` is not set")
    })?;
    if key.trim().is_empty() {
        anyhow::bail!("OpenAI-compatible API-key environment variable `{name}` is empty");
    }
    Ok(Some(key))
}

/// Provider for a separately-operated, local OpenAI-compatible server.
///
/// The bearer token is intentionally not exposed through `Debug`, accessors,
/// or errors. Most local deployments need no token; when one is required, use
/// [`Self::try_new_with_api_key_env`] and give it an environment-variable name.
#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    base_url: String,
    client: Client,
    api_key: Option<String>,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("base_url", &self.base_url)
            .field("has_api_key", &self.api_key.is_some())
            .finish()
    }
}

impl OpenAiCompatibleProvider {
    /// Construct a token-free local provider. Panics only for an invalid
    /// endpoint or a reqwest client initialization failure; callers reading
    /// user configuration should prefer [`Self::try_new`].
    pub fn new(base_url: impl AsRef<str>) -> Self {
        Self::try_new(base_url).unwrap_or_else(|error| {
            panic!("ollama-forge: could not initialize OpenAI-compatible provider: {error:#}")
        })
    }

    /// Construct a token-free local provider with a recoverable error.
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self> {
        Self::try_new_with_api_key_env(base_url, None)
    }

    /// Construct a local provider, reading a bearer token only from `api_key_env`.
    ///
    /// Passing `Some("MY_LOCAL_SERVER_TOKEN")` requires that exact environment
    /// variable to exist and be non-empty. The token is never accepted as a
    /// function argument, serialized, or included in an error message.
    pub fn try_new_with_api_key_env(
        base_url: impl AsRef<str>,
        api_key_env: Option<&str>,
    ) -> Result<Self> {
        let base_url = normalize_openai_compatible_endpoint(base_url.as_ref())?;
        let api_key = load_api_key_from_environment(api_key_env)?;
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .tcp_keepalive(Duration::from_secs(30))
            // The URL was constrained to loopback above. Never allow an
            // ambient HTTP(S)_PROXY setting to turn it into a network request.
            .no_proxy()
            // A local server must not be able to redirect a prompt, image, or
            // response body to another origin. `no_proxy` only controls proxy
            // routing; it does not stop HTTP 30x follow-ups.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build HTTP client for local OpenAI-compatible endpoint")?;
        Ok(Self {
            base_url,
            client,
            api_key,
        })
    }

    /// Normalized local `/v1` base URL. This contains no credentials.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_url(&self, path: &str) -> String {
        debug_assert!(path.starts_with('/'));
        format!("{}{path}", self.base_url)
    }

    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(api_key) => request.bearer_auth(api_key),
            None => request,
        }
    }

    fn build_generate_request(options: &GenerateOptions) -> Result<OpenAiCompletionRequest> {
        let mut messages = Vec::with_capacity(2);
        if let Some(system) = options.system.as_ref().filter(|system| !system.is_empty()) {
            messages.push(OpenAiMessage::text("system", system.clone()));
        }
        messages.push(OpenAiMessage::user_with_images(
            options.prompt.clone(),
            options.images.as_deref().unwrap_or_default(),
        )?);
        Ok(OpenAiCompletionRequest {
            model: options.model.clone(),
            messages,
            temperature: options.temperature,
            top_p: options.top_p,
            stop: options.stop.clone(),
            // This provider method returns one complete response. Streaming is
            // a future trait extension; emitting `true` here would require an
            // NDJSON/SSE parser and violate the LlmProvider response contract.
            stream: false,
            response_format: response_format(options.format.as_ref())?,
        })
    }

    fn build_chat_request(options: &ChatOptions) -> OpenAiCompletionRequest {
        OpenAiCompletionRequest {
            model: options.model.clone(),
            messages: options
                .messages
                .iter()
                .map(|message| OpenAiMessage::text(message.role.clone(), message.content.clone()))
                .collect(),
            temperature: options.temperature,
            top_p: options.top_p,
            stop: None,
            stream: false,
            response_format: None,
        }
    }

    async fn completion(
        &self,
        request: OpenAiCompletionRequest,
        requested_model: &str,
    ) -> Result<LlmResponse> {
        let endpoint = self.api_url("/chat/completions");
        let started = Instant::now();
        let response = self
            .authenticated(self.client.post(&endpoint))
            .json(&request)
            .send()
            .await
            .map_err(|error| endpoint_error("send chat completion request", &endpoint, error))?;
        let response = require_success(
            response,
            "chat completion",
            &endpoint,
            self.api_key.as_deref(),
        )
        .await?;
        parse_completion_response(response, requested_model, started, &endpoint).await
    }
}

#[derive(Debug, Serialize)]
struct OpenAiCompletionRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    content: OpenAiMessageContent,
}

impl OpenAiMessage {
    fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: OpenAiMessageContent::Text(content.into()),
        }
    }

    fn user_with_images(prompt: String, images: &[String]) -> Result<Self> {
        if images.is_empty() {
            return Ok(Self::text("user", prompt));
        }
        let mut parts = Vec::with_capacity(images.len() + 1);
        // Keep the prompt as an explicit first part even when it is blank: a
        // number of OpenAI-compatible servers reject an image-only message.
        parts.push(OpenAiContentPart::Text { text: prompt });
        for image in images {
            parts.push(OpenAiContentPart::ImageUrl {
                image_url: OpenAiImageUrl {
                    url: validated_image_data_url(image)?,
                    detail: "auto",
                },
            });
        }
        Ok(Self {
            role: "user".to_string(),
            content: OpenAiMessageContent::Parts(parts),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OpenAiMessageContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OpenAiContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Debug, Serialize)]
struct OpenAiImageUrl {
    url: String,
    detail: &'static str,
}

fn response_format(format: Option<&Value>) -> Result<Option<Value>> {
    match format {
        None => Ok(None),
        Some(Value::String(kind)) if kind == "json" => Ok(Some(json!({"type": "json_object"}))),
        Some(Value::Object(schema)) => Ok(Some(json!({
            "type": "json_schema",
            "json_schema": {
                "name": "forge_response",
                "strict": true,
                "schema": schema,
            }
        }))),
        Some(other) => anyhow::bail!(
            "unsupported OpenAI-compatible response format `{other}`; use `\"json\"` or a JSON Schema object"
        ),
    }
}

/// Convert raw base64 image bytes into a data URL without accepting an
/// arbitrary `data:` or remote URL supplied by a caller.
fn validated_image_data_url(input: &str) -> Result<String> {
    let encoded = input.trim();
    if encoded.is_empty() {
        anyhow::bail!("attached image is empty");
    }
    if encoded.len() > MAX_IMAGE_BASE64_CHARS {
        anyhow::bail!(
            "attached image exceeds the {}-character local safety limit",
            MAX_IMAGE_BASE64_CHARS
        );
    }
    validate_standard_base64(encoded)?;
    let magic = decode_base64_prefix(encoded, 16)?;
    let mime = image_mime_from_magic(&magic).ok_or_else(|| {
        anyhow::anyhow!(
            "attached image must be a base64-encoded JPEG, PNG, GIF, or WebP raster image"
        )
    })?;
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn validate_standard_base64(encoded: &str) -> Result<()> {
    if encoded.len() % 4 != 0 {
        anyhow::bail!("attached image base64 must use standard padding");
    }
    let padding = if encoded.ends_with("==") {
        2
    } else if encoded.ends_with('=') {
        1
    } else {
        0
    };
    let data_length = encoded.len().saturating_sub(padding);
    if encoded[..data_length]
        .bytes()
        .any(|byte| base64_value(byte).is_none())
        || encoded[data_length..].bytes().any(|byte| byte != b'=')
    {
        anyhow::bail!("attached image is not standard base64 data");
    }
    if padding > 0 && data_length == 0 {
        anyhow::bail!("attached image base64 has invalid padding");
    }
    Ok(())
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode only enough bytes to identify a raster image. Full decoding is left
/// to the local inference server, avoiding a second in-memory copy here.
fn decode_base64_prefix(encoded: &str, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit);
    for chunk in encoded.as_bytes().chunks_exact(4) {
        let a = base64_value(chunk[0]).ok_or_else(|| anyhow::anyhow!("invalid base64"))?;
        let b = base64_value(chunk[1]).ok_or_else(|| anyhow::anyhow!("invalid base64"))?;
        if chunk[2] == b'=' && chunk[3] != b'=' {
            anyhow::bail!("invalid base64 padding");
        }
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2]).ok_or_else(|| anyhow::anyhow!("invalid base64"))?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3]).ok_or_else(|| anyhow::anyhow!("invalid base64"))?
        };
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
        if output.len() >= limit {
            output.truncate(limit);
            break;
        }
    }
    Ok(output)
}

fn image_mime_from_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn endpoint_error(operation: &str, endpoint: &str, error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{operation} at {endpoint}: {error:#}")
}

async fn require_success(
    response: Response,
    operation: &str,
    endpoint: &str,
    api_key: Option<&str>,
) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<could not read local server error body: {error:#}>"));
    anyhow::bail!(
        "OpenAI-compatible {operation} failed with {status} from {endpoint}: {}",
        truncate_error_body(&redact_api_key(&body, api_key))
    );
}

fn redact_api_key(body: &str, api_key: Option<&str>) -> String {
    match api_key.filter(|key| !key.is_empty()) {
        Some(key) => body.replace(key, "[redacted]"),
        None => body.to_string(),
    }
}

fn truncate_error_body(body: &str) -> String {
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        body.to_string()
    } else {
        format!(
            "{}… [truncated]",
            body.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>()
        )
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiCompletionResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiAssistantMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiAssistantMessage {
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
}

async fn parse_completion_response(
    response: Response,
    requested_model: &str,
    started: Instant,
    endpoint: &str,
) -> Result<LlmResponse> {
    let body = response
        .text()
        .await
        .map_err(|error| endpoint_error("read chat completion response", endpoint, error))?;
    let parsed: OpenAiCompletionResponse = serde_json::from_str(&body)
        .map_err(|error| endpoint_error("parse chat completion response", endpoint, error))?;
    let choice = parsed.choices.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!("OpenAI-compatible chat completion from {endpoint} had no choices")
    })?;
    let content = extract_assistant_content(choice.message.content)?;
    let usage = parsed.usage.unwrap_or(OpenAiUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
    });
    Ok(LlmResponse {
        content,
        model: parsed
            .model
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| requested_model.to_string()),
        tokens_generated: usage.completion_tokens,
        context_used: usage.prompt_tokens,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn extract_assistant_content(content: Option<Value>) -> Result<String> {
    match content {
        Some(Value::String(text)) => Ok(text),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>();
            if text.is_empty() {
                anyhow::bail!(
                    "OpenAI-compatible completion returned content parts without text output"
                );
            }
            Ok(text)
        }
        Some(Value::Null) | None => anyhow::bail!(
            "OpenAI-compatible completion returned no assistant text (tool calls are not supported by this provider)"
        ),
        Some(_) => anyhow::bail!(
            "OpenAI-compatible completion returned assistant content in an unsupported format"
        ),
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    created: Option<i64>,
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible-local"
    }

    async fn generate(&self, options: GenerateOptions) -> Result<LlmResponse> {
        let requested_model = options.model.clone();
        let request = Self::build_generate_request(&options)?;
        self.completion(request, &requested_model).await
    }

    async fn chat(&self, options: ChatOptions) -> Result<LlmResponse> {
        let requested_model = options.model.clone();
        let request = Self::build_chat_request(&options);
        self.completion(request, &requested_model).await
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let endpoint = self.api_url("/models");
        let response = self
            .authenticated(self.client.get(&endpoint))
            .send()
            .await
            .map_err(|error| endpoint_error("list models", &endpoint, error))?;
        let response =
            require_success(response, "model list", &endpoint, self.api_key.as_deref()).await?;
        let body = response
            .text()
            .await
            .map_err(|error| endpoint_error("read model list response", &endpoint, error))?;
        let parsed: OpenAiModelsResponse = serde_json::from_str(&body)
            .map_err(|error| endpoint_error("parse model list response", &endpoint, error))?;
        Ok(parsed
            .data
            .into_iter()
            .filter(|model| !model.id.trim().is_empty())
            .map(|model| ModelInfo {
                name: model.id,
                // The OpenAI `/v1/models` response does not include resident
                // size. Do not invent a VRAM number for a separately-managed
                // inference server.
                size: 0,
                size_human: "server-managed".to_string(),
                modified_at: model
                    .created
                    .map(|created| created.to_string())
                    .unwrap_or_default(),
                digest: String::new(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_openai_compatible_endpoint, OpenAiCompatibleProvider, MAX_IMAGE_BASE64_CHARS,
    };
    use crate::providers::{ChatMessage, ChatOptions, GenerateOptions, LlmProvider};
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn endpoint_normalization_is_loopback_only_and_canonicalizes_v1() {
        assert_eq!(
            normalize_openai_compatible_endpoint("localhost:8000/").unwrap(),
            "http://127.0.0.1:8000/v1"
        );
        assert_eq!(
            normalize_openai_compatible_endpoint("http://127.0.0.1:8000/v1///").unwrap(),
            "http://127.0.0.1:8000/v1"
        );
        assert_eq!(
            normalize_openai_compatible_endpoint("https://[::1]:8443").unwrap(),
            "https://[::1]:8443/v1"
        );

        for invalid in [
            "",
            "ftp://127.0.0.1:8000",
            "http://0.0.0.0:8000",
            "http://192.168.1.10:8000",
            "http://example.test:8000",
            "http://127.0.0.1:8000/v1/chat/completions",
            "http://127.0.0.1:8000/v1?token=nope",
            "http://user:secret@127.0.0.1:8000",
        ] {
            assert!(
                normalize_openai_compatible_endpoint(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn image_parts_accept_only_safe_raster_base64() {
        let request = OpenAiCompatibleProvider::build_generate_request(&GenerateOptions {
            prompt: "inspect the screenshot".to_string(),
            images: Some(vec!["iVBORw0KGgo=".to_string()]), // PNG signature
            ..Default::default()
        })
        .unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["messages"][0]["content"][0]["type"], "text");
        assert_eq!(value["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(
            value["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );

        let error = OpenAiCompatibleProvider::build_generate_request(&GenerateOptions {
            images: Some(vec!["https://example.test/secret.png".to_string()]),
            ..Default::default()
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("base64"));

        let too_large = "A".repeat(MAX_IMAGE_BASE64_CHARS + 1);
        let error = OpenAiCompatibleProvider::build_generate_request(&GenerateOptions {
            images: Some(vec![too_large]),
            ..Default::default()
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("safety limit"));
    }

    #[tokio::test]
    async fn list_models_uses_v1_loopback_and_env_only_bearer_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, body) = read_http_request(&mut socket).await;
            assert!(head.starts_with("GET /v1/models HTTP/1.1\r\n"), "{head}");
            assert!(body.is_empty());
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer local-test-secret\r\n"),
                "authorization header missing: {head}"
            );
            write_json_response(
                &mut socket,
                200,
                &json!({"data":[{"id":"MiniMax-M3","created":1730000000}]}),
            )
            .await;
        });

        let env_name = format!(
            "OLLAMAX_OPENAI_PROVIDER_TEST_{}",
            uuid::Uuid::new_v4().simple()
        );
        std::env::set_var(&env_name, "local-test-secret");
        let provider = OpenAiCompatibleProvider::try_new_with_api_key_env(
            format!("http://localhost:{port}"),
            Some(&env_name),
        )
        .unwrap();
        let models = provider.list_models().await.unwrap();
        std::env::remove_var(&env_name);

        assert_eq!(provider.base_url(), format!("http://127.0.0.1:{port}/v1"));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "MiniMax-M3");
        assert_eq!(models[0].size, 0);
        assert_eq!(models[0].size_human, "server-managed");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn generate_posts_openai_message_parts_and_parses_usage() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, body) = read_http_request(&mut socket).await;
            assert!(
                head.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
                "{head}"
            );
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["model"], "DeepSeek-V4-Flash");
            assert_eq!(request["stream"], false);
            assert_eq!(request["temperature"], 0.2);
            assert_eq!(request["top_p"], 0.8);
            assert_eq!(request["response_format"]["type"], "json_object");
            assert_eq!(request["messages"][0]["role"], "system");
            assert_eq!(request["messages"][1]["content"][0]["type"], "text");
            assert_eq!(request["messages"][1]["content"][1]["type"], "image_url");
            assert!(request.get("top_k").is_none());
            write_json_response(
                &mut socket,
                200,
                &json!({
                    "model":"DeepSeek-V4-Flash",
                    "choices":[{"message":{"role":"assistant","content":r#"{"action":"answer"}"#}}],
                    "usage":{"prompt_tokens":17,"completion_tokens":5}
                }),
            )
            .await;
        });

        let provider = OpenAiCompatibleProvider::try_new(format!("127.0.0.1:{port}/v1")).unwrap();
        let response = provider
            .generate(GenerateOptions {
                model: "DeepSeek-V4-Flash".to_string(),
                prompt: "Look at this".to_string(),
                system: Some("Return only JSON".to_string()),
                temperature: Some(0.2),
                top_p: Some(0.8),
                format: Some(json!("json")),
                images: Some(vec!["iVBORw0KGgo=".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(response.model, "DeepSeek-V4-Flash");
        assert_eq!(response.content, r#"{"action":"answer"}"#);
        assert_eq!(response.context_used, 17);
        assert_eq!(response.tokens_generated, 5);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn chat_preserves_messages_and_http_errors_do_not_expose_api_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (_head, body) = read_http_request(&mut socket).await;
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["messages"][0]["role"], "user");
            assert_eq!(request["messages"][0]["content"], "hello");
            write_json_response(
                &mut socket,
                401,
                &json!({"error":{"message":"local server rejected this request: must-not-appear-in-error"}}),
            )
            .await;
        });

        let env_name = format!(
            "OLLAMAX_OPENAI_PROVIDER_TEST_{}",
            uuid::Uuid::new_v4().simple()
        );
        std::env::set_var(&env_name, "must-not-appear-in-error");
        let provider = OpenAiCompatibleProvider::try_new_with_api_key_env(
            format!("127.0.0.1:{port}"),
            Some(&env_name),
        )
        .unwrap();
        let error = provider
            .chat(ChatOptions {
                model: "local-model".to_string(),
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
                temperature: None,
                top_p: None,
                num_ctx: None,
                stream: false,
            })
            .await
            .unwrap_err();
        std::env::remove_var(&env_name);

        let rendered = format!("{error:#}");
        assert!(rendered.contains("401"));
        assert!(rendered.contains("local server rejected this request"));
        assert!(!rendered.contains("must-not-appear-in-error"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn loopback_provider_never_follows_a_redirect_to_another_origin() {
        let redirect_target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = redirect_target.local_addr().unwrap().port();
        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector_port = redirector.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = redirector.accept().await.unwrap();
            let (_head, _body) = read_http_request(&mut socket).await;
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{target_port}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let provider =
            OpenAiCompatibleProvider::try_new(format!("127.0.0.1:{redirector_port}")).unwrap();
        let error = provider
            .generate(GenerateOptions {
                model: "local-model".to_string(),
                prompt: "do not forward this prompt".to_string(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("307"));
        // If reqwest followed the Location, this accept would resolve. The
        // timeout proves the original loopback request did not get replayed.
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(150),
            redirect_target.accept()
        )
        .await
        .is_err());
        server.await.unwrap();
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let header_end = loop {
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "client closed before completing the request");
            bytes.extend_from_slice(&chunk[..count]);
        };
        let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = head
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length: ")
                    .or_else(|| line.strip_prefix("Content-Length: "))
            })
            .and_then(|length| length.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "client closed before completing request body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        (
            head,
            bytes[header_end..header_end + content_length].to_vec(),
        )
    }

    async fn write_json_response(socket: &mut tokio::net::TcpStream, status: u16, value: &Value) {
        let body = serde_json::to_string(value).unwrap();
        let status_text = if status == 200 { "OK" } else { "Unauthorized" };
        let response = format!(
            "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

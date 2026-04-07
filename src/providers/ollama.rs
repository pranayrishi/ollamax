use super::{ChatOptions, GenerateOptions, LlmProvider, LlmResponse, ModelInfo, RunningModel};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::{debug, error};

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    base_url: String,
    client: Client,
}

#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<GenerateOptionsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<Vec<i32>>,
    /// Ollama's keep_alive — accepts "30m", "1h", "0", or seconds as a number.
    /// We always send it because the server default (5m) bites every model switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
    /// Ollama `format` parameter (v0.5+). Either the literal string `"json"`
    /// for free-form valid JSON, or a full JSON Schema document for
    /// schema-constrained decoding. We pass it through as raw JSON so
    /// callers can hand us either form.
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct GenerateOptionsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_gpu: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    main_gpu: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessageDto>,
    stream: bool,
    options: Option<ChatOptionsDto>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessageDto {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatOptionsDto {
    temperature: Option<f32>,
    top_p: Option<f32>,
    num_ctx: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `context`/`total_duration` arrive from Ollama; surfaced in Debug only
struct GenerateResponse {
    response: String,
    model: String,
    done: bool,
    context: Option<Vec<i32>>,
    total_duration: Option<u64>,
    eval_count: Option<usize>,
    prompt_eval_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `total_duration` arrives from Ollama; surfaced in Debug only
struct ChatResponse {
    message: ChatMessageDto,
    model: String,
    done: bool,
    total_duration: Option<u64>,
    eval_count: Option<usize>,
    prompt_eval_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelDto>,
}

#[derive(Debug, Deserialize)]
struct ModelDto {
    name: String,
    size: u64,
    modified_at: String,
    digest: String,
}

/// `/api/ps` response. Lists models currently loaded into VRAM/RAM.
#[derive(Debug, Deserialize)]
struct PsResponse {
    models: Vec<PsModelDto>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PsModelDto {
    name: String,
    model: String,
    size: u64,
    /// Bytes of *VRAM* this model is consuming. May be 0 on CPU-only.
    size_vram: Option<u64>,
    digest: String,
    expires_at: Option<String>,
}

impl OllamaProvider {
    /// Build a provider against `base_url`. Panics only if `reqwest` itself
    /// can't construct a TLS-enabled client — which on a healthy install
    /// effectively never happens. The previous `.expect()` was the same
    /// behavior; this version surfaces a clearer message if it ever does.
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|e| panic!("ollama-forge: could not build HTTP client (reqwest): {e}"));

        Self {
            base_url: base_url.into(),
            client,
        }
    }

    /// Fallible variant — for callers (libraries, tests) that don't want to
    /// take down the process on a TLS-init failure.
    pub fn try_new(base_url: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    /// Stream a generate request, calling `on_token` with each text chunk as
    /// it arrives. Returns the total number of bytes streamed. Used by
    /// `forge chat` so users see tokens flow in instead of waiting 20s for a
    /// buffered blob.
    ///
    /// Implementation note: we use `Response::chunk()` rather than
    /// `bytes_stream()` to avoid pulling in the `futures` crate just for
    /// `StreamExt`. NDJSON line-buffering is done by hand because chunks
    /// can split a line in half.
    pub async fn generate_streaming<F>(
        &self,
        opts: GenerateOptions,
        mut on_token: F,
    ) -> Result<usize>
    where
        F: FnMut(&str),
    {
        let mut request = Self::build_generate_request(&opts);
        request.stream = true;

        let resp = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await
            .context("send streaming generate request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Ollama API error {status}: {body}\n\
                 Hint: is `ollama serve` running at {} and is `{}` pulled?",
                self.base_url,
                opts.model
            );
        }

        let mut response = resp;
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        let mut total = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .context("read streaming chunk from Ollama")?
        {
            buf.extend_from_slice(&chunk);
            // Drain complete lines from buf — Ollama emits one JSON object per line.
            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let trimmed = std::str::from_utf8(&line).map(|s| s.trim()).unwrap_or("");
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: GenerateResponse = match serde_json::from_str(trimmed) {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("skipping malformed NDJSON line: {e}");
                        continue;
                    }
                };
                if !parsed.response.is_empty() {
                    total += parsed.response.len();
                    on_token(&parsed.response);
                }
                if parsed.done {
                    return Ok(total);
                }
            }
        }
        Ok(total)
    }

    /// Look up a model's manifest digest. This is the SHA the replay log
    /// records — pinning it lets `forge replay` detect when the user has
    /// pulled a different version of the same tag.
    ///
    /// Strategy: query `/api/tags` (the list endpoint) and find the entry
    /// matching `model`. Older Ollama versions don't expose `digest` on
    /// `/api/show` reliably; the list endpoint has been stable since v0.1.x.
    /// Returns `None` (not an error) if Ollama is unreachable, so callers
    /// can fall back gracefully.
    pub async fn model_digest(&self, model: &str) -> Option<String> {
        let models = self.list_models().await.ok()?;
        models
            .into_iter()
            .find(|m| m.name == model)
            .map(|m| m.digest)
            .filter(|d| !d.is_empty())
    }

    /// Models currently resident in VRAM/RAM (Ollama `/api/ps`).
    /// Returns `(name, vram_bytes, expires_at)` for each loaded model.
    pub async fn running_models(&self) -> Result<Vec<RunningModel>> {
        let resp = self
            .client
            .get(format!("{}/api/ps", self.base_url))
            .send()
            .await
            .context("send /api/ps to ollama")?;
        if !resp.status().is_success() {
            anyhow::bail!("ollama /api/ps returned {}", resp.status());
        }
        let parsed: PsResponse = resp.json().await.context("parse /api/ps response")?;
        Ok(parsed
            .models
            .into_iter()
            .map(|m| RunningModel {
                name: m.name,
                size_vram_bytes: m.size_vram.unwrap_or(0),
                expires_at: m.expires_at,
            })
            .collect())
    }

    pub async fn health_check(&self) -> Result<bool> {
        match self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
        {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(e) => {
                error!("Ollama health check failed: {}", e);
                Ok(false)
            }
        }
    }

    fn build_generate_request(opts: &GenerateOptions) -> GenerateRequest {
        let options = GenerateOptionsDto {
            temperature: opts.temperature,
            top_p: opts.top_p,
            top_k: opts.top_k,
            num_ctx: opts.num_ctx,
            num_gpu: opts.num_gpu,
            main_gpu: opts.main_gpu,
            repeat_penalty: opts.repeat_penalty,
            stop: opts.stop.clone(),
            seed: opts.seed,
        };

        GenerateRequest {
            model: opts.model.clone(),
            prompt: opts.prompt.clone(),
            system: opts.system.clone(),
            stream: opts.stream,
            options: Some(options).filter(|o| {
                o.temperature.is_some()
                    || o.top_p.is_some()
                    || o.top_k.is_some()
                    || o.num_ctx.is_some()
                    || o.num_gpu.is_some()
                    || o.main_gpu.is_some()
                    || o.repeat_penalty.is_some()
                    || o.stop.is_some()
                    || o.seed.is_some()
            }),
            context: None,
            keep_alive: opts.keep_alive.clone(),
            format: opts.format.clone(),
        }
    }

    fn build_chat_request(opts: &ChatOptions) -> ChatRequest {
        let options = ChatOptionsDto {
            temperature: opts.temperature,
            top_p: opts.top_p,
            num_ctx: opts.num_ctx,
        };

        ChatRequest {
            model: opts.model.clone(),
            messages: opts
                .messages
                .iter()
                .map(|m| ChatMessageDto {
                    role: m.role.clone(),
                    content: m.content.clone(),
                })
                .collect(),
            stream: opts.stream,
            options: Some(options)
                .filter(|o| o.temperature.is_some() || o.top_p.is_some() || o.num_ctx.is_some()),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn generate(&self, opts: GenerateOptions) -> Result<LlmResponse> {
        let start = Instant::now();
        let stream = opts.stream;
        let request = Self::build_generate_request(&opts);

        debug!("Generating with model: {} (stream={})", opts.model, stream);

        let response = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send generate request to Ollama")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Ollama returned error: {} - {}", status, body);
            anyhow::bail!(
                "Ollama API error {}: {}\n\
                Hint: is `ollama serve` running at {} and is the model `{}` pulled? \
                Try `ollama list` and `ollama pull {}`.",
                status,
                body,
                self.base_url,
                opts.model,
                opts.model
            );
        }

        // Ollama returns NDJSON when stream=true (one JSON object per line) and a
        // single JSON document when stream=false. `.json()` only handles the latter.
        let body = response
            .text()
            .await
            .context("Failed to read Ollama response body")?;
        let (content, model, eval_count, prompt_eval_count) = if stream {
            let mut buf = String::new();
            let mut last_model = opts.model.clone();
            let mut last_eval = None;
            let mut last_prompt_eval = None;
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                let chunk: GenerateResponse = serde_json::from_str(line)
                    .with_context(|| format!("Failed to parse NDJSON chunk: {}", line))?;
                buf.push_str(&chunk.response);
                last_model = chunk.model;
                if chunk.done {
                    last_eval = chunk.eval_count;
                    last_prompt_eval = chunk.prompt_eval_count;
                }
            }
            (buf, last_model, last_eval, last_prompt_eval)
        } else {
            let result: GenerateResponse =
                serde_json::from_str(&body).context("Failed to parse Ollama JSON response")?;
            (
                result.response,
                result.model,
                result.eval_count,
                result.prompt_eval_count,
            )
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(LlmResponse {
            content,
            model,
            tokens_generated: eval_count.unwrap_or(0),
            context_used: prompt_eval_count.unwrap_or(0),
            duration_ms,
        })
    }

    async fn chat(&self, opts: ChatOptions) -> Result<LlmResponse> {
        let start = Instant::now();
        let stream = opts.stream;
        let request = Self::build_chat_request(&opts);

        debug!("Chatting with model: {} (stream={})", opts.model, stream);

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send chat request to Ollama")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Ollama returned error: {} - {}", status, body);
            anyhow::bail!(
                "Ollama API error {}: {}\n\
                Hint: is `ollama serve` running at {} and is the model `{}` pulled?",
                status,
                body,
                self.base_url,
                opts.model
            );
        }

        let body = response
            .text()
            .await
            .context("Failed to read Ollama response body")?;
        let (content, model, eval_count, prompt_eval_count) = if stream {
            let mut buf = String::new();
            let mut last_model = opts.model.clone();
            let mut last_eval = None;
            let mut last_prompt_eval = None;
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                let chunk: ChatResponse = serde_json::from_str(line)
                    .with_context(|| format!("Failed to parse NDJSON chunk: {}", line))?;
                buf.push_str(&chunk.message.content);
                last_model = chunk.model;
                if chunk.done {
                    last_eval = chunk.eval_count;
                    last_prompt_eval = chunk.prompt_eval_count;
                }
            }
            (buf, last_model, last_eval, last_prompt_eval)
        } else {
            let result: ChatResponse =
                serde_json::from_str(&body).context("Failed to parse Ollama JSON response")?;
            (
                result.message.content,
                result.model,
                result.eval_count,
                result.prompt_eval_count,
            )
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(LlmResponse {
            content,
            model,
            tokens_generated: eval_count.unwrap_or(0),
            context_used: prompt_eval_count.unwrap_or(0),
            duration_ms,
        })
    }

    async fn preload(&self, model: &str, keep_alive: &str) -> Result<()> {
        // Empty prompt + non-zero keep_alive = warm-load only.
        // See ollama/ollama docs: /api/generate with empty prompt loads the model.
        let req = GenerateRequest {
            model: model.to_string(),
            prompt: String::new(),
            system: None,
            stream: false,
            options: None,
            context: None,
            keep_alive: Some(keep_alive.to_string()),
            format: None,
        };
        // Per-call timeout: a 70B cold-load can legitimately take 60-90s,
        // so we go with 120s. The default client-wide timeout is 300s
        // which is too generous for the "model isn't installed and ollama
        // hangs trying to pull" failure mode.
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            self.client
                .post(format!("{}/api/generate", self.base_url))
                .json(&req)
                .send(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "preload of `{model}` timed out after 120s. \
                 Is the model already pulled? Try `ollama pull {model}` first."
            )
        })?
        .context("Failed to send preload request to Ollama")?;
        if !resp.status().is_success() {
            anyhow::bail!("Ollama preload of `{}` failed: {}", model, resp.status());
        }
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .context("Failed to list models from Ollama")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to list models: {}", response.status());
        }

        let result: ModelsResponse = response.json().await?;

        Ok(result
            .models
            .into_iter()
            .map(|m| ModelInfo {
                name: m.name,
                size: m.size,
                size_human: format_size(m.size),
                modified_at: m.modified_at,
                digest: m.digest,
            })
            .collect())
    }
}

fn format_size(bytes: u64) -> String {
    // Binary (1024-based) units, labeled correctly per IEC 80000-13.
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn format_size_uses_binary_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }
}

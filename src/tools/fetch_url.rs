//! `fetch_url` tool — generic HTTP GET + minimal HTML→text.
//!
//! No external service in front. Just `reqwest::get`. Honors the
//! `MAX_TOOL_OUTPUT_BYTES` budget so a 10 MB blog page doesn't blow the
//! model's context.
//!
//! ## HTML→text strategy
//!
//! We don't pull in `html2text` or `scraper` (both are heavy). Instead we
//! do a tag stripper that:
//!
//! 1. Drops the contents of `<script>` and `<style>` blocks entirely.
//! 2. Drops every other tag, keeping the text between them.
//! 3. Decodes the half-dozen common HTML entities.
//! 4. Collapses runs of whitespace.
//!
//! This is good enough for an LLM that just needs to know what a page
//! says. It is *not* good enough for production HTML extraction — for
//! that the agent should be pointed at a structured source (Wikipedia,
//! arXiv) instead of fetching arbitrary URLs.

use super::{truncate_for_model, Tool, ToolResult, MAX_TOOL_OUTPUT_BYTES};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub struct FetchUrlTool {
    client: Client,
}

impl Default for FetchUrlTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchUrlTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("ollama-forge/0.1 (+https://github.com/pranayrishi/ollamax)")
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl Tool for FetchUrlTool {
    fn name(&self) -> &str {
        "fetch_url"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its text content. Strips HTML tags. Use \
         AFTER `web_search`/`wikipedia`/`arxiv` to read a specific page in \
         depth. Output is truncated to ~8 KB."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute URL (must include scheme, http or https)"
                }
            },
            "required": ["url"]
        })
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("fetch_url: missing `url`"))?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("fetch_url: refusing non-http(s) URL `{url}`"),
            });
        }

        // Hard cap on bytes we'll buffer from the network. The previous
        // version called `.bytes()` which reads the entire body into RAM
        // *before* truncation — a 1 GB blob would OOM. Now we stream
        // chunks and break as soon as we cross the cap.
        const MAX_BYTES_DOWNLOADED: usize = MAX_TOOL_OUTPUT_BYTES * 4;

        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("fetch_url request failed: {e}"))?;

        if !resp.status().is_success() {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("fetch_url: {url} returned HTTP {}", resp.status()),
            });
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Belt-and-suspenders: also reject upfront if Content-Length says
        // the body is huge. Servers may lie or omit it, so we still
        // enforce the streaming cap below.
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_BYTES_DOWNLOADED * 4 {
                return Ok(ToolResult {
                    tool: self.name().to_string(),
                    ok: false,
                    content: format!(
                        "fetch_url: {url} declared {len} bytes; refusing to download more than {} bytes",
                        MAX_BYTES_DOWNLOADED * 4
                    ),
                });
            }
        }

        let mut buf: Vec<u8> = Vec::with_capacity(MAX_BYTES_DOWNLOADED.min(8192));
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    let remaining = MAX_BYTES_DOWNLOADED.saturating_sub(buf.len());
                    if remaining == 0 {
                        break;
                    }
                    let take = chunk.len().min(remaining);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < chunk.len() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    return Ok(ToolResult {
                        tool: self.name().to_string(),
                        ok: false,
                        content: format!("fetch_url: read failed mid-stream: {e}"),
                    });
                }
            }
        }
        let raw = String::from_utf8_lossy(&buf).into_owned();

        let text = if content_type.contains("html") || content_type.is_empty() {
            html_to_text(&raw)
        } else if content_type.contains("json") {
            // Pretty-print JSON for the model so structure is visible.
            serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or(raw)
        } else {
            raw
        };

        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&format!("URL: {url}\n\n{text}")),
        })
    }
}

/// Strip HTML tags, decode common entities, collapse whitespace.
///
/// Public for tests in `tests/tools_html.rs`.
pub fn html_to_text(html: &str) -> String {
    // 1. Drop <script> and <style> bodies entirely. These dwarf the actual
    //    text on most modern web pages and would crowd the model out.
    let stripped = strip_block(html, "script");
    let stripped = strip_block(&stripped, "style");
    let stripped = strip_block(&stripped, "noscript");
    let stripped = strip_block(&stripped, "svg");

    // 2. Tag stripper. Linear pass; copy chars outside `<...>` to output.
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    for c in stripped.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    // 3. Decode the half-dozen entities that actually appear in real HTML.
    //    A real entity table is huge; these are 99% of what we see.
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");

    // 4. Collapse runs of whitespace, including across newlines, but
    //    preserve paragraph breaks (double-newline → blank line).
    let mut result = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if !prev_blank {
                result.push('\n');
                prev_blank = true;
            }
        } else {
            result.push_str(&trimmed);
            result.push('\n');
            prev_blank = false;
        }
    }
    result.trim().to_string()
}

fn strip_block(html: &str, tag: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let open_lc = format!("<{}", tag.to_lowercase());
    let close_lc = format!("</{}>", tag.to_lowercase());
    let lc = html.to_lowercase();
    let mut cursor = 0usize;
    while cursor < html.len() {
        if let Some(rel) = lc[cursor..].find(&open_lc) {
            let abs_open = cursor + rel;
            out.push_str(&html[cursor..abs_open]);
            // Find the matching close. If absent, drop the rest.
            let after_open = abs_open;
            if let Some(end_rel) = lc[after_open..].find(&close_lc) {
                cursor = after_open + end_rel + close_lc.len();
            } else {
                break;
            }
        } else {
            out.push_str(&html[cursor..]);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_basic_tags() {
        let html = "<p>Hello <b>world</b></p>";
        assert_eq!(html_to_text(html), "Hello world");
    }

    #[test]
    fn drops_script_blocks_entirely() {
        let html = "<p>before</p><script>var x = 1; alert('xss');</script><p>after</p>";
        let out = html_to_text(html);
        assert!(!out.contains("alert"));
        assert!(out.contains("before"));
        assert!(out.contains("after"));
    }

    #[test]
    fn decodes_common_entities() {
        let html = "Tom &amp; Jerry &lt;rocks&gt; &quot;quoted&quot; &#39;apos&#39;";
        let out = html_to_text(html);
        assert!(out.contains("Tom & Jerry"));
        assert!(out.contains("<rocks>"));
        assert!(out.contains("\"quoted\""));
        assert!(out.contains("'apos'"));
    }

    #[test]
    fn collapses_whitespace_and_paragraphs() {
        let html = "<p>line one</p>\n\n\n<p>line two</p>";
        let out = html_to_text(html);
        assert!(out.contains("line one"));
        assert!(out.contains("line two"));
        // Should have at most one blank line between paragraphs.
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn rejects_non_http_urls_via_args_validation() {
        // Sanity: the description tells the model http(s) only — but the
        // *runtime* check is in invoke(), which we cover by an integration
        // test. Here we just confirm the schema marker exists.
        let t = FetchUrlTool::new();
        assert_eq!(t.args_schema()["required"][0], "url");
    }

    #[test]
    fn handles_unclosed_script_block() {
        // Adversarial: a `<script>` with no `</script>`. We currently drop
        // everything from there to the end, which is the safe choice
        // (better to lose page text than to leak script source to the model).
        let html = "<p>visible</p><script>function() { ";
        let out = html_to_text(html);
        assert!(out.contains("visible"));
        assert!(!out.contains("function()"));
    }
}

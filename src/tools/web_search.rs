//! `web_search` tool — DuckDuckGo Instant Answer JSON API.
//!
//! Free, no API key, public endpoint at `https://api.duckduckgo.com/`.
//! Returns an `Abstract` (when DDG has a Wikipedia-style answer for the
//! query), a list of `RelatedTopics`, and the source URL. Limited compared
//! to a real search engine but stable, fast, and never gets us blocked.
//!
//! For deeper research the agent should chain: `web_search` to find a URL,
//! then `fetch_url` to read it, or `wikipedia`/`arxiv` for richer hits.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub struct WebSearchTool {
    client: Client,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("ollama-forge/0.1 (+https://github.com/pranayrishi/ollamax)")
                .build()
                .expect("reqwest client"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DdgResponse {
    #[serde(rename = "Heading")]
    heading: Option<String>,
    #[serde(rename = "Abstract")]
    abstract_text: Option<String>,
    #[serde(rename = "AbstractURL")]
    abstract_url: Option<String>,
    #[serde(rename = "AbstractSource")]
    abstract_source: Option<String>,
    #[serde(rename = "RelatedTopics")]
    related_topics: Option<Vec<RelatedTopic>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RelatedTopic {
    Single {
        #[serde(rename = "Text")]
        text: Option<String>,
        #[serde(rename = "FirstURL")]
        first_url: Option<String>,
    },
    /// DDG sometimes nests topics under category headings.
    Group {
        #[serde(rename = "Topics")]
        #[allow(dead_code)]
        topics: Option<Vec<RelatedTopic>>,
    },
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the public web (DuckDuckGo Instant Answer API). Use for \
         general factual queries, definitions, and to discover URLs. \
         Returns an abstract paragraph and up to 8 related links."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Plain English search query"
                }
            },
            "required": ["query"]
        })
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("web_search: missing `query`"))?;

        let url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_redirect=1&no_html=1",
            urlencoding_minimal(query)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ddg request failed: {e}"))?;
        if !resp.status().is_success() {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("DDG returned HTTP {}", resp.status()),
            });
        }

        let parsed: DdgResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ddg JSON parse failed: {e}"))?;

        let mut out = String::new();
        out.push_str(&format!("Search: {query}\n"));
        if let Some(h) = parsed.heading.as_deref() {
            if !h.is_empty() {
                out.push_str(&format!("Heading: {h}\n"));
            }
        }
        if let Some(abs) = parsed.abstract_text.as_deref() {
            if !abs.is_empty() {
                out.push_str(&format!("\nAbstract: {abs}\n"));
                if let Some(src) = parsed.abstract_source.as_deref() {
                    out.push_str(&format!("Source: {src}\n"));
                }
                if let Some(u) = parsed.abstract_url.as_deref() {
                    out.push_str(&format!("Source URL: {u}\n"));
                }
            }
        }

        if let Some(topics) = parsed.related_topics {
            let mut count = 0usize;
            out.push_str("\nRelated:\n");
            for t in topics {
                if count >= 8 {
                    break;
                }
                if let RelatedTopic::Single { text, first_url } = t {
                    let (Some(text), Some(url)) = (text, first_url) else {
                        continue;
                    };
                    out.push_str(&format!("- {text}\n  ({url})\n"));
                    count += 1;
                }
            }
            if count == 0 {
                out.push_str("(none)\n");
            }
        }

        if out.lines().count() <= 2 {
            // DDG returned essentially nothing — be honest with the model.
            out.push_str(
                "\nDDG returned no abstract or related topics for this query. \
                 Try rephrasing or use the `wikipedia`/`arxiv`/`fetch_url` tool instead.\n",
            );
        }

        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&out),
        })
    }
}

/// Tiny percent-encoder for query strings. Pulling in the `urlencoding`
/// crate just for this would be silly; the rules are dead simple.
fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_handles_spaces_and_unicode() {
        assert_eq!(urlencoding_minimal("hello world"), "hello+world");
        assert_eq!(urlencoding_minimal("a+b"), "a%2Bb");
        assert_eq!(urlencoding_minimal("café"), "caf%C3%A9");
    }

    #[test]
    fn schema_has_required_query() {
        let t = WebSearchTool::new();
        let s = t.args_schema();
        assert_eq!(s["required"][0], "query");
    }
}

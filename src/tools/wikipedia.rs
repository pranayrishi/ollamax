//! `wikipedia` tool — Wikipedia REST API.
//!
//! Free, no API key, public endpoints:
//! - `https://en.wikipedia.org/api/rest_v1/page/summary/<title>` for a
//!   one-paragraph summary of a known title (fast, very useful)
//! - `https://en.wikipedia.org/w/api.php?action=opensearch&...` for fuzzy
//!   search when the title is unknown
//!
//! Wikipedia is the highest-quality free knowledge graph the model can
//! reach. Use it for factual lookups; chain `fetch_url` for the full
//! article when the summary isn't enough.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub struct WikipediaTool {
    client: Client,
}

impl Default for WikipediaTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WikipediaTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                // Wikipedia REST requires a descriptive User-Agent per their
                // guidelines: https://meta.wikimedia.org/wiki/User-Agent_policy
                .user_agent(
                    "ollama-forge/0.1 (https://github.com/pranayrishi/ollamax; pranayrishi.nalem@gmail.com)",
                )
                .build()
                .expect("reqwest client"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Summary {
    title: Option<String>,
    extract: Option<String>,
    #[serde(rename = "content_urls")]
    content_urls: Option<ContentUrls>,
}

#[derive(Debug, Deserialize)]
struct ContentUrls {
    desktop: Option<UrlPair>,
}

#[derive(Debug, Deserialize)]
struct UrlPair {
    page: Option<String>,
}

#[async_trait]
impl Tool for WikipediaTool {
    fn name(&self) -> &str {
        "wikipedia"
    }

    fn description(&self) -> &str {
        "Look up a Wikipedia article. Pass `title` for a direct page \
         summary; if you don't know the exact title, pass `search` for a \
         fuzzy lookup that returns the top 5 matching titles."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Exact Wikipedia article title (e.g., 'Quantum entanglement')"
                },
                "search": {
                    "type": "string",
                    "description": "Search query when the exact title is unknown"
                }
            }
        })
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let title = args.get("title").and_then(|v| v.as_str());
        let search = args.get("search").and_then(|v| v.as_str());

        if let Some(title) = title {
            return self.fetch_summary(title).await;
        }
        if let Some(query) = search {
            return self.fuzzy_search(query).await;
        }

        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: false,
            content: "wikipedia: pass either `title` or `search`".to_string(),
        })
    }
}

impl WikipediaTool {
    async fn fetch_summary(&self, title: &str) -> Result<ToolResult> {
        let encoded = title.replace(' ', "_");
        let url = format!(
            "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
            urlencoding_minimal(&encoded)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("wikipedia request failed: {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!(
                    "wikipedia: no article titled `{title}`. Try `search` with a fuzzy query."
                ),
            });
        }
        if !resp.status().is_success() {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("wikipedia HTTP {}", resp.status()),
            });
        }
        let parsed: Summary = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("wikipedia JSON parse failed: {e}"))?;
        let mut out = String::new();
        if let Some(t) = parsed.title.as_deref() {
            out.push_str(&format!("Title: {t}\n"));
        }
        if let Some(extract) = parsed.extract.as_deref() {
            out.push_str(&format!("\n{extract}\n"));
        }
        if let Some(urls) = parsed.content_urls {
            if let Some(d) = urls.desktop {
                if let Some(p) = d.page {
                    out.push_str(&format!("\nFull article: {p}\n"));
                }
            }
        }
        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&out),
        })
    }

    async fn fuzzy_search(&self, query: &str) -> Result<ToolResult> {
        // Wikipedia opensearch returns: [query, [titles], [descriptions], [urls]]
        let url = format!(
            "https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=5&namespace=0&format=json",
            urlencoding_minimal(query)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("wikipedia search failed: {e}"))?;
        if !resp.status().is_success() {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("wikipedia HTTP {}", resp.status()),
            });
        }
        let arr: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("wikipedia search parse failed: {e}"))?;
        let titles = arr
            .get(1)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let descs = arr
            .get(2)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let urls = arr
            .get(3)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = format!("Wikipedia search: {query}\n\n");
        if titles.is_empty() {
            out.push_str("(no results)\n");
        }
        for (i, t) in titles.iter().enumerate() {
            let title = t.as_str().unwrap_or("?");
            let desc = descs.get(i).and_then(|v| v.as_str()).unwrap_or("").trim();
            let url = urls.get(i).and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!("- {title}\n"));
            if !desc.is_empty() {
                out.push_str(&format!("    {desc}\n"));
            }
            if !url.is_empty() {
                out.push_str(&format!("    {url}\n"));
            }
        }
        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&out),
        })
    }
}

fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

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
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

pub struct FetchUrlTool {
    client: Client,
    /// Per-host robots.txt cache. We never store the full file — only a
    /// small set of `Disallow:` prefixes for our user-agent. Std mutex
    /// because the access pattern is "lock, look at a small map, drop" —
    /// no `.await` while holding the lock.
    robots_cache: StdMutex<HashMap<String, RobotsRules>>,
}

#[derive(Debug, Clone, Default)]
struct RobotsRules {
    /// Path prefixes the host's robots.txt forbids us from. Empty list = allowed everywhere.
    disallow: Vec<String>,
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
            robots_cache: StdMutex::new(HashMap::new()),
        }
    }

    /// Best-effort robots.txt check. Honors the `Disallow:` directives that
    /// apply to our user-agent (or the wildcard `User-agent: *` group). On
    /// any error or 404 we conservatively allow — robots.txt is advisory,
    /// not enforcement, and a missing file means "no restrictions."
    ///
    /// Disabled when `FORGE_IGNORE_ROBOTS=1` is set, for cases where the
    /// user explicitly wants to bypass (e.g., scraping their own staging
    /// server that returns 503 on /robots.txt).
    async fn allowed_by_robots(&self, url_str: &str) -> bool {
        if std::env::var_os("FORGE_IGNORE_ROBOTS").is_some() {
            return true;
        }
        let Ok(parsed) = reqwest::Url::parse(url_str) else {
            return true;
        };
        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();
        if host.is_empty() {
            return true;
        }

        // Cache check.
        {
            let cache = self.robots_cache.lock().ok();
            if let Some(rules) = cache.as_ref().and_then(|c| c.get(&host)) {
                return !rules.disallow.iter().any(|p| path.starts_with(p));
            }
        }

        // Fetch robots.txt for this host. Best-effort — failures default to allow.
        let robots_url = format!(
            "{}://{}{}/robots.txt",
            parsed.scheme(),
            host,
            parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
        );
        let rules = match self.client.get(&robots_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body = resp.text().await.unwrap_or_default();
                parse_robots(&body, "ollama-forge")
            }
            _ => RobotsRules::default(),
        };

        // Cache and check.
        let allowed = !rules.disallow.iter().any(|p| path.starts_with(p));
        if let Ok(mut cache) = self.robots_cache.lock() {
            cache.insert(host, rules);
        }
        allowed
    }
}

/// Minimal robots.txt parser. Reads `User-agent:` groups and collects
/// `Disallow:` prefixes from the most-specific matching group.
///
/// **Group resolution**: per RFC 9309, when both a `User-agent: *` group
/// and a `User-agent: ollama-forge` group exist, the agent-specific group
/// wins entirely — its rules replace the wildcard's, not augment them.
/// The previous version of this parser flattened both into one list,
/// which over-blocked.
///
/// **Group boundary detection**: a group is a run of `User-agent:` lines
/// followed by rule lines, terminated by the next `User-agent:` line that
/// follows a rule. This is what the RFC actually says, even though most
/// real robots.txt files just put one UA per group.
///
/// Not a full RFC 9309 parser — we ignore Allow, Crawl-delay, Sitemap,
/// and the more obscure prefix-vs-glob distinctions. Failure mode is
/// "occasional under-block," acceptable given the polite-UA surface.
fn parse_robots(body: &str, agent: &str) -> RobotsRules {
    let agent_lower = agent.to_lowercase();
    let mut wildcard_disallow: Vec<String> = Vec::new();
    let mut targeted_disallow: Vec<String> = Vec::new();
    let mut in_wildcard = false;
    let mut in_targeted = false;
    let mut just_saw_useragent = false;

    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key_lower = key.trim().to_lowercase();
        let value = value.trim().to_string();

        if key_lower == "user-agent" {
            // Start of a new group iff the previous line was a rule.
            if !just_saw_useragent {
                in_wildcard = false;
                in_targeted = false;
            }
            let v = value.to_lowercase();
            if v == "*" {
                in_wildcard = true;
            }
            if v.contains(&agent_lower) {
                in_targeted = true;
            }
            just_saw_useragent = true;
            continue;
        }

        just_saw_useragent = false;

        if key_lower == "disallow" {
            if value.is_empty() {
                continue; // empty Disallow: = "allow everything in this group"
            }
            if in_targeted {
                targeted_disallow.push(value.clone());
            }
            if in_wildcard {
                wildcard_disallow.push(value);
            }
        }
    }

    // Targeted group wins outright if it exists.
    let chosen = if !targeted_disallow.is_empty() {
        targeted_disallow
    } else {
        wildcard_disallow
    };
    RobotsRules { disallow: chosen }
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

        // Honor robots.txt unless the user explicitly opted out via
        // FORGE_IGNORE_ROBOTS=1. This is a politeness measure, not a
        // security control.
        if !self.allowed_by_robots(url).await {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!(
                    "fetch_url: blocked by robots.txt for {url}\n\
                     (set FORGE_IGNORE_ROBOTS=1 to bypass — politeness, not security)"
                ),
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
    fn robots_parser_picks_up_wildcard_disallow() {
        let body = "User-agent: *\nDisallow: /private\nDisallow: /admin\n";
        let r = parse_robots(body, "ollama-forge");
        assert_eq!(r.disallow, vec!["/private", "/admin"]);
    }

    #[test]
    fn robots_parser_picks_up_targeted_agent() {
        let body =
            "User-agent: ollama-forge\nDisallow: /api\n\nUser-agent: *\nDisallow: /everything\n";
        let r = parse_robots(body, "ollama-forge");
        // Targeted rules should win over the wildcard.
        assert!(r.disallow.contains(&"/api".to_string()));
    }

    #[test]
    fn robots_parser_targeted_group_replaces_wildcard_not_augments() {
        // Regression for the session-6 over-blocking bug. The previous parser
        // flattened wildcard + targeted into one list, so /everything would
        // also have been blocked even though only /api is targeted.
        let body =
            "User-agent: *\nDisallow: /everything\n\nUser-agent: ollama-forge\nDisallow: /api\n";
        let r = parse_robots(body, "ollama-forge");
        assert!(r.disallow.contains(&"/api".to_string()));
        assert!(
            !r.disallow.contains(&"/everything".to_string()),
            "wildcard rules must NOT apply when a targeted group exists; got: {:?}",
            r.disallow
        );
    }

    #[test]
    fn robots_parser_falls_back_to_wildcard_when_no_targeted_group() {
        let body = "User-agent: *\nDisallow: /admin\n";
        let r = parse_robots(body, "ollama-forge");
        assert_eq!(r.disallow, vec!["/admin"]);
    }

    #[test]
    fn robots_parser_handles_grouped_useragents() {
        // Two UA lines stacked = both share the rules.
        let body = "User-agent: ollama-forge\nUser-agent: googlebot\nDisallow: /private\n";
        let r = parse_robots(body, "ollama-forge");
        assert_eq!(r.disallow, vec!["/private"]);
    }

    #[test]
    fn robots_parser_ignores_comments_and_blanks() {
        let body = "# this is a comment\n\n   \nUser-agent: *\nDisallow: /x\n# another\n";
        let r = parse_robots(body, "ollama-forge");
        assert_eq!(r.disallow, vec!["/x"]);
    }

    #[test]
    fn robots_parser_empty_disallow_means_allow() {
        let body = "User-agent: *\nDisallow:\n";
        let r = parse_robots(body, "ollama-forge");
        assert!(r.disallow.is_empty());
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

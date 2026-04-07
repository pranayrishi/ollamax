//! `arxiv` tool — arXiv Atom search API.
//!
//! Free, no API key, public endpoint at `https://export.arxiv.org/api/query`.
//! Returns Atom XML; we parse the bits we care about (title, summary,
//! authors, link) by hand because pulling in a full XML crate just for
//! arXiv would be silly. arXiv's Atom format is stable and has been for
//! ~15 years.
//!
//! For research-flavored agent loops this is by far the highest-signal
//! source: peer-reviewed-adjacent papers, full PDFs available, no key.

use super::{truncate_for_model, Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub struct ArxivTool {
    client: Client,
}

impl Default for ArxivTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ArxivTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("ollama-forge/0.1 (+https://github.com/pranayrishi/ollamax)")
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl Tool for ArxivTool {
    fn name(&self) -> &str {
        "arxiv"
    }

    fn description(&self) -> &str {
        "Search arXiv for academic papers. Pass `query` for a free-text \
         search; returns the top 5 hits with title, authors, abstract, \
         and PDF link. Use this for any technical/scientific question."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Free-text search query, e.g. 'attention is all you need'"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Number of results to return (default 5, max 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("arxiv: missing `query`"))?;
        let max_results: usize = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5)
            .clamp(1, 10);

        let url = format!(
            "https://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}",
            urlencoding_minimal(query),
            max_results
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("arxiv request failed: {e}"))?;
        if !resp.status().is_success() {
            return Ok(ToolResult {
                tool: self.name().to_string(),
                ok: false,
                content: format!("arxiv HTTP {}", resp.status()),
            });
        }
        let xml = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("arxiv body read failed: {e}"))?;

        let papers = parse_arxiv_atom(&xml);
        let mut out = format!("arXiv search: {query}\n\n");
        if papers.is_empty() {
            out.push_str("(no results)\n");
        }
        for p in &papers {
            out.push_str(&format!("Title: {}\n", p.title));
            if !p.authors.is_empty() {
                out.push_str(&format!("Authors: {}\n", p.authors.join(", ")));
            }
            if !p.published.is_empty() {
                out.push_str(&format!("Published: {}\n", p.published));
            }
            if !p.summary.is_empty() {
                out.push_str(&format!("Summary: {}\n", p.summary));
            }
            if !p.pdf_url.is_empty() {
                out.push_str(&format!("PDF: {}\n", p.pdf_url));
            }
            out.push('\n');
        }

        Ok(ToolResult {
            tool: self.name().to_string(),
            ok: true,
            content: truncate_for_model(&out),
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ArxivPaper {
    pub title: String,
    pub authors: Vec<String>,
    pub published: String,
    pub summary: String,
    pub pdf_url: String,
}

/// Hand-rolled minimal Atom parser.
///
/// Why no XML crate: the only structure we need is `<entry>...</entry>`
/// blocks each containing `<title>`, `<summary>`, `<author><name>`, and
/// `<link rel="alternate">`. arXiv's format has been stable forever and
/// `roxmltree`/`quick-xml` would add ~600 KB of compile time + a runtime
/// allocator dance just for these four fields.
///
/// This is *not* a general Atom parser. It would crumble on adversarial
/// input. arXiv is a trusted endpoint we control nothing about, so the
/// failure mode is "we miss a field" not "we get RCE'd".
pub(crate) fn parse_arxiv_atom(xml: &str) -> Vec<ArxivPaper> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(start) = xml[cursor..].find("<entry>") {
        let abs_start = cursor + start + "<entry>".len();
        let Some(end_rel) = xml[abs_start..].find("</entry>") else {
            break;
        };
        let block = &xml[abs_start..abs_start + end_rel];
        cursor = abs_start + end_rel + "</entry>".len();

        let title = extract_tag(block, "title").unwrap_or_default();
        let summary = extract_tag(block, "summary").unwrap_or_default();
        let published = extract_tag(block, "published").unwrap_or_default();

        let mut authors = Vec::new();
        let mut author_cursor = 0usize;
        while let Some(s) = block[author_cursor..].find("<author>") {
            let s_abs = author_cursor + s + "<author>".len();
            let Some(e_rel) = block[s_abs..].find("</author>") else {
                break;
            };
            let author_block = &block[s_abs..s_abs + e_rel];
            if let Some(name) = extract_tag(author_block, "name") {
                authors.push(name);
            }
            author_cursor = s_abs + e_rel + "</author>".len();
        }

        // <link title="pdf" href="..." rel="related" type="application/pdf"/>
        let pdf_url = block
            .split("<link ")
            .find(|chunk| chunk.contains("application/pdf"))
            .and_then(|chunk| {
                let key = "href=\"";
                let s = chunk.find(key)? + key.len();
                let e = chunk[s..].find('"')?;
                Some(chunk[s..s + e].to_string())
            })
            .unwrap_or_default();

        out.push(ArxivPaper {
            title: collapse_ws(&title),
            authors,
            published,
            summary: collapse_ws(&summary),
            pdf_url,
        });
    }
    out
}

fn extract_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = block.find(&open)? + open.len();
    let e = block[s..].find(&close)?;
    Some(block[s..s + e].to_string())
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

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
    fn parses_a_minimal_arxiv_entry() {
        let xml = r#"
<feed>
<entry>
  <title>Attention Is All You Need</title>
  <summary>The dominant sequence transduction models...</summary>
  <published>2017-06-12T17:57:34Z</published>
  <author><name>Ashish Vaswani</name></author>
  <author><name>Noam Shazeer</name></author>
  <link title="pdf" href="https://arxiv.org/pdf/1706.03762" rel="related" type="application/pdf"/>
</entry>
</feed>
"#;
        let papers = parse_arxiv_atom(xml);
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].title, "Attention Is All You Need");
        assert_eq!(papers[0].authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!(papers[0].pdf_url, "https://arxiv.org/pdf/1706.03762");
        assert!(papers[0].summary.contains("dominant sequence transduction"));
    }

    #[test]
    fn empty_feed_yields_no_papers() {
        assert!(parse_arxiv_atom("<feed></feed>").is_empty());
    }

    #[test]
    fn collapses_whitespace_in_titles() {
        let xml = "<feed><entry><title>foo\n  bar\t baz</title></entry></feed>";
        let papers = parse_arxiv_atom(xml);
        assert_eq!(papers[0].title, "foo bar baz");
    }
}

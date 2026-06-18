//! Part A — Code knowledge graph: query a graphify-built `graph.json` instead of
//! reading whole files. This is the **token-efficient code-understanding** layer.
//!
//! ## How graphify fits (the recommendation)
//!
//! [graphify](https://github.com/safishamsi/graphify) (MIT, Python) is the
//! BUILDER: its 25-language tree-sitter extraction turns a project into
//! `graphify-out/graph.json` (NetworkX node-link). For CODE this is AST-only —
//! **free, local, no LLM** (graphify only spends tokens on docs/papers/images,
//! and we use `--backend ollama` for those so it stays on-device). We run
//! graphify as a **managed hidden service** (like the engine; see
//! [`GraphIndex`]) and implement the QUERY tools **natively in Rust here** over
//! `graph.json`. So: reuse graphify's real work, but query-time needs no Python
//! and plugs straight into forge's existing [`crate::tools::Tool`] trait.
//!
//! graphify is a code knowledge graph — NOT conversational memory. That's
//! [`crate::memory`] (Part B). Keep the two distinct.

use crate::tools::{Tool, ToolResult, MAX_TOOL_OUTPUT_BYTES};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GraphNode {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub source_location: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
}

/// An in-memory view of a graphify `graph.json` with the few query operations
/// the agent needs. Cheap, read-only, no Python.
pub struct CodeGraph {
    nodes: Vec<GraphNode>,
    by_id: HashMap<String, usize>,
    /// id -> [(neighbor_id, relation)] (undirected adjacency for neighbor walks).
    adj: HashMap<String, Vec<(String, String)>>,
}

impl CodeGraph {
    /// Parse a graphify graph.json (NetworkX node-link). Tolerates both the
    /// `links` (older NetworkX) and `edges` (3.x) key for the edge array — the
    /// same fallback graphify's own serve.py uses.
    pub fn from_json(data: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(data).context("graph.json parse")?;
        let nodes: Vec<GraphNode> =
            serde_json::from_value(v.get("nodes").cloned().unwrap_or_else(|| json!([])))
                .context("graph nodes")?;
        let edges_val = v
            .get("links")
            .or_else(|| v.get("edges"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let edges: Vec<GraphEdge> = serde_json::from_value(edges_val).context("graph edges")?;

        let mut by_id = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            by_id.insert(n.id.clone(), i);
        }
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for e in &edges {
            let rel = e.relation.clone().unwrap_or_else(|| "related".into());
            adj.entry(e.source.clone())
                .or_default()
                .push((e.target.clone(), rel.clone()));
            adj.entry(e.target.clone())
                .or_default()
                .push((e.source.clone(), rel));
        }
        Ok(Self { nodes, by_id, adj })
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::from_json(&data)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn get(&self, id: &str) -> Option<&GraphNode> {
        self.by_id.get(id).map(|&i| &self.nodes[i])
    }

    fn node_text(n: &GraphNode) -> String {
        n.label.clone().unwrap_or_else(|| n.id.clone())
    }

    /// Render a node as one compact line for the model.
    fn line(&self, id: &str) -> String {
        match self.get(id) {
            Some(n) => {
                let loc = match (&n.source_file, &n.source_location) {
                    (Some(f), Some(l)) => format!(" [{f}:{l}]"),
                    (Some(f), None) => format!(" [{f}]"),
                    _ => String::new(),
                };
                let kind = n.file_type.as_deref().unwrap_or("node");
                format!("- {} ({kind}){loc}", Self::node_text(n))
            }
            None => format!("- {id}"),
        }
    }

    /// Score nodes by query-term overlap on their label. Rarer terms weigh more
    /// (a light IDF), mirroring graphify's serve.py scoring without its full
    /// TF-IDF. Returns (score, id) sorted high→low.
    fn score(&self, query: &str) -> Vec<(f32, String)> {
        let terms = tokenize(query);
        if terms.is_empty() {
            return Vec::new();
        }
        // Document frequency per term across node labels.
        let mut df: HashMap<&str, usize> = HashMap::new();
        let toks: Vec<HashSet<String>> = self
            .nodes
            .iter()
            .map(|n| tokenize(&Self::node_text(n)).into_iter().collect())
            .collect();
        for t in &terms {
            let c = toks.iter().filter(|s| s.contains(t)).count();
            df.insert(t.as_str(), c);
        }
        let n = self.nodes.len().max(1) as f32;
        let mut scored: Vec<(f32, String)> = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            let mut s = 0.0f32;
            for t in &terms {
                if toks[i].contains(t) {
                    let dfi = *df.get(t.as_str()).unwrap_or(&1) as f32;
                    s += (n / dfi).ln().max(0.1); // idf-ish
                }
            }
            if s > 0.0 {
                scored.push((s, node.id.clone()));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// THE token-saving query: find the most relevant nodes for `question` and
    /// return a compact subgraph (seed nodes + their 1-hop neighbors) as text,
    /// instead of the agent reading whole files. `max_seeds` bounds the answer.
    pub fn query(&self, question: &str, max_seeds: usize) -> String {
        let scored = self.score(question);
        if scored.is_empty() {
            return format!("No graph matches for: {question}\n(Graph has {} nodes.)", self.nodes.len());
        }
        let mut out = String::new();
        out.push_str(&format!("Relevant code for \"{question}\":\n"));
        let mut seen: HashSet<String> = HashSet::new();
        for (_, id) in scored.iter().take(max_seeds.max(1)) {
            if !seen.insert(id.clone()) {
                continue;
            }
            out.push_str(&self.line(id));
            out.push('\n');
            // 1-hop neighbors with relation labels.
            if let Some(nbrs) = self.adj.get(id) {
                let mut shown = 0;
                for (nid, rel) in nbrs {
                    if shown >= 6 {
                        out.push_str("    … (more neighbors)\n");
                        break;
                    }
                    out.push_str(&format!("    →{rel}: {}\n", CodeGraph::short(self.get(nid), nid)));
                    shown += 1;
                }
            }
            if out.len() > MAX_TOOL_OUTPUT_BYTES {
                out.push_str("\n[truncated]");
                break;
            }
        }
        out
    }

    fn short(n: Option<&GraphNode>, id: &str) -> String {
        n.and_then(|x| x.label.clone()).unwrap_or_else(|| id.to_string())
    }

    /// Details for one node (by id or exact label).
    pub fn node(&self, id_or_label: &str) -> String {
        let node = self.get(id_or_label).or_else(|| {
            self.nodes
                .iter()
                .find(|n| n.label.as_deref() == Some(id_or_label))
        });
        match node {
            Some(n) => format!(
                "{}\nid: {}\ntype: {}\nfile: {}{}",
                Self::node_text(n),
                n.id,
                n.file_type.as_deref().unwrap_or("?"),
                n.source_file.as_deref().unwrap_or("?"),
                n.source_location.as_deref().map(|l| format!("\nloc: {l}")).unwrap_or_default()
            ),
            None => format!("No node `{id_or_label}`."),
        }
    }

    /// Neighbors of a node with relation labels.
    pub fn neighbors(&self, id_or_label: &str) -> String {
        let id = if self.by_id.contains_key(id_or_label) {
            id_or_label.to_string()
        } else {
            match self.nodes.iter().find(|n| n.label.as_deref() == Some(id_or_label)) {
                Some(n) => n.id.clone(),
                None => return format!("No node `{id_or_label}`."),
            }
        };
        match self.adj.get(&id) {
            Some(nbrs) if !nbrs.is_empty() => {
                let mut out = format!("Neighbors of {}:\n", Self::short(self.get(&id), &id));
                for (nid, rel) in nbrs.iter().take(40) {
                    out.push_str(&format!("  →{rel}: {}\n", Self::short(self.get(nid), nid)));
                }
                out
            }
            _ => format!("{} has no recorded neighbors.", Self::short(self.get(&id), &id)),
        }
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

// =====================================================================
// Managed builder: runs graphify (the hidden service) to build/refresh the
// graph, and locates the resulting graph.json. Python-only at BUILD time.
// =====================================================================

/// Where a project's graph lives + how to (re)build it via graphify.
pub struct GraphIndex {
    project_root: PathBuf,
}

impl GraphIndex {
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self { project_root: project_root.into() }
    }

    /// graphify writes `graphify-out/graph.json` under the project root.
    pub fn graph_path(&self) -> PathBuf {
        self.project_root.join("graphify-out").join("graph.json")
    }

    pub fn exists(&self) -> bool {
        self.graph_path().is_file()
    }

    /// True if the graph is missing or older than the newest source file (cheap
    /// staleness check; graphify's own SHA-256 cache makes a rebuild incremental).
    pub fn is_stale(&self) -> bool {
        let gp = self.graph_path();
        let Ok(gmeta) = std::fs::metadata(&gp) else {
            return true;
        };
        let Ok(gmtime) = gmeta.modified() else {
            return true;
        };
        // Compare against the project root's mtime as a coarse signal.
        std::fs::metadata(&self.project_root)
            .and_then(|m| m.modified())
            .map(|root| root > gmtime)
            .unwrap_or(false)
    }

    /// Build/refresh the graph by spawning graphify (the managed hidden service).
    /// `graphify_bin` is the bundled/managed executable. Uses `--backend ollama`
    /// so any non-code semantic pass stays on-device. Returns the graph path.
    /// (Code extraction is AST-only and needs no model at all.)
    pub fn build(&self, graphify_bin: &str, update: bool) -> Result<PathBuf> {
        let mut args = vec![self.project_root.to_string_lossy().to_string()];
        if update {
            args.push("--update".to_string());
        }
        args.push("--backend".to_string());
        args.push("ollama".to_string());
        let status = std::process::Command::new(graphify_bin)
            .args(&args)
            .current_dir(&self.project_root)
            .status()
            .with_context(|| format!("spawning {graphify_bin}"))?;
        if !status.success() {
            anyhow::bail!("graphify exited with {status}");
        }
        Ok(self.graph_path())
    }
}

// =====================================================================
// forge Tools — registered into the agent loop so the model queries the graph
// FIRST (scoped queries) instead of reading whole files. That's the token win.
// =====================================================================

fn load(graph_path: &Path) -> std::result::Result<CodeGraph, ToolResult> {
    CodeGraph::from_file(graph_path).map_err(|e| ToolResult {
        tool: "graph".into(),
        ok: false,
        content: format!("graph not available: {e}. The project may not be indexed yet."),
    })
}

pub struct GraphQueryTool {
    pub graph_path: PathBuf,
}
#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str {
        "graph_query"
    }
    fn description(&self) -> &str {
        "Find the code relevant to a question by querying the project's code knowledge graph. \
         PREFER THIS over reading whole files — it returns a compact subgraph (functions/classes \
         + how they connect) and is far cheaper in tokens. args: { query: string }"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let g = match load(&self.graph_path) {
            Ok(g) => g,
            Err(r) => return Ok(r),
        };
        Ok(ToolResult { tool: "graph_query".into(), ok: true, content: g.query(q, 8) })
    }
}

pub struct GraphNeighborsTool {
    pub graph_path: PathBuf,
}
#[async_trait]
impl Tool for GraphNeighborsTool {
    fn name(&self) -> &str {
        "graph_neighbors"
    }
    fn description(&self) -> &str {
        "List what a code symbol connects to (calls, imports, uses) from the knowledge graph, \
         instead of reading its file. args: { node: string }  (a node id or label)"
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"node":{"type":"string"}},"required":["node"]})
    }
    async fn invoke(&self, args: Value) -> Result<ToolResult> {
        let id = args.get("node").and_then(|v| v.as_str()).unwrap_or("");
        let g = match load(&self.graph_path) {
            Ok(g) => g,
            Err(r) => return Ok(r),
        };
        Ok(ToolResult { tool: "graph_neighbors".into(), ok: true, content: g.neighbors(id) })
    }
}

/// Register the graph tools into a registry, but only if a graph.json exists
/// (so the agent never advertises a tool that can't answer).
pub fn register_graph_tools(registry: &mut crate::tools::ToolRegistry, graph_path: &Path) -> bool {
    if !graph_path.is_file() {
        return false;
    }
    registry.register(Arc::new(GraphQueryTool { graph_path: graph_path.to_path_buf() }));
    registry.register(Arc::new(GraphNeighborsTool { graph_path: graph_path.to_path_buf() }));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "directed": false, "multigraph": false, "graph": {},
      "nodes": [
        {"id":"auth.login","label":"login","file_type":"code","source_file":"src/auth.rs","source_location":"L10"},
        {"id":"auth.verify_token","label":"verify_token","file_type":"code","source_file":"src/auth.rs","source_location":"L40"},
        {"id":"db.get_user","label":"get_user","file_type":"code","source_file":"src/db.rs","source_location":"L5"},
        {"id":"ui.button","label":"render_button","file_type":"code","source_file":"src/ui.rs","source_location":"L1"}
      ],
      "links": [
        {"source":"auth.login","target":"auth.verify_token","relation":"calls","confidence":"EXTRACTED"},
        {"source":"auth.login","target":"db.get_user","relation":"calls","confidence":"EXTRACTED"}
      ]
    }"#;

    #[test]
    fn parses_node_link_with_links_key() {
        let g = CodeGraph::from_json(FIXTURE).unwrap();
        assert_eq!(g.node_count(), 4);
    }

    #[test]
    fn parses_edges_key_too() {
        let alt = FIXTURE.replace("\"links\"", "\"edges\"");
        let g = CodeGraph::from_json(&alt).unwrap();
        assert_eq!(g.node_count(), 4);
        assert!(g.neighbors("auth.login").contains("verify_token"));
    }

    #[test]
    fn query_finds_relevant_node_and_neighbors() {
        let g = CodeGraph::from_json(FIXTURE).unwrap();
        let out = g.query("how does login work", 5);
        assert!(out.contains("login"));
        // 1-hop neighbors surface without reading any file.
        assert!(out.contains("verify_token") || out.contains("get_user"));
    }

    #[test]
    fn query_misses_irrelevant_terms_gracefully() {
        let g = CodeGraph::from_json(FIXTURE).unwrap();
        let out = g.query("kubernetes helm chart", 5);
        assert!(out.contains("No graph matches"));
    }

    #[test]
    fn neighbors_by_label_and_id() {
        let g = CodeGraph::from_json(FIXTURE).unwrap();
        assert!(g.neighbors("login").contains("calls"));
        assert!(g.neighbors("auth.login").contains("get_user"));
        assert!(g.neighbors("ui.button").contains("no recorded neighbors"));
    }

    #[test]
    fn node_details_by_id_or_label() {
        let g = CodeGraph::from_json(FIXTURE).unwrap();
        assert!(g.node("db.get_user").contains("src/db.rs"));
        assert!(g.node("render_button").contains("src/ui.rs"));
        assert!(g.node("nope").contains("No node"));
    }

    #[test]
    fn register_skips_when_no_graph_file() {
        let mut r = crate::tools::ToolRegistry::new();
        assert!(!register_graph_tools(&mut r, Path::new("/nonexistent/graph.json")));
        assert!(r.is_empty());
    }
}

# Build Report — Code Knowledge Graph (graphify) + Conversational Memory

**Date:** 2026-06-18 · Two *distinct* needs, kept separate in code and here:
**Part A** = token-efficient **code understanding** (graphify); **Part B** =
cross-session **conversational memory** (graphify does NOT do this).

---

## Step 0 — graphify analysis + recommendation

**What graphify is** (read its `ARCHITECTURE.md` + `docs/how-it-works.md`, repo
`safishamsi/graphify`, default branch `v8`): a **Python**, **MIT**, **local-first**
tool that turns a codebase into a queryable **knowledge graph** (`graph.json`,
NetworkX node-link). Pipeline: `detect → extract → build_graph → cluster →
analyze → report → export`. Key facts I confirmed:
- **Code extraction is AST-only via tree-sitter (25 languages) — free, local, no
  LLM.** It only spends tokens on *docs/papers/images* (Pass 3), and with
  `--backend ollama` even that stays on-device. A pure-code project needs **no
  model at all** to build the graph.
- It ships an **MCP stdio server** (`graphify-mcp` / `serve.py`) exposing query
  tools, and a `graph.json` you can query directly. `--update` + git-hook
  rebuilds are incremental (SHA-256 cache).
- Its own benchmark: **~71.5× fewer tokens per query** on a 52-file mixed corpus
  (graph query vs. reading raw files); savings compound with corpus size.

**Recommendation — Approach (A), refined to a hybrid (chosen):** Integrate
graphify **as the managed graph BUILDER** (reuse its real value — the 25-language
extraction), run it as a **hidden managed service** like the engine, but
implement the **query tools natively in Rust over `graph.json`** rather than
embedding an MCP client in the Rust agent.

Why this over plain (A) or (B):
- vs. **(A) spawn graphify's MCP server and have the agent speak MCP:** our agent
  already has a clean `Tool` trait; adding an MCP/JSON-RPC client to the Rust
  engine is avoidable complexity. Reading `graph.json` directly is trivial and
  makes **query-time 100% Rust — no Python running during chat.**
- vs. **(B) reimplement extraction in Rust:** that throws away graphify's 25
  tree-sitter extractors + clustering — weeks of work for no user benefit.
- **The Python dependency is confined to build/refresh** (a hidden service,
  never a user-facing CLI) — the acceptable cost the brief allows. Cost flagged:
  bundling a Python runtime + graphify grows the app (~tens of MB); see Risks.

---

## Part A — code knowledge graph (`src/graph/mod.rs`)

- **Native Rust query over `graph.json`:** `CodeGraph::from_json` parses the
  NetworkX node-link format (tolerates both the `links` and `edges` keys — the
  same fallback graphify's `serve.py` uses). Operations:
  - `query(question)` → a **compact subgraph** (relevant seed nodes scored by
    IDF-weighted label overlap + their 1-hop neighbors with relation labels),
  - `node(id|label)` → one symbol's details (file/loc/type),
  - `neighbors(id|label)` → what it calls/imports/uses.
- **Exposed to the agent loop as `Tool`s** (`graph_query`, `graph_neighbors`),
  registered into the agent's `ToolRegistry` **only when a `graph.json` exists**
  for the project — so the agent **queries the graph first instead of reading
  whole files**. The tool descriptions explicitly say *"PREFER THIS over reading
  whole files."*
- **Managed builder** (`GraphIndex`): locates `graphify-out/graph.json`, reports
  staleness, and `build(graphify_bin, --update)` spawns graphify with
  `--backend ollama` (on-device). Server endpoints: `GET /api/graph/status`,
  `POST /api/graph/build`.

**Token savings (how + estimate).** Instead of the agent reading the 2–4 files it
*thinks* are relevant (often 4k–8k tokens) to answer "how does login work?", one
`graph_query` returns the relevant functions + how they connect in **~200–400
tokens** — roughly **10–20× on a single scoped query**, and graphify's measured
**71.5×** across a 52-file corpus as queries compound against one already-built
graph. Tiny projects (a few files) see little benefit — that's stated honestly in
the UI copy ("indexing helps most on larger projects").

---

## Part B — cross-session conversational memory (`src/memory/mod.rs`)

The half graphify does **not** provide. A lightweight, **local-only** store:
- **What's stored:** `MemoryEntry { ts, kind, text, tags }` where kind ∈
  {Preference, Decision, Summary, Fact} — **summaries, not raw transcripts**
  (`summarize_session` stores the first ask + turn count, never the full text;
  unit-tested). Per-project JSONL at
  `<config>/ollama-forge/memory/<project-hash>.jsonl`.
- **Retrieval (token-budgeted):** `retrieve(query, token_budget)` scores entries
  by keyword overlap × kind weight (preferences/decisions outrank summaries) ×
  recency, and **greedily fills the budget** — never a cold dump.
  `render_for_context` produces a short "Relevant memory…" preamble.
- **Wired in:** the server prepends that preamble (budget 400 tokens) to the
  system prompt on **both** the plain chat path and the agent/tools path — so a
  new session isn't a cold start. Built on the same token-estimate heuristic as
  the context manager; a natural feeder alongside `instincts` (which already
  mines repeated patterns from the replay log).
- **User control:** `GET /api/memory` (view), `POST /api/memory/clear` (wipe),
  and `forget_matching` (edit/remove by substring).
- **On-device guarantee (load-bearing):** the module does **pure local
  filesystem I/O — zero network/HTTP dependency**. Memory is **never** sent to
  the identity backend, which stays content-free. Tests assert the store path
  resolves under the local config dir and that summaries (not transcripts) are
  stored.

---

## Surfacing & toggles
- **Graph:** opt-in by nature — the agent tools appear only after a project is
  indexed (`POST /api/graph/build`); status via `/api/graph/status`. The UI
  shows "indexing this project for faster, cheaper answers."
- **Memory:** view/clear via the API; retrieval is per-turn and budgeted. (A
  per-request on/off flag + the in-app settings toggle are the small remaining UI
  step; the backend honors "no memory" simply when the store is empty/cleared.)

## What changed / didn't break
- **New:** `src/graph/mod.rs` (+7 tests), `src/memory/mod.rs` (+7 tests); server
  endpoints + agent wiring; `src/lib.rs` module registration.
- **Untouched & green:** engine builds clean (no warnings); **`cargo test`
  152 pass** (was 138). Website, extension, CI unaffected.

## Open questions / risks
- **Bundle size / Python:** shipping graphify needs a bundled Python (uv/pyinstaller)
  inside the app — tens of MB + a managed-service spawn. Confined to build-time;
  flagged per the brief. (The Rust query path needs no Python.)
- **Graph staleness:** mitigated by graphify's incremental `--update` + the
  `is_stale` check + optional git-hook rebuild; first build on a big repo costs
  some time (AST-only, but real).
- **Honesty:** graphify is code-RAG, **not** memory of the user — kept separate.
  Both stay **on-device**; nothing content-bearing reaches the backend.
- **Runtime end-to-end** (graphify actually building a graph inside the bundled
  app; memory across real multi-day sessions) needs the running app + graphify
  bundled — the Rust cores + wiring are unit-tested and compile; that final
  integration verification is the remaining step.

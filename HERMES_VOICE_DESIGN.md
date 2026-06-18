# Research + Design: Hermes-Class Agent + Voice Demo Navigation

**Status: Phase 0 (research + design) — delivered for sign-off BEFORE building.**
**Date: 2026-06-18.** Backend model-efficiency (MLX/MoE/quant) is out of scope (you're
handling it separately); this doc flags where it gates quality/latency.

This was produced by a 5-agent research workflow + adversarial synthesis that
**fetched the real Hermes sources** and **read forge's actual code**. Confidence is
tagged throughout; unverifiable claims are explicitly fenced off.

---

## 0. Honesty preamble — what's verified vs not

My training cutoff predates Hermes Agent, so everything here came from live fetches.

**Reachable and read (HTTP 200):** `hermes-agent.nousresearch.com/docs/llms-full.txt`
(2.9 MB, ingested), `/llms.txt`, the `NousResearch/hermes-agent` GitHub raw
README/LICENSE/pyproject, and `agentskills.io`.

**Safe to cite as fact (verified from Nous source):** Hermes is **MIT-licensed**,
**Python 3.11–3.13 via `uv`**, **local-first** (llama.cpp/MLX/Ollama/vLLM, any
OpenAI-compatible endpoint), a **full MCP client**, uses the **agentskills.io
`SKILL.md`** standard, has **persistent memory** + **self-authored skills** +
**sub-agent delegation** + a **single natural-language cron tool** + a
**messaging gateway**.

**NOT verifiable from Nous source (came only from a third-party mirror — I will
NOT present these as fact):** the exact "70 tools / 28 toolsets," the "64k context"
floor, the worker-pool size (one source said 3-concurrent, another 8 — a
contradiction), "~166 skills," and "Curator" as a named component. Also: Hermes
isn't in the agentskills.io showcase, so its conformance is self-reported. One
doc/README discrepancy: the README implies FTS5 **+ summarization** for recall,
but the docs say `session_search` (FTS5) makes **no LLM calls** — so memory
*summarization* and memory *search* are separate mechanisms, not one.

The design below leans only on the verified pillars.

---

## 1. Hermes pillars (verified) and what makes it dominant

| Pillar | What Hermes does (verified) |
|---|---|
| **Persistent memory** | `MEMORY.md` + a frozen `USER.md` user-model snapshot + an FTS5 `session_search` over past sessions (lexical, no LLM). Optional Honcho user modeling. |
| **Self-improving skills** | `agentskills.io` `SKILL.md` with progressive disclosure; the agent **authors its own skills** from experience and prunes stale ones. |
| **Sub-agent delegation** | `delegate_task` spawns isolated, short-lived children with their own context, a restricted toolset, and their own terminal. |
| **Rich tools + autonomy** | A large tool set across many toolsets (web/browser/vision/file/terminal) + **one natural-language cron** tool for scheduling. |
| **Model-agnostic / local-first / MCP** | Runs any local model; is a **full MCP client**, so it gains capability by connecting to MCP servers rather than hard-coding integrations. |
| **Messaging gateway** | 21+ chat platforms (Telegram/Discord/Slack/…) behind one gateway. |

**The dominance pattern:** Hermes isn't one giant program — it's a thin agent loop
over **open standards** (`SKILL.md`) and an **open protocol** (MCP), with memory and
delegation on top. That's the key insight for our build.

---

## 2. THE decision — integrate vs reimplement

**Recommendation: HYBRID, leaning reimplement (B). Do NOT wrap the Python Hermes engine.**

Hermes's value decomposes into three things forge can own natively:
1. an **open standard** forge already parses — `SKILL.md` (`src/skills/` is already an
   Anthropic-style SKILL.md parser, ~65–70% spec-clean);
2. an **open protocol** forge can speak — **MCP** (forge has none yet, but the `Tool`
   trait + `ToolRegistry` is a clean seam for an MCP client);
3. an **agent loop + memory** forge already has (`src/agent/`, `src/memory/`).

Wrapping the real Hermes would import a **Python/uv + Honcho + SQLite + cron +
long-running gateway process into every agent turn**, duplicate forge subsystems,
and **break the single-binary Code-OSS packaging** we just made runnable.

**Why this differs from the graphify decision (which DID bundle Python):** graphify's
tree-sitter extraction is **build-time only** (run once, query the JSON in Rust).
Hermes would need a **live Python agent on every turn** — a categorically heavier,
always-on dependency. So the precedent doesn't transfer.

**The one capability worth pulling toward Hermes is an MCP *client*** — and this
deliberately **reverses the graphify-era "no MCP client" stance**, because here
**MCP is the product surface**, not an internal format. A Rust MCP stdio/HTTP client
(crate: `rmcp`) that launches an `mcp_servers` config and registers each remote tool
as a forge `Tool` gives us GitHub/filesystem/DB/browser capability **without Hermes**,
and interops with the whole MCP ecosystem.

So: **adopt the standards Hermes is built on (SKILL.md + MCP), reimplement only the
self-improving learning loop on forge primitives, and skip the Python engine.**

---

## 3. Forge-primitive reuse map (what exists → what to add)

Verified by reading the code. Percentages are "how close to Hermes-class."

| Capability | forge today | % | What to add |
|---|---|---|---|
| **Memory** | `src/memory/` retrieve/remember are **read** into chat+agent, but `summarize_session` **has no caller** (nothing writes memory back) | ~70% | Call `summarize_session` + `remember` at **stream end**, gated by `SecurityGuard` (no secrets persisted). |
| **Skills** | `src/skills/` parses Anthropic `SKILL.md`; only used in **Build** today | ~65% | Apply the **matched skill in the agent loop**; spec-clean (name==dir, hyphen/length, scripts/refs/assets progressive disclosure). |
| **Skill *learning*** | `src/instincts/` detects repeated patterns but **refuses auto-promotion** (deliberate) | — | **Opt-in** "promote this pattern to a skill" + a dedup pass. Keep it opt-in — auto-promotion contradicts the instincts design. |
| **Sub-agent delegation** | `src/orchestrator`+`executor` do multi-**model** fan-out, but each worker is a single `generate()` call | ~50% | Make a worker a **full `Agent`**; expose a `spawn_subagent` **Tool** with an isolated context + restricted toolset. |
| **Tools** | `src/tools/` = 4 web + 2 graph; clean `Tool` trait; **no file/shell/MCP** | ~75% | **MCP client** (`rmcp`, per-server allowlist) + `FsRead/Write/Edit` + a **sandboxed Shell** (consent + audit + kill-switch). |
| **Scheduling** | none (only an 80 ms UI spinner) | ~5% | A small **on-device scheduler** in `forge serve` (NL → cron) — *candidate to defer*; see plan. |

Everything above is **local-first / zero-egress** except the new file/shell/MCP tools
and the scheduler, which need **sandboxing + explicit consent + an audit trail +
kill-switch** to preserve the privacy guarantee.

---

## 4. The 3-tab reframe (the cleanest win)

**Verified:** `/api/research` (Agent) and `/api/chat` *with tools on* both call the
**same** `run_agent_streamed` (`src/server/mod.rs:~1023`) — so **Agent and
tools-chat are backend-identical today**. Build (Orchestrator) is genuinely distinct.
Reframe by **re-pointing endpoints, not rewriting**:

- **Chat** = `/api/chat`, **tools OFF** — pure-local, zero-egress, single-model, the
  safe quick-Q&A default.
- **Agent** = the **single home** of `run_agent_streamed`, upgraded into the
  flagship: memory write-back, opt-in skills, file/shell/MCP tools, a `delegate`
  tool, (optional) scheduler.
- **Build** = Orchestrator parallel multi-model synthesis — **unchanged**.

The three tabs then answer **talk / act-autonomously / build-with-many-models** —
genuinely distinct.

---

## 5. Voice-activated demo navigation — design

**Flow:** push-to-talk hotkey → capture mic in the **webview** (the extension host
has no `getUserMedia`) → **local Whisper STT** → normalize to intent → resolve via
the **existing code graph + intent-expansion** → **reveal** the file+symbol live.

- **STT (verified API surface):** **whisper.cpp via the `smart-whisper` Node addon**
  (Metal-accelerated, 16 kHz mono Float32, model loaded once, async), default
  **`base.en` (~142 MB)**, `small.en` as an accuracy upgrade. **Fallback: a bundled
  whisper.cpp CLI subprocess** so a native-addon build miss (the `node-pty` failure
  this repo already hit) never bricks it. **MLX-Whisper** = opt-in Apple-Silicon fast
  lane only. **Web Speech API is excluded** — it streams audio to Google (not local).
- **Capture pattern:** **push-to-talk**, transcribe one short utterance on key-release
  (not continuous streaming) — right for "go to the login handler."
- **Intent → code (reuse, no new infra):** the transcript runs through a
  `hub`-style normalize/stem/expand front-end, then `CodeGraph::query` (TF-IDF over
  node labels). Every `GraphNode` already carries `source_file` + `source_location`
  (e.g. `L10`) = a ready-made jump target. **Embeddings are DEFERRED** — ship on the
  existing lexical graph first; add an additive Ollama `/api/embeddings` path **only
  if** demo testing shows paraphrase misses.
- **Editor jump (verified API):** `await vscode.window.showTextDocument(Uri.file(f))`
  → set `editor.selection` → `editor.revealRange(range, InCenterIfOutsideViewport)`.
- **Activation/UX:** push-to-talk keybinding (e.g. `Cmd+Shift+V`) + a **status-bar mic
  item** (Idle/Listening/Transcribing/Resolving) + a transient
  **"Heard: '…' → src/auth.rs:42"** confirmation with **Undo / Next**. Wake-word stays
  **off** by default (always-on mic + false triggers); opt-in later.

---

## 6. UI design — first-timer legible & trustworthy

Inherits the existing token system (grayscale + single amber accent, 4 px scale,
WCAG-checked, mapped to `--vscode-*`) and the webview CSP — **no new color world**.
Five trust patterns leading agent UIs converge on, each backed by **data forge
already produces**:

1. **Plan card (Intent Preview)** — before acting, show the numbered plan in plain
   language with **Run / Edit plan / I'll do it myself**, paired with an **Autonomy
   Dial** (Observe → Confirm-each → Auto), **defaulting to Confirm-each** for newcomers.
2. **Activity Timeline** — upgrade the flat step list into a vertical timeline keyed
   on `AgentStep` (status dot, tool name, `result_preview`, a chevron expanding
   `args` — captured but never shown today — and an elapsed tick so a slow local model
   doesn't look hung).
3. **Step-level intervention** — replace the single global Stop with **Pause / Stop /
   Resume**.
4. **Memory drawer** — a collapsible "Remembered for this task" listing the
   `MemoryEntry` items actually pulled (with a privacy note).
5. **Skill creation & approval** — when instincts detect a repeated tool-chain, offer
   "Save as a skill?" (opt-in).
6. **Sub-agents lane** — a collapsible section, one row per worker (+ an Auditor row),
   **out of** the linear message stream.
7. **Voice overlay** — a **bottom-anchored bar** (keeps context visible) with a mic orb
   state machine (idle/listening/transcribing), a **live transcript**, the **target
   preview**, and **Undo/Next**.
8. **Discoverability** — instructional **empty states** ("what the Agent does" + 3
   clickable starters) and behavior-triggered progressive disclosure.

---

## 7. Phased build plan (recommended)

**Phase 1 — Agent tab (do now):**
- 1a. **3-tab reframe** (re-point Chat to tools-off; Agent = sole `run_agent_streamed`). *(small, high-impact)*
- 1b. **Memory write-back** at stream end, `SecurityGuard`-gated. *(engine, testable)*
- 1c. **Apply matched skill in the agent loop** + spec-clean SKILL.md. *(engine, testable)*
- 1d. **MCP client** (`rmcp`) behind an allowlist + register remote tools. *(engine, testable)*
- 1e. **File tools** (`FsRead/Write/Edit`) + **sandboxed Shell** with consent/audit. *(engine, testable)*
- 1f. **`delegate` sub-agent tool** (worker = full Agent, isolated ctx/tools). *(engine, testable)*
- 1g. **Agent-tab UI**: Plan card + Autonomy Dial + Activity Timeline + Memory drawer + Sub-agents lane + step controls. *(webview)*

**Phase 2 — Voice navigation (do now):**
- 2a. whisper.cpp STT (smart-whisper addon + CLI fallback), push-to-talk capture.
- 2b. transcript → graph/hub intent → file+line target.
- 2c. editor reveal + status-bar state machine + Heard→target + Undo/Next.

**Defer (recommended):**
- **The 20+ messaging-platform connectors** — wrong shape for a desktop IDE; large
  surface, little demo value. Revisit only if you want remote control.
- **The NL scheduler** (1 capability) — useful but not core to "autonomous agent +
  voice demo"; small follow-up.
- **Embeddings** — only if lexical recall misses in testing.
- **Opt-in skill auto-promotion polish / Curator-style dedup** — after the loop works.

---

## 8. Open questions / risks (honest)

1. **Hermes internals are partly unverifiable** — tool counts, worker-pool size,
   context floor came only from a third-party mirror (and contradict each other). We
   build to the **verified** pillars + open standards, not to those numbers.
2. **No Python Hermes** — by recommendation. If you specifically want upstream-Hermes
   parity/freshness, that's option A (wrap it) and changes the cost a lot — say so.
3. **Local-model latency gates the Agent** — a cold 7B first token is ~21 s (per the
   perf report); Confirm-each-step can feel like constant waiting. Mitigation: confirm
   only **write/consequential** actions, stream read-only tool calls through. Your
   separate model-efficiency work improves this; we don't block on it.
4. **STT accuracy on code identifiers** ("verify_token", "showTextDocument") and the
   **graph must be indexed** (nav quality = graph coverage). base.en is sub-second on
   Apple Silicon but small.en is more accurate.
5. **Native-addon portability** — `smart-whisper` is compiled; we ship the **CLI
   subprocess fallback** so a prebuilt-binary miss never bricks voice (lesson from the
   `node-pty` CI failure).
6. **New file/shell/MCP tools widen the trust surface** — they need sandboxing +
   explicit consent + audit + kill-switch to keep the local-first/zero-egress promise.
7. **Bundle size** — base.en (~142 MB) inflates the download; option to lazy-download
   the model on first voice use.
8. **Built on the VS Code pivot** — voice + Agent UI assume the `forge-vscode`
   extension (the carried-over AI layer), which is verified-packageable but the fork
   itself still needs a real build machine.

---

## 9. What I need signed off before building

1. **Approach** — Hybrid/reimplement (adopt SKILL.md + MCP, native learning loop, **no
   Python Hermes**). ✅ recommended.
2. **Phase-1 scope** — build 1a–1g now; **defer** messaging connectors + scheduler +
   embeddings. (Or tell me to include the scheduler / drop a tool.)
3. **STT bundle** — `base.en` (~142 MB, fast) default vs `small.en` (~466 MB, more
   accurate) vs lazy-download on first use.
4. **Shell tool** — include the sandboxed shell now (powerful, but widest trust
   surface), or defer it and ship file + MCP tools first?

# Build Report — Curated Offline Models · Web Access for All Models · Multi-Agent Orchestration

**Date:** 2026-06-18 · **Scope:** Rust backend (`forge`) + VS Code extension. No
website changes. Built on the existing provider trait / tools / agent / router /
orchestrator — extended, not rebuilt.

---

## TL;DR

| Feature | What shipped | State |
| :- | :- | :- |
| 1. Curated free offline models | Data-driven, hardware-tiered **model registry** (`src/models/mod.rs`) + `GET /api/models/catalog` + `forge models` CLI | ✅ built + tested |
| 2. Internet access for all models | Opt-in **web-tools toggle in normal chat** reusing the existing agent loop, with at-use-time egress disclosure | ✅ built + tested |
| 3. Multi-agent orchestration | Verified the existing **decompose → tier-route → parallel → merge** path is wired end-to-end; added routing tests | ✅ verified + tested |

**138 cargo tests pass** (was 126: +12), release + debug build clean, extension
`node --check` clean. An adversarial review ran over the new code; all findings
are fixed (below).

**Honesty up front:**
- **Free = the local open-weight models** (run via Ollama). **Cloud models
  (GPT/Claude/Gemini) are NOT implemented** this round — they remain paid /
  bring-your-own-key and are *deliberately excluded* from the registry. Ollama
  stays the single local engine. The "never silently route to a paid provider"
  rule holds **by construction** (no cloud code path exists).
- **Web tools keep inference local, but your queries + fetched pages leave the
  machine.** This is disclosed at use time (server `meta` event, the VS Code
  chat banner, the `forge research` CLI line, and the setting description) — it is
  *not* the same "nothing leaves your machine" as pure-local chat.

---

## Feature 1 — Curated, free, hardware-tiered model registry

**`src/models/mod.rs`** — a data-driven registry, explicitly *not* a hardcoded
gospel list:

- **Kept current at runtime, not trusted from a snapshot:**
  - `mark_installed(installed)` reconciles the seed against what Ollama actually
    has locally (`/api/tags`), so the catalog reflects *your* machine.
  - `verify_in_library(tag)` does a best-effort live check that a tag still
    resolves in the Ollama library (so a drifted name surfaces as "unverified"
    instead of 404-ing at pull time). Off by default (networked); enabled with
    `?verify=true` / `forge models --verify`.
  - The `seed()` list is a clearly-labeled mid-2026 starting point that's easy to
    edit.
- **Hardware tiering:** `HardwareTier` (Modest ~8 GB / Single ~16–24 GB /
  HighEnd multi-GPU). `fits(free_vram_mb)` filters to models whose estimated
  resident size (+25% headroom for KV cache) actually fits — driven by the
  existing `VramSentinel`. Unknown VRAM (CPU) returns the Modest tier only.
- **Honest default:** `recommend()` prefers an *installed*, commercial-friendly,
  coding model that fits; falls back through fits-and-licensed → fits → (only
  when VRAM is unknown) the smallest. **When VRAM is known but nothing fits it
  returns `None`** — it never recommends a model the machine can't load.
- **License surfaced:** every entry carries its license; `commercial_friendly()`
  is `true` *only* for Apache-2.0 / MIT / modified-MIT. Gemma, Llama-community,
  and Mistral-research are flagged **not** commercial-safe (a ⚠ in `forge models`
  and `commercialFriendly:false` in the JSON), so the custom-license caveats are
  visible.

**Surfaced via:**
- **`GET /api/models/catalog[?verify=true]`** → hardware tier + free VRAM, the
  recommended default, and every model with `installed` / `fits` / `license` /
  `commercialFriendly` / `pullCommand` / `libraryVerified`, plus a `note` stating
  cloud is paid/excluded.
- **`forge models [--verify] [--fits-only]`** → the catalog grouped by tier with
  the per-machine recommendation and `ollama pull` hints.
- **Follow-up (documented, not done):** the VS Code model picker currently lists
  installed models via `/api/models`; wiring it to render the *tiered catalog*
  (with one-click pull) from `/api/models/catalog` is the remaining UI step. The
  data + CLI are ready.

---

## Feature 2 — Internet access for the offline (and all) models

The free web tools (`web_search`/`wikipedia`/`arxiv`/`fetch_url`) already existed
for the research agent. They are now available in **normal chat**, opt-in:

- **`/api/chat` gains a `tools` flag.** When set, chat is served by the **exact
  same agent loop** as `/api/research` — factored into a shared
  `run_agent_streamed()` so it's one system surfaced in two places, not a fork.
  The model decides when to search/fetch; tool steps stream to the UI (the
  webview already renders `step`/`answer` events).
- **Reuses the existing safeguards:** per-tool rate limiter, output truncation,
  the streaming byte cap, and `robots.txt` handling in `fetch_url` — unchanged.
- **Clearly toggleable + disclosed:** extension setting **`forge.webTools`**
  (default **off** = pure-local). When on, the server's `meta` event carries a
  `disclosure` string that the chat panel now renders **in the conversation at
  use time** (🌐 banner), and `forge research` prints the same egress note. The
  secret-scanner `warnings` for attached context are carried onto this path too
  (so a credential warning still reaches you precisely when content can leave).

---

## Feature 3 — Multi-agent orchestration across the lineup

The orchestrator was already wired; this round **verified it end-to-end** and
**strengthened its tests**:

- **End-to-end path (confirmed):** `/api/build` → `Orchestrator::execute_with_progress`
  → `analyze_complexity` → `split_into_tiered_subtasks_vram_aware` (decompose +
  assign each subtask a right-sized model) → `execute_parallel_with_progress`
  (concurrent, per-model) → `merge_results` (low-temp dedup). Progress events
  stream to the UI. Same path the CLI `forge build` uses.
- **Leverages the broader lineup, VRAM-aware:** architecture/planning subtasks
  route to the **biggest** installed model, boilerplate (frontend/tests) to the
  **smallest**, balanced work to the analyzer's pick — and if two distinct models
  wouldn't fit in free VRAM, everything **collapses to one** that does.
- **New tests** (`src/router/mod.rs`): arch→largest / boilerplate→smallest;
  VRAM-too-small→collapse-to-one; single-model→no overrides.
- **Cloud kept opt-in by construction:** Auto routing only ever picks from
  *installed local* Ollama models and is documented to never escalate to a paid
  provider — and no cloud provider exists to escalate to.

---

## Adversarial review — findings & fixes (all addressed)

A 2-lens review (correctness + free/paid & local/networked honesty) ran over the
new code. Confirmed sound: orchestrator end-to-end wiring, the plain
(token-stream) chat path is preserved at the default (tools off), research still
works after the refactor, license flags are accurate, and no paid-provider
routing exists. Fixed:

- **HIGH — installed-tag over-matching.** Matching on the colon-less family
  marked `qwen3:14b`/`qwen3:235b` "installed" when only `qwen3:4b` was pulled
  (and could recommend a 150 GB model as "installed"). **Fix:** match the full
  seed tag exactly or as a prefix up to a separator (`tag_matches`); added a
  regression test that `qwen3:4b` does *not* mark other sizes installed.
- **HIGH — secret warnings dropped on the web-tools chat path.** Attached context
  flows to the agent (and can reach the web), but the secret-scan `warnings` were
  discarded on that path. **Fix:** plumbed `warnings` (+ `routing` + `trimmed`)
  through `run_agent_streamed` into the `meta` event.
- **HIGH — egress disclosure emitted but not rendered** in the VS Code chat UI.
  **Fix:** the `meta` handler now renders the 🌐 disclosure banner when tools are on.
- **LOW — `recommend()` could return a non-fitting model** in the sub-smallest
  VRAM band. **Fix:** returns `None` when VRAM is known and nothing fits; test added.
- **LOW — `forge research` lacked an explicit egress line.** **Fix:** added it.
- Cleared a cosmetic `unused_mut`.

---

## What changed / didn't break

**New:** `src/models/mod.rs` (+9 tests). **Extended:** `src/server/mod.rs`
(catalog endpoint + shared `run_agent_streamed` + chat `tools` toggle),
`src/cli/mod.rs` + `src/main.rs` (`forge models`; research egress line),
`src/router/mod.rs` (+3 routing tests), `src/lib.rs` (module),
`editor-integrations/forge-vscode/` (`forge.webTools` setting, chat body flag,
disclosure render). **Unchanged & green:** CLI, the website, CI, the existing
agent/tools/orchestrator behavior. `cargo test` 138 pass; release build clean;
extension JS valid.

---

## Open questions / risks

- **Cloud providers are not implemented.** If you want BYO-key GPT/Claude/Gemini,
  that's a separate, larger piece (API clients + streaming + the secret-scanner
  gating before any code leaves the machine). The provider trait/pool is ready
  for it; nothing here implies cloud works.
- **"Free/local" accuracy:** the only place free/local could become misleading is
  the web-tools path — handled by the at-use-time disclosure. Keep that disclosure
  if the UI is reworked.
- **Registry VRAM numbers are estimates** (Q4 resident + 25% headroom); they
  gate *fit*, not exact placement. Real placement still depends on context length.
- **Model names drift.** The seed is reconciled live (installed + library check),
  but run `forge models --verify` periodically and update the seed when the
  library moves.
- **Extension model-picker UI** doesn't yet render the tiered catalog / one-click
  pull (API + CLI do) — the documented next step.

---

## Note on the separate v0.1.0 download release
Unrelated to this work: the `v0.1.0` release run is still blocked on the
`build (macos-x64)` job — the **private-repo Intel-Mac runner** has been
queuing/compiling far longer than the others (which all succeeded). Publish is
gated on it. Options: wait, cancel + re-run for a faster runner, or drop the
Intel-Mac target from the matrix. Not related to these code changes.

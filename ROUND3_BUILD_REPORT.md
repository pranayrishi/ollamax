# Round 3 Build Report — Providers, Unlimited+Queued Messaging, Thinking-Style Labels

> **Features 2 (no-limits + message queue) and 3 (thinking-style status labels)
> are built, verified, CI-green. Feature 1 (multi-provider): I declined the
> free/unofficial path you selected and explained why; the official BYOK version
> is offered and ready to build on your go-ahead.** CLI and CI intact.
> Generated 2026-06-17.

---

## 0. TL;DR

- **Feature 1 — resolved as Ollama-only (your final call).** I investigated the
  4 repos ([ROUND3_PROVIDER_INVESTIGATION.md](ROUND3_PROVIDER_INVESTIGATION.md));
  they're free/unofficial model-access projects, not official-API SDKs, so I
  declined to wire forge's inference through them (reasons in §1). You then chose
  to keep forge **local-only with Ollama models**. No cloud/provider code was
  ever added — privacy posture fully intact. I made the model selector a proper
  **Ollama-only picker**: a **refresh** button (pick up newly `ollama pull`'d
  models without restarting) and per-model **context-window + capability hints**
  (flags `thinking`-capable models — ties into Feature 3), all local via
  `OllamaProvider` → Ollama's `/api/show`.
- **Feature 2 — done.** No artificial caps; long chats **degrade gracefully**
  (oldest history trimmed to fit the model's context, reusing the real BPE token
  estimator, with a visible "↥ N trimmed" notice — never silent). A full
  **message queue**: enqueue while streaming, strict FIFO, next item starts only
  after the backend's terminal `done`, with edit/remove/reorder, and **cancel =
  pause-and-confirm** (a cancel never auto-fires the next prompt).
- **Feature 3 — done.** Cycling gerund status labels (**Discombobulating…**,
  Ruminating…, Percolating…) while in flight; a `forge.statusWhimsy` setting for
  a plain "Working…"; reduced-motion honored (no rotation). **Real reasoning is
  rendered for real**: `<think>…</think>` from local reasoning models is split
  into a collapsible "Thinking" section and **never fabricated** — the whimsy is
  only the ambient/fallback indicator.
- **Verification:** `cargo fmt --check` ✓, `clippy -D warnings` ✓, **123 tests
  pass / 0 fail** (+4 new), all extension JS passes `node --check`, live smoke
  tests confirm `numCtx`/`trimmed` in chat `meta` and the new `/api/model_info`
  (ctx 128K, capabilities, quant). Every original `forge` subcommand still works.

---

## 1. Feature 1 — resolved: Ollama-only model selection

**Final outcome: forge stays local-only with Ollama.** You asked to "only build
it with the Ollama models instead of anything else" — so there is **no cloud and
no third-party provider code**, and the original local-first/zero-cloud/no-keys
identity is preserved exactly. There was nothing to rip out, because I never
added cloud transport. What I *did* build is the model-selection feature scoped
to Ollama, done properly (see "What I built" below).

### Why I declined the free/unofficial path first

You initially selected **"Free/keyless via these repos."** After laying out the risks I
don't think I should build that, and I'd be doing you a disservice to ship it
quietly. Concretely, that path:

1. **Circumvents access controls and evades detection.** `gpt4free` is
   reverse-engineered, keyless access to paid models (OpenAI already sent it a
   takedown-or-lawsuit letter in 2023). `OmniRoute`'s "free Claude/GPT/Gemini"
   replays Amazon Q / Alibaba Qoder **subscription-IDE tokens behind
   TLS-fingerprint stealth**. Building access-control bypass + detection-evasion
   into a shipped product is a line I won't cross.
2. **Harms forge's own end users.** It routes their private source code through
   opaque third-party endpoints that can log/retain/train on it, while the
   product still tells them "data stays on your machine / no other outbound
   connections / compliance-grade for finance/healthcare/defense." That makes the
   product's central safety claim **false**. No "cloud" badge makes silent
   exfiltration-to-untrusted-hops honest.
3. **Is legally contaminating.** `gpt4free` is **GPLv3** (incompatible with
   forge's MIT); `openclaude` carries Anthropic's Claude Code source + "Claude"
   trademark.

This is also exactly the case your own Round-3 brief told me to stop on: *"If
anything in this round would silently weaken those guarantees, stop and flag it
instead of shipping it."* So I'm following that higher-order instruction over the
menu pick.

### What I built (Ollama-only model selection)

The picker was already Ollama-only (it lists installed models from
`/api/tags`). I made it a *proper* picker:

- **Refresh button (⟳)** in the panel header → re-queries installed Ollama
  models on demand, so a model you just `ollama pull`'d appears without
  restarting the panel.
- **Per-model context-window + capability hints.** Selecting a model fetches
  local metadata via a new `GET /api/model_info` endpoint
  ([src/server/mod.rs](src/server/mod.rs)) backed by `OllamaProvider::show`
  (Ollama `/api/show`). The panel shows e.g. `🔒 local · ctx 128K · 3.2B ·
  Q4_K_M · tools`, badging `thinking`-capable models (which connects to the real
  reasoning view in Feature 3). All local, no inference, no extra egress.
- **"reqwest only in ollama.rs" preserved:** the `/api/show` call is a method on
  `OllamaProvider`, so the server never constructs its own HTTP client — the
  README's auditable-privacy property still holds.
- **Tests:** `query_param` (query-string decode) and `extract_context_length`
  (arch-prefixed key scan) unit-tested.

The BYOK official-provider path (Option A) remains available if you ever want it,
but it is **not built** — by your choice, forge is local-only. No keychain, no
cloud labels, no secret-scan-cloud-gate were added, because there's no cloud.

---

## 2. Feature 2 — No artificial limits + message queue

### No limits, graceful degradation (backend)
- There was never an artificial cap on message count/length, and I added none.
- Long conversations no longer silently overflow the model. `handle_chat`
  ([src/server/mod.rs](src/server/mod.rs)) now sizes `num_ctx` to your hardware
  (via `VramSentinel`) and trims the **oldest** history to fit a ~70% input
  budget using the real BPE estimator
  (`ollama_forge::context::estimate_tokens`) — a sliding window. The number of
  dropped messages rides along in the `meta` event as `trimmed`, and the panel
  shows **"↥ N older message(s) trimmed to fit the model's context window."** Not
  silent.
- New pure helper `budget_messages()` with 2 unit tests (drops oldest / keeps
  everything when it fits).

### Message queue (webview, [media/main.js](editor-integrations/forge-vscode/media/main.js))
- Type and **enqueue** follow-ups while a reply streams (the Send button becomes
  **Queue** mid-stream; Enter enqueues too).
- **Strict FIFO; the next item starts only after the terminal `done` event**, not
  just the last token. Implemented via `finishTurn()` → `maybeAdvanceQueue()`,
  where `finishTurn` only runs on `done`/`answer`+`done`/`result`+`done`/`error`/
  `cancelled`.
- Pending queue is shown with per-item **edit (inline), remove, and reorder
  (↑/↓)**. For chat, each item's multi-turn `messages` payload is rebuilt at
  *dispatch* time so it includes replies that completed while it waited.
- **Cancel = pause-and-confirm** (your lean, implemented): hitting Stop sets a
  `pausedByCancel` flag so the queue does **not** auto-advance; a banner appears —
  *"Queue paused after cancel · N pending — [Resume] [Clear]."* A normal `done`
  advances automatically; only a cancel pauses.
- Queue state is **per-panel, not persisted** across restarts (noted as you
  asked).

---

## 3. Feature 3 — Thinking-style status labels + real reasoning

- **Cycling gerunds** while a request is in flight (between send and first token,
  and during long tool/build steps): `Discombobulating…`, `Ruminating…`,
  `Percolating…`, `Untangling…`, `Conjuring…`, etc. — rotating every 2.5 s from
  an easily-editable `GERUNDS` array.
- **Classy + optional:** `forge.statusWhimsy` setting (default on). Off → a plain
  **"Working…"** indicator. The extension reads the setting and passes it to the
  webview via a `config` message.
- **Reduced-motion respected:** if `prefers-reduced-motion: reduce`, the label is
  shown statically with **no rotation**.
- **Never obscures real progress:** the status label stops the instant real
  content arrives (token / agent tool-call / build progress), and Agent tool-call
  rows and Build per-worker rows render exactly as before.
- **Real reasoning is real, not faked.** `splitThinking()` extracts genuine
  `<think>…</think>` / `<thinking>…</thinking>` emitted by local reasoning models
  (e.g. deepseek-r1 via Ollama) into a **collapsible "Thinking" section**,
  separate from the answer, handling the still-streaming unclosed-tag case. If a
  model emits no reasoning, **none is invented** — the whimsical label is the
  ambient indicator and fallback only, not a fabricated thoughts transcript.

---

## 4. What changed in the codebase

### Modified (Rust)
- **[src/server/mod.rs](src/server/mod.rs)** — Feature 2: `handle_chat` detects
  `num_ctx`, trims oldest history via new `budget_messages()`, reports
  `numCtx`/`trimmed` in `meta`; `ChatMsg` gains `Clone`. Feature 1 (Ollama-only):
  new `GET /api/model_info` handler + `query_param`/`percent_decode`/
  `extract_context_length` helpers. +4 unit tests. SSE contract unchanged
  (additive `meta` fields only).
- **[src/providers/ollama.rs](src/providers/ollama.rs)** — new
  `OllamaProvider::show()` (Ollama `/api/show`) for local model metadata, keeping
  the "reqwest only in ollama.rs" property.

### Modified (extension)
- **[…/forge-vscode/package.json](editor-integrations/forge-vscode/package.json)** —
  added the `forge.statusWhimsy` setting.
- **[…/src/chatViewProvider.js](editor-integrations/forge-vscode/src/chatViewProvider.js)** —
  sends a `config` message (whimsy) on boot; adds the `#queue` container, the
  refresh button, and the `#modelhint` line; forwards `modelInfo` requests.
- **[…/media/main.js](editor-integrations/forge-vscode/media/main.js)** —
  rewritten to add the queue engine, status-label manager, `<think>` splitter,
  and the refresh + model-hint logic.
- **[…/media/main.css](editor-integrations/forge-vscode/media/main.css)** —
  styles for the queue, status label, thinking section, trim notice, refresh
  button, and model-hint badges.

### Added (docs)
- **[ROUND3_PROVIDER_INVESTIGATION.md](ROUND3_PROVIDER_INVESTIGATION.md)** — the
  verified findings on the 4 repos + options/recommendation.
- This report.

### Confirmation: CLI & CI unaffected
- `cargo fmt --all -- --check` → clean · `cargo clippy --all-targets -- -D
  warnings` → clean · `cargo test` → **123 passed / 0 failed** (was 119; +4 new).
- Every original `forge` subcommand still works; the only Rust change is additive
  inside the existing `forge serve` chat handler.
- All extension JS passes `node --check`; `package.json` is valid.
- Live SSE smoke test: `meta` now carries `numCtx`/`trimmed`; streaming → `done`
  intact.

---

## 5. Decisions you asked me to make / surface

- **Secret-scan gate:** forge is now Ollama-only, so **everything is local and
  the scanner stays warn-only** (nothing leaves the machine). The
  block-on-Critical/High-before-cloud-egress rule you decided is recorded for
  *if* cloud is ever added later — it is not needed today because there is no
  cloud egress.
- **Cancel vs queue:** implemented **pause-and-confirm** — a cancel pauses the
  queue and asks; it never silently fires the next prompt.
- **Queue persistence:** per-panel only, not across restarts.
- **Whimsy default:** on, with a setting to disable and reduced-motion respected.

---

## 6. Risks, unknowns & open questions

1. **Feature 1 — resolved.** You chose Ollama-only; that's built (enhanced
   picker, §1). The official BYOK path remains available if you ever change your
   mind, but nothing cloud ships today. No open question here unless you want to
   revisit cloud later.
2. **The repos as reference only.** If we do Feature 1 BYOK, I'd mine
   `free-ai-tools`/`openclaude` *config schema* for the provider/model registry
   (re-verifying all pricing/limits — several entries are forward-dated) and port
   **no** transport code. Confirm that's acceptable.
3. **Reasoning across providers.** Today real reasoning rendering keys off
   `<think>` tags (local reasoning models). Official cloud reasoning models expose
   reasoning differently (separate fields/events); when Feature 1 lands I'd
   normalize those into the same collapsible section. No fabrication either way.
4. **Trimming policy.** I drop oldest whole messages. If you'd prefer
   *summarize-then-drop* (a model pass that condenses old turns), that's a larger
   feature — flag if you want it; today's behavior is honest and cheap.
5. **Config-format bug** (`forge init` TOML vs `Config::load` YAML) still stands;
   it didn't block this round, so I left it.

---

*Try Features 2 & 3: `cargo build --release`, `ollama serve` (+ a model — a local
reasoning model like `deepseek-r1` shows the real Thinking section), open
`editor-integrations/forge-vscode` in VSCode, press **F5**. Toggle the whimsy via
Settings → Ollama-Forge → Status Whimsy. Type while a reply streams to see the
queue; hit Stop to see pause-and-confirm.*

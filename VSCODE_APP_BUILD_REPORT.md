# Build Report — Ollama-Forge → VSCode-Based App with a Cursor-Style Chat Panel

> Round 1: **Phase 1 (`forge serve`) and Phase 2 (VSCode extension + side chat
> panel) are built, runnable, and verified.** Phase 3 (fork/rebrand/package
> Code-OSS) is scaffolded and planned, not executed. The existing CLI and CI are
> intact. Generated 2026-06-17.

---

## 0. TL;DR

- Added a **`forge serve`** subcommand: a local-only (127.0.0.1) HTTP+SSE
  backend that exposes Chat / Agent / Build / models / status / cancel, **reusing
  the existing inference code paths** (`OllamaProvider`, the `Agent` loop, the
  `Orchestrator`). Rules, skills, the secret scanner, and replay logging behave
  exactly as in the CLI.
- Added a **pure-JavaScript VSCode extension** (`editor-integrations/forge-vscode`)
  with a **side-docked webview chat panel**: Chat/Agent/Build mode toggle, model
  picker, streaming, stop/cancel, editor-context attach (file / selection / @files),
  and structured rendering of agent tool-calls and build progress.
- **No new Rust dependencies** (hand-rolled HTTP/SSE on the existing `tokio`
  stack). **No npm dependencies / no build step** for the extension (runs from
  source in the Extension Development Host).
- **Verified live**: `forge serve` streamed real tokens from local Ollama over
  SSE, and the extension's exact Node SSE parser correctly parsed `meta → token
  → done` against the running server.
- CI gates green: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`,
  **119 tests pass / 0 fail** (was ~110; +13 new). Every original `forge`
  subcommand still works.
- **Working app name: `ForgeCode`** (placeholder, needs your confirmation — see
  §5). Also flagging the existing 3-way repo-name inconsistency.

---

## 1. What I built this round & how to try it

### Phase 1 — `forge serve` (Rust backend)

A new subcommand starts a persistent local server:

```bash
cargo build --release            # or: cargo build  (debug)
forge serve                      # binds 127.0.0.1:7878 by default
forge serve --port 0             # OS-assigned port, printed on startup
```

On startup it prints a machine-readable line the extension parses:

```
FORGE_SERVE_READY {"host":"127.0.0.1","port":49597,"version":"0.1.0 (ecc317b…)"}
```

Endpoints (all JSON; the three streaming ones are `text/event-stream`):

| Method & path | Reuses | Streams |
| :-- | :-- | :-- |
| `GET /health` | — | no |
| `GET /api/status` | `VramSentinel` + `OllamaProvider::health_check` | no |
| `GET /api/models` | `OllamaProvider::list_models` | no |
| `POST /api/chat` | `OllamaProvider::generate_streaming` | SSE: `meta`→`token`*→`done`/`error`/`cancelled` |
| `POST /api/research` | `agent::Agent::run` | SSE: `meta`→`step`*→`answer`→`done` |
| `POST /api/build` | `orchestrator::Orchestrator::execute_with_progress` | SSE: `meta`→`progress`*→`result`→`done` |
| `POST /api/cancel` | cancellation registry | no |

Try it with curl (the live transcript I captured):

```bash
forge serve --port 7878 &
curl -s http://127.0.0.1:7878/api/status            # hardware + Ollama health
curl -s http://127.0.0.1:7878/api/models            # installed models
curl -s -N -XPOST http://127.0.0.1:7878/api/chat \
  -H 'Content-Type: application/json' \
  -d '{"id":"1","model":"llama3.2:latest","messages":[{"role":"user","content":"Say hello in 3 words."}]}'
# → data: {"type":"meta",...}
#   data: {"type":"token","text":"Hello"}
#   data: {"type":"token","text":" there"} ...
#   data: {"type":"done","bytes":19}
```

### Phase 2 — VSCode extension + side chat panel

Pure JS, **no install step**:

1. `cargo build --release` (so the `forge` binary exists), and have
   `ollama serve` running with a model pulled.
2. Open `editor-integrations/forge-vscode` in VSCode and press **F5** (or
   `code --extensionDevelopmentPath="$(pwd)/editor-integrations/forge-vscode"`).
3. If `forge` isn't on PATH, set **Settings → Ollama-Forge → Server Path** to
   `…/target/release/forge`.
4. Click the **anvil icon** in the Activity Bar. Drag the panel to the
   **Secondary Side Bar** (right) for the Cursor/Windsurf placement.

The panel (described, since a headless screenshot isn't possible):

```
┌──────────────────────────────────────────────┐
│ [Chat][Agent][Build]        model: qwen…7b ▾  │  ← mode toggle + model picker
│ ● Local · Ollama ✓ · apple-silicon · 11.4 GB  │  ← status line (differentiators)
├──────────────────────────────────────────────┤
│ You                                            │
│   refactor this function …   ↳ context: x.rs   │
│ Assistant · qwen2.5-coder:7b                   │
│   ⚠ secret scan: 1 finding in attached context │  ← scan runs before send
│   Here's the refactor: ```rust … ``` (streaming)│
│   ── agent mode: round 1 web_search (ok) … ──  │  ← structured tool-calls
│   ── build mode: ⏳ worker Frontend on 3b … ── │  ← per-worker progress
├──────────────────────────────────────────────┤
│ [+ file] [+ selection] [@ files]               │  ← editor context
│ ┌────────────────────────────┐  ┌──────┐      │
│ │ Ask anything…              │  │ Send │      │
│ └────────────────────────────┘  └ Stop ┘      │
└──────────────────────────────────────────────┘
```

---

## 2. Architecture decisions (and why)

**Transport: SSE over a hand-rolled HTTP/1.1 server (not WebSocket, not a new
framework).**
- Chat is one-directional server→client streaming — exactly SSE's shape.
  Cancellation is the only client→server signal mid-stream, and a separate
  `POST /api/cancel` (carrying the request `id`) handles it cleanly, so I didn't
  need WebSocket's bidirectional channel, upgrade handshake, or frame masking.
- This codebase deliberately avoids heavy deps (hand-rolled URL-encoding, HTML
  stripping, NDJSON parsing). Adding `axum`/`hyper`/`tungstenite` would have
  churned `Cargo.lock` and clashed with that ethos. SSE framing (`data: {json}\n\n`)
  is trivial to emit correctly by hand over the `tokio` net stack already pulled
  in by `features=["full"]`. **Net new Rust dependencies: zero.**

**Where the network boundary sits: extension host, never the webview.**
- The webview talks to the extension host via `postMessage`; the **extension
  host** (Node, full network access) makes the HTTP/SSE calls to `forge serve`.
  The webview's CSP is `connect-src 'none'` — it *cannot* make network calls.
  This is a hard guarantee that the chat UI can't phone home, reinforcing the
  zero-telemetry promise, and it sidesteps webview CSP/networking pitfalls.

**Backend launch & lifecycle: the extension owns it.**
- `ForgeBackend.ensureStarted()` spawns `forge serve --port 0`, reads the
  `FORGE_SERVE_READY` line from stdout to discover the OS-assigned port, then
  uses that base URL. The process is killed on extension deactivate. Power users
  can instead run their own `forge serve --port N` and set `forge.serverPort` to
  attach without spawning.

**Panel placement: Activity Bar container (documented), draggable to the right.**
- A `viewsContainers.activitybar` + webview view is the portable, manifest-only
  way to register a side panel. VSCode has no manifest key to *force* the
  Secondary Side Bar by default, so the panel registers on the left and the user
  drags it right (VSCode remembers it). The Phase 3 fork *can* default it to the
  right via a startup layout — noted in `desktop/README.md`.

**Editor context passing.**
- `+ file` / `+ selection` / `@ files` are resolved in the extension host
  (`vscode.window.activeTextEditor`, `workspace.findFiles` quick-pick), truncated
  to 16 KB each, sent as `context: [{path, content}]`. Chat/Agent prepend it to
  the prompt; the secret scanner runs over it first and any findings surface as a
  warning event before the model sees it.

**Cancellation is real, not cosmetic.**
- Each streaming request registers a `tokio::sync::Notify` under its `id`.
  `POST /api/cancel` fires it; the handler's `select!` aborts the generation task
  (which drops the reqwest stream to Ollama) and emits a `cancelled` event. The
  extension also destroys the socket client-side. `Notify::notify_one` stores a
  permit so an early cancel isn't lost.

**Multi-turn chat via prompt-flattening (a deliberate simplification).**
- Chat folds the conversation + attached context into a single `/api/generate`
  prompt so it reuses `generate_streaming` verbatim. Native `/api/chat` streaming
  is a clean follow-up but wasn't needed to ship a working panel.

---

## 3. What changed in the existing codebase

### Added (Rust)
- **`src/server/mod.rs`** — the `forge serve` backend (HTTP/SSE, handlers,
  cancellation registry, plus a `maybe_log_replay` mirroring the CLI's). Includes
  6 unit tests.
- **`src/codeblocks/mod.rs`** — the labeled-code-block extractor, **moved out of
  `main.rs`** so both `forge build --output` and the server's build endpoint
  share one path-traversal guard. Includes 2 unit tests.
- **`tests/server_protocol.rs`** — 5 integration tests that drive the real server
  over loopback TCP (health, 404, OPTIONS/CORS, cancel-unknown-id, malformed
  body) — no Ollama required.

### Modified (Rust)
- **`src/lib.rs`** — registered `pub mod codeblocks;` and `pub mod server;`.
- **`src/cli/mod.rs`** — added the `Serve { port, host }` subcommand.
- **`src/main.rs`** — dispatch for `Commands::Serve`; the build path now calls
  `ollama_forge::codeblocks::extract_and_write_code_blocks`; removed the three
  now-relocated extractor functions. **No behavior change to any command.**

### Added (extension + scaffold, no Rust impact)
- **`editor-integrations/forge-vscode/`** — `package.json`, `src/extension.js`,
  `src/backend.js`, `src/chatViewProvider.js`, `media/main.js`, `media/main.css`,
  `media/forge.svg`, `README.md`, `.vscodeignore`.
- **`desktop/`** — `README.md` (Phase 3 plan), `product.json.example`,
  `scripts/bootstrap.sh`, `scripts/bundle-forge.sh` (scaffolds that `exit 0`
  before mutating anything).

### Confirmation: CLI and CI unaffected
- `cargo fmt --all -- --check` → clean.
- `cargo clippy --all-targets -- -D warnings` → clean.
- `cargo test` → **119 passed, 0 failed** (baseline ~110; +13 new).
- `forge --help` lists `serve` alongside every original subcommand; the existing
  `tests/build_extractor.rs` (an independent reference parser) still passes
  unchanged.
- `forge serve` is purely additive — no existing command's code path was altered
  except the build-output extractor's *location* (same logic, now shared).

---

## 4. Verification evidence

- **Live SSE chat** against local Ollama (`llama3.2:latest`): received
  `meta` → 4 `token` events (`Hello` / ` there` / ` friend` / `!`) → `done`.
- **`/api/status`** returned real detection: `apple-silicon`, 11468 MB free VRAM,
  `recommended_model: qwen2.5-coder:7b`, `ollamaHealthy: true`.
- **Extension's Node SSE parser** (the exact code from `backend.js`) run against
  the live server parsed `meta`/`token`/`done`/`_end` correctly.
- **All JS** passes `node --check`; `package.json` and `product.json.example` are
  valid JSON; both shell scaffolds pass `bash -n`.
- What is **not** yet exercised: the VSCode webview rendering itself (needs an
  interactive host — can't run headlessly) and live Agent/Build over the panel
  (protocol is identical to chat, which is proven; Build needs a multi-model rig).

---

## 5. Phase 3 plan (scaffolded; full plan in `desktop/README.md`)

Concrete path to the downloadable, rebranded app:

1. **Fork Code-OSS** at a pinned tag; confirm a clean vanilla build first.
2. **Rebrand** via `product.json` (name, bundle id, data folder, icons, About
   strings) — template in `desktop/product.json.example`.
3. **Bundle** the Phase 2 extension as a *built-in* and ship the `forge` binary
   in `resources/app/bin`, with the extension's `serverPath` defaulted to it —
   `desktop/scripts/bundle-forge.sh`.
4. **First-run UX**: detect Ollama via `/api/status`; offer install / `ollama
   serve` / `ollama pull <recommended_model>`.
5. **Package** per OS (`.dmg` / `.deb` / `.AppImage` / Windows installer).

**Licensing / telemetry / signing — flagged (not legal advice):**
- Code-OSS source is **MIT**, but **Microsoft's name/logo/icons are proprietary**
  and must be replaced in a fork (steps 2–3).
- The **Microsoft Marketplace ToS forbids non-MS products** → point
  `extensionsGallery` at **Open VSX** (done in the example). Implication: any
  third-party extension we want available must be on Open VSX; our own chat panel
  ships as a built-in and needs no gallery.
- **Strip Microsoft telemetry** (`enableTelemetry: false`, blank keys) to honor
  the zero-telemetry promise.
- **Signing/notarization** is required for a trustworthy download: Apple
  Developer ID + notarization (macOS), Authenticode/EV (Windows). Needs paid
  accounts + CI secrets.

**Effort estimate (honest):** ~2–3 working weeks to a first *signed* release
(fork+rebrand 3–5d, bundle+layout 2–3d, first-run 2–4d, packaging 3–6d, signing
3–5d), **plus continuous fork-rebasing** onto new Code-OSS releases, plus
auto-update (~1–2 weeks, separate). Matching Cursor/Windsurf is a maintained-fork
commitment, not a one-shot. Crucially, **Phases 1–2 already deliver the product
experience** inside stock VSCode today; Phase 3 is distribution, not new
capability.

---

## 6. Risks, unknowns & open questions for you

1. **App name + repo name.** I used `ForgeCode` as a placeholder for the desktop
   app. Please confirm, and pick one canonical project name — the repo currently
   carries `ollamax` (Cargo `repository`), `ollama-forge` (crate/README), and
   `Ollama-Optimizer` (local dir). I recommend standardizing on `ollama-forge` +
   app `ForgeCode`.
2. **Secret-scan policy in the UI: warn vs block.** Today the panel *warns* (a
   banner) when attached context contains secrets but still sends it. The CLI's
   `audit` is hard-fail. Do you want chat to **block** sending on Critical/High by
   default (with an override), to fully honor "refuses to send"? Easy to switch.
3. **Native `/api/chat` streaming vs prompt-flattening.** I flattened multi-turn
   into `/api/generate` to reuse existing code. Want me to add a streaming
   `/api/chat` path for true role-aware multi-turn (better with some chat
   templates)?
4. **Build endpoint & file writing.** The panel shows merged build output inline;
   files are written only when an `output_dir` is supplied to the backend. Should
   the panel expose a "write to folder" affordance (with a confirmation), reusing
   the shared `codeblocks` extractor?
5. **Panel default placement.** Manifest can't force the Secondary Side Bar; users
   drag it right today. Acceptable for the extension, or should I prioritize the
   right-default in the Phase 3 fork?
6. **WebSocket later?** SSE covers everything now. If we ever want richer
   client→server interaction mid-stream (e.g. live tool approvals in agent mode),
   WebSocket may be worth revisiting. Flagging, not recommending yet.
7. **Config format bug carried over.** The pre-existing `forge init` (TOML) vs
   `Config::load` (YAML `config.yaml`) mismatch from the earlier analysis still
   stands; `forge serve` reads `Config` the same way the CLI does, so it inherits
   the same behavior. Worth fixing project-wide, but I left it untouched to avoid
   scope creep here.

---

*Try it in ~3 commands: `cargo build --release` → `ollama serve` (+ a pulled
model) → open `editor-integrations/forge-vscode` in VSCode and press F5. Full
extension docs: [`editor-integrations/forge-vscode/README.md`](editor-integrations/forge-vscode/README.md).
Full Phase 3 plan: [`desktop/README.md`](desktop/README.md).*

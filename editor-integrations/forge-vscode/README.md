# Ollama-Forge — VSCode Extension (Cursor-style chat panel)

A side-docked AI chat panel for [Ollama-Forge](../../README.md). Chat, run the
tool-using research **Agent**, and kick off a parallel **Build** — all against
your **local** Ollama daemon. No cloud, no API keys, no telemetry.

This extension is intentionally **thin**: it is pure JavaScript with **no build
step and no npm dependencies**. It talks to the Rust backend (`forge serve`)
over a local HTTP/SSE API; it never does inference itself and never bypasses
`OllamaProvider`.

```
 webview (this UI)  ──postMessage──▶  extension host (Node)  ──HTTP/SSE──▶  forge serve  ──▶  Ollama
   (no network; CSP connect-src 'none')                                    (127.0.0.1 only)
```

## Prerequisites

1. **Build the `forge` binary** (from the repo root):
   ```bash
   cargo build --release        # produces target/release/forge
   ```
2. **Ollama running** with at least one model:
   ```bash
   ollama serve                 # in another terminal
   ollama pull qwen2.5-coder:7b
   ```

## Run it (no install, no npm)

1. Open the **`editor-integrations/forge-vscode`** folder in VSCode.
2. Press **F5** ("Run Extension"). This opens an *Extension Development Host*
   window with the extension loaded.
   - Alternatively, from a terminal:
     ```bash
     code --extensionDevelopmentPath="$(pwd)/editor-integrations/forge-vscode"
     ```
3. In the dev window, set the path to your built binary if `forge` isn't on your
   PATH: **Settings → Ollama-Forge → Server Path** →
   `/abs/path/to/Ollama-Optimizer/target/release/forge`.
4. Click the **anvil icon** in the Activity Bar → the **Chat** panel opens.
   - To get the Cursor/Windsurf right-side feel, **drag the panel to the
     Secondary Side Bar** (right edge). VSCode remembers the placement.

The extension auto-launches `forge serve --port 0` and discovers the port from
its `FORGE_SERVE_READY` line. To instead attach to a server you started
yourself, run `forge serve --port 7878` and set **Server Port** to `7878`.

## Using it

- **Mode toggle** (top-left): **Chat** (multi-turn) · **Agent** (web/wiki/arxiv
  tools) · **Build** (parallel multi-model orchestration with live per-worker
  progress).
- **Model picker** (top-right) is populated from your installed Ollama models;
  the hardware-recommended default is preselected.
- **Context**: `+ file` (active editor), `+ selection`, `@ files` (workspace
  quick-pick). Attached items show as chips and are scanned for secrets before
  being sent — findings appear as a warning banner.
- **Stop** cancels an in-flight request (drops the socket and calls
  `/api/cancel`).
- The **status line** shows Ollama health, detected GPU, free VRAM, and the
  recommended model — the product's differentiators, surfaced in the UI.

## Auto model routing (Feature 2)

The model picker defaults to **Auto** — the existing `TaskRouter` classifies each
task (Simple → Complex/Architect) and picks a **local** model by size tier
(simple → smallest, complex → largest). The choice + a one-line "why" is shown on
each reply (`🔀 Auto: Complex task (0.67) → …`). A manual pick always overrides.
Auto stays **local-only** and never escalates to a paid cloud provider.

## The Central Hub (Feature 3)

A **separate Activity Bar panel** ("Forge Hub") of domain **packages** (Web dev,
Game dev, Data/ML, Security, …54 categories). Click a category's **+** to
*activate* its package: the backend compiles the domain into **rules + skill
scaffolds + curated references**, and the extension writes them into your
`rules/` and `skills/` config dirs so the agent steers toward idiomatic output.
It's transparent — the panel shows exactly what each package injects (and it's
reversible: delete the files).

**Support these maintainers** is an explicit, **opt-in** action: review the exact
repo list in your browser and consciously star all or some. Never automatic.
Needs `forge.accountServer` set and a GitHub sign-in.

## Usage telemetry (anonymous, opt-out)

The app sends **anonymous usage metadata** (counts of chat/agent/build, which
model/provider, token counts, and language inferred from a file *extension*) to
power your **web usage dashboard**. It **never** sends prompts, code, file
contents, file paths, or repo names — `src/telemetry.js` builds a strict
allowlisted payload and the backend rejects anything content-shaped. Control it:

- **Settings → Ollama-Forge → Telemetry** (`forge.telemetry`, default on) — off
  sends nothing. A one-time notice on first run lets you turn it off immediately.
- View / export / delete your data at `<accountServer>/dashboard`.

## Sign in with GitHub or Google (optional — identity)

The panel header shows a **Sign in with GitHub** control when an account server
is configured. It links the app to the *same account* as the website (the
Cursor/Windsurf model). It is **identity only** — local inference works fully
signed-out; sign-in never gates anything.

1. Deploy the account backend (see [`../../website/README.md`](../../website/README.md))
   or run it locally (`http://localhost:3000`).
2. Set **`forge.accountServer`** to that URL.
3. Click **Sign in with GitHub** → the system browser opens, you authorize, and
   the app receives a token via a one-shot `127.0.0.1` loopback (OAuth Authorization
   Code + **PKCE**). If loopback is awkward, use **device code** (shows a code to
   type in the browser).

The token is stored in VSCode **SecretStorage** (OS-keychain-backed). The app
never sees the GitHub client secret; your code never goes to the account server.

## Settings

| Setting | Default | Meaning |
| :-- | :-- | :-- |
| `forge.serverPath` | `forge` | Path to the `forge` binary. |
| `forge.serverPort` | `0` | If > 0, attach to an existing `forge serve` instead of launching one. |
| `forge.autoStartServer` | `true` | Launch the backend when the panel opens. |
| `forge.statusWhimsy` | `true` | Playful rotating status words; off → plain "Working…". |
| `forge.accountServer` | `""` | Account backend URL for GitHub sign-in (identity only; blank hides sign-in). |

## Packaging (optional)

To produce a `.vsix` you can share, install `@vscode/vsce` (needs network) and
run `vsce package`. For the **built-in / desktop-app** path, this whole folder
is bundled into the Code-OSS fork — see
[`../../desktop/README.md`](../../desktop/README.md).

## Notes / limitations (this round)

- Chat is multi-turn by flattening the conversation into one `/api/generate`
  prompt (reuses the existing streaming path). Native `/api/chat` streaming is a
  clean follow-up.
- Markdown rendering is intentionally minimal (fenced code + inline code).
- Build writes files to disk only when an `output_dir` is provided to the
  backend; the panel currently shows the merged output inline.

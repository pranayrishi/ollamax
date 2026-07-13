# Ollama-Forge — standalone desktop app

An Electron shell around the existing **forge engine**, modeled on the Sattva AI
desktop app. It is the **only** user-facing surface: users download and open the
app — no terminal, no CLI, no one-liner. The engine runs **hidden** inside the
app as `forge serve` (a local HTTP+SSE backend); the window hosts the existing
Ask / Agent / Team / Build UI, which talks to that local server. Inference stays local
(app → forge serve → local Ollama by default). An advanced user may explicitly
select a separately operated, literal-loopback OpenAI-compatible server; the
app never starts one, discovers one on the network, or automatically falls back
to a cloud provider. The desktop shell also supports explicit push-to-talk using
a local `whisper.cpp` runtime and optional on-device speech output; it never
silently substitutes a hosted STT/TTS API.

## Architecture

```
Ollama-Forge.app
├── (Electron shell: main.js spawns the engine + opens the window)
├── Contents/Resources/bin/forge        ← the engine (forge serve --port 0, hidden)
│                                            └─ local Ollama, or an explicitly selected loopback endpoint
├── Contents/Resources/voice/            ← manifest; optionally staged local speech assets
└── renderer/                           ← the REUSED panel UI
    ├── index.html
    ├── vscode-shim.js   acquireVsCodeApi() → bridge
    ├── bridge.js        the panel's host protocol, but over HTTP/SSE to forge serve
    ├── theme.css        maps VS Code theme vars to the app palette
    └── main.js/main.css ← copied verbatim from the extension's media/ (single source)
```

The packaged `voice/manifest.json` describes the local speech-runtime contract.
The checked-in manifest deliberately says `bundled: false`. For an `app-v*`
release, CI builds pinned `whisper.cpp` v1.9.1 natively, verifies the reviewed
`ggml-base.en.bin` size and SHA-256, stages its license, and flips the manifest
to `bundled: true` only in that disposable packaging workspace. Do not assume a
download contains those assets until the matching public workflow has passed;
when they are absent the app shows actionable local setup instead of falling
back to cloud speech.

- **No rewrite of the AI:** `main.js` (the chat UI) runs unchanged; `bridge.js`
  is the same logic `chatViewProvider.js` used, pointed at `forge serve`.
- **Sign-in** reuses the existing desktop OAuth (GitHub/Google loopback) via the
  account server (`FORGE_ACCOUNT_SERVER`).

## Local model runtimes

Ollama is the default local runtime. The model list also includes models that
an operator has declared in `forge.toml` under a separately run local
OpenAI-compatible server. The endpoint must resolve to a literal loopback
address and `/v1`; the user selects it explicitly as
`local:<endpoint>/<model>`. This is the same selector used by Chat, Agent,
Research, and Team. It does not make a server-class checkpoint an Ollama pull,
and Auto routing never silently chooses it.

```toml
[[local_endpoints]]
id = "lab"
url = "http://127.0.0.1:8000"
max_parallel_requests = 2

[[local_endpoints.models]]
id = "deepseek-v4-flash"
served_model = "DeepSeek-V4-Flash"
label = "Lab DeepSeek V4 Flash"
thinking = true
```

`api_key_env` may name an environment variable for a token required by that
*local* server; the token is not stored in the config or shown in the picker.
The endpoint's request limit is shared across Team roles, including parallel
read-only scouts. Build/Orchestrator remains Ollama-only and rejects `local:`
selectors instead of pretending it can provision or manage a server.

DeepSeek V4 Flash/Pro and MiniMax M3 remain separately self-hosted,
server-class choices—not casual laptop downloads and never cloud fallbacks. The
generic compatibility adapter sends text plus images only when the operator
declares the model vision-capable. It does not expose DeepSeek V4's
model-specific encoding/reasoning channel, MiniMax M3 video/native tools or
structured reasoning, or a separate streamed thinking lane. `thinking = true`
is a picker disclosure, not a claim that those provider-specific features are
available. See the root [model-runtime guide](../README.md#model-catalog-and-local-runtimes)
for the consumer-local Qwen, Gemma 4, and DeepSeek-R1 options and all model
license/deployment caveats.

## Local voice and spatial context

The Electron shell adds a Clicky-style interaction surface without a Clicky
dependency or hosted speech provider:

- **Push-to-talk is explicit.** Holding the microphone control requests
  microphone permission, records locally, creates an in-memory WAV, and invokes
  local `whisper.cpp` when a runtime is available. It can be configured with
  `OLLAMAX_WHISPER_PATH` and `OLLAMAX_WHISPER_MODEL`; otherwise a staged local
  `whisper-cli` plus `ggml-base.en.bin` is discovered when a release includes
  them. The current manifest declares those assets unbundled, so a package must
  be checked rather than assumed to contain them. If neither is available, the
  control is disabled with setup guidance—never redirected to a hosted STT API.
- **Speech output is local.** macOS uses `say`, Windows uses local SAPI, Linux
  uses `espeak-ng`/`espeak` when available, and `OLLAMAX_TTS_PATH` may point to
  an explicitly chosen local command. There is no ElevenLabs or other hosted
  TTS fallback.
- **The cursor cue is visual only.** `⌘/Ctrl+Alt+Space` requests the same
  explicit voice toggle as the microphone control. A small transparent,
  click-through cue appears near the pointer with a fixed local status such as
  “Listening” or “Selected region attached.” It receives no transcript,
  prompt, model output, file path, or pixels, and it cannot click, move the
  pointer, inspect windows, or use accessibility APIs.
- **Select region is a deliberate lasso action.** The app captures displays
  only after the user presses the region button, crops and caps the selected
  area in memory, and drops the full-display capture when selection finishes or
  is cancelled. Only the crop reaches a literal-loopback local vision runtime.
  It is visual context, not permission to control the mouse, accessibility
  APIs, shell, or filesystem.
- **Visual analysis remains bounded.** Chat requires a local vision-capable
  model (Auto can choose an installed local Ollama vision model). Agent and
  Team first create an untrusted local visual brief, then continue through the
  normal workspace-autonomy and approval flow. The brief is transient, omitted
  from memory and replay logs, and disables web tools for that visual turn. A
  local Ollama vision model or explicitly configured loopback vision endpoint
  is required for screen-region work.

## Develop

```
cargo build --release            # build the engine (from repo root)
cd desktop-app && npm ci
npm start                        # prepares UI + engine, launches the app
```

## Package (unsigned, Sattva-style)

```
npm run dist                     # electron-builder → release/  (.dmg/.zip, .exe, .AppImage/.deb)
```
Unsigned via `mac.identity: null` + `hardenedRuntime` + `scripts/afterPack.js`
(ad-hoc `codesign --sign -` of every component incl. the bundled engine, and the
RunAsNode fuse off). **First launch on macOS: right-click → Open → Open** (the
app isn't Developer-ID signed yet). CI: `.github/workflows/release-app.yml`
(`app-v*` tag). The app tag only stages installers on a draft. After its
workflow succeeds, push the matching `v*` tag so `.github/workflows/release.yml`
can verify the complete CLI/VS Code/desktop asset contract and publish it:

```bash
git tag app-vX.Y.Z && git push origin app-vX.Y.Z
# Wait for release-app to succeed and attach the installers to the draft.
git tag vX.Y.Z && git push origin vX.Y.Z
```

Do not label `app-v*` itself as a public download and do not claim that the
currently published `v0.2.0` installers include this source-tree voice or
spatial work.

> The standalone app re-introduces the unsigned first-launch warning that the
> curl one-liner avoided (a browser-downloaded `.app` is quarantined). A truly
> warning-free double-click needs paid signing + notarization — the future step.

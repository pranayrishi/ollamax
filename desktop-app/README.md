# Ollama-Forge — standalone desktop app

An Electron shell around the existing **forge engine**, modeled on the Sattva AI
desktop app. It is the **only** user-facing surface: users download and open the
app — no terminal, no CLI, no one-liner. The engine runs **hidden** inside the
app as `forge serve` (a local HTTP+SSE backend); the window hosts the existing
Ask / Agent / Team / Build UI, which talks to that local server. Inference stays local
(app → forge serve → Ollama).

## Architecture

```
Ollama-Forge.app
├── (Electron shell: main.js spawns the engine + opens the window)
├── Contents/Resources/bin/forge        ← the engine (forge serve --port 0, hidden)
└── renderer/                           ← the REUSED panel UI
    ├── index.html
    ├── vscode-shim.js   acquireVsCodeApi() → bridge
    ├── bridge.js        the panel's host protocol, but over HTTP/SSE to forge serve
    ├── theme.css        maps VS Code theme vars to the app palette
    └── main.js/main.css ← copied verbatim from the extension's media/ (single source)
```

- **No rewrite of the AI:** `main.js` (the chat UI) runs unchanged; `bridge.js`
  is the same logic `chatViewProvider.js` used, pointed at `forge serve`.
- **Sign-in** reuses the existing desktop OAuth (GitHub/Google loopback) via the
  account server (`FORGE_ACCOUNT_SERVER`).

## Develop

```
cargo build --release            # build the engine (from repo root)
cd desktop-app && npm install
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
(`app-v*` tag).

> The standalone app re-introduces the unsigned first-launch warning that the
> curl one-liner avoided (a browser-downloaded `.app` is quarantined). A truly
> warning-free double-click needs paid signing + notarization — the future step.

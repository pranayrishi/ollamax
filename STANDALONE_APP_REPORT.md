# Build Report — Standalone Desktop App (unsigned, modeled on Sattva)

**Date:** 2026-06-18 · **Goal:** make the product a **standalone, double-clickable
app** (Cursor/Windsurf form factor) instead of "a CLI + a VS Code panel," reusing
the existing engine + UI, packaged unsigned the way the user's **Sattva AI** app
is. The engine stays — embedded and hidden inside the app.

---

## TL;DR

- Built **`desktop-app/`** — an **Electron** shell (modeled exactly on Sattva)
  that on launch spawns the bundled **`forge` engine** as a **hidden**
  `forge serve` backend, then opens a window hosting the **existing chat UI**,
  which talks to that local server over HTTP/SSE. Inference stays local.
- **Reused, not rewritten:** the chat UI (`media/main.js`) runs **unchanged** via
  a `vscode-shim` + a `bridge` that speaks its exact message protocol against
  `forge serve` — the same logic the VS Code extension's host used.
- **Unsigned, Sattva-style:** `electron-builder` with `mac.identity: null` +
  `hardenedRuntime` + entitlements + an `afterPack` hook that **ad-hoc-signs**
  every component (incl. the bundled engine) and turns the **RunAsNode fuse off**.
- **CI:** `.github/workflows/release-app.yml` builds + publishes the app per OS.
- **Honest status:** I could **not** run `electron-builder` / launch + verify the
  packaged GUI in this environment (same limit as signing/binaries). The project,
  the packaging config, and the CI are complete and verified as far as static
  checks go; **producing + verifying the actual `.dmg`/`.exe`/AppImage is the
  `npm run dist` / CI step.** I did **not** retire the website's working one-liner
  yet — see "Sequencing" — because doing so before the app is published would
  strand users.

---

## Step 1 — What Sattva AI actually does (analyzed at `/Users/rishinalem/SattvaAI/desktop`)

- **Framework:** Electron + **electron-builder** (TypeScript `electron/` → `tsc`;
  Vite for the renderer). Targets: mac `dmg`+`zip`, win `nsis`+`portable`, linux
  `AppImage`+`deb`.
- **Backend embedding:** a PyInstaller `sattva-backend` is bundled via
  `extraResources` and **spawned by the main process** from `process.resourcesPath`.
- **Unsigned distribution (the key part):**
  - `mac.identity: null` → electron-builder does **ad-hoc** signing, not Developer ID.
  - `hardenedRuntime: true` + `assets/entitlements.mac.plist` (JIT,
    `disable-library-validation`, network client/server, files).
  - **`scripts/afterPack.js`** (the crux): on macOS it (1) sets the **RunAsNode
    fuse off** via `@electron/fuses` so a stray `ELECTRON_RUN_AS_NODE` (common in
    IDE/terminal contexts) can't make the app exit on launch, and (2)
    **`codesign --force --sign - --options runtime --entitlements …`** every
    framework, helper app, the bundled backend, and the app itself.
- **The first-launch reality (what this buys):** ad-hoc signing does **not**
  remove the Gatekeeper "unidentified developer" warning for a browser-downloaded
  app — but it makes the bundle internally consistent so it opens via
  **right-click → Open** instead of the harder "app is damaged" failure that
  plagues naively-zipped unsigned Electron apps. That's how Sattva ships free.

I **replicated all of this** in `desktop-app/` (bundling the `forge` binary where
Sattva bundles its Python backend; entitlements trimmed to what we need —
network + spawn-child + files; the apple-events/automation entitlements Sattva
used for screen control were dropped).

---

## Step 2 — Reusing the engine + UI (no AI rewrite)

- **Launch the hidden engine** (`desktop-app/main.js`): spawns
  `forge serve --port 0` with `windowsHide`, parses the `FORGE_SERVE_READY {json}`
  line for the ephemeral port, exposes `http://127.0.0.1:<port>` to the renderer,
  and kills it on quit. Never on PATH, never a terminal — same backend the
  extension used.
- **Reuse the UI:** `scripts/prepare.mjs` copies the extension's
  `media/{main.js,main.css,hub.js,hub.css}` into `renderer/` (single source of
  truth — the app and panel can't drift). `renderer/index.html` is the same DOM
  the panel's webview used.
- **Adapt off the VS Code webview API:** `renderer/vscode-shim.js` provides
  `acquireVsCodeApi()`; `renderer/bridge.js` implements the **host side of the
  panel's exact message protocol** (`ready`/`send`/`cancel`/`modelInfo`/
  `pickFiles`/`signIn`…) by calling `forge serve` directly over **HTTP + SSE** —
  this is the same logic `chatViewProvider.js` ran, just pointed at the local
  server instead of the extension host. `renderer/theme.css` maps the VS Code
  `--vscode-*` theme variables the CSS relies on to the product's dark palette.
- **Sign-in** reuses the existing desktop OAuth (GitHub/Google loopback + PKCE)
  via the account server (`FORGE_ACCOUNT_SERVER`); the app opens the system
  browser, same account as the website.
- **First-run:** the engine already detects hardware + recommends a model; the
  app surfaces status and (once wired to the catalog) the Ollama prompt.

## Step 3 — Packaging & distribution

- `desktop-app/package.json` electron-builder config replicates Sattva (unsigned,
  ad-hoc, `afterPack`), bundles `forge` as `extraResources/bin`, targets mac
  (arm64; Intel deferred like the CLI), win x64, linux x64.
- `release-app.yml` builds the native engine + the app per OS and publishes
  installers + checksums to `ollamax-releases` on an `app-v*` tag.

---

## ⚠️ The unsigned first-launch reality (honest)

The curl one-liner avoided the warning **only** because curl downloads aren't
quarantined. A browser-downloaded **`.app` is quarantined**, so the
"unidentified developer" warning **returns** on first launch — opened via
**right-click → Open → Open** (exactly as Sattva requires). The download page
must say so plainly; a truly warning-free double-click needs **paid signing +
notarization** (the future step), not this round.

## Sequencing — why I did NOT retire the one-liner/CLI yet

The directive is to make the app the only surface and retire the CLI/one-liner.
I built the app, but I could not package/verify a running GUI in this environment.
**Retiring the only working install path before a verified app is published would
leave users with nothing (or broken links).** So the responsible order is:
1. Run `release-app.yml` (or `npm run dist`) → produce installers.
2. **Verify on a real machine** the app launches, spawns the engine, and chats.
3. Publish the installers; **then** flip the website to lead with the app and
   retire the one-liner/CLI copy. I can do that flip in one short follow-up once
   the app build is confirmed.

---

## Panel-activation + Todo-Tree

- **Panel activation is moot:** the standalone app *is* the experience now; the
  anvil-icon/extension path is superseded (the UI logic is reused inside the app).
- **The `Todo-Tree: Failed to find vscode-ripgrep` error is NOT ours.** Confirmed
  by search: `Todo-Tree`/`vscode-ripgrep` appear **nowhere** in our source
  (`src/`, `editor-integrations/`, `website/src`). It's the third-party **Todo
  Tree** extension you have installed — unrelated to Ollama-Forge. Nothing to fix
  on our side.

---

## What changed / didn't break

- **New:** `desktop-app/` (the Electron app) + `.github/workflows/release-app.yml`.
- **Untouched & green:** the engine (`cargo test` **138 pass**, zero Rust
  changed), the website, the existing CI, the extension. All workflows valid YAML.
- **Verified here:** `node --check` passes on every app file incl. the copied UI;
  `prepare.mjs` correctly stages the UI + engine; the bridge implements the panel's
  exact protocol; the engine binary launches `forge serve` (proven in prior rounds).

## How to test on a clean Mac
```
cargo build --release
cd desktop-app && npm install && npm run dist      # → release/Ollama-Forge-*.dmg
```
Open the `.dmg`, drag to Applications, **right-click → Open → Open** on first
launch. Expect: window opens, engine starts hidden, models load, chat streams
from local Ollama. (Requires Ollama installed.)

## Open questions / risks
- **Unverified packaged GUI:** I couldn't run electron-builder / launch the app
  here. The most likely first-run issues to watch: the renderer's theme shim vs.
  the panel CSS, and the SSE fetch parsing under Electron's CSP (`connect-src`
  allows `127.0.0.1:*`). Verify with `npm start` before publishing.
- **Central Hub** sidebar: the chat surface is wired; embedding the Hub view in
  the same window (it was a separate webview) is the next UI step.
- **Signing** remains the eventual fix for a warning-free double-click.
- The website one-liner/CLI retirement is intentionally **pending** the verified
  app publish (above).

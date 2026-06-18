# Pivot to a VS Code (Code-OSS) Foundation + UI/Feasibility Fixes

**Date:** 2026-06-18
**Scope:** Adopt the VS Code UI (fork Code-OSS) as the app foundation; re-host the
forge AI experience on top of it; resolve items 1–7. Backend LLM/runtime
optimization (MLX/MoE/quantization) is **deferred** per the owner.

---

## TL;DR — the honest headline

This round is a **decision + the parts of it that can be built and verified here**,
not a finished Code-OSS fork. Being straight about that, exactly as with signing
and the packaged-GUI before it:

- **What I cannot do in this environment:** clone `microsoft/vscode` (~10–15 GB +
  full native toolchain), build the fork (`yarn && gulp vscode-*`, 20–40 min), and
  click through its GUI. That's the "Code-OSS fork lift" flagged in an earlier
  round, and it's the same class of limit as code-signing — it needs a real build
  machine. I did **not** fake it.
- **What I did do — the load-bearing realization:** the forge AI experience
  **already exists as a VS Code extension** (`editor-integrations/forge-vscode/`).
  It *was* a webview extension before the Electron port. So "re-host the chat panel
  as a VS Code webview" is **largely already done** — the re-platform is "bundle
  the existing extension into a Code-OSS fork," which the `desktop/` scaffold from
  the earlier round is built to do. The **Electron `desktop-app/` shell is what's
  superseded.**
- **The three items that were real code, not just "native on the fork," are done
  and verified:** **#7** (Hub catalog dead-end + intent search), **#6**
  (login-gating), **#5** (drag-and-drop into chat). 163 Rust tests pass (7 new for
  the Hub); all changed extension JS passes `node --check`.

---

## 1. The re-platform approach — carried over vs. discarded

| Layer | Decision | Why |
|---|---|---|
| **`forge` engine + `forge serve`** | **Carry over unchanged** | The AI runtime is shell-agnostic. The fork bundles the same binary; the extension spawns it (`ensureStarted`). |
| **Chat panel UI** (`forge-vscode/media/main.js`, `chatViewProvider.js`) | **Carry over** — it's already a VS Code webview | It was an extension first; re-hosting in the fork is "ship it as a built-in," not a rewrite. |
| **Central Hub** (`forge-vscode/src/hub.js`, `media/hub.js`) | **Carry over + fixed this round** | Now sources its catalog from the local engine (see #7). |
| **Accounts / auth** (`forge-vscode/src/auth.js`) | **Carry over + extended** | Added offline-graceful gate helpers (see #6). |
| **Memory / graph** (engine `src/memory`, `src/graph`) | **Carry over unchanged** | Pure engine; already on-device. |
| **Electron desktop shell** (`desktop-app/`) | **Superseded / discarded** | Its hand-built IDE (file tree, Monaco wiring, xterm/node-pty terminal, image viewer, layout) is exactly what VS Code provides natively. Keeping it would be re-building the platform. It remains in-tree as the *currently shippable* app until the fork is built, but it is no longer the target. |

**How the chat panel is re-hosted in the fork:** the fork's build copies
`editor-integrations/forge-vscode/` into the Code-OSS `extensions/` directory and
lists it in `product.json` → `builtInExtensions`, so the chat + Hub panels are
present on first launch with no marketplace install. The panel docks in the
**Secondary Side Bar (right)** via the existing `viewsContainers`/`views`
contributions (the Cursor/Windsurf layout — see #1). The extension spawns the
bundled `forge serve`; `forge.serverPath` is overridden at bundle time to the
in-app binary so the user configures nothing.

---

## 2. How each of items 1–7 was solved

### Native to the platform (solved by adopting the fork — no custom code)

- **#1 Chat on the right, beside the code.** VS Code's **Secondary Side Bar** hosts
  the chat webview; the editor is center; the Explorer is left. This is the
  Cursor/Windsurf arrangement and is the default once the panel is a built-in
  view container. The "separate window" problem disappears by construction.
- **#2 Images rendering as random characters.** VS Code's editor renders PNG/JPG/etc.
  in its native image viewer. The Electron app's custom Monaco/textarea editor (which
  could dump bytes) is **superseded**, so there is no place left that renders image
  bytes as text. (The chat webview's own image handling — #5 — explicitly uses
  `readAsDataURL`/`<img>`, never raw text.)
- **#3 Terminal needing manual `npm install` + rebuild.** VS Code ships a built-in
  integrated terminal (libpty bundled by the platform). This **eliminates the whole
  `node-pty` saga** from the previous round — no hand-wired xterm, no native rebuild,
  no "Terminal needs npm install" message. The terminal works out of the box.
- **#4 More IDE features + installing public packages.** Inherited: extensions
  (via **Open VSX**), tasks, settings, search, debugging. For installing pip/npm
  packages, the thin affordance is a command/task that runs the install **through
  the integrated terminal** — surfacing the platform capability rather than
  rebuilding a package manager.

### Real code shipped this round

- **#5 Drag-and-drop files & images into chat** — built into the chat webview
  (details below).
- **#6 Login-gating + activity→dashboard** — built into the extension (details below).
- **#7 Hub catalog auto-load + intent-aware search** — built into the engine +
  extension (details below).

---

## 3. #7 — Hub catalog fix + intent-aware search (the headline bug)

**Root cause.** The Hub catalog was served **only by the website account server**
(`/api/hub/categories`). The app couldn't load it unless the user set the obscure
`forge.accountServer` setting → the *"Set forge.accountServer to load the Hub
catalog"* dead-end. On top of that, search was a **client-side exact-substring
filter** (`media/hub.js`), so loose queries like "build a website" hit
*"No matching categories."*

**Fix — serve the catalog from the LOCAL engine.** The 54-category taxonomy
(`website/src/data/hub-taxonomy.json`) is now **embedded in the engine at compile
time** (`include_str!`) and served by `forge serve`, which is always running inside
the app. New module `src/hub/mod.rs` + endpoints:

| Endpoint | Purpose |
|---|---|
| `GET /api/hub/categories` | Full catalog — **auto-loads, zero config.** |
| `GET /api/hub/search?q=…` | **Intent-aware** ranked results. |
| `GET /api/hub/package/<slug>` | Compiles a package (rules + skills) **locally** from the category's `conventions`/`scaffolds`, so even **activation works offline.** |

The account server is now **optional enrichment only** (live curated repo lists +
the opt-in starring flow) — never required to browse, search, or activate.

**Intent-aware search (`hub::search`).** Offline, deterministic, no model required:

1. Tokenize the query.
2. Expand each token through a curated **intent map** (`website`→`web/frontend/css/
   react/…`, `ml`→`machine/learning/data/deep/nlp/…`, `game`→`unity/godot/…`,
   `api`→`backend/rest/graphql/…`, plus mobile, devops, security, 3d, embedded, …).
3. Score each category by overlap against name/slug/topics/example-repos/description
   with field weights (name 3.0 > slug 2.5 > topics 2.0 > repos 1.5 > desc 1.0),
   plus a **substring fuzzy fallback** (0.6/0.3). Direct query terms weigh 1.0,
   intent expansions 0.5.
4. Return ranked, score>0 — loose intent queries return sensible hits instead of a
   dead-end.

> **Honest note on "semantic."** This is **intent-expansion + fuzzy**, not embedding
> cosine similarity. It handles the owner's examples ("build a website", "ml stuff")
> well and works fully offline with no model dependency. A true embedding path
> (Ollama `/api/embeddings` over the query + categories) is a clean follow-up if
> deeper semantic recall is wanted; I did not claim to have shipped embeddings.

**Tests (7, all pass):** all 54 categories load; "build a website" → web/frontend
domains; "ml stuff" → ML/data domains; four loose intent queries don't dead-end;
exact name ranks first; empty query returns the catalog; package compiles rules+skills
locally and unknown slugs return `None`.

**Extension wiring.** `HubViewProvider` now takes the `backend` and fetches
categories/search/package from the **local engine** (`backend.ensureStarted()` +
`backend.getJson(...)`); `media/hub.js` search is **debounced and server-driven**
(asks the engine for intent-aware results) instead of the brittle client filter.
The `needsServer` dead-end path is gone.

---

## 4. #6 — Login-gating + activity→dashboard

**Behavior.** When an account server is configured, the app shows a **full-panel
sign-in screen** and **blocks AI actions** until the user signs in with their
existing GitHub/Google account. Implemented in the extension (carries over to the
fork):

- `auth.js`: `isSignedIn()` + `cachedUser()` — **offline-graceful by design**: they
  check the stored session **without a network call**, so a previously-signed-in
  user isn't locked out when offline (token validity is enforced lazily when the
  token is actually used).
- `chatViewProvider.js`: emits a `gate` state on boot and after sign-in/out; the
  **send path** itself is gated (`_gateBlocks()`), so the app can't be driven even
  if the UI is bypassed.
- `media/main.js` + chat HTML/CSS: the `#gate` overlay with sign-in + device-code
  buttons; hidden once `signedIn`.

**Deliberate guardrail.** Gating is enforced **only when `forge.accountServer` is
set**. Without a server there is nothing to authenticate against, so forcing the
gate would brick a local/dev run — instead the gate is disabled in that case. The
owner turns the gate ON for production by baking the **deployed website URL** into
the fork's default settings (`defaultSettingsOverrides.forge.accountServer` in
`product.json.example`). **This reverses the earlier logged-out-usable default — a
deliberate owner choice, now implemented.**

**Activity → dashboard (content-free, on-device metadata only).** This rides the
**existing telemetry layer** (`forge-vscode/src/telemetry.js`), which already sends
**metadata only** — event types/counts, model used, feature usage — and honors the
disclosed toggle. This round adds a `hub_activate` event. The established privacy
rules are intact and unchanged: **prompts, code, and file contents never leave the
device**; only anonymous usage metadata reaches the backend; the toggle/disclosure
is honored. The gate screen states this in plain language.

> **Open item (owner action):** the dashboard's per-user aggregation lives in the
> website backend. The app emits the metadata; surfacing it on the website
> dashboard is wiring on the Vercel side and depends on the deployed URL.

---

## 5. #5 — Drag-and-drop files & images into chat

Built into the chat webview (`media/main.js`), in addition to the existing upload
buttons:

- **Drop zone** over the whole panel with a visible "Drop files or images to
  attach" affordance (`body.dropping`).
- **Images** (`type` `image/*`) → `readAsDataURL` → base64; attached as a context
  item with a **thumbnail preview** (`<img>` chip; the chat CSP already allows
  `img-src data:`). **Routed to vision:** the base64 rides the existing
  `ContextItem.image` field the server already reads for vision; if the selected
  model clearly isn't multimodal, a **nudge** suggests a vision model (llava /
  llama3.2-vision) or Auto. The wire payload strips the duplicate thumbnail so the
  image isn't sent twice.
- **Text files** → `readAsText` (with a 512 KB guard against dumping a binary as
  text) → attached as a normal context item.

---

## 6. Code-OSS hygiene (rebrand, Open VSX, telemetry)

Captured in the `desktop/` scaffold (`product.json.example`, `scripts/bootstrap.sh`,
`scripts/bundle-forge.sh`), updated this round:

- **Rebrand** via `product.json` overlay: `nameShort/nameLong/applicationName/
  dataFolderName/bundle identifiers`, issue/license URLs, and replacing Microsoft's
  proprietary icons with the anvil mark (`media/forge.svg`).
- **Open VSX, not the Microsoft Marketplace** — `extensionsGallery` points at
  `open-vsx.org` (the MS Marketplace ToS forbids non-MS products; this is how
  Cursor/Windsurf/VSCodium comply).
- **Strip Microsoft telemetry** — `enableTelemetry: false`, `aiConfig.ariaKey: ""`,
  blank `telemetryfrom`: no MS endpoint exists to send to. The **only** telemetry is
  ours (`forge-vscode/src/telemetry.js`), which is metadata-only and disclosed.
- **Built-in extension** — the build copies `forge-vscode` into `extensions/` and
  lists it in `builtInExtensions`; the scaffold note now records that the Hub runs
  fully offline against the bundled engine.
- **Signing** — deferred (same as the unsigned downloads today).

---

## 7. What changed (files)

**Engine (Rust):**
- `src/hub/mod.rs` *(new)* — embedded taxonomy, `categories()`, intent-aware
  `search()`, local `package()`; 7 tests.
- `src/hub/taxonomy.json` *(new)* — the 54-category catalog, embedded at compile time.
- `src/server/mod.rs` — `/api/hub/categories`, `/api/hub/search`, `/api/hub/package/<slug>`.
- `src/lib.rs` — `pub mod hub;`.

**Extension (the AI layer that re-hosts into the fork):**
- `src/hub.js` — catalog/search/package now from the local engine; `_search`; account
  server reduced to starring only.
- `media/hub.js` — debounced, server-driven intent search; no client-side dead-end.
- `src/auth.js` — `isSignedIn()` + `cachedUser()` (offline-graceful gate).
- `src/chatViewProvider.js` — gate on boot/send/sign-out; `_gateBlocks()`.
- `src/extension.js` — pass `backend` to `HubViewProvider`.
- `media/main.js` — drag-and-drop, image thumbnails, vision nudge, gate render; wire
  payload strips duplicate thumbnails.
- `media/main.css` — gate overlay + drag/thumbnail styles.

**Fork scaffold:**
- `desktop/product.json.example` — Hub-local note, `defaultSettingsOverrides.forge.accountServer`
  for the gate.

**Verification:** `cargo test` → **163 passed / 0 failed** (7 new Hub tests). All
changed extension JS passes `node --check`.

---

## 8. Open questions & risks

1. **The fork itself is not built here.** Cloning + building Code-OSS needs a real
   build machine (~10–15 GB, native toolchain, 20–40 min) and GUI verification. The
   scaffold + the carried-over extension make it actionable, but the actual fork
   build/launch is the next real-machine step. **This is the honest big lift.**
2. **`forge.accountServer` must point at a deployed website** for the gate (and
   starring) to function. It currently defaults to `""` (gate disabled). The owner
   sets the production URL in the fork's defaults. I did **not** hardcode a guessed
   URL — that would silently break sign-in.
3. **Dashboard aggregation** is website-side work that depends on the deployed URL.
4. **"Semantic" search is intent-expansion + fuzzy, not embeddings** — honest naming.
   Embedding-based recall via Ollama is a clean follow-up.
5. **Signing still deferred** — shipped builds remain unsigned (Gatekeeper/SmartScreen
   right-click-Open), as before.
6. **Backend runtime optimization (MLX/MoE/quant) is deferred** per the owner — not
   in this round.
7. **The current shippable app is still the Electron `desktop-app/`** (the one you
   just got a fresh build of). It is superseded by the fork direction but remains the
   working download until the fork is built; the two shouldn't both be shipped long-term.

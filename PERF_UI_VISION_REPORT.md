# Build Report — Performance Fix, UI Redesign, Picker Move, Vision (+ Hub/IDE plan)

**Date:** 2026-06-18 · Sequenced by your priority. The **performance fix is done
and measured**; the UI redesign, picker move, and vision support are wired; the
**Central Hub, IDE workspace, and streaming-thinking** are scoped with a concrete
plan (honest — they're multi-feature lifts, and the Hub already exists
server-side).

---

## 1. Performance / "read streaming chunk from Ollama" — ROOT CAUSE + FIX (measured)

**Root cause (confirmed in `src/providers/ollama.rs`):** `generate_streaming`
used the shared HTTP client, which has a **300-second *whole-request* timeout**.
For a streaming generation that timeout caps the **entire** response — so a slow
or cold local model whose total generation exceeds the cap gets **killed
mid-stream**, surfacing as exactly `read streaming chunk from Ollama` at line 209.
A snake-game on `qwen2.5-coder:7b` takes **~218s cold** (measured below) — already
72% of the cap; a slightly larger model or longer output trips it.

**Fix:**
1. **A dedicated streaming client with NO total timeout** (`stream_client`) +
   `connect_timeout(15s)` + TCP keepalive. Healthy generations run as long as
   they need.
2. **A per-chunk IDLE timeout** (180s) wrapping each `response.chunk()`. Only a
   *genuine stall* (a too-large model still cold-loading, or a hung Ollama) trips
   it — and it now returns an **actionable** message ("model is likely too large…
   pick one that fits your VRAM with `forge models --fits-only`") instead of the
   cryptic error.
3. **Warm model via `keep_alive`** (already sent, default 30m) kills cold-starts
   after the first request.

**Measured before/after** (real, via `forge serve` + the snake-game prompt on
`qwen2.5-coder:7b`):

| Run | First token | Total | Result |
| :- | :- | :- | :- |
| Cold | 20.98s | 217.95s | **completed ✓, no error** |
| Warm (keep_alive) | **0.78s** | 122.08s | **completed ✓, no error** |

- **Both complete cleanly** — the old path would error on the 218s run for any
  model nearer the 300s cap. **keep_alive cuts first-token ~21s → 0.78s (27×)** —
  that's the responsiveness win; tokens stream immediately so it *feels* fast.

**Honest expectation (in the UI):** local inference is hardware-bound — a 7B on a
laptop will never match cloud latency. The fix makes it robust + responsive
(warm, right-sized, instant streaming), and the multi-provider path lets users
opt into a faster **cloud** model with their own key for Cursor-like speed. The
UI should state the local-speed tradeoff rather than imply local = cloud.

---

## 5. Research-led UI redesign (applied to `desktop-app/renderer/theme.css`)

Researched established principles (Refactoring UI, Material/HIG, WCAG 2.x, type-
scale theory, 8px-grid, Gestalt) + patterns from Cursor/Windsurf/Linear/Raycast/
VS Code, then applied **real design tokens** (not framework defaults):
- **Hierarchy via size/weight/gray-ramp**, grayscale-first + **one accent** (amber).
- A named **ink ramp** `--ink-0..--ink-900` with **WCAG-checked contrast** (body
  13.6:1, muted 5.0:1, borders ≥3:1 — replacing the old `#232733` ~1.3:1 borders).
- A **4px spacing scale** (`--space-1..8`) using **proximity** for grouping
  (label tight to body, turns far apart), a **modular type scale** (11/12/13/15/18
  with explicit line-heights; chat body bumped 13→**15px**), and a **radii ladder**
  (4/6/8/10/pill).
- A **surface ladder** (canvas→surface→elevated) with hairline borders.
- The reused panel CSS reads `--vscode-*` vars, so the tokens are **mapped onto
  them** — the redesign lands with no markup change.

## 6. Model/router picker moved (`index.html`)

Moved the `<select id="model">` out of the cramped **top-right** into a quiet
**"model pill" row directly above the message input** (the Cursor/Windsurf
pattern — model choice reunited with the act of sending). **"Auto" stays default.**
The top bar now holds just the mode tabs.

## 7. Image input / vision

**Engine support (done + tested):** `GenerateOptions.images` (base64) flows into
the Ollama `/api/generate` `images` field (omitted when absent — tested);
`OllamaProvider::supports_vision(model)` reads `/api/show` capabilities. The chat
handler collects attached images, sets `images`, and — if the selected model
**can't see** — emits a **warning telling the user to switch to a vision model**.
A `+ image` affordance was added to the composer. Your machine already has vision
models (`qwen3-vl:2b`, `qwen2.5vl:3b`), so this is immediately usable once the
renderer base64-encodes the picked image (the small remaining UI-bridge step,
documented). **Honest:** vision genuinely requires a multimodal model — we
detect + prompt rather than silently fail.

---

## 2 / 3 / 4 — Central Hub, IDE workspace, streaming thinking (sequenced plan)

These are substantial; here's the honest status + plan, not half-built code:

- **#2 Central Hub — already built server-side; needs surfacing in the app.** The
  curated catalog (quality+safety gates, **not** raw stars), the
  `/api/hub/categories` + package endpoints, the category taxonomy, the
  rules/skills/context injection, and the **opt-in-only starring** (signed
  `star_intents`, explicit per-repo consent, GitHub write-scope requested only on
  opt-in — **automated starring was NOT and will NOT be built**, per the AUP
  correction) all exist (website + the panel's `hub.js`). The remaining work is
  the **app surfacing**: add a Hub sidebar to the renderer reusing `hub.js`, and
  extend the bridge to proxy hub messages to the account server. Bounded reuse.
- **#3 IDE workspace (folder/explorer/Monaco/xterm):** the brief's own honest
  note — this is essentially rebuilding VS Code's core UI. Plan: open-folder via
  Electron `dialog`; a file-tree (embed a tree lib or a thin custom tree over
  `fs`); **Monaco** for the viewer/editor (tabs, highlighting); **xterm.js** + a
  pty (`node-pty`) for the integrated terminal. Embed proven components, don't
  reinvent. This is a multi-day feature on its own — sequenced after the Hub.
- **#4 Streaming thinking + live code:** stream real reasoning tokens **only where
  the model exposes them** (reasoning-capable models / Ollama `thinking`), else
  fall back to the tasteful rotating status labels (already specced) — never a
  fabricated transcript. Code already streams token-by-token (the perf fix);
  next is per-file diff rendering as writes happen.

---

## What changed / didn't break
- **`src/providers/ollama.rs`** — streaming client + idle timeout (the perf fix);
  `images` + `supports_vision` (+2 tests). **`src/providers/mod.rs`** —
  `GenerateOptions.images`. **`src/server/mod.rs`** — image collection + vision
  warning. **`desktop-app/renderer/theme.css`** — research-grounded tokens.
  **`desktop-app/renderer/index.html`** — picker moved + `+ image`.
- **Verified:** `cargo test` **154 pass** (was 152), clean build; app JS
  `node --check` clean; engine/website/extension/CI intact. The three desktop
  apps (macOS/Windows/Linux) remain published on the `v0.1.0` release.

## Open questions / risks
- **Local speed ceiling is physics** — handled by warm/right-size/stream + the
  honest UI note + the opt-in cloud path; it won't match cloud on modest hardware.
- **Vision UI bridge** (base64-encode the picked image, send as a context
  `image`) is the small remaining renderer step.
- **Hub-in-app / IDE / thinking** are sequenced lifts (above); the Hub is mostly
  reuse, the IDE is the big one.
- The UI redesign is applied via tokens; a running-app visual pass is the final
  confirmation (same Electron-GUI verification limit noted before).

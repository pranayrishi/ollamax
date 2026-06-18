# Full Audit → Reconcile → Complete → Verified Release

**Date: 2026-06-18.** Method: I **mounted the actual published `.dmg`** from
`ollamax-releases` and inspected its contents — not just grepped the repo. That's
how the headline gap below was found.

---

## 0. The root cause of "committed ≠ shipped" (and the fix)

Two distinct failures created the "I was told it's done but it's not in my
download" gap:

**(A) The published download was the wrong app.** For most of the project,
`ollamax-releases` shipped the **Electron `desktop-app/`** (a custom shell with its
*own* renderer). New work landed in the **`forge-vscode` extension** + the **Rust
engine**, which the Electron shell only partially copied and never fully wired. So
"committed to the repo" genuinely wasn't "in the Electron download." **Resolution:**
the product direction is the **Code-OSS fork** (`ForgeCode`), which bundles the
*whole* `forge-vscode` extension + engine — so the extension's features ship as-is.
The fork now builds in CI (`release-fork.yml`).

**(B) Even the new fork app was broken — it shipped with NO engine.** I mounted
`ForgeCode-macos-arm64.dmg` and found the extension (UI, agent, voice) present but
**`extensions/forge-vscode/bin/` absent** — the only `forge` in the app was VS
Code's own `bin/code`. Cause: `.vscodeignore`'s `bin/**` made gulp's built-in
packaging strip the staged engine. Without `forge serve`, the app launches but does
nothing. **Fix (this pass):** `bootstrap.sh` now copies the engine into the *built*
app's extension `bin/` after gulp (guaranteed), and `.vscodeignore` no longer
ignores `bin/`. A clean rebuild is in flight; **the engine-present artifact is
pending verification** (I'll confirm by re-mounting the new `.dmg`).

This is the systemic fix: features now ride the fork (bundles the latest extension
+ engine), and the engine is guaranteed into the app.

---

## 1. Status inventory (verified against the published artifact where noted)

Legend: ✅ shipped & should work · 🟡 partial / blocked · ❌ not built · ⏭️ superseded/deferred.
"Code ✅" = present in the fork artifact's bundled extension/engine (verified by
mounting the dmg); "works" depends on the engine fix (B) landing + a GUI launch I
can't do here.

| # | Feature | Status | Evidence / note |
|---|---|---|---|
| — | **Code-OSS fork app (ForgeCode)** | ✅ | Verified in dmg: rebranded `nameLong=ForgeCode`, Open VSX gallery, `enableTelemetry=false`, `forge-vscode` bundled as built-in. |
| 1 | Standalone app on the VS Code fork | ✅ | The fork builds + publishes (macOS verified; win/linux in flight). Supersedes the Electron shell. |
| 2 | Chat right / editor center / explorer left | 🟡 | Editor+explorer = native VS Code ✅. Chat docks to the **left** activity bar (verified package.json), not the right secondary side bar. User-movable; right-by-default needs a workbench default. |
| 3 | Multi-provider BYOK (OpenAI/Anthropic/Gemini) | ❌ | **Not built** — `src/providers/` is Ollama-only. Picker-above-input ✅, Auto-routing ✅, secret-scan gate ✅, but no cloud providers. |
| 4 | Curated free offline models registry | ✅ code | `src/models/` (hardware-tiered). Ships in engine. |
| 5 | Internet access / web tools | ✅ code | `src/tools/` (search/wiki/arxiv/fetch); now Agent-tab (off in Chat). |
| 6 | Multi-agent orchestration | ✅ code | `src/orchestrator`+`executor` (Build mode). |
| 7 | No message limits + queue | ✅ code | Webview queue (verified in bundled `main.js`). |
| 8 | Streaming thinking + live code | ✅ code | Thinking events + token streaming. |
| 9 | Image upload + drag-and-drop + vision | ✅ code | Drag-drop in bundled `main.js`; vision in `ollama.rs`. |
| 10 | Central Hub (auto-load, intent search, opt-in star) | ✅ code | `src/hub/` local catalog + intent search; extension `hub.js`. Catalog served by the local engine (so needs fix B). |
| 11 | Login required + account linking | 🟡 | Auth + gate code present; **gate is OFF in the build** (no account-server URL baked in). "Required" not enforced. Needs the deployed website URL (flagged below). |
| 12 | Per-user web dashboard (metadata only) | 🟡 | Telemetry code present (metadata-only); dashboard is website-side (Vercel), not in the app; only flows when signed-in + server set. |
| 13 | Scale + security hardening | ✅ code | `SecurityGuard`, `VramSentinel`, secret scanning. |
| 14 | Cross-platform installers | 🟡 | Electron: all 3 ✅. Fork: macOS ✅, **win/linux building now**. Intel-Mac dropped (decision). Unsigned right-click-Open ✅. |
| 15 | Native IDE (explorer, image preview, **terminal**, packages, tabs) | ✅ | **Fork win** — these are native VS Code (the dmg IS VS Code: `out/`, full workbench). The Electron app lacked a working terminal; the fork has it. |
| 16 | Memory + graphify | ✅ code | `src/memory/` (write-back) + `src/graph/` query tools; graphify is build-time per-project. |
| 17 | Performance/streaming fix | ✅ code | `ollama.rs` stream-idle-timeout fix. |
| 18 | Hermes agent + voice | ✅ code | All agent waves; `voice.js` verified present in the dmg. Voice needs whisper.cpp configured. |
| 19 | Research-led UI overhaul | ✅ code | Theme/UX in bundled `main.css`/`main.js`. |

**The pattern:** almost everything is **code-✅ but was *blocked* by gap (B)** (no
engine → nothing runs). With the engine fix, the ✅-code items should become
functional in one app. The genuine non-gaps are **#3 (not built)** and the partials
**#2, #11, #12, #14**.

---

## 2. Reconciliation (newer decisions win)

| Older request | Superseded by | Resolution |
|---|---|---|
| Custom Electron / Sattva shell | **VS Code (Code-OSS) fork** | Fork is the product. Electron app stays published temporarily (it's the only one fully cross-platform today) but is **deprecated**; the fork replaces it. |
| App usable logged-out | **Login required** | Gate code exists; **enabling it needs the deployed account-server URL** (see open questions). |
| CLI / one-liner installer | **Standalone app** | CLI retired as the user-facing path; the engine binary still ships *inside* the app. |
| Backend MLX/MoE/quant | **Deferred** | Out of scope; not built (correctly). |

**Flagged for you (genuinely ambiguous / need input):**
- **#3 multi-provider BYOK** was requested but never built (Ollama-only). Do you
  still want cloud providers, or is local-only the intended product now?
- **#11 login-required** can't be enabled without your **deployed website URL**
  (baking a wrong URL bricks sign-in). What's the production account-server URL?
- **#2 chat-on-the-right** by default needs a Code-OSS workbench customization
  (it's left by default, user-movable). Worth the fork patch, or fine as-is?
- **Two apps published** (Electron `Ollama-Forge-*` + fork `ForgeCode-*`). Once the
  fork is verified working, do you want me to **retire the Electron downloads**?

---

## 3. What I completed + verified THIS pass

- **Found gap (B)** by mounting the real `.dmg` (the engine was missing).
- **Fixed the engine bundling** (post-gulp copy + `.vscodeignore`) — the
  highest-value fix; it's the difference between a dead app and a working one.
- **Queued the fork on all 3 platforms** (`release-fork.yml` + win/linux jobs).
- **Updated the docs** to reflect the fork building in CI.
- Clean rebuild **in flight**; the engine-present artifact is **pending re-mount
  verification** (added below once confirmed).

---

## 4. Honest scope statement + prioritized plan

**This is too large to truthfully call "all done" in one pass.** What's genuinely
completable + verifiable now is the **engine fix → one working fork app**, plus the
audit/reconcile. The rest is real work:

1. **(now, in flight)** Engine-in-app fix → verify by re-mounting the new dmg →
   one working fork download (macOS), with win/linux following.
2. **(needs your input)** Login-required: give me the deployed account-server URL →
   I bake it into the fork build (`FORGE_ACCOUNT_SERVER`) → gate enforced.
3. **(small)** Chat-on-the-right default: workbench layout patch in the fork.
4. **(large, needs decision)** Multi-provider BYOK (#3): a real engine feature
   (provider trait + OpenAI/Anthropic/Gemini clients + key storage + the secret-scan
   gate before cloud). Only if you still want it.
5. **(verification I can't do)** A real **GUI smoke test** of each feature needs a
   human launching the app with Ollama running — I can't click the packaged window.
   I verify *presence* in the artifact; you verify *behavior*.

---

## 5. Open questions
1. Production **account-server URL** (to enable login-required + dashboard sync)?
2. Keep **multi-provider BYOK** in scope, or is the product **local-only**?
3. After fork verification, **retire the Electron app** downloads?
4. Chat-on-the-right by default — worth the workbench patch?

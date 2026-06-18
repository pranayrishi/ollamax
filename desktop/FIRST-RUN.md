# First-run experience (desktop app)

What the bundled app does the first time it launches. Goal: a non-technical
student can go from download → working local AI in a couple of minutes.

## Flow

1. **Welcome** — one screen explaining: local-first, your code stays on your
   machine, anonymous usage metadata you can turn off (the honest telemetry
   line), sign-in is optional.

2. **Detect Ollama** (the app shells out / probes `http://127.0.0.1:11434`):
   - **Running** → ✓ continue.
   - **Installed but not running** → offer "Start Ollama" (`ollama serve`).
   - **Not installed** → deep-link to https://ollama.com/download with OS-aware
     instructions; re-check on return.

3. **Recommend + pull a model** — the app already detects hardware
   (`VramSentinel`) and recommends a model. Offer one click to
   `ollama pull <recommended_model>` with the braille progress spinner
   (`forge preload` UX). Skippable.

4. **Sign in (optional)** — "Sign in with GitHub or Google" (the Round 4/6 PKCE
   loopback flow). Skipping is fine — local inference never requires an account.
   Sign-in unlocks the Central Hub's account features + the web usage dashboard.

5. **Telemetry disclosure** — the one-time opt-out notice (also wired in the
   extension's `activate()`): "anonymous usage metadata, counts only, turn off
   anytime." Honors the choice immediately.

6. **Done** — opens the chat panel (Auto routing default) with the Forge Hub in
   the Activity Bar.

## Notes

- The app **requires a local Ollama daemon** — it is the inference engine. The
  app never bundles model weights (too large) but makes pulling one one-click.
- Bundled pieces: the rebranded Code-OSS shell, the chat panel + Central Hub as
  **built-in** extensions, and the `forge` binary under `resources/app/bin`
  (the extension's `serverPath`/`accountServer` default to the bundled binary +
  the production account server).
- Auto-update: not built this round. Options to add later — a Squirrel/electron-
  updater-style feed, or "check GitHub Releases for a newer tag" with a manual
  download prompt. Documented in `desktop/README.md`.

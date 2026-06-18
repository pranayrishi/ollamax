# Ollama-Forge — first run (UNSIGNED build)

Thanks for downloading Ollama-Forge. **This is an unsigned build, so your OS will
warn you the first time you run it. That's expected** (we haven't paid for code
signing yet — it's planned). Here's how to get past it and set up in ~2 minutes.

> **This download is a quick-setup bundle, not a one-click app.** It contains the
> `forge` engine + the VS Code panel and a short setup step — not a single
> double-click installer (yet).

## What's in this bundle
- **`forge`** (`forge.exe` on Windows) — the local AI coding CLI + server.
- **`forge-vscode.vsix`** — the VS Code chat / agent / build panel (optional).
- **`install.sh`** / **`install.ps1`** — sets the above up for you.

## Prerequisite: Ollama (required)
Ollama runs the models locally on your machine. Install it from
**https://ollama.com/download**, then pull a model:

    ollama pull qwen2.5-coder:7b

## Quick setup
- **macOS / Linux** — in a terminal in this folder:  `./install.sh`
- **Windows** — `powershell -ExecutionPolicy Bypass -File install.ps1`
  (or right-click `install.ps1` → *Run with PowerShell*)

## Getting past the "unsigned / unidentified developer" warning
- **macOS:** the first time, **right-click `forge` (or the app) → Open** rather
  than double-clicking, then confirm. (`install.sh` also clears the quarantine
  flag with `xattr` so the CLI just runs.)
- **Windows:** if SmartScreen shows *"Windows protected your PC"*, click
  **More info → Run anyway**.

## Use it
- **CLI:**  `forge --help`  (e.g. `forge chat`, `forge build`)
- **Editor:** open VS Code → the **anvil** icon in the Activity Bar → the **Chat**
  panel. If `forge` isn't auto-found, set *Settings → Ollama-Forge → Server Path*
  to the installed `forge`.

Your code stays on your machine. See https://github.com/pranayrishi for the
project.

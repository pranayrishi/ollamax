# Build Report — One-Line Terminal Installer (avoids the Gatekeeper warning, unsigned)

**Date:** 2026-06-18 · **Goal:** a share-and-go install that doesn't trigger the
macOS "unidentified developer" / Windows SmartScreen warning — **without** paid
signing. Lead with `curl … | sh`; keep the manual `.tar.gz`/`.zip` as an honest
fallback. No `.dmg` detour.

---

## The install commands (live now)

**macOS / Linux**
```
curl -fsSL https://github.com/pranayrishi/ollamax-releases/releases/latest/download/install.sh | sh
```
**Windows (PowerShell)**
```
irm https://github.com/pranayrishi/ollamax-releases/releases/latest/download/install.ps1 | iex
```

Both URLs are public, auth-free, and resolve today (HTTP 200) — I uploaded the
scripts to the existing `v0.1.0` release, and the release workflow now keeps
`…/releases/latest/download/install.{sh,ps1}` current on every tag.

---

## Why this avoids the warning (the actual mechanism)

macOS Gatekeeper blocks an app because **quarantine-aware apps** (browsers, Mail)
stamp downloaded files with the `com.apple.quarantine` extended attribute;
Gatekeeper then refuses to run the unsigned binary ("cannot be opened because it
is from an unidentified developer"). Windows does the equivalent with a
"mark-of-the-web" Zone.Identifier that triggers SmartScreen.

**`curl` and `irm` do not set those flags.** A binary fetched by the installer
just runs — the warning never appears. This is exactly how Homebrew, rustup, and
Ollama ship unsigned CLI tools. The installer **also** strips quarantine
defensively (`xattr -dr com.apple.quarantine` / `Unblock-File`), belt-and-suspenders.

This is **not** a security bypass — the build is simply unsigned for now. Only
Apple Developer ID **signing + notarization** (and Windows Authenticode) removes
the warning for *browser*/`.dmg` downloads. That remains the future paid step.
**A `.dmg` would not help** — an unsigned app inside a browser-downloaded `.dmg`
is still quarantined and still blocked — so we deliberately did not build one.

---

## What the script does (step by step)

`desktop/install.sh` (POSIX `sh`; transparent + idempotent — echoes each step):

1. **Detect OS + arch** (`uname`) → choose the right bundle (macOS arm64/x86_64,
   Linux x86_64). Windows is handled by `install.ps1`.
2. **Download the bundle via `curl`** to a temp dir — *not quarantined*.
3. **Verify the SHA-256** against the published `.sha256` sidecar (fails closed on
   mismatch).
4. **Install `forge`** to `~/.local/bin` (override with `FORGE_BIN_DIR`), `chmod +x`,
   and **strip quarantine** on it; print a PATH hint if needed.
5. **Install the VS Code panel** (`code --install-extension …vsix`) if the `code`
   CLI is present; otherwise print how.
6. **Check Ollama**: if present, compute the hardware-recommended model (via the
   just-installed `forge models`) and either prompt to pull it (real terminal) or
   print the exact `ollama pull` command; if absent, point to ollama.com/download.
7. Print clear next steps (open the panel → sign in).

`install.ps1` mirrors this with `Invoke-WebRequest` + `Expand-Archive` +
`Unblock-File`, installs `forge.exe` to `%LOCALAPPDATA%\Programs\OllamaForge`, adds
it to the user PATH, installs the `.vsix`, and checks Ollama.

---

## What changed

- **New:** `desktop/install.sh`, `desktop/install.ps1` (the hosted one-liners).
- **`.github/workflows/release.yml`** — the publish job now checks out the repo,
  stages `install.sh`/`install.ps1` into `dist/`, and publishes them as release
  assets (so `latest/download/install.*` stays current); release notes lead with
  the one-liner.
- **`website/src/components/CopyCommand.tsx`** (new) — copy-to-clipboard command
  block (with fallback to select-all).
- **`website/src/app/download/page.tsx`** — **leads with the one-liner** (macOS/
  Linux + Windows, each with a Copy button and a link to read the script), the
  "signed installers are coming…" line, the Ollama/VS Code prerequisites, and the
  manual `.tar.gz`/`.zip` grid demoted to a collapsed **"Prefer a manual
  download? (advanced)"** section with the honest per-OS bypass steps.
- **`website/src/components/DownloadButtons.tsx`** — homepage note now points to
  the one-liner as the smoothest, no-warning path.
- Uploaded both scripts to the live `v0.1.0` release.

Nothing else changed: the CLI, the app, CI, and the rest of the website are
intact. The website builds clean; `release.yml` is valid YAML.

---

## Verification

**Tested the real one-liner on this Mac (Apple Silicon), end to end:**
```
curl -fsSL …/install.sh | sh    # in a sandboxed HOME
```
- Detected `Darwin/arm64`, curl-downloaded the bundle, **verified the checksum**,
  installed `forge`, installed the VS Code panel, handled "no Ollama model pull"
  gracefully, printed next steps.
- **Proof the warning is avoided:** `xattr` on the installed binary shows
  **zero** `com.apple.quarantine` attributes, and `forge 0.1.0 (8ede1b3)` runs.
- Stable URLs return **HTTP 200** (`install.sh` 4.9 KB, `install.ps1` 3.7 KB).

**To confirm on a clean Mac (you):**
1. **One-liner path (should show NO warning):** on a Mac that's never seen the
   app, run the `curl … | sh` command, then `forge --help` and open the VS Code
   panel — no Gatekeeper prompt.
2. **Manual path (warning expected, bypass works):** download
   `ollama-forge-macos-arm64.tar.gz` in a browser, unpack, double-click `forge` →
   you'll see the warning → right-click → **Open** → **Open** → it runs.

**Windows** `install.ps1` I could not execute here (no Windows/pwsh), but it's the
direct `Invoke-WebRequest` analogue; verify with the `irm … | iex` command on a
clean Windows box (no SmartScreen prompt expected).

---

## Honest limitations

- **The manual `.tar.gz`/`.zip` path still triggers the warning** — that's
  inherent to browser downloads of unsigned builds; the page says so and gives the
  bypass. The one-liner is the smooth path.
- **No `.dmg`** — it wouldn't fix the warning (see above); not built on purpose.
- **Signing + notarization is still the future step** for a true one-click,
  warning-free experience from any download method.
- **Intel macOS** is still "coming soon" (its CI build is queued); the installer
  errors clearly if run on an unbuilt arch. Apple Silicon, Windows x64, and Linux
  x64 are live.
- The Windows variant is unverified-on-Windows from here (no runner); logic mirrors
  the tested macOS/Linux path.

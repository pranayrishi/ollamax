# Desktop App + Signed Distribution (Code-OSS fork)

> **Round 6 update.** The signed release pipeline is now defined in
> [`.github/workflows/release-desktop.yml`](../.github/workflows/release-desktop.yml)
> (mac notarized · Windows Authenticode · Linux AppImage/.deb), with signing
> scripts in [`scripts/sign-macos.sh`](scripts/sign-macos.sh) +
> [`scripts/sign-windows.ps1`](scripts/sign-windows.ps1) and the first-run UX in
> [`FIRST-RUN.md`](FIRST-RUN.md). It runs end-to-end once the fork is wired
> (bootstrap + bundle scripts) and the signing secrets are provided. I cannot
> produce *actually* signed binaries here (no Apple/Windows certs, no multi-GB
> fork build), so this is the working pipeline + scaffold, per the brief.

## Signing setup (owner — required for shippable mac/Windows builds)

| Secret | Where to get it | CI secret name |
| :-- | :-- | :-- |
| Apple Developer ID Application cert (.p12, base64) | Apple Developer Program ($99/yr) → Certificates | `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD` |
| Apple Team ID + notary creds | Apple Developer + an app-specific password | `APPLE_TEAM_ID`, `APPLE_NOTARY_APPLE_ID`, `APPLE_NOTARY_PASSWORD` |
| Windows code-signing cert (.pfx, base64) — **EV preferred** so SmartScreen trusts it | a CA (DigiCert/Sectigo/etc.) | `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD` |

Add these in GitHub → Settings → Secrets → Actions. Tag a release (`git tag v0.2.0
&& git push --tags`) → `release-desktop.yml` builds, signs, notarizes, and
publishes installers + SHA-256 to the GitHub Release. Then set the website
`NEXT_PUBLIC_DOWNLOAD_*` (+ `_SHA256`) env vars to those URLs so the
`/download` page lights up. **Auto-update** is not built (documented in
FIRST-RUN.md).

---

# Phase 3 background — forking & rebranding Code-OSS

> **STATUS: scaffold + plan.** This directory documents and stages the work to
> turn the Phase 2 extension into a standalone, branded, installable desktop app
> in the Cursor/Windsurf form factor. It does **not** perform the multi-GB
> Code-OSS clone/build/sign in this round — that is the next round. The scripts
> here ([`scripts/bootstrap.sh`](scripts/bootstrap.sh),
> [`scripts/bundle-forge.sh`](scripts/bundle-forge.sh)) print their procedure
> and `exit 0` before mutating anything.

## What "done" looks like

A user downloads one installer (`.dmg` / `.deb` / `.AppImage` / Windows setup),
opens **Ollamax**, and the AI chat panel is already docked on the right. The
app bundles the `forge` binary; on first run it detects Ollama and offers to
install it / pull a model. No terminal, no extension install, no API keys.

## Naming (needs your confirmation)

The repo currently carries three names: `ollamax` (Cargo `repository`),
`ollama-forge` (crate/README), `Ollama-Optimizer` (local dir). **Recommendation:
standardize the project on `ollama-forge` and name the desktop app
`Ollamax`.** All scaffold files use `Ollamax` as a placeholder —
search-and-replace once you confirm.

## Architecture: how the pieces ship together

```
Ollamax.app (Code-OSS fork)
├── (rebranded Code-OSS shell: product.json, icons, about strings)
├── resources/app/extensions/forge-vscode/   ← Phase 2 extension, as a built-in
└── resources/app/bin/forge                   ← Phase 1 Rust binary (forge serve)
                                                 └─ talks to local Ollama @ :11434
```

The desktop app is just Code-OSS + our built-in extension + our bundled binary.
**Nothing about Phases 1–2 changes** — the extension still launches
`forge serve` and streams over SSE. The fork's only job is to (a) make the panel
present by default, (b) rebrand, and (c) package + sign.

## Step-by-step plan

### 1. Fork Code-OSS and confirm a clean build
- Clone `microsoft/vscode` at a **pinned release tag** (not `main`) — see
  `VSCODE_TAG` in [`scripts/bootstrap.sh`](scripts/bootstrap.sh).
- Install the toolchain the checkout's `.nvmrc`/`README` pins (Node + yarn +
  native build tools). Confirm a vanilla `yarn && yarn gulp vscode-<platform>`
  produces a runnable app *before* changing anything. (~10 GB disk, 20–40 min
  first build.)

### 2. Rebrand via `product.json`
- Overlay the keys in [`product.json.example`](product.json.example):
  `nameShort`/`nameLong`/`applicationName`, bundle id, data folder, icons.
- Replace Microsoft's proprietary icons under `resources/{darwin,win32,linux}`
  with our anvil mark (source: `editor-integrations/forge-vscode/media/forge.svg`).
- Update About/window/welcome strings.

### 3. Bundle the chat panel as a built-in + ship the binary
- Run [`scripts/bundle-forge.sh`](scripts/bundle-forge.sh): copies
  `editor-integrations/forge-vscode` into the fork's `extensions/` and the
  release `forge` binary into `resources/app/bin/`.
- Override the extension's `forge.serverPath` default to the bundled binary path
  so the user needs zero configuration.
- Optionally default the chat view into the **Secondary Side Bar** (right) for
  the Cursor/Windsurf placement — set via a default `workbench` layout / a
  first-run `setStartupView`.

### 4. First-run experience (detect Ollama)
- On first launch, the extension (or a small welcome walkthrough) checks
  `GET /api/status` → `ollamaHealthy`. If false:
  - Detect whether `ollama` is installed; if not, deep-link to
    https://ollama.com/download.
  - If installed but not serving, offer to run `ollama serve`.
  - Offer to `ollama pull <recommended_model>` (the status payload already
    returns the hardware-recommended model).

### 5. Package per OS
- macOS: `.dmg` (or `.zip`) from `yarn gulp vscode-darwin-arm64` + a packaging
  step.
- Linux: `.deb` and `.AppImage`.
- Windows: Inno Setup / `.exe` installer.
- **Auto-update** would require hosting an update feed and wiring the fork's
  update channel; documented as out-of-scope this round.

## Licensing, telemetry & signing — read before shipping

> I am not a lawyer; these are flags, not legal advice. Cursor and Windsurf both
> had to handle each of these.

- **Code is MIT, branding is not.** The "Code - OSS" source is MIT, but
  Microsoft's **name, logo, and icons are trademarked/proprietary** and must be
  removed/replaced in any fork. The `product.json` overlay + icon replacement in
  steps 2–3 are exactly this.
- **Do not use the Microsoft Marketplace.** Its ToS forbids use by non-Microsoft
  products. Point `extensionsGallery` at **Open VSX** (done in
  `product.json.example`). Implication: any extension we want available to users
  must exist on Open VSX (our own built-in chat panel does not depend on the
  gallery at all).
- **Strip Microsoft telemetry.** Set `enableTelemetry: false` and leave the
  telemetry keys blank so there is no endpoint to send to — consistent with the
  product's zero-telemetry promise. Audit the build for any remaining MS
  telemetry collectors.
- **Code signing / notarization** is required for a downloadable app users will
  trust:
  - macOS: Apple Developer ID signing **+ notarization** (else Gatekeeper blocks
    it). Needs a paid Apple Developer account.
  - Windows: **Authenticode** signing (ideally EV) to avoid SmartScreen warnings.
  - Linux: sign repo metadata / checksums.
  Not done this round; budget for accounts + CI secrets.

## Realistic effort estimate

| Work | Rough effort |
| :-- | :-- |
| Fork + clean build + rebrand (product.json, icons, strings) | 3–5 days |
| Bundle binary + built-in extension + default right-side layout | 2–3 days |
| First-run Ollama detection/install walkthrough | 2–4 days |
| Per-OS packaging (dmg/deb/AppImage/win) | 3–6 days |
| Signing + notarization (incl. account setup, CI secrets) | 3–5 days |
| Auto-update infrastructure | 1–2 weeks (separate) |
| **Ongoing**: rebasing the fork onto new Code-OSS releases | continuous |

The honest summary: matching Cursor/Windsurf is a **maintained-fork** commitment
(weeks to a first signed release, then continuous rebasing), not a one-shot task.
The good news is Phases 1–2 already deliver the actual product — a working,
local, side-docked chat experience — inside stock VSCode today. Phase 3 is
packaging and distribution, not new product capability.

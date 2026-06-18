# Build Report — Unsigned Cross-Platform Downloads via GitHub Releases

**Date:** 2026-06-18 · **Goal:** make the website download buttons hand visitors a
real, working app — the fast, free, **unsigned** route — built per-OS in CI,
published to **GitHub Releases**, with the buttons wired to those URLs.

---

## TL;DR

- **What downloads:** a per-OS **bundle** = the `forge` engine + the VS Code
  chat/agent/build **panel (`.vsix`)** + a one-step **install script** + a
  first-run README. It is labeled honestly on the site as a *"quick-setup
  bundle, not a one-click app"* (~2 min setup). I did **not** ship a fake
  one-click installer (see "Why not a real app" below).
- **Platforms:** macOS **Apple Silicon** + **Intel**, **Windows x64**, **Linux
  x64** — each built **natively** in CI, each with a **SHA-256** checksum.
- **Hosting (your choice):** source stays in the **private** `pranayrishi/ollamax`;
  binaries publish to the **public** `pranayrishi/ollamax-releases` (I created +
  seeded it) so anonymous visitors can download. The site links to the stable
  `…/releases/latest/download/<asset>` URLs.
- **Proven working:** I built + assembled + installed the **macOS** bundle
  end-to-end locally (`forge 0.1.0` runs after install). The other three use
  identical CI logic on native runners.
- **Two gated owner steps remain** (a PAT + the first tag) — checklist below.

---

## Why not a real one-click app this round (honest)

You gave a decision tree; here's what I picked and why:

- **Code-OSS fork (.dmg/.exe/.AppImage):** not feasible now. The fork doesn't
  exist yet (the `desktop/` scripts are scaffolds), and a real build is a
  multi-GB, multi-day, continuously-rebased effort. I won't claim a build I can't
  produce or verify.
- **Electron wrapper:** the chat UI is built for a VS Code webview (it calls
  `acquireVsCodeApi()` and message-bridges to the extension host). Re-hosting it
  in Electron is real new code I can't test/package here — too risky to ship as
  "working."
- **✅ Per-OS bundle (chosen):** genuinely buildable + verifiable **today**, and
  it actually works — the `forge` CLI runs standalone, and the `.vsix` installs
  the editor panel. Labeled honestly. **Fastest real path wins.**

The signed one-click app remains the future path (`release-desktop.yml` scaffold
+ `desktop/README.md`); what it adds is in "Limitations."

---

## Part 1 — What gets built, and how

Extended **`.github/workflows/release.yml`** (kept the existing native-build
approach, added to it):

1. **`vsix` job** — packages the extension once with `@vscode/vsce@3`
   (pinned) → `forge-vscode.vsix`, shared to all build jobs.
2. **`build` matrix (native, no cross-compile)** — `cargo build --release` +
   smoke-test `forge --version`, then assembles the bundle:

   | Asset | Runner | What's inside |
   | :- | :- | :- |
   | `ollama-forge-macos-arm64.tar.gz` | `macos-latest` (arm64) | `forge` + `.vsix` + `install.sh` + README |
   | `ollama-forge-macos-x64.tar.gz` | `macos-13` (Intel) | same |
   | `ollama-forge-windows-x64.zip` | `windows-latest` | `forge.exe` + `.vsix` + `install.ps1` + README |
   | `ollama-forge-linux-x64.tar.gz` | `ubuntu-latest` | `forge` + `.vsix` + `install.sh` + README |

   Each gets a matching **`.sha256`** sidecar.
3. **`release` job** — publishes all bundles + checksums + the standalone
   `forge-vscode.vsix` to a Release in the **public** `ollamax-releases` repo
   (cross-repo via `softprops/action-gh-release` with `repository:` + a PAT).

The bundle's **`install.sh`/`install.ps1`** put `forge` on PATH, clear the macOS
quarantine / Windows "downloaded" flag, install the `.vsix` if `code` is present,
and check for Ollama. **`README-FIRST.md`** explains the unsigned warnings +
prerequisites.

### How to cut a new version
```bash
git tag v0.1.0 && git push origin v0.1.0      # in pranayrishi/ollamax
```
The workflow builds all four bundles and publishes them to
`pranayrishi/ollamax-releases` under that tag. (Bump `version` in
`editor-integrations/forge-vscode/package.json` for the `.vsix` as needed.)

> `workflow_dispatch` is **build-only** (no publish) — a safe smoke test. To
> dry-run the real cross-repo publish + token, push a throwaway tag like
> `v0.0.1-test`, check the release in `ollamax-releases`, then delete it.

---

## Part 2 — The website

- **`src/lib/downloads.ts`** (new) — single source of truth: the bundle list +
  `assetUrl()`/`checksumUrl()` building `<NEXT_PUBLIC_RELEASES_REPO>/releases/latest/download/<asset>`
  (defaults to the public repo). Stable across versions.
- **`DownloadButtons.tsx`** (homepage `#download`/CTA) — real per-OS download
  links (macOS defaults to Apple Silicon), + an honest note: *"Unsigned build ·
  CLI + VS Code panel (quick setup) · needs Ollama"* and a link to `/download`
  for Intel Mac + checksums.
- **`DownloadGrid.tsx`** (`/download`) — OS/arch detection highlights the right
  bundle (and, when a browser can't reveal the Mac chip, tells you to pick
  Apple Silicon vs Intel instead of falsely "highlighting" one), real download +
  **checksum** links.
- **`/download` page** — prominent **unsigned first-launch** steps (per OS) and
  the **Ollama prerequisite**, and the honest *"quick-setup bundle, not a
  one-click app"* framing.

`NEXT_PUBLIC_RELEASES_REPO` is a **build-time** value on Vercel — set it (or
accept the default) and **redeploy**.

---

## Owner checklist (the steps only you can do)

1. **Create the cross-repo publish token.** GitHub → Settings → Developer
   settings → **Fine-grained PAT**:
   - Resource owner: `pranayrishi`; Repository access: **only**
     `pranayrishi/ollamax-releases`; Permissions: **Contents → Read and write**.
   - Copy the token. In **`pranayrishi/ollamax` → Settings → Secrets and
     variables → Actions → New secret**: name **`RELEASES_REPO_TOKEN`**, paste it.
2. **(Done for you)** Public releases repo `pranayrishi/ollamax-releases` is
   created + seeded with a README (a Release needs a commit to tag — don't delete it).
3. **Publish the first release:** `git tag v0.1.0 && git push origin v0.1.0`
   (optionally test with `v0.0.1-test` first, then delete it). Watch the run at
   `pranayrishi/ollamax → Actions`.
4. **Vercel:** the download URLs default to `ollamax-releases`, so they work with
   no env change. If you ever host elsewhere, set **`NEXT_PUBLIC_RELEASES_REPO`**
   (Production) and **redeploy** (it's build-time inlined).
5. **Verify** each button on production downloads a file (see below).

---

## Verification

**Proven locally (macOS, end-to-end):** built `forge` (`forge 0.1.0`), packaged
the `.vsix`, assembled `ollama-forge-macos-arm64.tar.gz`, ran `install.sh` in a
sandbox — it installed `forge` and the installed binary ran. Website builds (28
routes), 16 tests pass, all workflows valid YAML, `tsc` clean.

**Adversarial review (2 lenses) ran on the pipeline + wiring.** It confirmed
asset names match the website byte-for-byte (no 404s), the cross-repo wiring,
env inlining, the Hero→CTA→buttons chain (real links, no dead spans), and the
honest labeling. Findings, all **fixed**:
- *Empty releases repo would 422 on first publish* → seeded it with a README.
- *Install scripts globbed `forge-vscode-*.vsix` but the file is `forge-vscode.vsix`*
  → globs changed to `forge-vscode*.vsix` (and the local run confirms the panel installs).
- *macOS arch-unknown (Safari/Firefox) highlighted nothing yet claimed it did* →
  banner now tells you to pick your chip.
- *`vsce@latest` unpinned* → pinned `@vscode/vsce@3`.

**What I could NOT verify (you must):** I can't run GitHub's Windows/Linux/Intel-
Mac runners or download from your live site, so I did **not** execute a real CI
release or a production button click. After step 3+5 above, confirm from a clean
browser that each platform's button downloads a file that unpacks and runs.

---

## Limitations (stated plainly)

- **Unsigned** → macOS "unidentified developer" / Windows SmartScreen warnings on
  first launch. Disclosed on the download page + in each bundle's README, with
  the exact bypass steps. Signing later (Apple Developer ID + notarization;
  Windows Authenticode/EV) removes the warnings — that's the `release-desktop.yml`
  path.
- **Not a one-click app** → it's a CLI + VS Code-extension bundle; the editor
  panel needs VS Code installed. The `forge` CLI works on its own.
- **Arch coverage:** macOS arm64 + x64, Windows **x64**, Linux **x64**. No
  Windows-arm64 or Linux-arm64 (no free native runners for those); add later via
  self-hosted/emulated runners.
- **Ollama is required** (local inference engine) — disclosed; the installer
  checks for it.
- **CI cost:** the build runs in the **private** source repo, where Actions
  minutes are limited and **macOS bills at 10×**. Occasional releases are fine on
  the free tier; heavy iteration could exhaust it. (Fully-free CI would require a
  public build repo.)
- **Auto-update:** out of scope. Would need an update feed (Squirrel/electron-
  updater) or a "check Releases for a newer tag" prompt.

---

## Files changed / added
- **`.github/workflows/release.yml`** — +Windows, +`vsix` job, bundle assembly,
  checksums, fixed asset names, cross-repo publish (pinned vsce, doc'd prereqs).
- **`.github/workflows/release-desktop.yml`** — moved off the `v*` trigger
  (manual-only) so the two release workflows don't collide.
- **`desktop/bundle/install.sh`, `install.ps1`, `README-FIRST.md`** (new) — the
  bundle's setup + first-run docs.
- **`website/src/lib/downloads.ts`** (new) — download URL source of truth.
- **`website/src/components/DownloadButtons.tsx`, `DownloadGrid.tsx`** — real
  links, checksums, arch-aware highlight, honest notes.
- **`website/src/app/download/page.tsx`** — unsigned + Ollama disclosures, honest framing.
- **`website/.env.example`** — `NEXT_PUBLIC_RELEASES_REPO` documented.
- **Created:** public repo `pranayrishi/ollamax-releases` (seeded with a README).

No secrets are in the repo. I have **not** committed/pushed these source changes
or pushed a release tag — say the word and I'll commit + push to `main` (no
attribution, as before). The first published Release also needs your
`RELEASES_REPO_TOKEN` (step 1) before a tag will publish.

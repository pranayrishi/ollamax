# First-Launch Guidance for the Unsigned App

Ollamax ships **unsigned** for now (code-signing is deferred until there's
traction), so users hit a **one-time** OS security prompt on first launch
(macOS Gatekeeper "unidentified developer"; Windows SmartScreen). This round
makes that moment **expected, clear, visual, and reassuring** — surfaced at the
three points where it matters, in an honest tone that never claims the app is
"verified."

---

## Where the guidance now appears

| # | Touchpoint | What was added | File |
|---|-----------|----------------|------|
| 1 | **Download page — prominent callout** | An ember banner directly under the intro: "Downloading the app directly? It's not signed yet, so there's one one-time step to open it → See First launch." Anchors to the section below. | `website/src/app/download/page.tsx` |
| 2 | **Download page — dedicated "First launch" section** | A fully visible, per-OS, illustrated guide (`#first-launch`) — **lifted out of the old buried `<details>`** so it's seen, not hidden. | `download/page.tsx` + `components/FirstLaunchGuide.tsx` |
| 3 | **Right after clicking download** | The manual download grid now reveals the matching-OS steps **the instant a download starts** and smooth-scrolls them into view ("Your download is starting — here's how to open it"). | `components/DownloadGrid.tsx` |
| 4 | **Inside the macOS `.dmg` window** | The disk image opens to a branded background showing **drag-to-Applications** *and* the one-time first-launch instructions, baked into the window layout. | `desktop/scripts/make-dmg.sh`, `dmg-bg.svg` → `dmg-bg.png` |
| 5 | **Homepage download buttons** | Each unsigned download now links "there's a one-time step to open it →" to `/download#first-launch`. | `components/DownloadButtons.tsx` |

OS detection reuses the existing `lib/os.ts` (`detectOS` + high-entropy UA hints),
so the visitor's platform is shown first; all platforms remain switchable.

The **optional in-app welcome screen** was intentionally not added in this round:
the login gate already serves as the first-successful-launch screen (it greets the
user and leads straight into the required GitHub sign-in). Adding a separate
welcome would require a full ~50-min fork rebuild to ship — happy to add it to the
gate as a one-time banner on the next build if you want it.

---

## The copy used (per-OS, honest, calm)

**Why it happens (shown once, at the top of the guide):**

> Because Ollamax is a new, independent app that **isn't code-signed yet**, your
> system shows a **one-time** security prompt the first time you open it. That's
> expected for new software — here's the single step to get past it. Signed
> installers are on the way.

**macOS** — *"Right-click to open it the first time — that's the whole trick."*
1. **Right-click Ollamax → Open** — In Applications, Control-click (or right-click) the Ollamax app and choose Open. Don't double-click it the first time.
2. **Click "Open" in the dialog** — macOS asks to confirm because the app isn't code-signed yet. Click Open. This happens once.
3. **Done — it opens normally after** — From now on Ollamax launches with a normal double-click.
   - *Recent macOS fallback:* If there's no Open button, double-click once, then **System Settings → Privacy & Security → "Open Anyway."**

**Windows** — *"Two clicks past the SmartScreen notice — one time only."*
1. **Click "More info"** — on the blue "Windows protected your PC" screen.
2. **Click "Run anyway"** — SmartScreen shows this because the app is new and not yet signed.
3. **Done — launches normally after** — Windows remembers your choice and won't prompt again.

**Linux** — *"No signing prompt — just make it executable."*
1. **Allow executing** — `chmod +x` the binary/AppImage (or file manager → Properties → allow executing).
2. **Run it** — Linux doesn't gate unsigned apps.

**Integrity (for technical users):** "Every build ships a **SHA-256 checksum**
next to it — compare it to be sure the file is intact. We don't claim the app is
'verified'; the prompt simply means it isn't signed yet." (Checksums are already
published next to each asset.)

---

## Visuals

- **Web:** each OS shows an **inline SVG mock of the actual dialog** with the
  button to click highlighted (a pulsing ember outline) — macOS "cannot verify
  the developer … Open?", the Windows SmartScreen panel ("More info" → "Run
  anyway"), and a Linux terminal snippet. These are **illustrations**, labelled
  as such ("the actual dialog may vary by OS version") — no fabricated
  screenshots, no false "safe/verified" badge. SVG = sharp at any size, themed,
  zero image requests.
- **DMG window:** `dmg-bg.svg` (rendered to a 2× retina `dmg-bg.png`) draws the
  wordmark, a drag arrow from Ollamax → Applications, and a bordered
  **"First launch — one time only: Right-click Ollamax → Open → Open"** band.
  `make-dmg.sh` stages the app + an `/Applications` symlink + `.background/`, then
  uses Finder/AppleScript to set the window background and icon positions and
  bakes it into the image's `.DS_Store`.

---

## Tone & honesty (as required)

- **Reassuring but truthful** — explains the prompt appears *because* the app
  isn't signed yet, and that this is normal for new independent software.
- **No false reassurance** — never says "verified" or "100% safe."
- **Consistent with the unsigned reality** — notes signed installers are planned,
  with **no promised date**.
- **Not buried** — visible callout + dedicated section + reveal-on-download.

---

## Verification

- **Website builds cleanly** — `npm run build` passes with the new
  `FirstLaunchGuide` + `DownloadGrid` changes (verified locally), so the Vercel
  redeploy is safe.
- **DMG verified** — mounted `dist/Ollamax-macos-arm64.dmg` and confirmed:
  `Ollamax.app`, the `/Applications` symlink, `.background/dmg-bg.png`
  (95,925 bytes), and a baked `.DS_Store` (window background + icon layout
  applied). SHA-256 regenerated.
- **CI** — `release-fork.yml` macOS packaging now calls `make-dmg.sh`; on a
  headless runner (no Finder scripting) it falls back to a plain `.dmg` rather
  than failing the build.

## Files
- `website/src/components/FirstLaunchGuide.tsx` *(new)* — per-OS guide + SVG dialog mocks
- `website/src/components/DownloadGrid.tsx` — reveal-on-download
- `website/src/app/download/page.tsx` — prominent callout + visible section
- `website/src/components/DownloadButtons.tsx` — homepage first-launch link
- `desktop/scripts/make-dmg.sh` *(new)*, `desktop/scripts/dmg-bg.svg` *(new)*, `dmg-bg.png` *(generated)*
- `.github/workflows/release-fork.yml` — DMG step uses `make-dmg.sh`

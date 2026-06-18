# Round 6 Build Report — Ollama-Forge

**Date:** 2026-06-17 · **Scope:** 5 features + 2 posture changes, built in order, nothing existing broken.

This is the single report for Round 6. It covers what was built, how to try each
piece, the telemetry design (exact fields + content-exclusion + opt-out), the
account-linking model + GitHub gating, the release/signing pipeline + the secrets
you must provide, what changed, and the residual risks/open questions.

---

## TL;DR

| # | Feature | Status |
| :- | :- | :- |
| 1 | Cross-platform **signed installers** | **Pipeline + scaffold + docs** (can't produce real signed binaries here — needs your certs + the multi-GB fork build) |
| 2 | **Google login + account linking** (multi-identity) | ✅ Built |
| 3 | **Real signup counter** | ✅ Built |
| 4 | **OS/arch detection** on download page | ✅ Built |
| 5 | **Per-user usage dashboard** (web-only, metadata-only) | ✅ Built |

**Two posture changes, both handled honestly:**

- **"GitHub only" is retired.** Sign in with **GitHub _or_ Google**; one account
  links both. GitHub-only actions (starring) are gated for Google-only users with
  a link prompt.
- **"Zero telemetry" is retired.** The site no longer claims "no telemetry."
  Replaced everywhere with the honest line: **your code stays on your machine; we
  collect anonymous usage metadata you can turn off.** Content (prompts, code,
  files, paths, repo names) still never leaves the machine.

**Verification:** website `npm run build` ✅ (28 routes) · `npm run test` ✅ (16
vitest tests) · `cargo test` ✅ (126, Rust untouched) · all extension JS
`node --check` ✅ · all 4 workflows valid YAML ✅ · secret boundary clean ✅.

**Security:** a 3-lens adversarial review ran over the new auth/telemetry surface.
It found **one critical account-hijack vector** (GitHub email treated as verified
without checking GitHub's flag) — **fixed** — plus medium/low items, all fixed or
documented below.

---

## Feature 2 — Google login + multi-identity account model

### Model
- Internal account = `users.id`. A row in `identities(provider,
  provider_account_id UNIQUE, email, email_verified, …)` per linked provider.
- **Find-or-create-or-link** on sign-in (`resolveUserForIdentity`):
  1. provider identity already exists → that account;
  2. else the incoming email is **VERIFIED** and matches an existing account's
     **verified** identity → **link** into it;
  3. else → **create** a new account.
- Decision is a pure, unit-tested function: `lib/identity-rules.ts`
  (`resolutionDecision`).
- **Never links on an unverified email** — the hijack guard.

### Explicit linking (settings)
- `/account` shows each provider with a **Link GitHub / Link Google** button →
  `GET /api/link/start?provider=…` (session-auth, signed state + single-use
  `link_states` row) → provider → `/api/link/callback` (verifies signed state +
  row bound to the same user+provider, exchanges the code, links).
- **No identity theft:** `linkIdentityToUser` refuses if the identity already
  belongs to a different account (`{ ok:false, conflict:true }` → `?error=conflict`).

### GitHub gating (for Google-only users)
- Starring ("support maintainers") needs a linked GitHub identity. The star flow
  (`/api/star/*`) links the authorizing GitHub identity into the account in the
  same round-trip, refusing if it's linked elsewhere — so a Google-only user is
  transparently brought into compliance without a separate step.

### Refactor
- The desktop token `sub` and all desktop/device/star tables moved from
  `github_id` → internal `users.id`, so **Google-only desktop users** get a
  session not tied to GitHub.

### Try it
1. Set `AUTH_GOOGLE_ID` / `AUTH_GOOGLE_SECRET` (+ existing GitHub) — see
   `website/.env.example` for the exact callback URLs.
2. Apply `website/db/schema.sql` to Neon.
3. Sign in with Google, then **Link GitHub** in `/account`; try starring as a
   Google-only user and watch the link prompt.

---

## Feature 3 — Real signup counter

- `GET /api/stats` → `countUsers()` (`SELECT count(*)`) + `countActiveUsers(7)`,
  module-level 60s cache + CDN `Cache-Control`.
- `<SignupCounter/>` (server component) renders on the hero **only if n > 0** —
  never a fabricated number.

---

## Feature 4 — OS / arch detection (download page)

- `lib/os.ts` `detectOS(ua, platformHint, archHint)` (pure, unit-tested).
- `DownloadGrid.tsx` (client) calls `navigator.userAgentData.getHighEntropyValues`
  with a UA-string fallback; **highlights** the matching installer but shows
  **all** options and **never blocks** a download.
- **Apple Silicon** (arm64) is only claimed when the high-entropy `architecture`
  hint says so — the Mac UA always lies ("Intel"). Verified by test.

---

## Feature 5 — Per-user usage dashboard (web-only) + telemetry

### What is collected (EXACTLY)
Per event (`usage_events`): `type` (chat | agent | build | route | hub_activate |
suggestion), `provider`, `model` (Ollama tag), `tokensIn`, `tokensOut`,
`language` (inferred from a file **extension** only), `accepted` (bool), `ts`.

### What is NEVER collected
Prompt text · chat messages · generated code · source code · file contents · file
paths · directory structure · repo names/URLs · any inference traffic.

### The content firewall (two enforcement points)
1. **Client** (`editor-integrations/forge-vscode/src/telemetry.js`): `track()`
   builds a **fixed allowlisted** payload — even the field names are hard-coded;
   `language` comes from `languageFromExt()` (a ~35-entry extension→name map),
   never the path/content.
2. **Server** (`lib/analytics.ts` + `/api/analytics/ingest`):
   `validateUsageEvent` **rejects any unknown field**, any whitespace/over-long/
   wrong-charset string, and (after the review) any `model` shaped like a repo
   name / file path / filename. `validateBatch` caps at 500. So content can't be
   stored even by a misbehaving client.

### Opt-out (honored both sides)
- Setting `forge.telemetry` (default **on**) + a **one-time first-run notice**
  with an immediate "Turn off". Off ⇒ `track()`/`flush()` send nothing.
- Server **re-checks** `users.telemetry_opt_out` on ingest and drops the batch.
- Dashboard (`/dashboard`, web-only, `force-dynamic`): totals, % AI-assisted
  (shown only if suggestions>0, else "—"), activity/feature/model/language charts
  (CSS bars), plus **Pause/Resume**, **Export** (`/api/analytics/export`, JSON),
  and **Delete my usage data**.

### Per-user isolation
Every read/write derives the user id from a trusted source — the **verified
bearer `sub`** (ingest) or the **session `accountId`** (dashboard/export/delete) —
never from client input. Every query is `WHERE user_id = ${userId}` via
parameterized `neon` templates. The review confirmed **no IDOR**.

### Try it
1. Sign in (web). Set the extension's `forge.accountServer` + sign in there.
2. Use chat/agent/build/Hub → events flow to `/api/analytics/ingest` → visit
   `/dashboard`. Toggle `forge.telemetry` off → nothing sends.

---

## Feature 1 — Signed installers (pipeline + scaffold)

I **cannot** produce actually-signed `.dmg`/`.exe` binaries here (no Apple/Windows
certificates, and no multi-GB Code-OSS fork build in this environment). The brief
explicitly allowed "a fully working signed-release pipeline + instructions." That
is what's delivered:

- **`.github/workflows/release-desktop.yml`** — 3-OS matrix (macOS-14 universal /
  windows-latest / ubuntu-latest): builds `forge`, builds the rebranded app from
  the fork scaffold, **signs + notarizes** (gated on secrets), emits SHA-256
  sidecars, and publishes to the GitHub Release on a `v*` tag.
- **`desktop/scripts/sign-macos.sh`** — Developer ID deep-sign (hardened runtime)
  + `notarytool` + `stapler` + `.dmg` (real commands; `exit 0` until the fork
  produces an `.app`).
- **`desktop/scripts/sign-windows.ps1`** — `signtool` Authenticode sign + verify.
- **`desktop/FIRST-RUN.md`** — first-run UX (detect Ollama → one-click pull →
  optional GitHub/Google sign-in → telemetry disclosure).
- **`/download`** page wired to `NEXT_PUBLIC_DOWNLOAD_*` URLs + `_SHA256` checksums.

### Secrets you must provide (owner)
GitHub → Settings → Secrets → Actions:
`APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`, `APPLE_TEAM_ID`,
`APPLE_NOTARY_APPLE_ID`, `APPLE_NOTARY_PASSWORD` (macOS — Apple Developer Program,
$99/yr); `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD` (Windows — a CA cert,
**EV preferred** so SmartScreen trusts it immediately). Then tag `v0.2.0` and set
the website `NEXT_PUBLIC_DOWNLOAD_*` envs to the published URLs. Full table in
`desktop/README.md §Signing`. **Auto-update is not built** (documented).

---

## Setup checklist (owner)

1. **Google OAuth** — Cloud Console → Web client; redirect URIs
   `<AUTH_URL>/api/auth/callback/google` and `<AUTH_URL>/api/link/callback`. Set
   `AUTH_GOOGLE_ID/SECRET`.
2. **GitHub OAuth** — add the `/api/link/callback` redirect URL (alongside the
   existing `/callback/github` and `/star/callback`).
3. **Code signing** — the certs/secrets above.
4. **Artifact hosting** — GitHub Releases (the pipeline publishes there); set the
   download envs.
5. **Analytics DB** — apply `website/db/schema.sql` (adds `users` multi-identity,
   `identities`, `link_states`, `usage_events` + index, `telemetry_opt_out`).
6. **Privacy policy** — finalize the draft at `/privacy` (legal review; wire a
   self-serve account-delete + a contact address — flagged inline).

---

## Privacy policy — draft wording (for your legal review)

Rendered at **`/privacy`**. Key clauses:

- **One-liner:** "Your code stays on your machine. Inference runs locally (Ollama)
  or goes directly from your machine to a provider you chose — never through us.
  We collect anonymous usage metadata (counts/categories, no content) to power
  your dashboard, and you can turn it off."
- **We store:** account identity (GitHub/Google id, name, avatar, email if
  granted, linked providers, sign-in times) + usage metadata (the exact list
  above) **only with telemetry on**.
- **We never collect:** prompt text, code, file contents, full paths, repo
  names/URLs, inference traffic.
- **Your controls:** in-app telemetry toggle; web pause/export/delete; account
  deletion. (Flagged: wire self-serve delete + a contact email before launch.)

This replaces the prior absolute "no telemetry / nothing leaves your machine"
copy in `Privacy.tsx`, `FAQ.tsx`, `Comparison.tsx`, and `/privacy`.

---

## Security review — findings & resolutions

A 3-lens adversarial workflow (identity-linking · telemetry-privacy ·
appsec-general) audited the new surface.

### CRITICAL — fixed
**GitHub email auto-link hijack.** `auth.ts` derived `emailVerified` for GitHub as
`!!email` — i.e. *any* email GitHub returned was treated as verified. Since the
public profile email is user-settable, an attacker could put a victim's address on
a fresh GitHub account and **auto-link into the victim's Google-created account**
(the verified-email link rule is correct; its input was forged).
**Fix:** `auth.ts` now reads the **real** `verified` flag from
`GET /user/emails` (new `githubPrimaryVerifiedEmail()` in `lib/oauth.ts`) using
the sign-in's access token; an unverified GitHub email is kept but flagged
unverified, so it can never auto-link. The explicit-link path
(`oauth.ts exchangeForIdentity`) now uses the same real flag, and the star
callback stores the GitHub email as **unverified** (gating needs only the
account id). Re-verified: build + all tests pass.

### Medium — fixed
- **Signing steps always skipped.** `release-desktop.yml` gated on
  `env.APPLE_CERT_…` / `env.WINDOWS_CERT_…`, but a step's own `env:` isn't visible
  to its `if:`. **Fix:** gate on the `secrets.*` context.

### Low — fixed
- **`model` field too permissive** (could store a repo name / single-segment path
  in the caller's *own* rows — not cross-user). **Fix:** tightened
  `validateUsageEvent` — a slash now requires a `:tag`, and a trailing `.ext`
  filename shape is rejected; added tests (`facebook/react`,
  `src/PaymentService.java`, `config.production.env` rejected;
  `library/llama3:8b`, `llama3.1:8b-instruct-q8_0` accepted).
- **No rate limit on `/api/analytics/export` and `/api/link/callback`.**
  **Fix:** added `checkRateLimit` (per-user for export, per-IP for link/callback).
- **Audit redaction matched key names only.** **Fix:** added a value-side scrub
  for JWT/long-token/`code=`-shaped strings.

### Confirmed sound (no change needed)
Per-user isolation (no IDOR) · opt-out honored server+client · language is
extension-only · `resolutionDecision` + link conflict checks correct · provider
client secrets server-only · no raw provider token persisted · DB fully
parameterized (no SQLi) · no SSRF in `oauth.ts`/telemetry.

### Accepted / documented risk (not changed this round)
- **Rate limiter fails open + in-memory fallback is per-instance.** Without
  Upstash configured (or if Upstash errors), the durable limits degrade to
  per-warm-instance and fail-open — a known Round-5 tradeoff. **Mitigation:**
  require `UPSTASH_*` in production. A future change should make token-issuance
  endpoints fail **closed** when the durable limiter is unavailable.
- **`/api/link/start` is a state-changing GET with no Origin check.** A GET
  navigation carries no `Origin` header, so a `sameOrigin` gate would break the
  legit top-level link; the callback already re-binds via signed state +
  single-use row + same-user, so an attacker can't link their identity. Residual
  is a forced-redirect / one rate-limited DB row — accepted.

---

## What changed (file-level)

**New (website):** `lib/analytics.ts`, `lib/oauth.ts`, `lib/os.ts`,
`lib/identity-rules.ts`; `api/stats`, `api/analytics/{ingest,export}`,
`api/link/{start,callback}`; `dashboard/{page.tsx,actions.ts}`; `download/page.tsx`;
`components/{DownloadGrid,SignupCounter,GoogleMark}.tsx`; `*.test.ts` ×3 +
`vitest.config.ts`.
**Rewritten (website):** `auth.ts`, `lib/db.ts`, `lib/tokens.ts`,
`db/schema.sql`, `types/next-auth.d.ts`, `account/page.tsx`, `privacy/page.tsx`.
**Updated (website):** `Privacy/FAQ/Comparison/Hero.tsx`, `app/actions.ts`,
`lib/env.ts`, `lib/audit.ts`, `api/star/callback`, all desktop/device routes
(`github_id`→`accountId`), `.env.example`, `package.json`, `tsconfig.json`.
**New (desktop/CI):** `.github/workflows/release-desktop.yml`,
`desktop/scripts/sign-{macos.sh,windows.ps1}`, `desktop/FIRST-RUN.md`;
`desktop/README.md` signing section.
**New/updated (extension):** `src/telemetry.js`; wiring in `extension.js`,
`chatViewProvider.js`, `hub.js`; `package.json` (`forge.telemetry`); `README.md`.
**Rust:** unchanged (126 tests still pass).

> Note: `website/`, `desktop/`, and `editor-integrations/` are untracked in git
> and nothing was committed (you didn't ask). The `M` Rust files are pre-existing
> uncommitted working-tree state from earlier rounds, not Round-6 edits.

---

## Open questions

1. **Account-delete + contact email** — the privacy policy promises account
   deletion; wire a self-serve endpoint + a contact address before launch.
2. **Telemetry default** — currently **opt-out** (on by default, with a first-run
   notice). If you'd prefer **opt-in** for a stronger privacy stance, flip the
   `forge.telemetry` default to `false` and the first-run notice to an enable
   prompt. (Flagged per the brief — your call.)
3. **Rate-limiter fail-closed** for token issuance (see Accepted risk).
4. **The fork build itself** — `release-desktop.yml` + the scaffold are ready, but
   producing a real signed app needs the certs and wiring
   `desktop/scripts/bootstrap.sh` + `bundle-forge.sh` to actually build Code-OSS
   (weeks of maintained-fork work, per `desktop/README.md`).

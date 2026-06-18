# Fix Report — Login "Server Configuration" Error + Broken Download Buttons

**Date:** 2026-06-17 · **App:** `website/` (Next.js 15 + Auth.js v5), deployed on Vercel.

This report covers both reported issues. **Read this first:** one of the two
issues is fixable in code (and I fixed it), and the rest is **owner-side Vercel /
OAuth / hosting config** that I cannot do from the repo (and must not — no secrets
in the repo). I could **not** read your Vercel runtime logs (no dashboard access),
so for Issue 1 I diagnosed the mechanism from the actually-installed Auth.js code
and verified it with an independent adversarial review of `node_modules/@auth/core`.

---

## TL;DR

| Issue | Real cause | Who fixes it |
| :- | :- | :- |
| **1. Login "server configuration" error** | Auth.js v5 throws server-side because a required **env var/infra is missing in Vercel Production** — `AUTH_SECRET`, the 4 provider creds, **or `DATABASE_URL`/DB reachability/migrations** (the sign-in callback hits the DB on every login). Code is correct. | **You** (Vercel env + redeploy). |
| **1b. (separate, real code bug)** | CSP `form-action` listed `https://github.com` but **not** `https://accounts.google.com`, blocking Google's sign-in on the no-JS form-submission path; Google avatars were blocked by `img-src`. | **Fixed in code.** |
| **2. Download buttons do nothing** | `NEXT_PUBLIC_DOWNLOAD_*` are unset (signed installers were never built/hosted), so every button rendered an inert "coming soon" `<span>`. | **Fixed in code** (fallback to Releases) **+ you** (host installers to make them direct downloads). |

**The headline for Issue 1:** the `error=Configuration` page does **not** uniquely
mean "missing `AUTH_SECRET`." A throw inside the sign-in `jwt` callback — which
runs `resolveUserForIdentity()` against your Neon DB on **every** login — is
wrapped by Auth.js as `CallbackRouteError` and rendered as the **identical**
Configuration page. So **`DATABASE_URL` (or an unreachable DB / un-run migrations)
is just as likely as the auth secret.** Your Vercel runtime log names the exact
one — check it to disambiguate.

---

## Issue 1 — The login "server configuration" error

### What's actually wrong
`AUTH_SECRET`/provider-cred problems and **DB problems** both surface as the same
`error=Configuration` page. I traced the installed code (`next-auth@5.0.0-beta.31`,
`@auth/core`) to confirm the mechanism:

- `@auth/core/index.js:131` → any non-client-safe error becomes
  `type = "Configuration"`.
- `@auth/core/lib/actions/callback/index.js:388` → a throw inside the **jwt
  callback** becomes `CallbackRouteError`, which is **not** in the client-error
  set (`errors.js:412-430`) → renders as `Configuration`.
- Our `src/auth.ts` jwt callback calls `resolveUserForIdentity()` +
  `getLinkedProviders()` (both hit Neon) on every sign-in. If `DATABASE_URL` is
  unset/malformed, the DB is unreachable from Vercel, or the `users`/`identities`
  tables don't exist (migrations not applied), **it throws → Configuration page.**

### What is NOT wrong (verified)
The code is correct for Auth.js v5 — this is **not** a code bug:

- ✅ **Env var names are exactly right.** Auth.js v5 infers `AUTH_GITHUB_ID/SECRET`,
  `AUTH_GOOGLE_ID/SECRET`, and reads `AUTH_SECRET` (confirmed against
  `@auth/core/lib/utils/env.js`). Our `src/lib/env.ts` uses the same names.
- ✅ **No import/build-time throw.** `next build` succeeds with **zero** auth/DB
  env vars present; `src/lib/db.ts` creates the Neon client **lazily** (inside
  `db()`, not at module load), and `auth.ts` doesn't eagerly read env.
- ✅ **`trustHost: true` is set** (`src/auth.ts`), so Vercel's host is accepted.
- ✅ **Middleware doesn't break `/api/auth/*`** — it only appends headers via
  `NextResponse.next()`; no rewrite/redirect/block.
- ✅ Route handler (`app/api/auth/[...nextauth]/route.ts`) exports `GET`/`POST`
  correctly; sign-in server actions are correct.

### What I changed (code) — a real, *separate* bug
**`src/middleware.ts` CSP.** This does **not** fix the Configuration error (that's
env), but it's a genuine defect that would break Google login on the no-JS /
pre-hydration form-submission path (browsers enforce `form-action` across the
form's redirect hops), and it blocked Google avatars:

- `form-action`: added `https://accounts.google.com` (GitHub's host was already
  there; Google's was missing → Google's authorize redirect was blocked on that
  path). Matches the real endpoint `accounts.google.com/o/oauth2/v2/auth`.
- `img-src`: added `https://*.googleusercontent.com` (Google profile avatars).

> Scope honesty: in a normally-hydrated browser the sign-in redirect is a
> client-router top-level navigation, which CSP doesn't restrict — so this fix is
> for the progressive-enhancement/no-JS path and defense-in-depth. **It is correct
> and should be kept, but it is not what makes the Configuration error go away.**

### What YOU must change (the actual fix) — owner checklist
In **Vercel → Project → Settings → Environment Variables**, scoped to
**Production** (not just Preview/Development), then **redeploy** (Vercel injects
env at build/deploy — values added after the last deploy are not live until you
redeploy):

| Variable | Value / format | Notes |
| :- | :- | :- |
| `AUTH_SECRET` | `openssl rand -base64 32` output (≥32 chars) | Required in prod or Auth.js throws Configuration. |
| `AUTH_GITHUB_ID` | GitHub OAuth App **Client ID** | No quotes/whitespace/newlines. |
| `AUTH_GITHUB_SECRET` | GitHub OAuth App **Client secret** | |
| `AUTH_GOOGLE_ID` | Google OAuth **Client ID** (`...apps.googleusercontent.com`) | |
| `AUTH_GOOGLE_SECRET` | Google OAuth **Client secret** | |
| `DATABASE_URL` | Neon **pooled** connection string (`...-pooler...?sslmode=require`) | **Equally likely culprit.** Must be reachable from Vercel; use the pooled endpoint for serverless. |
| `APP_JWT_SECRET` | a *second* `openssl rand -base64 32` | Desktop-token signing (separate key). |
| `AUTH_URL` | `https://<your-domain>` (no trailing slash, https) | Optional with `trustHost`, but set it to be safe. |
| `NEXT_PUBLIC_SITE_URL` | `https://<your-domain>` | Used to build absolute callback URLs. |

Then **apply the DB schema** if you haven't: run `website/db/schema.sql` against
the Neon database (creates `users`, `identities`, `usage_events`, etc.).

**OAuth callback/redirect URLs** (must match the production domain exactly):

- **GitHub** OAuth App → *Authorization callback URL(s)* — GitHub allows multiple:
  - `https://<your-domain>/api/auth/callback/github`
  - `https://<your-domain>/api/link/callback`
  - `https://<your-domain>/api/star/callback`
- **Google** Cloud Console → OAuth client → *Authorized redirect URIs*:
  - `https://<your-domain>/api/auth/callback/google`
  - `https://<your-domain>/api/link/callback`
  - (and add `https://<your-domain>` as an Authorized JavaScript origin)

### How to confirm which env var is at fault (do this first)
Open **Vercel → the deployment → Logs / Functions** for a failed sign-in:
- `MissingSecret` / `assertConfig` → `AUTH_SECRET` or a provider cred is missing.
- `CallbackRouteError` (stack mentioning `resolveUserForIdentity` / `neon` /
  Postgres) → **`DATABASE_URL` / DB reachability / migrations.**

---

## Issue 2 — The download buttons do nothing

### What's actually wrong
The "Download the app" button (`Hero.tsx`) is `href="#download"`, which scrolls to
the `#download` section (`CTA.tsx`) that renders `<DownloadButtons/>`. Because
`NEXT_PUBLIC_DOWNLOAD_MACOS/WINDOWS/LINUX` are **unset** (the signed installers
were never built/hosted — that was owner setup from the distribution round), every
button rendered a **non-interactive `<span>coming soon</span>`** — so clicking did
nothing. The dedicated `/download` page (`DownloadGrid`) had the same dead state.

### What I changed (code)
Made the buttons **never dead** — three honest states in both `DownloadButtons.tsx`
and `DownloadGrid.tsx`:

1. **Artifact URL set** → direct download link to the installer.
2. **No artifact URL but `NEXT_PUBLIC_GITHUB_REPO` set** (the current state) →
   links to **`<repo>/releases`** (`target=_blank rel=noopener`), labelled
   "view releases ↗" / "Coming soon — view releases". Since
   `NEXT_PUBLIC_GITHUB_REPO=https://github.com/pranayrishi/ollamax` ships in
   `.env.example`, **this is what users hit now — a live link, not a no-op.**
3. **Neither set** → an honest disabled state.

### What YOU must do to make them *direct downloads*
The buttons can only download a file once the file exists. Either:
- Build + sign + publish installers (the `release-desktop.yml` pipeline +
  `desktop/` scaffold are ready — see `ROUND6_BUILD_REPORT.md` for the signing
  secrets), then set in Vercel (Production, **then redeploy** —
  `NEXT_PUBLIC_*` inline at **build** time):
  - `NEXT_PUBLIC_DOWNLOAD_MACOS` (+ `_SHA256`), `NEXT_PUBLIC_DOWNLOAD_WINDOWS`
    (+ `_SHA256`), `NEXT_PUBLIC_DOWNLOAD_LINUX` (+ `_SHA256`),
    `NEXT_PUBLIC_DOWNLOAD_LINUX_DEB` (+ `_SHA256`).
- Until then, the Releases-link fallback is the honest, working behavior.

---

## Verification

**What I verified (here):**
- ✅ `npm run build` — compiles, 28 routes; `tsc --noEmit` clean.
- ✅ `npm run test` — 16/16 pass.
- ✅ Independent adversarial review of the actually-installed `@auth/core` confirmed
  (a) the code is correct for v5 / no import-time throw, and (b) the
  `CallbackRouteError → Configuration` mechanism, widening the suspect list to
  `DATABASE_URL`/DB.
- ✅ Both code fixes reviewed correct, no regressions (the CSP change only widens an
  allowlist to the two intended hosts; the download change only turns dead spans
  into Releases links).

**What I could NOT verify (and you must):** I have no access to your Vercel
deployment, OAuth credentials, or a browser on production, so I did **not**
complete a real production login or download. After you apply the owner checklist
and **redeploy**, verify end-to-end on `https://<your-domain>`:
1. Click **GitHub** → authorize → land back signed in on `/account`.
2. Sign out, click **Google** → authorize → land back signed in on `/account`.
3. If either still shows the Configuration page, read the Vercel function log — it
   now names the specific missing var/infra (see "How to confirm" above).
4. Click each platform on `/` (`#download`) and `/download` → confirms a working
   link (Releases now; direct installer once hosted).

---

## Remaining risks (stated plainly)

- **Installers are not built/signed/hosted yet.** Until you publish them and set
  `NEXT_PUBLIC_DOWNLOAD_*` (+ redeploy), the download buttons link to GitHub
  Releases rather than downloading a file directly. That's intentional and honest,
  not a bug.
- **`DATABASE_URL` must use Neon's pooled endpoint** for serverless; an unpooled
  string can exhaust connections under load and would also throw → Configuration.
- **Redeploy discipline:** both `AUTH_*`/`DATABASE_URL` (runtime) and
  `NEXT_PUBLIC_*` (build-time inlined) require a redeploy after you set them. This
  alone explains many "I set it but it's still broken" cases.
- **Minor robustness nit** (not a cause): `resolveUserForIdentity` reads
  `created[0].id` without an empty-result guard; it only matters if the INSERT
  itself fails (already the DB-error path). Left as-is.

---

## Files changed (code)
- `website/src/middleware.ts` — CSP `form-action` + `img-src` now include Google.
- `website/src/components/DownloadButtons.tsx` — Releases fallback (no dead spans).
- `website/src/components/DownloadGrid.tsx` — Releases fallback on `/download`.
- `website/src/app/api/auth/[...nextauth]/route.ts`, `website/src/app/actions.ts`
  — corrected stale "GitHub is the only provider" comments (now GitHub + Google).

No secrets were added to the repo. Nothing was committed (say the word and I'll
commit + push to `main` with no attribution, as before).

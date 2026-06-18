# Round 4 Build Report — Marketing Site + GitHub-Only Login + Web↔Desktop Accounts

> A deployable Next.js website (`website/`), GitHub-only OAuth via Auth.js,
> a Postgres-backed account, and a desktop sign-in (PKCE loopback + device
> fallback) that resolves to the **same account** — Cursor/Windsurf style.
> The backend is **identity/distribution only**; it never receives prompts,
> code, or inference. CLI/app/CI unaffected. Generated 2026-06-17.

---

## 0. TL;DR

- **Website** (`website/`, Next.js 15 App Router + TS + Tailwind) — **`next build`
  passes** (17 routes + a nonce-CSP middleware). Sattva-style structure, our own
  honest copy (local-first / BYO models / open-source — **not** "no account
  needed"), **no fabricated social proof**, dark/responsive/accessible.
- **GitHub-only login** — Auth.js (NextAuth v5), GitHub provider only, JWT
  session, account upserted on the **stable `github_id`**. Client secret lives
  only on the server.
- **One account across web + desktop** — the VSCode app's "Sign in with GitHub"
  runs **OAuth Authorization Code + PKCE over a `127.0.0.1` loopback** (device
  flow as fallback), exchanges a single-use code for **our own** app tokens, and
  stores them in **VSCode SecretStorage** (OS keychain). The app never sees the
  GitHub secret or token.
- **Local-first preserved** — the backend has no route or table for prompts/code/
  inference. Login never gates local use.
- **Security-reviewed** — a 5-lens adversarial review ran against the auth code;
  I fixed the real findings (device-flow phishing, refresh-token reuse detection,
  access-TTL/JWT hardening, approve-endpoint CSRF, spoofable rate-limit IP,
  `unsafe-inline` CSP, loopback hardening, HTTPS-only transport). Details in §6.
- **CLI/CI untouched** — zero Rust changes this round; 123 tests still pass.

---

## 1. What I built & how to run it

### The website (`website/` — its own Vercel project)
- Landing `/` (Hero + Features + How-it-works + Comparison + Privacy + FAQ + CTA +
  Footer), `/privacy`, `/account`, `/desktop/activate`.
- Auth API under `/api/auth/*` (Auth.js) and `/api/desktop/*` + `/api/me`.

**Local dev:**
```bash
cd website
npm install
cp .env.example .env.local      # fill in (see checklist below)
psql "$DATABASE_URL" -f db/schema.sql
npm run dev                     # http://localhost:3000
```
The marketing pages render without a DB; the auth flows need `DATABASE_URL` + the
GitHub OAuth app.

**Deploy (Vercel):** import the repo, set **Root Directory = `website`**, add the
env vars, deploy. Full step-by-step in [website/README.md](website/README.md).

### The desktop sign-in (VSCode extension)
Set **`forge.accountServer`** (e.g. `http://localhost:3000` dev, your domain
prod), then click **Sign in with GitHub** in the panel header. Build the binary
and press F5 as before; sign-in is optional and never blocks local use.

---

## 2. Owner setup checklist (you must do this — I can't)

| # | Task | Where the value goes |
| :- | :-- | :-- |
| 1 | Register a **GitHub OAuth App** (`https://github.com/settings/developers`). Callback URL: `<site>/api/auth/callback/github` (one per environment). | `AUTH_GITHUB_ID`, `AUTH_GITHUB_SECRET` |
| 2 | Provision **Postgres** (Neon recommended); run `psql "$DATABASE_URL" -f db/schema.sql`. | `DATABASE_URL` |
| 3 | Generate two secrets: `openssl rand -base64 32` (×2). | `AUTH_SECRET`, `APP_JWT_SECRET` |
| 4 | Set `AUTH_URL` / `NEXT_PUBLIC_SITE_URL` to the site's URL. | env |
| 5 | (Optional) per-OS installer URLs. | `NEXT_PUBLIC_DOWNLOAD_{MACOS,WINDOWS,LINUX}` |
| 6 | Vercel project: Root Directory = `website`; add all env vars (Prod + Preview); set domain. | Vercel settings |

`.env.example` documents every variable and which are server-only vs.
client-safe (`NEXT_PUBLIC_*`). **The GitHub client secret and both signing
secrets are server-only** — never in any client, never committed.

---

## 3. The auth flows

### Web sign-in (Authorization Code)
```
Browser ──"Sign in with GitHub"──▶ /api/auth/signin/github (Auth.js)
   │  Auth.js handles state + CSRF + the OAuth dance
   ▼
GitHub authorize ──▶ /api/auth/callback/github
   │  jwt() callback: upsert users row by github_id; copy ONLY identity into JWT
   ▼
httpOnly+Secure+SameSite=Lax session cookie  ──▶  /account
```

### Desktop sign-in (PKCE over loopback) — primary
```
 VSCode extension (public client, NO secret)
   1. verifier = random; challenge = base64url(sha256(verifier)); state = random
   2. start loopback http server on 127.0.0.1:<random port>
   3. openExternal:  <backend>/api/desktop/start?code_challenge=…&code_challenge_method=S256
                       &redirect_uri=http://127.0.0.1:<port>/callback&state=…
        │
        ▼  backend /start  (validates loopback redirect + 43-char S256 challenge)
   GitHub web sign-in (if no session) ──▶ back to /start
        │  mint single-use code (hashed, 5-min TTL) bound to {github_id, challenge, redirect}
        ▼
   302 ──▶ http://127.0.0.1:<port>/callback?code=…&state=…
        │  extension verifies state (CSRF), closes the loopback server
        ▼
   POST <backend>/api/desktop/token { code, code_verifier, redirect_uri }
        │  backend: consume code (single-use), require+match redirect_uri,
        │  verify PKCE S256 (constant-time), issue OUR tokens
        ▼
   { access_token (15-min JWT), refresh_token (rotating), user }
        └──▶ stored in VSCode SecretStorage (OS keychain)
```

### Desktop sign-in (device flow) — fallback
```
 extension ─POST /api/desktop/device/start──▶ { device_code, user_code, verification_uri }
   shows user_code in a modal; opens verification_uri (NO code in the URL)
 user (browser, signed in) ─▶ /desktop/activate
   types the code → /api/desktop/device/info shows "who's asking" (app + when)
   → explicit confirm → /api/desktop/device/approve (Origin-checked)
 extension polls /api/desktop/device/token → { access_token, refresh_token, user }
```

### Token lifecycle
- **Access token:** stateless HS256 JWT (`jose`), `sub = github_id`, `iss`/`aud`
  pinned, **`algorithms: ["HS256"]`** enforced, **15-minute** TTL.
- **Refresh token:** opaque random, **stored as SHA-256 hash**, 30-day TTL,
  grouped by `family_id`. Refresh **rotates** the token; **reuse of an already-
  rotated token revokes the whole family** (theft detection).
- **Sign-out / revoke:** clears the keychain entry and revokes refresh tokens
  (`all: true` kills every session for the identity). Access tokens expire within
  ≤15 min (documented residual; see §6).

### Security decisions
- **Secret handling:** GitHub client secret & both signing secrets are server-
  only (env, never `NEXT_PUBLIC_`, never in the extension, never logged). The raw
  GitHub token is **never** copied into the session JWT (guarded with a comment).
- **PKCE:** S256, constant-time verify, single-use + 5-min code, `redirect_uri`
  required and matched, challenge format-validated.
- **Loopback:** only `http://127.0.0.1` / `http://[::1]` literals accepted
  (`localhost` dropped); `new URL().hostname` defeats `@`-userinfo tricks;
  `Referrer-Policy: no-referrer` + `Cache-Control: no-store` on the callback page.
- **Cookies:** Auth.js v5 defaults — `httpOnly`, `Secure` (prod), `SameSite=Lax`,
  built-in CSRF; custom POST endpoints add an explicit same-origin check.
- **Transport:** the extension refuses a non-HTTPS `accountServer` unless it's
  loopback.
- **CSP:** per-request **nonce** (middleware), `script-src` without
  `unsafe-inline`, `frame-ancestors 'none'`, `object-src 'none'`.

---

## 4. Data model — exactly what GitHub data is stored

`website/db/schema.sql` — four tables, **identity/distribution only**:

| Table | Columns | Holds |
| :-- | :-- | :-- |
| `users` | `github_id` (UNIQUE key), `login`, `name`, `avatar_url`, `email` (nullable), `created_at`, `last_login_at` | the account |
| `desktop_auth_codes` | `code_hash`, `github_id`, `code_challenge`, `redirect_uri`, `expires_at` | single-use PKCE codes (hashed) |
| `desktop_refresh_tokens` | `token_hash`, `github_id`, `family_id`, `expires_at`, `revoked_at` | hashed, rotating, revocable refresh tokens |
| `device_codes` | `device_code_hash`, `user_code`, `github_id`, `approved`, `consumed`, `user_agent`, `expires_at` | device-flow codes (hashed) |

**Stored from GitHub:** numeric id, login, name, avatar URL, and email *only if
granted*. **Never stored:** prompts, code, conversations, files, or any inference
— there is no column or route for them. Scopes requested: `read:user user:email`
(email is optional; see open questions).

---

## 5. What changed across the repos

### Added — `website/` (new, self-contained Vercel deployable)
Config (`package.json`, `tsconfig.json`, `next.config.mjs`, `postcss/tailwind`
configs, `middleware.ts`, `.env.example`, `.gitignore`, `README.md`); marketing
(`src/app/{layout,page,globals,privacy,account,desktop/activate}` + 11
components); auth (`src/auth.ts`, `src/app/api/auth/[...nextauth]`,
`src/app/api/desktop/{start,token,refresh,revoke,device/{start,token,approve,info}}`,
`src/app/api/me`, `src/app/api/health`); libs (`src/lib/{env,db,crypto,tokens,
ratelimit}`); `db/schema.sql`; `src/types/next-auth.d.ts`. **Committed
`package-lock.json`**; `node_modules`/`.next`/`.env*` gitignored.

### Modified — VSCode extension (`editor-integrations/forge-vscode/`)
- **Added** `src/auth.js` (`ForgeAuth`: PKCE loopback + device flow, SecretStorage,
  refresh/revoke, HTTPS-only transport).
- `src/extension.js` — construct `ForgeAuth`, register `forge.signIn` /
  `forge.signInDevice` / `forge.signOut`.
- `src/chatViewProvider.js` — account chip wiring, `signIn/out` handlers,
  `#account` element; sends account state independently of inference.
- `media/main.js` + `media/main.css` — render the account chip / sign-in.
- `package.json` — `forge.accountServer` setting + 3 commands.
- `README.md` — sign-in section.

### Unchanged — Rust crate / CLI / CI
**Zero Rust changes this round.** `cargo test` → **123 passed / 0 failed**,
clippy clean. The only modified tracked Rust files (`cli/mod.rs`,
`providers/ollama.rs`) are from prior rounds. The website does not touch `cargo`.

### Verification performed
- `npm run build` (website) — passes: type-check + lint + 17 routes + middleware.
- `node --check` — all extension JS parses.
- PKCE client/backend cross-check — `base64url(sha256(verifier))` agrees.
- Secret-boundary greps — no secret env names in client/marketing components; no
  `'use client'` file imports a server lib.
- `cargo test` — 123/0; CLI subcommands intact.

> What I **could not** verify here: a live end-to-end OAuth round-trip (needs your
> GitHub OAuth app + a real DB) and the VSCode webview rendering of the account
> chip (needs an interactive host). The protocol, types, and build are verified;
> the live flow is a documented manual checklist (§2 + website/README).

---

## 6. Security review — findings & fixes

A 5-lens adversarial review (OAuth/PKCE, token lifecycle, secret boundary,
web-appsec, local-first privacy) ran against the auth code. It **confirmed the
hard invariants hold** (no GitHub secret in any client; raw GitHub token never in
the session JWT; backend stores identity only; login never gates inference;
Neon queries parameterized → no SQLi; `/api/me` properly bearer-authed). Real
findings and what I did:

| Finding (severity) | Fix |
| :-- | :-- |
| **Device-flow phishing** (high) — `verification_uri_complete` prefilled the code → one-click account-injection | Removed code-in-URL; activate page now requires **typing** the code, shows **what's requesting access** (app + when) via a new `/device/info` step, and an **explicit warning + confirm**. Extension shows the code in a modal and opens the bare `verification_uri`. |
| **Refresh-token reuse not detected** (high) — rotation revoked only the presented token | Added `family_id` lineage; replay of an already-rotated/expired token now **revokes the whole family** (`token_reuse_detected`). |
| **Access token not revocable until TTL** (med) | Shortened access TTL **1h → 15 min**; documented the residual window. (A `token_version` deny-list is a noted follow-up for instant kill.) |
| **JWT alg / secret strength** (med) | Pinned `algorithms: ["HS256"]`; enforce `APP_JWT_SECRET` ≥ 32 chars; validate `sub` is numeric. |
| **Device-approve CSRF** (med) | Added same-origin (`Origin`) check on `/device/approve` and `/device/info`. |
| **Rate-limit IP spoofable** (med) | `clientIp` now trusts the platform hop (`x-vercel-forwarded-for` / `x-real-ip`), not the attacker-controlled left-most XFF. (Global limiter via Vercel KV is the documented production step.) |
| **CSP `unsafe-inline`** (med) | Moved CSP to a **per-request nonce** in `middleware.ts`; dropped `unsafe-inline` for scripts. |
| Loopback hardening (low) | Dropped `localhost` (IP literals only); validate 43-char S256 challenge; `redirect_uri` required+matched at `/token`; `Referrer-Policy: no-referrer` on the callback. |
| Cleartext identity transport (low) | Extension **requires HTTPS** for any non-loopback `accountServer`. |

The site rebuilds clean after all fixes.

---

## 7. Risks, unknowns & open questions

1. **`user:email` scope — keep it?** I request `read:user user:email` and store
   email nullable. If you don't need email yet, drop `user:email` to request even
   less. Your call.
2. **DB choice.** I built against `@neondatabase/serverless` (Neon) — best fit for
   Vercel serverless. It also works with Vercel Postgres/Supabase. Confirm Neon or
   tell me to switch.
3. **Session strategy.** I chose **JWT** sessions (no DB session table) to keep
   the schema minimal. If you'd prefer DB-backed sessions (server-side
   invalidation of web sessions), that's a small change + the Auth.js adapter
   tables.
4. **Global rate limiting.** The in-memory limiter is per-instance best-effort;
   production should back it with **Vercel KV / Upstash** for a hard global limit
   (especially on the device endpoints). Flagged, not built (no infra to
   provision from here).
5. **Access-token instant revocation.** 15-min TTL bounds exposure; a
   `users.token_version` claim checked in `/api/me` would make "sign out
   everywhere" instant. Easy follow-up if you want it.
6. **Live OAuth not exercised here.** I can't create your GitHub app or DB, so the
   end-to-end flow is verified by build + protocol + the manual checklist, not a
   live run. First real sign-in is the acceptance test.
7. **Download links.** The per-OS buttons read `NEXT_PUBLIC_DOWNLOAD_*` and show
   "coming soon" until set — they tie to the Phase 3 desktop distribution
   (`desktop/`), which isn't published yet.
8. **Social proof.** Testimonials/ratings/counters are omitted with a labeled
   placeholder in `CTA.tsx` — fill with real, attributable quotes when you have
   them; don't ship invented ones.

---

*Try it: `cd website && npm install && npm run build` (passes offline). To run
the flows, complete the §2 checklist, `npm run dev`, set `forge.accountServer` in
the extension, and click **Sign in with GitHub**. Full setup:
[website/README.md](website/README.md).*

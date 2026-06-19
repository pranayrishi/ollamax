# Sorting Login (Ollamax) — exact checklist

Login = the website (account server) deployed + reachable, with two OAuth apps,
and the app's `forge.accountServer` pointed at it. I've prepped everything that
doesn't require your accounts; the **4 steps marked 👤 are yours** (I can't create
OAuth apps or deploy to your Vercel/Neon).

---

## 0. 🔒 Security first
The Postgres string you pasted is a **live credential** and is now exposed. In the
Neon console, **reset that role's password** and use the NEW connection string
below as `DATABASE_URL`. Don't paste it back in chat — put it straight into Vercel
env / your local `.env.local` (which is gitignored).

## Secrets (generated for you — safe to use)
```
AUTH_SECRET=ENAq+Ot5sLZ4riNcuR1NvIOgb9/zovNbSEb72UXn3bE=
APP_JWT_SECRET=1bO1X+zQJqcAC56GBcpOVn13WOYEj8snpPctldvqv4k=
```

---

## 1. 👤 Create the GitHub OAuth app
github.com → Settings → Developers → **OAuth Apps** → New OAuth App.
- Homepage: your site URL (Vercel URL, or `http://localhost:3000` to test first).
- **Authorization callback URL** (add all three — GitHub allows multiple):
  - `<SITE>/api/auth/callback/github`
  - `<SITE>/api/link/callback`
  - `<SITE>/api/star/callback`
- Copy the **Client ID** + generate a **Client secret** → `AUTH_GITHUB_ID` / `AUTH_GITHUB_SECRET`.

## 2. 👤 Create the Google OAuth client
Google Cloud Console → APIs & Services → Credentials → **OAuth client ID** (Web).
- **Authorized redirect URIs:**
  - `<SITE>/api/auth/callback/google`
  - `<SITE>/api/link/callback`
- Copy Client ID + secret → `AUTH_GOOGLE_ID` / `AUTH_GOOGLE_SECRET`.

## 3. Apply the DB schema (once, to the ROTATED Neon DB)
```
psql "<your NEW Neon DATABASE_URL>" -f website/db/schema.sql
```
(Give me the rotated string and I'll run this for you.)

## 4. The full env (set in Vercel for prod, or website/.env.local to test locally)
```
AUTH_SECRET=ENAq+Ot5sLZ4riNcuR1NvIOgb9/zovNbSEb72UXn3bE=
APP_JWT_SECRET=1bO1X+zQJqcAC56GBcpOVn13WOYEj8snpPctldvqv4k=
AUTH_URL=<SITE>                 # http://localhost:3000  OR  https://<your>.vercel.app
NEXT_PUBLIC_SITE_URL=<SITE>
AUTH_GITHUB_ID=...               # step 1
AUTH_GITHUB_SECRET=...
AUTH_GOOGLE_ID=...               # step 2
AUTH_GOOGLE_SECRET=...
DATABASE_URL=<rotated Neon string>   # step 0
NEXT_PUBLIC_RELEASES_REPO=https://github.com/pranayrishi/ollamax-releases
```

## 5. Run it
- **Test locally (fast dry run, only on your machine):**
  `cd website && npm run dev` → it serves `http://localhost:3000`. Set VS Code
  setting **`forge.accountServer` = `http://localhost:3000`** → Ollamax now gates on
  sign-in (the extension allows http for localhost).
- **Launch to other users (required):** 👤 **deploy `website/` to Vercel** with the
  env above, then set `forge.accountServer` = your `https://<app>.vercel.app`.
  (Local localhost only works for *you* — other users need the deployed URL.)

## 6. Then I finish it
Give me the **deployed `https://…` URL** (and the 4 OAuth values if you want me to
fill env / run the local test). I will:
- set `forge.accountServer` in the extension + **bake it into the fork build**
  (`FORGE_ACCOUNT_SERVER`) so the standalone **Ollamax** app enforces login by default,
- verify a signed-out user is blocked,
- then **kick off the local Ollamax fork build** (your standalone app).

---

### What I've already verified/prepped
- The website has every endpoint the app needs (`/api/desktop/start|token|refresh`,
  `/api/me`, NextAuth GitHub+Google) — checked in source.
- Secrets generated; `.env*.local` is gitignored.
- The gate code is correct: when `forge.accountServer` is set, the app blocks
  chat/agent before any engine call until sign-in.
- (in progress) Verifying `website/` builds cleanly so your Vercel deploy won't
  fail on a code error.

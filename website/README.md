# Ollama-Forge — Website + Account Backend

Marketing site **and** the GitHub-only account backend, in one Next.js app
(App Router + TypeScript + Tailwind), deployable on **Vercel**. It is a separate
deployable from the Rust crate — nothing here touches `cargo`.

> **Identity / distribution only.** This backend never receives, proxies, logs,
> or stores prompts, code, or inference. There is no table for any of that.

## What's here

- **Marketing pages** — `/` (landing), `/privacy`, `/account`, `/desktop/activate`.
- **GitHub-only auth** — Auth.js (NextAuth v5), GitHub provider only, at
  `/api/auth/*`.
- **Desktop sign-in** — PKCE loopback + device-flow endpoints under
  `/api/desktop/*`, plus `/api/me`. Issues our own app tokens (the desktop app
  never sees the GitHub secret).

## Owner setup checklist (do this once)

### 1. Register a GitHub OAuth App
<https://github.com/settings/developers> → **New OAuth App**.

| Field | Value |
| :-- | :-- |
| Application name | Ollama-Forge (anything) |
| Homepage URL | `https://your-domain` (or `http://localhost:3000` for dev) |
| Authorization callback URL | `https://your-domain/api/auth/callback/github` |

For local dev, register a **second** OAuth App (or add the callback)
`http://localhost:3000/api/auth/callback/github`. Copy the **Client ID** and
generate a **Client secret**.

> The desktop loopback/device flows do **not** need their own GitHub callback —
> they authenticate through this site's `/api/auth/callback/github` and then
> hand the app our own code/token. So one callback URL per environment is enough.

### 2. Provision a Postgres database (Neon recommended)
<https://neon.tech> → create a project → copy the **pooled** connection string.
Then apply the schema:

```bash
psql "$DATABASE_URL" -f db/schema.sql
```

(Vercel Postgres or Supabase work too — any Postgres. The driver is
`@neondatabase/serverless`, which speaks to any Postgres over the Neon proxy or
a standard connection string.)

### 3. Set environment variables
Copy `.env.example` → `.env.local` (dev) and set the same keys in **Vercel →
Project → Settings → Environment Variables** (prod). Generate the two secrets
with `openssl rand -base64 32`.

| Var | Purpose | Where |
| :-- | :-- | :-- |
| `AUTH_SECRET` | Auth.js session/JWT signing | server only |
| `AUTH_URL` | this site's base URL (callbacks) | server |
| `NEXT_PUBLIC_SITE_URL` | public base URL (absolute links) | client-safe |
| `AUTH_GITHUB_ID` | GitHub OAuth client id | server |
| `AUTH_GITHUB_SECRET` | GitHub OAuth **client secret** | **server only — never client** |
| `APP_JWT_SECRET` | signs desktop app tokens (separate from `AUTH_SECRET`) | server only |
| `DATABASE_URL` | Postgres connection string | server only |
| `NEXT_PUBLIC_DOWNLOAD_{MACOS,WINDOWS,LINUX}` | per-OS installer URLs (optional) | client-safe |
| `NEXT_PUBLIC_GITHUB_REPO` | repo link in footer | client-safe |

### 4. Vercel project
- Import the repo; set **Root Directory = `website`**.
- Framework preset: Next.js (auto).
- Add the env vars above (Production + Preview).
- Deploy. Set your custom domain and update `AUTH_URL` / `NEXT_PUBLIC_SITE_URL`
  + the GitHub callback URL to match.

## Run locally

```bash
cd website
npm ci                         # Node 20.9+ (required by Next.js 16)
cp .env.example .env.local   # then fill it in
npm run dev                  # http://localhost:3000
```

The marketing pages render without a DB. The auth flows require `DATABASE_URL`
and the GitHub app to be configured.

## Point the desktop app at it

In the VSCode extension settings, set **`forge.accountServer`** to this site's
URL (`http://localhost:3000` for dev, your domain for prod). Then use
**Sign in with GitHub** in the chat panel. Sign-in is identity only — local
inference works signed-out.

## Scripts

| Command | What |
| :-- | :-- |
| `npm run dev` | Dev server |
| `npm run build` | Production build (type-checked + linted) |
| `npm run start` | Serve the production build |
| `npm run typecheck` | `tsc --noEmit` |

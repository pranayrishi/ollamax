# Ollama-Forge — Website + Account Backend

Marketing site **and** the GitHub-only account backend, in one Next.js app
(App Router + TypeScript + Tailwind), deployable on **Vercel**. It is a separate
deployable from the Rust crate — nothing here touches `cargo`.

> **Identity / distribution only.** This backend never receives, proxies, logs,
> or stores prompts, code, or inference. There is no table for any of that.
> Marketing copy must distinguish a feature present in the source tree from one
> present in a published installer. In particular, the public `v0.2.0` download
> predates the local voice, spatial-context, and expanded-model work; do not
> advertise those as available in that artifact.

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
| `NEXT_PUBLIC_RELEASES_REPO` | public releases-repository base URL (optional; asset names are fixed in `src/lib/downloads.ts`) | client-safe |
| `NEXT_PUBLIC_GITHUB_REPO` | repo link in footer | client-safe |

## Publish download links only after the complete release

The desktop release is deliberately two-stage. First, an `app-vX.Y.Z` tag runs
the Electron packaging workflow and attaches native installers to a draft in
the public releases repository. After that workflow succeeds, the matching
`vX.Y.Z` tag builds the CLI/VS Code bundles, verifies the full asset contract,
and publishes that draft:

```bash
git tag app-vX.Y.Z && git push origin app-vX.Y.Z
# Wait for release-app to finish successfully.
git tag vX.Y.Z && git push origin vX.Y.Z
```

Only then flip `published` to `true` for each verified asset in
`src/lib/downloads.ts` and deploy the site. Keep Intel macOS disabled until its
own native artifact exists. `app-v*` is staging, not a public download version.
Do not point `latest` or promotional copy at an app-only draft, and do not
back-port current source-tree claims to the public `v0.2.0` downloads.

For a build based on the current source tree, consumer-local inference uses
Ollama-local Qwen, Gemma 4, and DeepSeek-R1 models. DeepSeek V4 and MiniMax M3
are cataloged as separately operated, server-class local OpenAI-compatible
options, never as one-click Ollama downloads or cloud fallbacks. An operator
can configure a literal-loopback `/v1` endpoint and named served models, then
explicitly select `local:<endpoint>/<model>` in Chat, Agent, Research, or Team.
Auto routing never chooses it, and Build/Orchestrator remains Ollama-only. A
local-server bearer token, if needed, is named by `api_key_env`; its value is
not stored in the configuration or surfaced to the site.

The generic compatibility path is deliberately narrow: text plus images only
for a model the operator explicitly declares vision-capable. It does not enable
DeepSeek V4's model-specific encoding/reasoning channel, MiniMax M3 video,
native tools, or structured reasoning. Do not promote a declared `thinking`
flag as support for those provider-specific features. MiniMax M3 also carries
its own licensing/notice obligations.

## Desktop voice, lasso, and cursor cue disclosure

The Electron source uses local `whisper.cpp` only when a local runtime is
configured or a particular release has staged and validated it. Its checked-in
manifest deliberately declares the Whisper binary and GGML model **unbundled**.
For an `app-v*` release, CI builds pinned `whisper.cpp` v1.9.1 natively,
validates the reviewed `ggml-base.en.bin` size and SHA-256, stages the license,
and flips the manifest to `bundled: true` only in its disposable packaging
workspace. A package must still be checked rather than assumed to contain those
assets until its matching public workflow has passed. When unavailable, the
voice control explains local setup and has no AssemblyAI, ElevenLabs, or other
hosted STT/TTS fallback. Optional speech output is an operating-system voice or
an explicitly configured local command.

Screen-region context is an explicit lasso action. The full display capture is
kept in memory only long enough to make a bounded selected crop, then is
discarded; only that crop is sent to a local vision model. Agent and Team turn
the crop into an untrusted transient visual brief, exclude it from memory and
replay, and disable web tools for that visual turn. A small transparent,
click-through cursor cue reports fixed local voice/selection states near the
pointer. It cannot inspect the screen or windows, see prompts/transcripts or
pixels, move/click the mouse, or use accessibility APIs.

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

# Round 5 Build Report — Scale & Security, Decisive Auto-Routing, and the Central Hub

> Three features, built in order. **Auto-routing** makes the router the default
> and visible; **scale/security** hardens the Round-4 backend with a written
> threat model; **the Central Hub** adds a separate sidebar panel of domain
> "packages" compiled from curated public repos, plus an **explicit opt-in**
> "support maintainers" star flow. CLI / app / website / CI all intact.
> Generated 2026-06-17.

---

## 0. TL;DR

- **Auto model routing (F2):** the chat picker defaults to **Auto** → the
  existing `TaskRouter` classifies each task and picks a **local** model by size
  tier, with the decision + a one-line "why" shown on every reply. Local-only;
  never silently escalates to cloud. Stale `ModelConfig` ladder aligned. +3 Rust
  tests (**126** total); verified live (Simple → smallest, Complex → larger).
- **Scale & security (F1):** durable global rate limiter (Upstash, with
  in-memory fallback) on auth/star/hub; security **audit log** (token-redacted);
  **CI security scanning** (`cargo audit` + `npm audit` + gitleaks); a written
  **threat model** ([website/SECURITY-THREATMODEL.md](website/SECURITY-THREATMODEL.md)).
  Least-privilege scopes reaffirmed.
- **The Central Hub (F3):** a **54-category** data-driven taxonomy; a server-side
  **ingestion pipeline** (GitHub Search API, license + quality/safety gates that
  **don't trust stars alone**); packages **compile into rules + skill scaffolds +
  curated references** (transparent steering, not repo-dumping); a **separate
  Activity Bar Hub panel** with category cards + "+" that writes into your
  `rules/`/`skills/` dirs; and an **opt-in** "Support these maintainers" star flow.
- **Auto-starring: NOT built** (it violates GitHub's AUP). Only the explicit,
  transparent, opt-in version — write scope requested *only* at the star step,
  the elevated GitHub token **never persisted**. (§5)
- **Reviewed:** a 3-lens adversarial review confirmed the invariants hold and
  found hardening gaps; **all fixed and reverified** (§6).
- Site builds (**23 routes**), **126** Rust tests, all extension JS `node
  --check`, secret boundary intact, Hub serves 54 categories live.

---

## 1. Feature 2 — Decisive Auto routing

Reuses the existing router; makes it the default and visible.

- **What:** the model picker has an **"Auto"** option (now the default). When
  selected, `handle_chat` ([src/server/mod.rs](src/server/mod.rs)) calls the real
  `TaskRouter::analyze_complexity` to classify the latest user turn
  (Simple/Medium/Complex/Architect), then picks from the **installed local
  models sorted by size** (simple → smallest, architect → largest) — a *decisive*
  tier mapping (the raw `select_model_for_task` could grab a small "coder" model
  for a complex task via substring matching). The chosen model + reasoning ride
  in the `meta` event: `🔀 Auto: Complex task (score 0.67) → qwen2.5-coder:7b`.
- **Override + cost policy:** a manual pick always overrides. Auto only ever
  selects **installed local Ollama models** (`route_to_model` returns only those)
  — it **never** escalates to a paid cloud provider; cloud requires an explicit
  manual choice. Documented in the picker hint.
- **Ladder reconciliation:** `ModelConfig::default` in
  [src/router/mod.rs](src/router/mod.rs) updated from the stale
  `llama3.2/deepseek-coder` set to the canonical `qwen2.5-coder` ladder (the
  inconsistency flagged in the original analysis). These are fallback names only;
  behavior is unchanged.
- **Tests:** `route_auto` (simple vs complex), fallback when no models,
  `latest_user_text`.
- **Try it:** pick **Auto** in the panel; each reply shows the routed model + why.

---

## 2. Feature 1 — Scale to 5,000+ & Security Hardening

**Scale (not over-engineered):** managed Postgres (Neon, pooled), stateless
horizontally-scaling backend, CDN-served site. The real ceilings are **GitHub
API limits** (handled in the Hub: authenticated token, paced + capped queries,
caching, *scheduled* ingestion so clients never call GitHub) and **account
security** — not request volume. The threat model documents a pre-launch **load
test** rather than guessing.

**Security (a process):** a living **threat model**
([website/SECURITY-THREATMODEL.md](website/SECURITY-THREATMODEL.md)) plus concrete
code:
- **Durable global rate limiting** — [src/lib/ratelimit.ts](website/src/lib/ratelimit.ts)
  `checkRateLimit` uses Upstash Redis REST (fixed-window INCR+EXPIRE) when
  configured, else per-instance in-memory; applied (via `await`) to every
  auth/star/hub endpoint. Client IP from the trusted platform hop, not
  attacker-controlled XFF.
- **Audit log** — [src/lib/audit.ts](website/src/lib/audit.ts) emits structured,
  **token-redacted** JSON for sign-in/out, token issue/refresh/**reuse**/revoke,
  device approval, star scope grant, and stars applied.
- **CI scanning** — [.github/workflows/security.yml](.github/workflows/security.yml):
  `cargo audit` (RustSec), `npm audit --omit=dev`, gitleaks. Deps pinned
  (`Cargo.lock`, `package-lock.json`).
- **Least-privilege scopes** — identity login stays **read-only**
  (`read:user user:email`); the **`public_repo` write scope is requested only on
  explicit opt-in** for starring (the riskiest new surface — minimized).
- **Encryption** — HTTPS/TLS everywhere (extension refuses non-loopback http
  `accountServer`); managed-Postgres at-rest encryption; secrets server-only.
- **Sessions/tokens** (from Round 4, intact) — httpOnly/Secure/SameSite cookies;
  15-min access JWTs (pinned `alg`, `iss`/`aud`, numeric `sub`); hashed, rotated,
  reuse-detecting refresh tokens; OS-keychain storage in the app.
- **Pre-launch plan** — independent pentest, coordinated disclosure
  (`SECURITY.md`), secret-store verification, load test, Dependabot.

**Boundary reaffirmed:** the backend handles **identity, the Hub catalog, and
distribution only** — never prompts, code, or inference. (Confirmed by the
review: no route or table accepts user content.)

---

## 3. Feature 3 — The Central Hub

### Where it lives
A **separate Activity Bar container** ("Forge Hub", `forge-hub`) with its own
webview `forge.hubView` — distinct from the chat panel
([editor-integrations/forge-vscode/src/hub.js](editor-integrations/forge-vscode/src/hub.js)).
Category cards each with a **"+"** to activate; a detail view shows exactly what a
package injects + the curated repos + licenses + the opt-in star action.

### What a package is (honest mechanism)
Not whole repos dumped into context. A package **compiles** a domain into the
agent's **existing steering mechanisms**
([src/lib/hub/compile.ts](website/src/lib/hub/compile.ts)):
- **Rules** — generic, license-safe domain conventions → written to the user's
  `rules/` dir (the `rules_suffix` injected into every system prompt).
- **Skills** — scaffold recipes in forge's native `Skill` JSON shape → written to
  the `skills/` dir (loaded by `SkillsEngine`).
- **Curated references** — license-respecting **links** to gated repos (never
  fetched source) with attribution.
- **Routing hints** — a domain tag.

On "+" the extension fetches the compiled package and writes those files into
the same config dirs the `forge` binary reads. **Transparent and reversible** —
the panel shows the counts; deleting the files removes the steering.

### The catalog (server-side, continuously updated)
- **Taxonomy** — [website/src/data/hub-taxonomy.json](website/src/data/hub-taxonomy.json):
  **54 categories** (Web, Mobile, Game, 3D, Data/ML, NLP, CV, Backend, DevOps,
  Cloud, Databases, Security, Blockchain, Embedded, Robotics, Audio/DSP,
  Scientific, Bioinformatics, Design systems, …) curated from GitHub topics by a
  fan-out workflow. **Data-driven**: add a category by editing JSON, no code.
- **Ingestion** — [src/lib/hub/ingest.ts](website/src/lib/hub/ingest.ts) uses the
  GitHub **Search API** (no official "trending" API exists), authenticated with
  `GITHUB_INGEST_TOKEN`, paced + capped, storing metadata (stars, topics,
  language, **license**, pushed_at) in `hub_repos` + `hub_category_repos`.
  Triggered on a **schedule** via cron-protected
  [/api/hub/refresh](website/src/app/api/hub/refresh/route.ts). **Clients never
  call GitHub** — they read our cached catalog
  ([/api/hub/categories](website/src/app/api/hub/categories/route.ts),
  [/api/hub/package/[slug]](website/src/app/api/hub/package/[slug]/route.ts), both
  CDN-cacheable).
- **Curation (not stars alone)** — [src/lib/hub/quality.ts](website/src/lib/hub/quality.ts):
  a composite score (log-popularity + recency + **fork-ratio sanity** to penalize
  star-gaming + license presence) and a hard **gate** (license required, ≥200
  stars *necessary-not-sufficient*, maintained within 3y, denylist hook). Repos
  that fail are excluded from inclusion (link-only). The link-only posture caps
  the blast radius of a bad inclusion (no repo source ever enters a package).

### Licensing (required)
Every repo's SPDX license is tracked. **No-license (all-rights-reserved) repos
are excluded** from a package beyond a link. Permissive licenses (MIT/Apache/BSD/
…) are flagged; copyleft is linked + attributed but not used as reference
content. License is surfaced in the Hub UI and the package payload.

### Try it
Set `forge.accountServer`, open the **Forge Hub** icon, click a category's **+**
(writes its rules/skills), open a category to see the references + licenses, and
optionally **Support these maintainers**.

---

## 4. The repo catalog — ingestion, curation, refresh (specifics)

- **Discovery:** `ingestCategory` runs each category's `searchQueries`
  (`topic:/language:/stars:` filters), maps results → `HubRepo`, scores + gates,
  upserts. `ingestAll` walks all 54 (or a `?limit=N` slice) and **stops on rate
  limit** (403/429) to resume on the next scheduled run.
- **Rate-limit posture:** authenticated token → 5,000/hr; 3 queries/category,
  20/page, 800 ms pacing; conditional-request/ETag column present for future
  per-repo refresh; GraphQL is the documented next efficiency step.
- **Refresh:** `POST /api/hub/refresh` (Bearer `CRON_SECRET`, **constant-time**
  check) — wire a Vercel Cron at it (e.g. daily, chunked via `?limit=`).

---

## 5. Opt-in starring — and why automated starring was NOT built

**I did not build the auto-star mechanic.** As described it is **automated /
silent starring** — which GitHub's Acceptable Use Policies explicitly prohibit
(rank abuse; incentivized/inauthentic interactions). GitHub detects and purges
these stars and **suspends the accounts and OAuth apps** involved. Building it
would risk suspending **our users' GitHub accounts and our OAuth app**, inflate a
gamed metric, and surprise users. The good underlying goal — crediting
maintainers — is served the legitimate way:

**What I built** (explicit, transparent, opt-in, user-initiated):
1. In the Hub package view, **"Support these maintainers"** → the extension
   creates a server-side **star intent** (app-bearer auth) listing the exact
   repos, and opens `<site>/star/<id>` in the browser.
2. [/star/[id]](website/src/app/star/[id]/page.tsx): the user (GitHub-signed-in,
   intent-owner-checked) sees the **exact repo list** with per-repo checkboxes
   (license shown), copy stating it's optional with **no rewards/unlocking**.
3. On confirm, [/api/star/authorize](website/src/app/api/star/authorize/route.ts)
   requests the **`public_repo` scope — the only place it's ever requested**,
   never at login — carrying the chosen subset in a **signed state JWT** (no DB
   write on the GET).
4. [/api/star/callback](website/src/app/api/star/callback/route.ts) exchanges the
   code server-side, **verifies the authorizing GitHub account == the intent
   owner**, stars only the selected repos (constrained to the intent), then
   **discards the token** (used for seconds, never persisted — no token column,
   never logged). Every step is audit-logged.

No automated, silent, or incentivized starring anywhere. The user reviews and
consciously chooses every star.

---

## 6. Security review — findings & fixes

A 3-lens adversarial review (starring-OAuth, hub-ingest, privacy/boundary)
**confirmed the invariants hold**: identity + public catalog only (no
prompt/code/inference route or table); `public_repo` only at the star step; the
elevated token never persisted; client secret server-only; Neon queries
parameterized (no SQLi); SSRF closed (taxonomy-driven queries, hardcoded host);
explicit/opt-in/non-incentivized starring. Real gaps found → **all fixed and
the site rebuilt clean**:

| Finding (sev) | Fix |
| :-- | :-- |
| Callback didn't verify the authorizing GitHub account == the state-bound identity (med) | Callback now `GET /user`, asserts `id === decoded.gh` before starring; audits the authenticated id. |
| `/authorize` did a state-changing DB write on a GET (low) | The selected subset is now signed **into the state JWT**; `/authorize` performs **no DB write** before the redirect. |
| `/star/[id]` success page forgeable via query params (low/high-conf) | Success now renders only for the **owner of a consumed intent**. |
| Scope-denied path left the intent reusable (low) | Intent is **consumed** on scope-denial (and on account-mismatch). |
| `full_name` regex admitted `.`/`..` segments (low/high) | Strict validation rejecting dot-only/`..`; star URL segments `encodeURIComponent`'d. |
| `star/intent` stored client `html_url` → stored-href self-XSS (low/high) | `html_url` is **derived server-side** from `full_name`; `license_spdx` SPDX-validated. |
| Hub client wrote files using server `skill.name` unsanitized (low) | Client takes `basename`, allows `[\w.-]`, and confirms path stays within the dir. |
| `CRON_SECRET` non-constant-time compare (low/high) | Uses the constant-time `safeEqual`. |
| Audit redaction regex anchored (info) | Unanchored so token-shaped keys are always redacted. |

Accepted/documented (info): the rate limiter **fails open** (availability >
strictness) on read-only catalog GETs — fine given CDN caching + no writes; for a
hard global cap, configure Upstash in prod. The malware-repo defense is
**link-only inclusion** (no repo source enters a package) plus the quality gate;
a maintained denylist + manual allowlist for high-traffic categories is a noted
follow-up.

---

## 7. Owner setup checklist (do this; I can't)

| # | Task | Value → where |
| :- | :-- | :-- |
| 1 | Apply the updated schema (adds hub + star tables): `psql "$DATABASE_URL" -f website/db/schema.sql` | DB |
| 2 | Create a **GitHub ingestion token** — fine-grained PAT, **read-only public repos**, no write/private | `GITHUB_INGEST_TOKEN` |
| 3 | Provision **Upstash Redis** (durable global rate limit) | `UPSTASH_REDIS_REST_URL`, `UPSTASH_REDIS_REST_TOKEN` |
| 4 | Set a cron secret + wire a **Vercel Cron** at `POST /api/hub/refresh` (Bearer it; chunk with `?limit=`) | `CRON_SECRET` |
| 5 | Add **`<site>/api/star/callback`** as an additional **Authorization callback URL** on the GitHub OAuth App (for the elevated star flow). Identity login still uses `/api/auth/callback/github`. | GitHub OAuth App |
| 6 | Confirm identity scopes are read-only (`read:user user:email`); `public_repo` is requested only at the star step (already in code). | — |

Full env reference: [website/.env.example](website/.env.example). Scale/infra +
pre-launch review: [website/SECURITY-THREATMODEL.md](website/SECURITY-THREATMODEL.md).

---

## 8. What changed & verification

**Added (website):** `src/lib/audit.ts`, `src/lib/hub/{taxonomy,quality,ingest,
compile}.ts`, `src/data/hub-taxonomy.json` (54), `src/app/api/hub/{categories,
package/[slug],refresh}`, `src/app/api/star/{intent,authorize,callback}`,
`src/app/star/[id]/{page,StarList}.tsx` + `src/app/star/error`,
`SECURITY-THREATMODEL.md`. **Modified:** `src/lib/{ratelimit,db,tokens,env,
auth}.ts` + the device/token/refresh/revoke routes (durable limiter + audit),
`.env.example`, `db/schema.sql`.

**Added (extension):** `src/hub.js`, `media/{hub.js,hub.css,hub-icon.svg}`.
**Modified:** `src/{extension,auth,chatViewProvider}.js`, `media/main.js`,
`package.json` (Hub container/view + `forge.accountServer`), `README.md`.

**Modified (Rust):** `src/server/mod.rs` (Auto routing + tests),
`src/router/mod.rs` (ladder). **Added CI:** `.github/workflows/security.yml`.

**Confirmation nothing broke:**
- `cargo fmt --check` ✓, `clippy -D warnings` ✓, `cargo test` → **126/0**
  (CLI subcommands intact).
- Website `npm run build` ✓ (23 routes + middleware, type-checked + linted).
- All extension JS `node --check` ✓; `package.json` valid.
- Secret boundary: no secrets under `NEXT_PUBLIC`; no `'use client'` file imports
  a server lib; `client_secret` only in the server star callback.
- Live: Auto routing (simple→smallest, complex→larger); Hub `/api/hub/categories`
  serves all 54 categories.

> Not exercised here (needs owner infra): live GitHub ingestion (token), the
> real star round-trip (GitHub app + DB + the extra callback URL), Upstash, and a
> 5k load test. These are protocol- + build-verified and documented as the
> acceptance checklist.

---

## 9. Risks & open questions

1. **Catalog curation quality.** The gate is heuristic (stars-not-alone +
   license + recency + fork-sanity); `DENYLIST` ships empty. For high-traffic
   categories, do you want a **manual allowlist/review** step before `included=
   true`, and a maintained advisory denylist source? (Recommended.)
2. **License edge cases.** SPDX `NOASSERTION`/missing → excluded from inclusion
   (link-only). Dual-licensed / `LICENSE`-without-SPDX repos may be
   under-included. Confirm the conservative "permissive-only for reference,
   link+attribute everything else" policy is what you want.
3. **GitHub rate limits at scale.** 54 categories × queries fits one token's
   5k/hr with chunked cron; if you add many categories or refresh often, move to
   **GraphQL** + ETag conditional requests (columns are in place). Flag if you
   want me to implement the GraphQL path.
4. **Elevated star flow is live-untested** (no GitHub app here). First real
   "Support maintainers" is the acceptance test; needs the extra callback URL
   (checklist #5).
5. **Rate-limiter fail-open** on catalog GETs is intentional; configure Upstash
   in prod for a real global cap. Consider fail-closed specifically on
   `/api/star/authorize` if star-spam ever becomes a concern.
6. **Auto-routing quality** depends on the heuristic complexity scorer; it's now
   *visible*, so users can correct it with a manual pick. If you want, the next
   step is a tiny local-model "router" prompt for borderline tasks.

---

*Try the headline: `cd website && npm install && npm run build` (offline OK);
complete the §7 checklist; then in the app set `forge.accountServer`, open the
**Forge Hub**, and click a category's **+**. Pick **Auto** in chat to watch the
router choose. Full setup: [website/README.md](website/README.md).*

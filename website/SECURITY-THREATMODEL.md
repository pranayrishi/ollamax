# Threat Model — Ollama-Forge Accounts & Central Hub Backend

> Security is a process, not a checkbox. This is the living threat model for the
> hosted backend (identity + Hub catalog + distribution). It is reviewed each
> time the auth or starring surface changes. Last updated: Round 5.

## Scope & non-goals

**In scope:** the Next.js backend (auth, desktop token flows, Hub catalog,
opt-in starring), its database, and the desktop client's handling of tokens.

**Hard boundary (unchanged since Round 4):** the backend handles **identity, the
Hub catalog, and distribution only**. It **never** receives, proxies, logs, or
stores user prompts, code, or inference traffic. There is no route or table for
that data. Inference stays local (Ollama) or goes from the user's machine
directly to a provider they chose.

## Assets

| Asset | Sensitivity |
| :-- | :-- |
| GitHub OAuth **client secret**, `AUTH_SECRET`, `APP_JWT_SECRET`, `DATABASE_URL`, `GITHUB_INGEST_TOKEN` | Critical — server-only |
| User identity rows (github_id, login, name, avatar, optional email) | PII |
| Desktop access/refresh tokens | Session credentials |
| **Elevated `public_repo` grant** (starring) — the riskiest new surface | Write access to the user's GitHub |
| Hub catalog (public repo metadata) | Low (public data) |

## Trust boundaries

```
 user's machine (app + local Ollama)  │  the public internet  │  our backend (Vercel)  │  Postgres / GitHub API
   prompts/code NEVER cross ───────────┘                       (identity + catalog only)
```

## Threats & mitigations (STRIDE-flavored)

| Threat | Mitigation |
| :-- | :-- |
| **Spoofing identity** | GitHub-only OAuth (Auth.js), keyed on stable numeric `github_id`; desktop access tokens are signed HS256 JWTs with pinned `algorithms`, `iss`/`aud`, numeric-`sub` validation. |
| **Token theft / replay** | Short (15-min) access TTL; refresh tokens hashed at rest, rotated, with **reuse-detection → family revocation**; sign-out revokes server-side + clears the OS keychain. |
| **CSRF** | Auth.js built-in for its routes; hand-rolled POSTs (device approve/info, star) add an explicit same-origin check. |
| **Device-flow phishing** | No prefilled code; user types it; the approval page shows *what* is requesting access; explicit confirm. (RFC 8628 §5.4.) |
| **Open redirect / code interception** | Desktop loopback redirect restricted to `127.0.0.1`/`[::1]` literals; PKCE S256 constant-time; single-use 5-min codes; `redirect_uri` required+matched. |
| **Elevated-scope abuse (starring)** | `public_repo` requested **only** on explicit opt-in, never at login; the user reviews the **exact repo list** before authorizing; the write token is used immediately and **not persisted**; every star is audit-logged. No automated/silent/incentivized starring (GitHub AUP). |
| **Rate-limit bypass / brute force** | Durable global limiter (Upstash) on auth/star/hub endpoints; client IP taken from the trusted platform hop, not attacker-controlled XFF. |
| **Injection** | All DB access via parameterized `neon` tagged templates; inputs schema-checked. |
| **XSS** | Per-request nonce CSP (no `unsafe-inline` scripts); React escaping; avatar `img-src` pinned to githubusercontent. |
| **Supply chain** | Pinned deps (`package-lock`, `Cargo.lock`); CI runs `cargo audit` + `npm audit` + gitleaks (`.github/workflows/security.yml`). |
| **Secret leakage** | Secrets server-only (never `NEXT_PUBLIC_`, never in the client/extension, never logged — audit log redacts token-shaped fields). Raw GitHub token never copied into the session JWT. |
| **Repudiation** | Security audit log (`lib/audit.ts`) for sign-in/out, token issue/refresh/reuse/revoke, device approval, scope grants, and stars applied. |
| **Malicious catalog repo** | Curation gates (activity, dependents, fork-ratio, license present, denylist) before a repo enters a package; packages inject *distilled conventions/links*, not executable repo content. |

## Scale posture (5,000+ users)

- Managed Postgres (Neon) with pooled connections; the backend is stateless and
  scales horizontally on Vercel; the website is CDN-served.
- **The real ceilings are GitHub API limits and account security, not request
  volume.** Ingestion uses an authenticated token (5,000/hr), conditional
  requests/ETags, GraphQL where efficient, server-side caching, and a *scheduled*
  refresh so clients never call GitHub directly.
- Durable rate limiting (Upstash) makes per-IP/-user limits global.
- **Load test before launch** (e.g. k6/Artillery against `/api/me`,
  `/api/hub/catalog`, the token endpoints) to confirm headroom rather than
  guessing.

## Pre-launch checklist (do before public launch)

- [ ] Independent third-party security review / pentest of the auth + starring
      surface.
- [ ] Coordinated disclosure: keep `SECURITY.md` current with a security contact;
      consider a `security.txt`.
- [ ] Verify all secrets are in the platform secret store, none in git
      (gitleaks green) or client bundles.
- [ ] Confirm cookies are `httpOnly`+`Secure`+`SameSite` in production over HTTPS.
- [ ] Confirm the elevated `public_repo` grant is requested only on opt-in and
      the write token is never persisted.
- [ ] Run a load test at ~2× expected peak; confirm GitHub ingestion stays within
      rate limits.
- [ ] Enable Dependabot / scheduled `security.yml`; triage advisories.

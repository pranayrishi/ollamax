-- Ollama-Forge account backend schema.
--
-- IDENTITY / DISTRIBUTION / USAGE-METADATA ONLY. There is no table here for
-- prompts, code, file contents, file paths, repo names, or inference of any
-- kind — by design. The backend never receives that data. The usage_events
-- table (Round 6) holds COUNTS/CATEGORIES only (see the content-rejection guard
-- in src/lib/analytics.ts).
--
-- Apply once against your Postgres (Neon) database:
--   psql "$DATABASE_URL" -f db/schema.sql

-- =====================================================================
-- Multi-identity accounts (Round 6): one account can link BOTH a GitHub and a
-- Google identity. The account is the internal `users.id`; provider identities
-- live in `identities` and are linked by VERIFIED email (or explicitly).
-- =====================================================================
CREATE TABLE IF NOT EXISTS users (
  id               BIGSERIAL PRIMARY KEY,
  primary_email    TEXT,                          -- best-known email; nullable
  email_verified   BOOLEAN     NOT NULL DEFAULT false,
  name             TEXT,
  avatar_url       TEXT,
  telemetry_opt_out BOOLEAN    NOT NULL DEFAULT false, -- usage metadata; user-controlled
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_login_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A provider identity (github | google) linked to one account. We key on the
-- provider's STABLE account id, never email/login (those are mutable).
CREATE TABLE IF NOT EXISTS identities (
  id                  BIGSERIAL PRIMARY KEY,
  user_id             BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider            TEXT        NOT NULL,        -- 'github' | 'google'
  provider_account_id TEXT        NOT NULL,        -- stable id from the provider
  email               TEXT,
  email_verified      BOOLEAN     NOT NULL DEFAULT false,
  login               TEXT,                        -- github login / google given name
  avatar_url          TEXT,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (provider, provider_account_id)
);
CREATE INDEX IF NOT EXISTS idx_identities_user ON identities (user_id);
CREATE INDEX IF NOT EXISTS idx_identities_email ON identities (lower(email)) WHERE email_verified;

-- One-time authorization codes for the desktop loopback (PKCE) flow.
CREATE TABLE IF NOT EXISTS desktop_auth_codes (
  code_hash      TEXT        PRIMARY KEY,
  user_id        BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  code_challenge TEXT        NOT NULL,
  redirect_uri   TEXT        NOT NULL,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at     TIMESTAMPTZ NOT NULL
);

-- Long-lived refresh tokens for the desktop app. Hashed at rest; rotating,
-- family-grouped (reuse detection), revocable.
CREATE TABLE IF NOT EXISTS desktop_refresh_tokens (
  token_hash  TEXT        PRIMARY KEY,
  user_id     BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  family_id   TEXT        NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at  TIMESTAMPTZ NOT NULL,
  revoked_at  TIMESTAMPTZ
);

-- Device-authorization-flow codes (loopback-free fallback).
CREATE TABLE IF NOT EXISTS device_codes (
  device_code_hash TEXT        PRIMARY KEY,
  user_code        TEXT        NOT NULL UNIQUE,
  user_id          BIGINT,                        -- set when the user approves
  approved         BOOLEAN     NOT NULL DEFAULT false,
  consumed         BOOLEAN     NOT NULL DEFAULT false,
  user_agent       TEXT,
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at       TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_refresh_user ON desktop_refresh_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_family ON desktop_refresh_tokens (family_id);
CREATE INDEX IF NOT EXISTS idx_authcodes_expiry ON desktop_auth_codes (expires_at);
CREATE INDEX IF NOT EXISTS idx_devicecodes_expiry ON device_codes (expires_at);

-- Explicit account-linking intents (link a 2nd provider while signed in).
CREATE TABLE IF NOT EXISTS link_states (
  id          TEXT PRIMARY KEY,
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider    TEXT NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at  TIMESTAMPTZ NOT NULL
);

-- =====================================================================
-- Central Hub (Round 5): server-side curated catalog of PUBLIC repos.
-- =====================================================================
CREATE TABLE IF NOT EXISTS hub_repos (
  full_name     TEXT PRIMARY KEY,
  description   TEXT,
  stars         INTEGER NOT NULL DEFAULT 0,
  forks         INTEGER NOT NULL DEFAULT 0,
  language      TEXT,
  topics        TEXT[] NOT NULL DEFAULT '{}',
  license_spdx  TEXT,
  license_name  TEXT,
  html_url      TEXT NOT NULL,
  pushed_at     TIMESTAMPTZ,
  quality_score REAL NOT NULL DEFAULT 0,
  included      BOOLEAN NOT NULL DEFAULT false,
  etag          TEXT,
  fetched_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS hub_category_repos (
  category_slug TEXT NOT NULL,
  full_name     TEXT NOT NULL REFERENCES hub_repos(full_name) ON DELETE CASCADE,
  PRIMARY KEY (category_slug, full_name)
);
CREATE INDEX IF NOT EXISTS idx_hub_cat ON hub_category_repos (category_slug);

-- Opt-in "Support these maintainers" star intents (now keyed on the account;
-- the GitHub identity is resolved/linked at the star step).
CREATE TABLE IF NOT EXISTS star_intents (
  id          TEXT PRIMARY KEY,
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  repos       JSONB NOT NULL,
  category    TEXT,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at  TIMESTAMPTZ NOT NULL,
  consumed    BOOLEAN NOT NULL DEFAULT false
);

-- =====================================================================
-- Usage analytics (Round 6): METADATA ONLY. Per-user, content-free.
-- Powers the web dashboard. Validated/rejected server-side so no content
-- (prompt text, code, file contents/paths, repo names) can ever be stored.
-- =====================================================================
CREATE TABLE IF NOT EXISTS usage_events (
  id         BIGSERIAL PRIMARY KEY,
  user_id    BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  ts         TIMESTAMPTZ NOT NULL DEFAULT now(),
  type       TEXT        NOT NULL,   -- chat | agent | build | route | hub_activate | suggestion
  provider   TEXT,                   -- 'ollama' (local) | provider name
  model      TEXT,                   -- model TAG only (e.g. qwen2.5-coder:7b)
  tokens_in  INTEGER,
  tokens_out INTEGER,
  language   TEXT,                   -- inferred from file extension only (e.g. 'rust')
  accepted   BOOLEAN                 -- for suggestion events: applied/accepted?
);
CREATE INDEX IF NOT EXISTS idx_usage_user_ts ON usage_events (user_id, ts);

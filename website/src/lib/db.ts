// Database access layer (Neon serverless Postgres). All queries are
// parameterized via the `neon` tagged-template (values bound as SQL params, not
// interpolated), so this layer is injection-safe. Server-only.
import "server-only";
import { neon, type NeonQueryFunction } from "@neondatabase/serverless";
import { env } from "./env";

let _sql: NeonQueryFunction<false, false> | null = null;
function db(): NeonQueryFunction<false, false> {
  if (!_sql) _sql = neon(env.databaseUrl());
  return _sql;
}

// =====================================================================
// Multi-identity accounts
// =====================================================================

export type User = {
  id: number;
  primary_email: string | null;
  email_verified: boolean;
  name: string | null;
  avatar_url: string | null;
  telemetry_opt_out: boolean;
  created_at: string;
  last_login_at: string;
};

export type Identity = {
  id: number;
  user_id: number;
  provider: string;
  provider_account_id: string;
  email: string | null;
  email_verified: boolean;
  login: string | null;
  avatar_url: string | null;
};

export async function getUserById(id: number): Promise<User | null> {
  const rows = (await db()`SELECT * FROM users WHERE id = ${id}`) as User[];
  return rows[0] ?? null;
}

export async function getLinkedProviders(userId: number): Promise<string[]> {
  const rows = (await db()`SELECT provider FROM identities WHERE user_id = ${userId} ORDER BY provider`) as {
    provider: string;
  }[];
  return rows.map((r) => r.provider);
}

export async function getIdentity(provider: string, accountId: string): Promise<Identity | null> {
  const rows = (await db()`
    SELECT * FROM identities WHERE provider = ${provider} AND provider_account_id = ${accountId}
  `) as Identity[];
  return rows[0] ?? null;
}

/** The user's GitHub identity (for GitHub-only actions like starring), or null. */
export async function getGithubIdentity(userId: number): Promise<Identity | null> {
  const rows = (await db()`
    SELECT * FROM identities WHERE user_id = ${userId} AND provider = 'github' LIMIT 1
  `) as Identity[];
  return rows[0] ?? null;
}

async function touchUser(userId: number): Promise<void> {
  await db()`UPDATE users SET last_login_at = now() WHERE id = ${userId}`;
}

async function findUserIdByVerifiedEmail(email: string): Promise<number | null> {
  // Link ONLY by a verified email match (never unverified — that would let an
  // attacker hijack an account by signing up a provider with someone's email).
  const rows = (await db()`
    SELECT user_id FROM identities
    WHERE email_verified = true AND lower(email) = lower(${email}) LIMIT 1
  `) as { user_id: number }[];
  return rows[0]?.user_id ?? null;
}

type IdentityInput = {
  provider: string;
  providerAccountId: string;
  email: string | null;
  emailVerified: boolean;
  name: string | null;
  avatarUrl: string | null;
  login: string | null;
};

async function insertIdentity(userId: number, p: IdentityInput): Promise<void> {
  await db()`
    INSERT INTO identities (user_id, provider, provider_account_id, email, email_verified, login, avatar_url)
    VALUES (${userId}, ${p.provider}, ${p.providerAccountId}, ${p.email}, ${p.emailVerified}, ${p.login}, ${p.avatarUrl})
    ON CONFLICT (provider, provider_account_id) DO UPDATE SET
      email = EXCLUDED.email, email_verified = EXCLUDED.email_verified,
      login = EXCLUDED.login, avatar_url = EXCLUDED.avatar_url
  `;
}

/**
 * Find-or-create-or-link for a sign-in. Rules:
 *  1. If this provider identity already exists → that user (update profile).
 *  2. Else if the email is VERIFIED and matches an existing account's verified
 *     identity → LINK this new identity to that account.
 *  3. Else → create a new account with this identity.
 * Returns the resolved internal user id.
 */
export async function resolveUserForIdentity(p: IdentityInput): Promise<{ userId: number; linked: boolean }> {
  const existing = await getIdentity(p.provider, p.providerAccountId);
  if (existing) {
    await insertIdentity(existing.user_id, p); // refresh profile
    await touchUser(existing.user_id);
    return { userId: existing.user_id, linked: false };
  }

  let userId: number | null = null;
  if (p.email && p.emailVerified) {
    userId = await findUserIdByVerifiedEmail(p.email);
  }
  if (userId) {
    await insertIdentity(userId, p);
    await touchUser(userId);
    return { userId, linked: true };
  }

  const created = (await db()`
    INSERT INTO users (primary_email, email_verified, name, avatar_url)
    VALUES (${p.email}, ${p.emailVerified}, ${p.name}, ${p.avatarUrl})
    RETURNING id
  `) as { id: number }[];
  const newUserId = created[0].id;
  await insertIdentity(newUserId, p);
  return { userId: newUserId, linked: false };
}

/**
 * Explicitly link a provider identity to an ALREADY-signed-in account. Refuses
 * to steal an identity already linked to a different account.
 */
export async function linkIdentityToUser(
  userId: number,
  p: IdentityInput
): Promise<{ ok: boolean; conflict: boolean }> {
  const existing = await getIdentity(p.provider, p.providerAccountId);
  if (existing && existing.user_id !== userId) {
    return { ok: false, conflict: true }; // identity belongs to another account
  }
  await insertIdentity(userId, p);
  return { ok: true, conflict: false };
}

/** Public view returned to clients — never internal-only fields. */
export async function publicUser(userId: number): Promise<{
  id: number;
  email: string | null;
  name: string | null;
  avatarUrl: string | null;
  providers: string[];
  telemetryOptOut: boolean;
} | null> {
  const u = await getUserById(userId);
  if (!u) return null;
  const providers = await getLinkedProviders(userId);
  return {
    id: u.id,
    email: u.primary_email,
    name: u.name,
    avatarUrl: u.avatar_url,
    providers,
    telemetryOptOut: u.telemetry_opt_out,
  };
}

/** Real registered-user count (Feature 3). */
export async function countUsers(): Promise<number> {
  const rows = (await db()`SELECT count(*)::int AS n FROM users`) as { n: number }[];
  return rows[0]?.n ?? 0;
}

/** Users active in the last N days (≥1 usage event), defined precisely. */
export async function countActiveUsers(days: number): Promise<number> {
  const rows = (await db()`
    SELECT count(DISTINCT user_id)::int AS n FROM usage_events
    WHERE ts > now() - (${days} || ' days')::interval
  `) as { n: number }[];
  return rows[0]?.n ?? 0;
}

// ---- desktop loopback (PKCE) one-time codes ----

export async function createDesktopAuthCode(p: {
  codeHash: string;
  userId: number;
  codeChallenge: string;
  redirectUri: string;
  expiresAt: Date;
}): Promise<void> {
  await db()`
    INSERT INTO desktop_auth_codes (code_hash, user_id, code_challenge, redirect_uri, expires_at)
    VALUES (${p.codeHash}, ${p.userId}, ${p.codeChallenge}, ${p.redirectUri}, ${p.expiresAt.toISOString()})
  `;
}

export async function consumeDesktopAuthCode(codeHash: string): Promise<{
  user_id: number;
  code_challenge: string;
  redirect_uri: string;
  expires_at: string;
} | null> {
  const rows = (await db()`
    DELETE FROM desktop_auth_codes WHERE code_hash = ${codeHash}
    RETURNING user_id, code_challenge, redirect_uri, expires_at
  `) as { user_id: number; code_challenge: string; redirect_uri: string; expires_at: string }[];
  return rows[0] ?? null;
}

// ---- desktop refresh tokens ----

export async function createRefreshToken(p: {
  tokenHash: string;
  userId: number;
  familyId: string;
  expiresAt: Date;
}): Promise<void> {
  await db()`
    INSERT INTO desktop_refresh_tokens (token_hash, user_id, family_id, expires_at)
    VALUES (${p.tokenHash}, ${p.userId}, ${p.familyId}, ${p.expiresAt.toISOString()})
  `;
}

export async function findRefreshTokenAny(tokenHash: string): Promise<{
  user_id: number;
  family_id: string;
  revoked_at: string | null;
  expires_at: string;
} | null> {
  const rows = (await db()`
    SELECT user_id, family_id, revoked_at, expires_at
    FROM desktop_refresh_tokens WHERE token_hash = ${tokenHash}
  `) as { user_id: number; family_id: string; revoked_at: string | null; expires_at: string }[];
  return rows[0] ?? null;
}

export async function revokeRefreshToken(tokenHash: string): Promise<void> {
  await db()`UPDATE desktop_refresh_tokens SET revoked_at = now() WHERE token_hash = ${tokenHash}`;
}

export async function revokeRefreshFamily(familyId: string): Promise<void> {
  await db()`UPDATE desktop_refresh_tokens SET revoked_at = now() WHERE family_id = ${familyId} AND revoked_at IS NULL`;
}

export async function revokeAllRefreshTokens(userId: number): Promise<void> {
  await db()`UPDATE desktop_refresh_tokens SET revoked_at = now() WHERE user_id = ${userId} AND revoked_at IS NULL`;
}

// ---- device flow ----

export async function createDeviceCode(p: {
  deviceCodeHash: string;
  userCode: string;
  userAgent: string | null;
  expiresAt: Date;
}): Promise<void> {
  await db()`
    INSERT INTO device_codes (device_code_hash, user_code, user_agent, expires_at)
    VALUES (${p.deviceCodeHash}, ${p.userCode}, ${p.userAgent}, ${p.expiresAt.toISOString()})
  `;
}

export async function getPendingDeviceByUserCode(userCode: string): Promise<{
  user_agent: string | null;
  created_at: string;
  approved: boolean;
  consumed: boolean;
  expires_at: string;
} | null> {
  const rows = (await db()`
    SELECT user_agent, created_at, approved, consumed, expires_at
    FROM device_codes WHERE user_code = ${userCode}
  `) as {
    user_agent: string | null;
    created_at: string;
    approved: boolean;
    consumed: boolean;
    expires_at: string;
  }[];
  return rows[0] ?? null;
}

export async function approveDeviceCode(userCode: string, userId: number): Promise<boolean> {
  const rows = (await db()`
    UPDATE device_codes SET approved = true, user_id = ${userId}
    WHERE user_code = ${userCode} AND consumed = false AND expires_at > now()
    RETURNING user_code
  `) as { user_code: string }[];
  return rows.length > 0;
}

export async function getDeviceCode(deviceCodeHash: string): Promise<{
  user_id: number | null;
  approved: boolean;
  consumed: boolean;
  expires_at: string;
} | null> {
  const rows = (await db()`
    SELECT user_id, approved, consumed, expires_at FROM device_codes
    WHERE device_code_hash = ${deviceCodeHash}
  `) as { user_id: number | null; approved: boolean; consumed: boolean; expires_at: string }[];
  return rows[0] ?? null;
}

export async function consumeDeviceCode(deviceCodeHash: string): Promise<void> {
  await db()`UPDATE device_codes SET consumed = true WHERE device_code_hash = ${deviceCodeHash}`;
}

// ---- explicit account-linking states ----

export async function createLinkState(p: {
  id: string;
  userId: number;
  provider: string;
  expiresAt: Date;
}): Promise<void> {
  await db()`
    INSERT INTO link_states (id, user_id, provider, expires_at)
    VALUES (${p.id}, ${p.userId}, ${p.provider}, ${p.expiresAt.toISOString()})
  `;
}

export async function consumeLinkState(id: string): Promise<{ user_id: number; provider: string; expires_at: string } | null> {
  const rows = (await db()`
    DELETE FROM link_states WHERE id = ${id}
    RETURNING user_id, provider, expires_at
  `) as { user_id: number; provider: string; expires_at: string }[];
  return rows[0] ?? null;
}

// =====================================================================
// Central Hub catalog (public metadata only)
// =====================================================================

export type HubRepo = {
  full_name: string;
  description: string | null;
  stars: number;
  forks: number;
  language: string | null;
  topics: string[];
  license_spdx: string | null;
  license_name: string | null;
  html_url: string;
  pushed_at: string | null;
  quality_score: number;
  included: boolean;
};

export async function upsertHubRepo(r: HubRepo): Promise<void> {
  await db()`
    INSERT INTO hub_repos
      (full_name, description, stars, forks, language, topics, license_spdx, license_name,
       html_url, pushed_at, quality_score, included, fetched_at)
    VALUES
      (${r.full_name}, ${r.description}, ${r.stars}, ${r.forks}, ${r.language}, ${r.topics},
       ${r.license_spdx}, ${r.license_name}, ${r.html_url}, ${r.pushed_at}, ${r.quality_score},
       ${r.included}, now())
    ON CONFLICT (full_name) DO UPDATE SET
      description = EXCLUDED.description, stars = EXCLUDED.stars, forks = EXCLUDED.forks,
      language = EXCLUDED.language, topics = EXCLUDED.topics, license_spdx = EXCLUDED.license_spdx,
      license_name = EXCLUDED.license_name, html_url = EXCLUDED.html_url, pushed_at = EXCLUDED.pushed_at,
      quality_score = EXCLUDED.quality_score, included = EXCLUDED.included, fetched_at = now()
  `;
}

export async function mapRepoToCategory(category: string, fullName: string): Promise<void> {
  await db()`
    INSERT INTO hub_category_repos (category_slug, full_name)
    VALUES (${category}, ${fullName}) ON CONFLICT DO NOTHING
  `;
}

export async function getCategoryRepos(category: string, includedOnly = true): Promise<HubRepo[]> {
  return includedOnly
    ? ((await db()`
        SELECT r.* FROM hub_repos r JOIN hub_category_repos m ON m.full_name = r.full_name
        WHERE m.category_slug = ${category} AND r.included = true
        ORDER BY r.quality_score DESC LIMIT 60
      `) as HubRepo[])
    : ((await db()`
        SELECT r.* FROM hub_repos r JOIN hub_category_repos m ON m.full_name = r.full_name
        WHERE m.category_slug = ${category}
        ORDER BY r.quality_score DESC LIMIT 60
      `) as HubRepo[]);
}

export async function getCategoryCounts(): Promise<Record<string, number>> {
  const rows = (await db()`
    SELECT category_slug, count(*)::int AS n
    FROM hub_category_repos m JOIN hub_repos r ON r.full_name = m.full_name
    WHERE r.included = true GROUP BY category_slug
  `) as { category_slug: string; n: number }[];
  const out: Record<string, number> = {};
  for (const r of rows) out[r.category_slug] = r.n;
  return out;
}

// ---- star intents (keyed on the account) ----

export async function createStarIntent(p: {
  id: string;
  userId: number;
  repos: unknown;
  category: string | null;
  expiresAt: Date;
}): Promise<void> {
  await db()`
    INSERT INTO star_intents (id, user_id, repos, category, expires_at)
    VALUES (${p.id}, ${p.userId}, ${JSON.stringify(p.repos)}, ${p.category}, ${p.expiresAt.toISOString()})
  `;
}

export async function getStarIntent(id: string): Promise<{
  user_id: number;
  repos: { full_name: string; html_url: string; license_spdx: string | null }[];
  category: string | null;
  consumed: boolean;
  expires_at: string;
} | null> {
  const rows = (await db()`SELECT user_id, repos, category, consumed, expires_at FROM star_intents WHERE id = ${id}`) as {
    user_id: number;
    repos: { full_name: string; html_url: string; license_spdx: string | null }[];
    category: string | null;
    consumed: boolean;
    expires_at: string;
  }[];
  return rows[0] ?? null;
}

export async function consumeStarIntent(id: string): Promise<void> {
  await db()`UPDATE star_intents SET consumed = true WHERE id = ${id}`;
}

// =====================================================================
// Usage analytics (metadata only)
// =====================================================================

export type UsageEventInput = {
  type: string;
  provider: string | null;
  model: string | null;
  tokensIn: number | null;
  tokensOut: number | null;
  language: string | null;
  accepted: boolean | null;
  ts: Date;
};

export async function insertUsageEvents(userId: number, events: UsageEventInput[]): Promise<void> {
  for (const e of events) {
    await db()`
      INSERT INTO usage_events (user_id, ts, type, provider, model, tokens_in, tokens_out, language, accepted)
      VALUES (${userId}, ${e.ts.toISOString()}, ${e.type}, ${e.provider}, ${e.model},
              ${e.tokensIn}, ${e.tokensOut}, ${e.language}, ${e.accepted})
    `;
  }
}

export async function getUserRawEvents(userId: number, limit = 50_000): Promise<unknown[]> {
  return (await db()`
    SELECT ts, type, provider, model, tokens_in, tokens_out, language, accepted
    FROM usage_events WHERE user_id = ${userId} ORDER BY ts DESC LIMIT ${limit}
  `) as unknown[];
}

export async function deleteUserUsage(userId: number): Promise<number> {
  const rows = (await db()`
    WITH d AS (DELETE FROM usage_events WHERE user_id = ${userId} RETURNING 1)
    SELECT count(*)::int AS n FROM d
  `) as { n: number }[];
  return rows[0]?.n ?? 0;
}

export async function setTelemetryOptOut(userId: number, optOut: boolean): Promise<void> {
  await db()`UPDATE users SET telemetry_opt_out = ${optOut} WHERE id = ${userId}`;
}

/** Per-user aggregates for the dashboard. Strictly scoped to the one user. */
export async function getUserUsage(userId: number): Promise<{
  totals: { events: number; tokensIn: number; tokensOut: number };
  byType: { type: string; n: number }[];
  byModel: { model: string | null; provider: string | null; n: number }[];
  byLanguage: { language: string; n: number }[];
  daily: { day: string; n: number }[];
  suggestions: { made: number; accepted: number };
}> {
  const totals = (await db()`
    SELECT count(*)::int AS events, coalesce(sum(tokens_in),0)::int AS tin, coalesce(sum(tokens_out),0)::int AS tout
    FROM usage_events WHERE user_id = ${userId}
  `) as { events: number; tin: number; tout: number }[];
  const byType = (await db()`
    SELECT type, count(*)::int AS n FROM usage_events WHERE user_id = ${userId} GROUP BY type ORDER BY n DESC
  `) as { type: string; n: number }[];
  const byModel = (await db()`
    SELECT model, provider, count(*)::int AS n FROM usage_events
    WHERE user_id = ${userId} AND model IS NOT NULL GROUP BY model, provider ORDER BY n DESC LIMIT 20
  `) as { model: string | null; provider: string | null; n: number }[];
  const byLanguage = (await db()`
    SELECT language, count(*)::int AS n FROM usage_events
    WHERE user_id = ${userId} AND language IS NOT NULL GROUP BY language ORDER BY n DESC LIMIT 20
  `) as { language: string; n: number }[];
  const daily = (await db()`
    SELECT to_char(date_trunc('day', ts), 'YYYY-MM-DD') AS day, count(*)::int AS n
    FROM usage_events WHERE user_id = ${userId} AND ts > now() - interval '90 days'
    GROUP BY 1 ORDER BY 1
  `) as { day: string; n: number }[];
  const sugg = (await db()`
    SELECT count(*)::int AS made, coalesce(sum(CASE WHEN accepted THEN 1 ELSE 0 END),0)::int AS accepted
    FROM usage_events WHERE user_id = ${userId} AND type = 'suggestion'
  `) as { made: number; accepted: number }[];

  return {
    totals: { events: totals[0]?.events ?? 0, tokensIn: totals[0]?.tin ?? 0, tokensOut: totals[0]?.tout ?? 0 },
    byType,
    byModel,
    byLanguage,
    daily,
    suggestions: { made: sugg[0]?.made ?? 0, accepted: sugg[0]?.accepted ?? 0 },
  };
}

// App tokens issued to the DESKTOP client. OUR tokens, not the providers' —
// the app never sees a GitHub/Google client secret or raw provider token.
//
// Keyed on the internal account id (`users.id`) so a user who signed in with
// EITHER GitHub or Google gets the same desktop session.
import "server-only";
import { SignJWT, jwtVerify } from "jose";
import { env } from "./env";
import { randomToken, sha256 } from "./crypto";
import { createRefreshToken } from "./db";

const ACCESS_TTL_SECONDS = 15 * 60; // 15 minutes
const REFRESH_TTL_DAYS = 30;
const ISSUER = "ollama-forge";
const AUDIENCE = "ollama-forge-desktop";

function key(): Uint8Array {
  return new TextEncoder().encode(env.appJwtSecret());
}

export type AppTokenClaims = { sub: string; name: string | null };

/** Sign a short-lived desktop access JWT. `sub` is the internal user id. */
export async function signAccessToken(p: { userId: number; name?: string | null }): Promise<string> {
  return await new SignJWT({ name: p.name ?? null })
    .setProtectedHeader({ alg: "HS256", typ: "JWT" })
    .setSubject(String(p.userId))
    .setIssuer(ISSUER)
    .setAudience(AUDIENCE)
    .setIssuedAt()
    .setExpirationTime(`${ACCESS_TTL_SECONDS}s`)
    .sign(key());
}

export async function verifyAccessToken(token: string): Promise<AppTokenClaims | null> {
  try {
    const { payload } = await jwtVerify(token, key(), {
      issuer: ISSUER,
      audience: AUDIENCE,
      algorithms: ["HS256"],
    });
    const sub = String(payload.sub ?? "");
    if (!/^\d+$/.test(sub)) return null;
    return { sub, name: payload.name == null ? null : String(payload.name) };
  } catch {
    return null;
  }
}

export async function issueRefreshToken(userId: number, familyId?: string): Promise<string> {
  const token = randomToken(32);
  const expiresAt = new Date(Date.now() + REFRESH_TTL_DAYS * 24 * 60 * 60 * 1000);
  await createRefreshToken({
    tokenHash: sha256(token),
    userId,
    familyId: familyId ?? randomToken(16),
    expiresAt,
  });
  return token;
}

export const ACCESS_EXPIRES_IN = ACCESS_TTL_SECONDS;

// ---- starring (opt-in) OAuth state ----
const STAR_STATE_AUD = "ollama-forge-star-state";

export async function signStarState(intentId: string, userId: number, repos: string[]): Promise<string> {
  return await new SignJWT({ intent: intentId, uid: userId, repos })
    .setProtectedHeader({ alg: "HS256", typ: "JWT" })
    .setIssuer(ISSUER)
    .setAudience(STAR_STATE_AUD)
    .setIssuedAt()
    .setExpirationTime("10m")
    .sign(key());
}

export async function verifyStarState(
  state: string
): Promise<{ intent: string; userId: number; repos: string[] } | null> {
  try {
    const { payload } = await jwtVerify(state, key(), {
      issuer: ISSUER,
      audience: STAR_STATE_AUD,
      algorithms: ["HS256"],
    });
    const intent = String(payload.intent ?? "");
    const userId = Number(payload.uid ?? 0);
    const repos = Array.isArray(payload.repos) ? payload.repos.map(String) : [];
    if (!intent || !Number.isFinite(userId)) return null;
    return { intent, userId, repos };
  } catch {
    return null;
  }
}

// ---- explicit account-linking OAuth state ----
const LINK_STATE_AUD = "ollama-forge-link-state";

export async function signLinkState(linkId: string, userId: number, provider: string): Promise<string> {
  return await new SignJWT({ link: linkId, uid: userId, provider })
    .setProtectedHeader({ alg: "HS256", typ: "JWT" })
    .setIssuer(ISSUER)
    .setAudience(LINK_STATE_AUD)
    .setIssuedAt()
    .setExpirationTime("10m")
    .sign(key());
}

export async function verifyLinkState(
  state: string
): Promise<{ link: string; userId: number; provider: string } | null> {
  try {
    const { payload } = await jwtVerify(state, key(), {
      issuer: ISSUER,
      audience: LINK_STATE_AUD,
      algorithms: ["HS256"],
    });
    const link = String(payload.link ?? "");
    const userId = Number(payload.uid ?? 0);
    const provider = String(payload.provider ?? "");
    if (!link || !Number.isFinite(userId) || !provider) return null;
    return { link, userId, provider };
  } catch {
    return null;
  }
}

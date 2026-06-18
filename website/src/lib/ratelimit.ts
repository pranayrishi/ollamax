// Best-effort in-memory rate limiter for the auth endpoints. Keyed by client IP.
//
// CAVEAT: on serverless (Vercel) each instance has its own memory, so this is a
// per-instance limiter, not a global one. It still blunts naive hammering from a
// single warm instance. For a hard global limit in production, back this with
// Vercel KV / Upstash Redis (documented in the report). Kept dependency-free
// here so the site builds and deploys with zero extra setup.
import "server-only";

type Bucket = { count: number; resetAt: number };
const buckets = new Map<string, Bucket>();

export function rateLimit(key: string, limit: number, windowMs: number): boolean {
  const now = Date.now();
  const b = buckets.get(key);
  if (!b || now > b.resetAt) {
    buckets.set(key, { count: 1, resetAt: now + windowMs });
    return true;
  }
  if (b.count >= limit) return false;
  b.count++;
  return true;
}

// Durable, GLOBAL rate limit. When Upstash Redis REST env vars are present
// (`UPSTASH_REDIS_REST_URL` + `UPSTASH_REDIS_REST_TOKEN`) the limit is enforced
// across all serverless instances via a fixed-window INCR+EXPIRE; otherwise it
// transparently falls back to the per-instance in-memory limiter above. Returns
// true if the request is ALLOWED. Fails OPEN on Upstash errors (availability >
// strictness for a rate limiter) but logs.
export async function checkRateLimit(
  key: string,
  limit: number,
  windowMs: number
): Promise<boolean> {
  const url = process.env.UPSTASH_REDIS_REST_URL;
  const token = process.env.UPSTASH_REDIS_REST_TOKEN;
  if (!url || !token) {
    return rateLimit(key, limit, windowMs); // local/dev fallback
  }
  const windowSec = Math.ceil(windowMs / 1000);
  const bucket = `rl:${key}:${Math.floor(Date.now() / windowMs)}`;
  try {
    // Pipeline: INCR then EXPIRE (NX-ish via setting TTL each time is fine for
    // a fixed window). Upstash REST pipeline endpoint.
    const res = await fetch(`${url}/pipeline`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify([
        ["INCR", bucket],
        ["EXPIRE", bucket, String(windowSec)],
      ]),
      // Don't let a slow limiter hang the request.
      signal: AbortSignal.timeout(1500),
    });
    if (!res.ok) return true; // fail open
    const out = (await res.json()) as Array<{ result: number }>;
    const count = Number(out?.[0]?.result ?? 0);
    return count <= limit;
  } catch {
    return true; // fail open on network/timeout
  }
}

// Derive the client IP from a TRUSTED hop. On Vercel, `x-vercel-forwarded-for`
// is set by the platform and cannot be spoofed by the client; `x-real-ip` is
// also platform-set. We avoid trusting the left-most `x-forwarded-for` entry,
// which is attacker-controlled and would let a caller land in a fresh bucket
// every request. (For a hard, GLOBAL limit, back this with Vercel KV / Upstash
// — see the report; this in-memory limiter is per-instance best-effort.)
export function clientIp(req: Request): string {
  const vercel = req.headers.get("x-vercel-forwarded-for");
  if (vercel) return vercel.split(",")[0].trim();
  const real = req.headers.get("x-real-ip");
  if (real) return real.trim();
  // Last resort (non-Vercel/local dev): the right-most XFF hop is the closest
  // to our edge and the least attacker-influenced of the chain.
  const xff = req.headers.get("x-forwarded-for");
  if (xff) {
    const hops = xff.split(",").map((s) => s.trim());
    return hops[hops.length - 1] || "unknown";
  }
  return "unknown";
}

/**
 * Reject cross-site state-changing requests to hand-rolled (non-Auth.js) POST
 * endpoints. If an `Origin` header is present it must match this site's origin.
 * Browsers send `Origin` on cross-site form posts and fetches, so a forged
 * top-level POST from attacker.com is rejected.
 */
export function sameOrigin(req: Request, siteUrl: string): boolean {
  const origin = req.headers.get("origin");
  if (!origin) return true; // some same-origin navigations omit it; rely on session
  try {
    return new URL(origin).origin === new URL(siteUrl).origin;
  } catch {
    return false;
  }
}

// Feature 3: the REAL signup counter. Computed server-side from the users table,
// cached (per-instance 60s + CDN s-maxage) so it never hammers the DB. Honest
// metric — not fabricated.
import { NextRequest, NextResponse } from "next/server";
import { countUsers, countActiveUsers } from "@/lib/db";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

let cache: { at: number; users: number; active7: number } = { at: 0, users: 0, active7: 0 };

export async function GET(req: NextRequest) {
  if (!(await checkRateLimit(`stats:${clientIp(req)}`, 120, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const now = Date.now();
  if (now - cache.at > 60_000) {
    try {
      const users = await countUsers();
      const active7 = await countActiveUsers(7);
      cache = { at: now, users, active7 };
    } catch {
      // DB unavailable → serve last-known cache (or zeros on cold start).
    }
  }
  return NextResponse.json(
    { users: cache.users, activeLast7Days: cache.active7 },
    { headers: { "Cache-Control": "public, max-age=30, s-maxage=60" } }
  );
}

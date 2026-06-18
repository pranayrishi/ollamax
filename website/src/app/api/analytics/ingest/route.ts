// Feature 5: usage-metadata ingestion. The CONTENT FIREWALL lives here +
// src/lib/analytics.ts: every event is validated to be metadata-only; anything
// with an unexpected field or content-shaped string is REJECTED and never
// stored. Authenticated with the desktop app token. Honors the per-user
// telemetry opt-out server-side too (defense in depth — the app also gates it).
import { NextRequest, NextResponse } from "next/server";
import { verifyAccessToken } from "@/lib/tokens";
import { validateBatch, MAX_BATCH } from "@/lib/analytics";
import { insertUsageEvents, getUserById } from "@/lib/db";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  const authz = req.headers.get("authorization") || "";
  const claims = authz.startsWith("Bearer ") ? await verifyAccessToken(authz.slice(7)) : null;
  if (!claims) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  const userId = Number(claims.sub);
  if (!(await checkRateLimit(`analytics:${userId}`, 240, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  const events = Array.isArray(body) ? body : (body as { events?: unknown })?.events;
  const { events: clean, rejected, tooLarge } = validateBatch(events);
  if (tooLarge) {
    return NextResponse.json({ error: "batch_too_large", max: MAX_BATCH }, { status: 413 });
  }

  // Server-side opt-out check (the app also respects the toggle and sends
  // nothing when off — this is belt-and-suspenders).
  const user = await getUserById(userId);
  if (!user || user.telemetry_opt_out) {
    return NextResponse.json({ accepted: 0, rejected, optedOut: true });
  }

  if (clean.length > 0) {
    await insertUsageEvents(
      userId,
      clean.map((e) => ({
        type: e.type,
        provider: e.provider,
        model: e.model,
        tokensIn: e.tokensIn,
        tokensOut: e.tokensOut,
        language: e.language,
        accepted: e.accepted,
        ts: e.ts,
      }))
    );
  }
  return NextResponse.json({ accepted: clean.length, rejected });
}

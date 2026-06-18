// Device flow: pre-approval lookup. Given a typed user_code, return the pending
// request's context (when it started + the requesting app/browser) so the user
// can verify it's really their own device before approving. Anti-phishing:
// the user sees what they're authorizing instead of one-click approving.
import { NextRequest, NextResponse } from "next/server";
import { auth } from "@/auth";
import { getPendingDeviceByUserCode } from "@/lib/db";
import { checkRateLimit, clientIp, sameOrigin } from "@/lib/ratelimit";
import { env } from "@/lib/env";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  if (!sameOrigin(req, env.siteUrl())) {
    return NextResponse.json({ error: "bad_origin" }, { status: 403 });
  }
  if (!(await checkRateLimit(`device_info:${clientIp(req)}`, 60, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const session = await auth();
  if (!session?.user?.accountId) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  let body: { user_code?: string };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  const userCode = (body.user_code || "").toUpperCase().trim();
  if (!userCode) {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  const rec = await getPendingDeviceByUserCode(userCode);
  if (!rec || rec.consumed || new Date(rec.expires_at).getTime() < Date.now()) {
    return NextResponse.json({ found: false }, { status: 200 });
  }
  return NextResponse.json({
    found: true,
    alreadyApproved: rec.approved,
    createdAt: rec.created_at,
    userAgent: rec.user_agent,
  });
}

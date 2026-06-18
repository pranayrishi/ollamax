// Device flow, step 3 (user approves in the browser). Called by the
// /desktop/activate page form; the caller MUST have a GitHub web session.
import { NextRequest, NextResponse } from "next/server";
import { auth } from "@/auth";
import { approveDeviceCode } from "@/lib/db";
import { checkRateLimit, clientIp, sameOrigin } from "@/lib/ratelimit";
import { env } from "@/lib/env";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  // CSRF defense for this hand-rolled, cookie-authenticated endpoint (Auth.js's
  // own CSRF doesn't cover it): reject cross-site origins.
  if (!sameOrigin(req, env.siteUrl())) {
    return NextResponse.json({ error: "bad_origin" }, { status: 403 });
  }
  if (!(await checkRateLimit(`device_approve:${clientIp(req)}`, 30, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const session = await auth();
  const userId = session?.user?.accountId;
  if (!userId) {
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
  const ok = await approveDeviceCode(userCode, userId);
  if (!ok) {
    return NextResponse.json({ error: "invalid_or_expired_code" }, { status: 400 });
  }
  audit("device_approved", { userId });
  return NextResponse.json({ ok: true });
}

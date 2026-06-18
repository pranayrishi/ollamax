// Device flow, step 2 (the app polls here). OAuth-style responses:
//  - authorization_pending: user hasn't approved yet (keep polling)
//  - expired_token / invalid_grant: stop
//  - 200 with tokens: approved → issue and consume (single use)
import { NextRequest, NextResponse } from "next/server";
import { getDeviceCode, consumeDeviceCode, getUserById, publicUser } from "@/lib/db";
import { sha256 } from "@/lib/crypto";
import { signAccessToken, issueRefreshToken, ACCESS_EXPIRES_IN } from "@/lib/tokens";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  if (!(await checkRateLimit(`device_token:${clientIp(req)}`, 120, 60_000))) {
    return NextResponse.json({ error: "slow_down" }, { status: 429 });
  }
  let body: { device_code?: string };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  if (!body.device_code) {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }

  const hash = sha256(body.device_code);
  const rec = await getDeviceCode(hash);
  if (!rec) {
    return NextResponse.json({ error: "invalid_grant" }, { status: 400 });
  }
  if (new Date(rec.expires_at).getTime() < Date.now() || rec.consumed) {
    return NextResponse.json({ error: "expired_token" }, { status: 400 });
  }
  if (!rec.approved || !rec.user_id) {
    return NextResponse.json({ error: "authorization_pending" }, { status: 400 });
  }

  await consumeDeviceCode(hash);
  const user = await getUserById(rec.user_id);
  if (!user) {
    return NextResponse.json({ error: "no_account" }, { status: 400 });
  }
  const access_token = await signAccessToken({ userId: user.id, name: user.name });
  const refresh_token = await issueRefreshToken(user.id);
  return NextResponse.json({
    access_token,
    refresh_token,
    token_type: "Bearer",
    expires_in: ACCESS_EXPIRES_IN,
    user: await publicUser(user.id),
  });
}

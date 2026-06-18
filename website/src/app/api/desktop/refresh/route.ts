// Refresh a desktop access token. Implements refresh-token ROTATION: the
// presented refresh token is revoked and a new one issued, so a leaked refresh
// token has a limited useful life and reuse can be detected.
import { NextRequest, NextResponse } from "next/server";
import {
  findRefreshTokenAny,
  revokeRefreshToken,
  revokeRefreshFamily,
  getUserById,
  publicUser,
} from "@/lib/db";
import { sha256 } from "@/lib/crypto";
import { signAccessToken, issueRefreshToken, ACCESS_EXPIRES_IN } from "@/lib/tokens";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  if (!(await checkRateLimit(`desktop_refresh:${clientIp(req)}`, 120, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  let body: { refresh_token?: string };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  const { refresh_token } = body;
  if (!refresh_token) {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }

  const hash = sha256(refresh_token);
  // Look up the token regardless of state so we can distinguish "never existed"
  // from "exists but already rotated" (the reuse signal).
  const rec = await findRefreshTokenAny(hash);
  if (!rec) {
    return NextResponse.json({ error: "invalid_grant" }, { status: 401 });
  }

  // REUSE DETECTION: a token that's already revoked (rotated) or expired is
  // being replayed → assume compromise and revoke the entire rotation family.
  if (rec.revoked_at || new Date(rec.expires_at).getTime() < Date.now()) {
    await revokeRefreshFamily(rec.family_id);
    audit("desktop_token_reuse_detected", { userId: rec.user_id, familyId: rec.family_id });
    return NextResponse.json({ error: "token_reuse_detected" }, { status: 401 });
  }

  // Valid token: rotate within the same family.
  await revokeRefreshToken(hash);
  const user = await getUserById(rec.user_id);
  if (!user) {
    return NextResponse.json({ error: "no_account" }, { status: 400 });
  }
  const access_token = await signAccessToken({ userId: user.id, name: user.name });
  const new_refresh = await issueRefreshToken(user.id, rec.family_id);
  audit("desktop_token_refreshed", { userId: user.id, familyId: rec.family_id });

  return NextResponse.json({
    access_token,
    refresh_token: new_refresh,
    token_type: "Bearer",
    expires_in: ACCESS_EXPIRES_IN,
    user: await publicUser(user.id),
  });
}

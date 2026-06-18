// Desktop sign-in, step 2 (code → app tokens).
//
// The app exchanges the single-use code from its loopback callback, plus the
// PKCE `code_verifier`, for OUR tokens: a short-lived access JWT + an opaque
// refresh token. PKCE proves the caller is the same app that started the flow.
import { NextRequest, NextResponse } from "next/server";
import { consumeDesktopAuthCode, getUserById, publicUser } from "@/lib/db";
import { sha256, verifyPkceS256, safeEqual } from "@/lib/crypto";
import { signAccessToken, issueRefreshToken, ACCESS_EXPIRES_IN } from "@/lib/tokens";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  if (!(await checkRateLimit(`desktop_token:${clientIp(req)}`, 60, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }

  let body: { code?: string; code_verifier?: string; redirect_uri?: string };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  const { code, code_verifier, redirect_uri } = body;
  // redirect_uri is REQUIRED and always compared, so the code is bound to the
  // exact loopback URI it was issued for (standard Authorization Code semantics).
  if (!code || !code_verifier || !redirect_uri) {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }

  // Single-use: the code is deleted as it's read.
  const rec = await consumeDesktopAuthCode(sha256(code));
  if (!rec) {
    return NextResponse.json({ error: "invalid_grant" }, { status: 400 });
  }
  if (new Date(rec.expires_at).getTime() < Date.now()) {
    return NextResponse.json({ error: "expired_grant" }, { status: 400 });
  }
  if (!safeEqual(rec.redirect_uri, redirect_uri)) {
    return NextResponse.json({ error: "redirect_uri_mismatch" }, { status: 400 });
  }
  if (!verifyPkceS256(code_verifier, rec.code_challenge)) {
    return NextResponse.json({ error: "invalid_pkce" }, { status: 400 });
  }

  const user = await getUserById(rec.user_id);
  if (!user) {
    return NextResponse.json({ error: "no_account" }, { status: 400 });
  }

  const access_token = await signAccessToken({ userId: user.id, name: user.name });
  const refresh_token = await issueRefreshToken(user.id);
  audit("desktop_token_issued", { userId: user.id, flow: "loopback_pkce" });

  return NextResponse.json({
    access_token,
    refresh_token,
    token_type: "Bearer",
    expires_in: ACCESS_EXPIRES_IN,
    user: await publicUser(user.id),
  });
}

// Sign out / revoke. The app calls this on "Sign out" (and then clears its
// keychain entry). Revokes the presented refresh token; with `all: true` plus a
// valid bearer access token, revokes every refresh token for that identity.
import { NextRequest, NextResponse } from "next/server";
import { revokeRefreshToken, revokeAllRefreshTokens } from "@/lib/db";
import { sha256 } from "@/lib/crypto";
import { verifyAccessToken } from "@/lib/tokens";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  let body: { refresh_token?: string; all?: boolean };
  try {
    body = await req.json();
  } catch {
    body = {};
  }

  if (body.refresh_token) {
    await revokeRefreshToken(sha256(body.refresh_token));
  }

  if (body.all) {
    const auth = req.headers.get("authorization") || "";
    const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
    const claims = token ? await verifyAccessToken(token) : null;
    if (claims) {
      await revokeAllRefreshTokens(Number(claims.sub));
      audit("desktop_revoke", { userId: Number(claims.sub), scope: "all" });
    }
  }

  return NextResponse.json({ ok: true });
}

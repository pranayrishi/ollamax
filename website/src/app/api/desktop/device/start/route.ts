// Device-authorization fallback, step 1. For environments where a loopback
// listener is awkward, the app starts here, shows the user a short `user_code`,
// and sends them to `verification_uri` to approve in any browser.
import { NextRequest, NextResponse } from "next/server";
import { createDeviceCode } from "@/lib/db";
import { randomToken, sha256, userCode } from "@/lib/crypto";
import { env } from "@/lib/env";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function POST(req: NextRequest) {
  if (!(await checkRateLimit(`device_start:${clientIp(req)}`, 20, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const device_code = randomToken(32);
  const user_code = userCode();
  // Record the requesting app/browser so the approval page can show the user
  // *what* is asking for access (anti-phishing). Truncated to avoid bloat.
  const userAgent = (req.headers.get("user-agent") || "").slice(0, 200) || null;
  await createDeviceCode({
    deviceCodeHash: sha256(device_code),
    userCode: user_code,
    userAgent,
    expiresAt: new Date(Date.now() + 10 * 60 * 1000),
  });
  const verification_uri = `${env.siteUrl()}/desktop/activate`;
  // SECURITY: we deliberately DO NOT return a `verification_uri_complete` that
  // embeds the user_code. Per RFC 8628's security note, auto-filling the code
  // enables one-click phishing — the user must type the code they see in their
  // own app, which is what proves they initiated this flow.
  return NextResponse.json({
    device_code,
    user_code,
    verification_uri,
    interval: 5,
    expires_in: 600,
  });
}

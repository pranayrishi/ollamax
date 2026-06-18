// Explicit account linking, step 2: provider redirected back. Verify the signed
// state + single-use DB row (CSRF + binding), exchange the code, and attach the
// identity to the account — refusing if it's already linked elsewhere.
import { NextRequest, NextResponse } from "next/server";
import { consumeLinkState, linkIdentityToUser } from "@/lib/db";
import { verifyLinkState } from "@/lib/tokens";
import { exchangeForIdentity } from "@/lib/oauth";
import { env } from "@/lib/env";
import { audit } from "@/lib/audit";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET(req: NextRequest) {
  // Each call does an outbound provider token exchange; rate-limit by IP even
  // though the signed-state + single-use-row checks reject forgeries.
  if (!(await checkRateLimit(`link_callback:${clientIp(req)}`, 30, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const url = new URL(req.url);
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  if (!code || !state) {
    return NextResponse.redirect(`${env.siteUrl()}/account?error=oauth`);
  }
  const decoded = await verifyLinkState(state);
  if (!decoded) {
    return NextResponse.redirect(`${env.siteUrl()}/account?error=oauth`);
  }
  // Single-use DB row, bound to the same user + provider.
  const row = await consumeLinkState(decoded.link);
  if (!row || row.user_id !== decoded.userId || row.provider !== decoded.provider || new Date(row.expires_at).getTime() < Date.now()) {
    return NextResponse.redirect(`${env.siteUrl()}/account?error=oauth`);
  }

  const identity = await exchangeForIdentity(decoded.provider, code, `${env.siteUrl()}/api/link/callback`);
  if (!identity) {
    return NextResponse.redirect(`${env.siteUrl()}/account?error=oauth`);
  }
  const res = await linkIdentityToUser(decoded.userId, {
    provider: decoded.provider,
    providerAccountId: identity.providerAccountId,
    email: identity.email,
    emailVerified: identity.emailVerified,
    name: identity.name,
    avatarUrl: identity.avatarUrl,
    login: identity.login,
  });
  if (!res.ok) {
    audit("signin", { event: "link_conflict", provider: decoded.provider });
    return NextResponse.redirect(`${env.siteUrl()}/account?error=conflict`);
  }
  audit("signin", { event: "linked", provider: decoded.provider });
  return NextResponse.redirect(`${env.siteUrl()}/account?linked=${decoded.provider}`);
}

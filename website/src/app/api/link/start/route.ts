// Explicit account linking, step 1: an already-signed-in user links a 2nd
// provider. Session-authenticated; binds the link to the current account via a
// signed state + a single-use DB row.
import { NextRequest, NextResponse } from "next/server";
import { auth } from "@/auth";
import { createLinkState } from "@/lib/db";
import { providerAuthUrl } from "@/lib/oauth";
import { signLinkState } from "@/lib/tokens";
import { randomToken } from "@/lib/crypto";
import { env } from "@/lib/env";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET(req: NextRequest) {
  if (!(await checkRateLimit(`link_start:${clientIp(req)}`, 20, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const session = await auth();
  const userId = session?.user?.accountId;
  if (!userId) {
    return NextResponse.redirect(`${env.siteUrl()}/api/auth/signin`);
  }
  const provider = new URL(req.url).searchParams.get("provider") || "";
  if (provider !== "github" && provider !== "google") {
    return NextResponse.json({ error: "bad_provider" }, { status: 400 });
  }
  const linkId = randomToken(18);
  await createLinkState({
    id: linkId,
    userId,
    provider,
    expiresAt: new Date(Date.now() + 10 * 60 * 1000),
  });
  const state = await signLinkState(linkId, userId, provider);
  const dest = providerAuthUrl(provider, `${env.siteUrl()}/api/link/callback`, state);
  if (!dest) return NextResponse.json({ error: "bad_provider" }, { status: 400 });
  return NextResponse.redirect(dest);
}

// Opt-in starring, step 2: the user has reviewed the repos on /star/[id] and
// chosen a subset. THIS is the only place we request the elevated `public_repo`
// write scope — never at login. We persist the chosen subset, then send the
// user through GitHub's authorization for that scope.
import { NextRequest, NextResponse } from "next/server";
import { auth } from "@/auth";
import { getStarIntent } from "@/lib/db";
import { signStarState } from "@/lib/tokens";
import { env } from "@/lib/env";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET(req: NextRequest) {
  if (!(await checkRateLimit(`star_authorize:${clientIp(req)}`, 30, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const session = await auth();
  const userId = session?.user?.accountId;
  if (!userId) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  const url = new URL(req.url);
  const intentId = url.searchParams.get("intent") || "";
  const selected = (url.searchParams.get("repos") || "").split(",").filter(Boolean);

  const intent = await getStarIntent(intentId);
  if (!intent || intent.consumed || new Date(intent.expires_at).getTime() < Date.now()) {
    return NextResponse.json({ error: "invalid_or_expired_intent" }, { status: 400 });
  }
  // The intent must belong to the signed-in account.
  if (intent.user_id !== userId) {
    return NextResponse.json({ error: "forbidden" }, { status: 403 });
  }
  // Constrain the selection to repos from the ORIGINAL intent (never trust an
  // arbitrary list) and carry it in the signed state — no DB write on this GET.
  const allowed = new Set(intent.repos.map((r) => r.full_name));
  const chosen = selected.filter((s) => allowed.has(s)).slice(0, 100);
  if (chosen.length === 0) {
    return NextResponse.json({ error: "no_repos_selected" }, { status: 400 });
  }

  const state = await signStarState(intentId, userId, chosen);
  const gh = new URL("https://github.com/login/oauth/authorize");
  gh.searchParams.set("client_id", env.githubId());
  gh.searchParams.set("redirect_uri", `${env.siteUrl()}/api/star/callback`);
  gh.searchParams.set("scope", "public_repo"); // elevated, ONLY for starring
  gh.searchParams.set("state", state);
  gh.searchParams.set("allow_signup", "false");
  return NextResponse.redirect(gh);
}

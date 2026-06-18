// Opt-in starring, step 3: GitHub redirects back after the user authorized the
// elevated public_repo scope. We exchange the code for a token, star ONLY the
// repos in the (now narrowed) intent, then DISCARD the token — it is never
// persisted. This minimizes the write surface: a write-capable GitHub token
// exists only for the few seconds it takes to apply the user's chosen stars.
import { NextRequest, NextResponse } from "next/server";
import { verifyStarState } from "@/lib/tokens";
import { getStarIntent, consumeStarIntent, getIdentity, linkIdentityToUser } from "@/lib/db";
import { env } from "@/lib/env";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

type GhUser = { id?: number; login?: string; email?: string; name?: string; avatar_url?: string };

export async function GET(req: NextRequest) {
  const url = new URL(req.url);
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  if (!code || !state) {
    return NextResponse.redirect(`${env.siteUrl()}/star/error`);
  }
  const decoded = await verifyStarState(state);
  if (!decoded) {
    return NextResponse.redirect(`${env.siteUrl()}/star/error`);
  }
  const intent = await getStarIntent(decoded.intent);
  if (!intent || intent.consumed || intent.user_id !== decoded.userId) {
    return NextResponse.redirect(`${env.siteUrl()}/star/error`);
  }

  // Exchange the code for an elevated token (server-side, with the client
  // secret). This token is used immediately and never stored.
  let accessToken = "";
  try {
    const tokRes = await fetch("https://github.com/login/oauth/access_token", {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({
        client_id: env.githubId(),
        client_secret: env.githubSecret(),
        code,
        redirect_uri: `${env.siteUrl()}/api/star/callback`,
      }),
    });
    const tok = (await tokRes.json()) as { access_token?: string; scope?: string };
    if (!tok.access_token || !(tok.scope || "").includes("public_repo")) {
      // Elevation declined → consume the intent so it can't be re-driven.
      await consumeStarIntent(decoded.intent);
      return NextResponse.redirect(`${env.siteUrl()}/star/${decoded.intent}?done=1&ok=0&fail=0&err=scope`);
    }
    accessToken = tok.access_token;
  } catch {
    return NextResponse.redirect(`${env.siteUrl()}/star/error`);
  }

  // Identify the GitHub account that actually authorized, then LINK it to this
  // account (this is also how a Google-only user gains GitHub access for
  // starring — the gating). Refuse if that GitHub identity already belongs to a
  // DIFFERENT account (no identity theft / account merge by starring).
  let gh: GhUser | null = null;
  try {
    const uRes = await fetch("https://api.github.com/user", {
      headers: {
        Authorization: `Bearer ${accessToken}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "ollama-forge-hub",
      },
    });
    gh = (await uRes.json()) as GhUser;
  } catch {
    gh = null;
  }
  const ghId = gh && Number(gh.id);
  if (!ghId) {
    accessToken = "";
    await consumeStarIntent(decoded.intent);
    return NextResponse.redirect(`${env.siteUrl()}/star/${decoded.intent}?done=1&ok=0&fail=0&err=mismatch`);
  }
  const existing = await getIdentity("github", String(ghId));
  if (existing && existing.user_id !== decoded.userId) {
    accessToken = "";
    await consumeStarIntent(decoded.intent);
    return NextResponse.redirect(
      `${env.siteUrl()}/star/${decoded.intent}?done=1&ok=0&fail=0&err=linked_elsewhere`
    );
  }
  // Link the GitHub identity for gating, but store its email as UNVERIFIED:
  // the star scope (public_repo) can't read /user/emails, and the public
  // profile email is user-settable — marking it verified here would poison the
  // verified-email auto-link table. Gating only needs the providerAccountId.
  await linkIdentityToUser(decoded.userId, {
    provider: "github",
    providerAccountId: String(ghId),
    email: gh?.email ?? null,
    emailVerified: false,
    name: gh?.name ?? null,
    avatarUrl: gh?.avatar_url ?? null,
    login: gh?.login ?? null,
  });
  audit("star_scope_authorized", { userId: decoded.userId, githubId: ghId, count: decoded.repos.length });

  // Star only the repos the user selected (signed into the state) AND that were
  // in the original intent — never an arbitrary list.
  const allowed = new Set(intent.repos.map((r) => r.full_name));
  const toStar = decoded.repos.filter((f) => allowed.has(f));
  let ok = 0;
  let fail = 0;
  for (const full of toStar) {
    const [owner, repo] = full.split("/");
    try {
      const res = await fetch(
        `https://api.github.com/user/starred/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`,
        {
          method: "PUT",
          headers: {
            Authorization: `Bearer ${accessToken}`,
            Accept: "application/vnd.github+json",
            "Content-Length": "0",
            "User-Agent": "ollama-forge-hub",
          },
        }
      );
      if (res.status === 204 || res.status === 304) ok++;
      else fail++;
    } catch {
      fail++;
    }
  }
  // The token goes out of scope here and is never written anywhere.
  accessToken = "";
  await consumeStarIntent(decoded.intent);
  audit("stars_applied", { userId: decoded.userId, ok, fail });

  return NextResponse.redirect(`${env.siteUrl()}/star/${decoded.intent}?done=1&ok=${ok}&fail=${fail}`);
}

// Opt-in starring, step 1: the desktop Hub creates a "star intent" listing the
// exact repos a user might choose to star. Authenticated with the app bearer
// token (the user's identity). Returns a URL to open in the browser, where the
// user reviews the list and consciously stars all or a subset. NEVER automatic.
import { NextRequest, NextResponse } from "next/server";
import { verifyAccessToken } from "@/lib/tokens";
import { createStarIntent } from "@/lib/db";
import { randomToken } from "@/lib/crypto";
import { env } from "@/lib/env";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";
import { audit } from "@/lib/audit";

export const runtime = "nodejs";

type RepoRef = { full_name: string; html_url: string; license_spdx: string | null };

export async function POST(req: NextRequest) {
  if (!(await checkRateLimit(`star_intent:${clientIp(req)}`, 30, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const authz = req.headers.get("authorization") || "";
  const claims = authz.startsWith("Bearer ") ? await verifyAccessToken(authz.slice(7)) : null;
  if (!claims) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  let body: { repos?: RepoRef[]; category?: string };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ error: "invalid_request" }, { status: 400 });
  }
  // Validate full_name strictly (reject dot-only / `..` segments) and DERIVE
  // html_url server-side — never store a client-supplied URL (that prevents a
  // bogus `javascript:` href from ever landing in the DB or the /star page).
  // license_spdx is accepted only if it matches the SPDX charset, else null.
  const FULL = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
  const SPDX = /^[A-Za-z0-9.+-]{1,40}$/;
  const safeSegments = (fn: string) =>
    !fn.split("/").some((s) => s === "." || s === ".." || s.includes(".."));
  const repos = (body.repos || [])
    .filter((r) => r && typeof r.full_name === "string" && FULL.test(r.full_name) && safeSegments(r.full_name))
    .slice(0, 100)
    .map((r) => ({
      full_name: r.full_name,
      html_url: `https://github.com/${r.full_name}`,
      license_spdx:
        typeof r.license_spdx === "string" && SPDX.test(r.license_spdx) ? r.license_spdx : null,
    }));
  if (repos.length === 0) {
    return NextResponse.json({ error: "no_repos" }, { status: 400 });
  }
  const id = randomToken(18);
  await createStarIntent({
    id,
    userId: Number(claims.sub),
    repos,
    category: body.category ?? null,
    expiresAt: new Date(Date.now() + 30 * 60 * 1000),
  });
  audit("star_intent_created", { userId: Number(claims.sub), count: repos.length, category: body.category });
  return NextResponse.json({ id, url: `${env.siteUrl()}/star/${id}` });
}

// Hub catalog: the compiled package for one category (rules + skills +
// curated references). This is what the desktop Hub applies on "+". Public,
// cached, no identity/content.
import { NextRequest, NextResponse } from "next/server";
import { compilePackage } from "@/lib/hub/compile";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET(req: NextRequest, ctx: { params: Promise<{ slug: string }> }) {
  if (!(await checkRateLimit(`hub_package:${clientIp(req)}`, 120, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const { slug } = await ctx.params;
  let pkg;
  try {
    pkg = await compilePackage(slug);
  } catch {
    return NextResponse.json({ error: "catalog_unavailable" }, { status: 503 });
  }
  if (!pkg) {
    return NextResponse.json({ error: "unknown_category" }, { status: 404 });
  }
  return NextResponse.json(pkg, {
    headers: { "Cache-Control": "public, max-age=300, s-maxage=900" },
  });
}

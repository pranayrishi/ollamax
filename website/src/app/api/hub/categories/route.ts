// Hub catalog: list categories (data-driven from the taxonomy) + how many
// curated repos each has. Public, cached metadata only — no identity/content.
import { NextRequest, NextResponse } from "next/server";
import { CATEGORIES } from "@/lib/hub/taxonomy";
import { getCategoryCounts } from "@/lib/db";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET(req: NextRequest) {
  if (!(await checkRateLimit(`hub_categories:${clientIp(req)}`, 120, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  let counts: Record<string, number> = {};
  try {
    counts = await getCategoryCounts();
  } catch {
    // DB not provisioned yet → still return the taxonomy with 0 counts.
  }
  const categories = CATEGORIES.map((c) => ({
    slug: c.slug,
    name: c.name,
    description: c.description,
    topics: c.githubTopics.slice(0, 6),
    repoCount: counts[c.slug] ?? 0,
  }));
  return NextResponse.json(
    { categories },
    { headers: { "Cache-Control": "public, max-age=300, s-maxage=900" } }
  );
}

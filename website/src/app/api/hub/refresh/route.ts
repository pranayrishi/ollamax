// Scheduled catalog refresh (cron-protected). Runs the GitHub ingestion so
// packages "constantly update as popular repos arise." Protect with CRON_SECRET
// and point a Vercel Cron at it (e.g. daily). Clients never trigger ingestion.
import { NextRequest, NextResponse } from "next/server";
import { ingestAll } from "@/lib/hub/ingest";
import { audit } from "@/lib/audit";
import { safeEqual } from "@/lib/crypto";

export const runtime = "nodejs";
export const maxDuration = 300; // ingestion is paced; allow time

export async function POST(req: NextRequest) {
  const secret = process.env.CRON_SECRET;
  const authz = req.headers.get("authorization") || "";
  const presented = authz.startsWith("Bearer ") ? authz.slice(7) : "";
  // Constant-time compare to avoid leaking the secret via response timing.
  if (!secret || !safeEqual(presented, secret)) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  // Optional ?limit=N to ingest only the first N categories per invocation
  // (keeps a single run inside the function time + GitHub rate budget).
  const limit = Number(new URL(req.url).searchParams.get("limit") || "0") || undefined;
  const results = await ingestAll(limit);
  const totals = results.reduce(
    (a, r) => ({ found: a.found + r.found, included: a.included + r.included }),
    { found: 0, included: 0 }
  );
  audit("hub_refresh", { categories: results.length, ...totals });
  return NextResponse.json({ ok: true, ...totals, results });
}

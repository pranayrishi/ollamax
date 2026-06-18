import { NextResponse } from "next/server";

export const runtime = "nodejs";

// Liveness only. Deliberately does not touch the DB or reveal config.
export async function GET() {
  return NextResponse.json({ ok: true, service: "ollama-forge-accounts" });
}

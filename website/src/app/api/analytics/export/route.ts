// Feature 5: user data export. The signed-in user downloads their OWN raw usage
// metadata (per-user isolated — never another user's). Session-authenticated.
import { NextResponse } from "next/server";
import { auth } from "@/auth";
import { getUserRawEvents } from "@/lib/db";
import { checkRateLimit } from "@/lib/ratelimit";

export const runtime = "nodejs";

export async function GET() {
  const session = await auth();
  const userId = session?.user?.accountId;
  if (!userId) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  // Export returns a large per-user payload; cap how often a session can pull it.
  if (!(await checkRateLimit(`analytics_export:${userId}`, 6, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }
  const events = await getUserRawEvents(userId);
  const payload = JSON.stringify(
    { exported_at: new Date().toISOString(), user_id: userId, note: "metadata only — no content", events },
    null,
    2
  );
  return new NextResponse(payload, {
    headers: {
      "Content-Type": "application/json",
      "Content-Disposition": 'attachment; filename="ollama-forge-usage.json"',
      "Cache-Control": "no-store",
    },
  });
}

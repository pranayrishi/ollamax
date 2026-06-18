// Who-am-I for the desktop app. Bearer access token → fresh user profile.
// This is the only "account" data the app reads; there is no prompt/code data
// here because the backend never receives any.
import { NextRequest, NextResponse } from "next/server";
import { verifyAccessToken } from "@/lib/tokens";
import { publicUser } from "@/lib/db";

export const runtime = "nodejs";

export async function GET(req: NextRequest) {
  const authz = req.headers.get("authorization") || "";
  const token = authz.startsWith("Bearer ") ? authz.slice(7) : "";
  const claims = token ? await verifyAccessToken(token) : null;
  if (!claims) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  const user = await publicUser(Number(claims.sub));
  if (!user) {
    return NextResponse.json({ error: "no_account" }, { status: 404 });
  }
  return NextResponse.json({ user });
}

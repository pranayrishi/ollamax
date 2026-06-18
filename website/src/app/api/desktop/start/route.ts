// Desktop sign-in, step 1 (loopback + PKCE).
//
// The desktop app opens the system browser here with a PKCE `code_challenge`
// and a loopback `redirect_uri`. We authenticate the user via GitHub (reusing
// any existing web session), then hand a SHORT-LIVED, SINGLE-USE code back to
// the app's loopback listener. The app never sees the GitHub secret/token.
import { NextRequest, NextResponse } from "next/server";
import { auth } from "@/auth";
import { createDesktopAuthCode } from "@/lib/db";
import { randomToken, sha256 } from "@/lib/crypto";
import { checkRateLimit, clientIp } from "@/lib/ratelimit";

export const runtime = "nodejs";

// Only loopback http URLs are allowed as the redirect target. This is the
// critical guard: it stops an attacker from pointing `redirect_uri` at their
// own server to steal the authorization code. We pin IP LITERALS only — not
// "localhost" — because the OS resolver could map localhost elsewhere in
// adversarial setups, whereas 127.0.0.1 / ::1 are unambiguous loopback.
// `new URL().hostname` also defeats userinfo tricks (http://127.0.0.1@evil.com
// parses hostname = evil.com → rejected).
function isLoopbackRedirect(uri: string): boolean {
  try {
    const u = new URL(uri);
    return u.protocol === "http:" && (u.hostname === "127.0.0.1" || u.hostname === "[::1]");
  } catch {
    return false;
  }
}

// A PKCE S256 challenge is base64url(sha256(...)) = exactly 43 chars. Reject
// anything else as defense-in-depth against malformed/low-entropy challenges.
const S256_CHALLENGE = /^[A-Za-z0-9_-]{43}$/;

export async function GET(req: NextRequest) {
  const url = new URL(req.url);
  const codeChallenge = url.searchParams.get("code_challenge");
  const method = url.searchParams.get("code_challenge_method");
  const redirectUri = url.searchParams.get("redirect_uri");
  const state = url.searchParams.get("state") ?? "";

  if (!codeChallenge || method !== "S256" || !redirectUri) {
    return NextResponse.json(
      { error: "invalid_request", detail: "code_challenge (S256) and redirect_uri are required" },
      { status: 400 }
    );
  }
  if (!S256_CHALLENGE.test(codeChallenge)) {
    return NextResponse.json(
      { error: "invalid_request", detail: "code_challenge must be a 43-char base64url S256 value" },
      { status: 400 }
    );
  }
  if (!isLoopbackRedirect(redirectUri)) {
    return NextResponse.json(
      { error: "invalid_redirect_uri", detail: "redirect_uri must be a loopback http URL" },
      { status: 400 }
    );
  }

  // Authenticate via GitHub. If there's no web session yet, bounce through the
  // Auth.js sign-in page and come straight back here afterward.
  const session = await auth();
  const userId = session?.user?.accountId;
  if (!userId) {
    const signin = new URL("/api/auth/signin", url.origin);
    signin.searchParams.set("callbackUrl", url.pathname + url.search);
    return NextResponse.redirect(signin);
  }

  if (!(await checkRateLimit(`desktop_start:${clientIp(req)}`, 30, 60_000))) {
    return NextResponse.json({ error: "rate_limited" }, { status: 429 });
  }

  // Mint a single-use code bound to this PKCE challenge + redirect.
  const code = randomToken(32);
  await createDesktopAuthCode({
    codeHash: sha256(code),
    userId,
    codeChallenge,
    redirectUri,
    expiresAt: new Date(Date.now() + 5 * 60 * 1000),
  });

  const dest = new URL(redirectUri);
  dest.searchParams.set("code", code);
  if (state) dest.searchParams.set("state", state);
  return NextResponse.redirect(dest);
}

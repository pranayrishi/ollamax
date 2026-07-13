import { NextRequest, NextResponse } from "next/server";

// Nonce-based Content-Security-Policy. A per-request nonce replaces the blanket
// `'unsafe-inline'` for scripts, so injected inline <script> won't execute.
// Next.js reads the nonce from this CSP header and stamps it onto its own
// framework/bootstrap scripts automatically.
export function proxy(req: NextRequest) {
  const nonce = btoa(crypto.randomUUID());
  const csp = [
    "default-src 'self'",
    `script-src 'self' 'nonce-${nonce}' 'strict-dynamic'`,
    "style-src 'self' 'unsafe-inline'",
    // Avatar hosts: GitHub + Google (Google serves from *.googleusercontent.com).
    "img-src 'self' https://avatars.githubusercontent.com https://*.googleusercontent.com data:",
    "media-src 'self' https://d8j0ntlcm91z4.cloudfront.net",
    "connect-src 'self' https://github.com https://api.github.com",
    "font-src 'self'",
    "frame-ancestors 'none'",
    "base-uri 'self'",
    // form-action MUST list every OAuth provider's authorize host: browsers apply
    // this directive to the redirect target of a form submission, so the sign-in
    // POST→302 to the provider is blocked if its host is missing. Both providers
    // here, or Google sign-in fails even when the env vars are correct.
    "form-action 'self' https://github.com https://accounts.google.com",
    "object-src 'none'",
  ].join("; ");

  const requestHeaders = new Headers(req.headers);
  requestHeaders.set("x-nonce", nonce);
  requestHeaders.set("content-security-policy", csp);

  const res = NextResponse.next({ request: { headers: requestHeaders } });
  res.headers.set("content-security-policy", csp);
  return res;
}

export const config = {
  // Apply to pages; skip Next internals and static assets.
  matcher: [
    {
      source: "/((?!_next/static|_next/image|favicon.ico).*)",
      missing: [
        { type: "header", key: "next-router-prefetch" },
        { type: "header", key: "purpose", value: "prefetch" },
      ],
    },
  ],
};

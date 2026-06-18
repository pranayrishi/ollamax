import { dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));

/** @type {import('next').NextConfig} */

// Static security headers applied to every response. The Content-Security-Policy
// is NOT here — it's set per-request (with a fresh nonce) in `src/middleware.ts`
// so inline scripts can't execute. These are the static, request-independent
// headers. `frame-ancestors 'none'` is enforced by the middleware CSP;
// X-Frame-Options here covers older agents.
const securityHeaders = [
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "X-Frame-Options", value: "DENY" },
  { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  {
    key: "Strict-Transport-Security",
    value: "max-age=63072000; includeSubDomains; preload",
  },
  { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
];

const nextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  // Pin the tracing root to this directory (a parent lockfile would otherwise
  // be auto-selected). On Vercel, set the project root to `website/`.
  outputFileTracingRoot: __dirname,
  async headers() {
    return [{ source: "/:path*", headers: securityHeaders }];
  },
};

export default nextConfig;

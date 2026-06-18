// Centralized, server-only environment access. Importing this from a client
// component would leak nothing secret (we only read process.env on the server),
// but to be safe every consumer here is itself server-only (route handlers /
// server components). `NEXT_PUBLIC_*` values are the only ones safe for the
// browser and are read directly where needed.

function required(name: string): string {
  const v = process.env[name];
  if (!v) {
    throw new Error(
      `Missing required env var ${name}. See website/.env.example and set it in .env.local (dev) or Vercel (prod).`
    );
  }
  return v;
}

function optional(name: string): string | undefined {
  return process.env[name] || undefined;
}

// A signing secret must be present AND long enough to resist offline brute force
// of the HMAC. 32+ chars ≈ the `openssl rand -base64 32` we document.
function requiredSecret(name: string, minLen = 32): string {
  const v = required(name);
  if (v.length < minLen) {
    throw new Error(
      `${name} is too short (${v.length} chars). Use at least ${minLen} chars of entropy, e.g. \`openssl rand -base64 32\`.`
    );
  }
  return v;
}

export const env = {
  // Public site URL (used to build absolute callback/verification URLs).
  siteUrl: () =>
    process.env.NEXT_PUBLIC_SITE_URL || process.env.AUTH_URL || "http://localhost:3000",

  authSecret: () => required("AUTH_SECRET"),
  githubId: () => required("AUTH_GITHUB_ID"),
  githubSecret: () => required("AUTH_GITHUB_SECRET"),
  googleId: () => required("AUTH_GOOGLE_ID"),
  googleSecret: () => required("AUTH_GOOGLE_SECRET"),
  appJwtSecret: () => requiredSecret("APP_JWT_SECRET"),
  databaseUrl: () => required("DATABASE_URL"),

  downloads: {
    macos: optional("NEXT_PUBLIC_DOWNLOAD_MACOS"),
    windows: optional("NEXT_PUBLIC_DOWNLOAD_WINDOWS"),
    linux: optional("NEXT_PUBLIC_DOWNLOAD_LINUX"),
  },
};

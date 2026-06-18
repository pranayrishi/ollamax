// Minimal provider OAuth helpers for the EXPLICIT account-linking flow (linking
// a 2nd provider to an already-signed-in account). Auth.js handles primary
// sign-in; this is the hand-rolled link path. Client secrets stay server-only.
import "server-only";
import { env } from "./env";

export type LinkedIdentity = {
  providerAccountId: string;
  email: string | null;
  emailVerified: boolean;
  name: string | null;
  avatarUrl: string | null;
  login: string | null;
};

// Fetch the GitHub account's PRIMARY email and whether GitHub has VERIFIED it.
// SECURITY: account-linking-by-email must only ever trust a provider-asserted
// verified address — never the public profile email (which a user can set to an
// unverified value). The `verified` flag is the only thing standing between
// "link to the matching account" and account hijack, so we read it explicitly
// from /user/emails (needs the user:email scope) rather than inferring it.
export async function githubPrimaryVerifiedEmail(
  accessToken: string
): Promise<{ email: string | null; verified: boolean }> {
  try {
    const list = (await fetch("https://api.github.com/user/emails", {
      headers: {
        Authorization: `Bearer ${accessToken}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "ollama-forge",
      },
    }).then((r) => (r.ok ? r.json() : null))) as
      | Array<{ email: string; primary: boolean; verified: boolean }>
      | null;
    if (!Array.isArray(list)) return { email: null, verified: false };
    const primary = list.find((e) => e && e.primary) ?? list[0];
    if (primary && primary.verified) return { email: primary.email, verified: true };
    return { email: primary ? primary.email : null, verified: false };
  } catch {
    return { email: null, verified: false };
  }
}

export function providerAuthUrl(provider: string, redirectUri: string, state: string): string | null {
  if (provider === "github") {
    const u = new URL("https://github.com/login/oauth/authorize");
    u.searchParams.set("client_id", env.githubId());
    u.searchParams.set("redirect_uri", redirectUri);
    u.searchParams.set("scope", "read:user user:email");
    u.searchParams.set("state", state);
    u.searchParams.set("allow_signup", "false");
    return u.toString();
  }
  if (provider === "google") {
    const u = new URL("https://accounts.google.com/o/oauth2/v2/auth");
    u.searchParams.set("client_id", env.googleId());
    u.searchParams.set("redirect_uri", redirectUri);
    u.searchParams.set("response_type", "code");
    u.searchParams.set("scope", "openid email profile");
    u.searchParams.set("state", state);
    return u.toString();
  }
  return null;
}

export async function exchangeForIdentity(
  provider: string,
  code: string,
  redirectUri: string
): Promise<LinkedIdentity | null> {
  try {
    if (provider === "github") {
      const tok = (await fetch("https://github.com/login/oauth/access_token", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          client_id: env.githubId(),
          client_secret: env.githubSecret(),
          code,
          redirect_uri: redirectUri,
        }),
      }).then((r) => r.json())) as { access_token?: string };
      if (!tok.access_token) return null;
      const u = (await fetch("https://api.github.com/user", {
        headers: {
          Authorization: `Bearer ${tok.access_token}`,
          Accept: "application/vnd.github+json",
          "User-Agent": "ollama-forge",
        },
      }).then((r) => r.json())) as Record<string, unknown>;
      if (!u.id) return null;
      // Use the GitHub-asserted VERIFIED email — not the public profile email —
      // so this linked identity only becomes an auto-link target if the address
      // is genuinely verified.
      const ve = await githubPrimaryVerifiedEmail(tok.access_token);
      return {
        providerAccountId: String(u.id),
        email: ve.email ?? (u.email as string) ?? null,
        emailVerified: ve.verified,
        name: (u.name as string) ?? null,
        avatarUrl: (u.avatar_url as string) ?? null,
        login: (u.login as string) ?? null,
      };
    }
    if (provider === "google") {
      const tok = (await fetch("https://oauth2.googleapis.com/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          client_id: env.googleId(),
          client_secret: env.googleSecret(),
          code,
          redirect_uri: redirectUri,
          grant_type: "authorization_code",
        }),
      }).then((r) => r.json())) as { access_token?: string };
      if (!tok.access_token) return null;
      const u = (await fetch("https://openidconnect.googleapis.com/v1/userinfo", {
        headers: { Authorization: `Bearer ${tok.access_token}` },
      }).then((r) => r.json())) as Record<string, unknown>;
      if (!u.sub) return null;
      return {
        providerAccountId: String(u.sub),
        email: (u.email as string) ?? null,
        emailVerified: Boolean(u.email_verified),
        name: (u.name as string) ?? null,
        avatarUrl: (u.picture as string) ?? null,
        login: (u.given_name as string) ?? null,
      };
    }
  } catch {
    return null;
  }
  return null;
}

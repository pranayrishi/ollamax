// Auth.js (NextAuth v5) — GitHub AND Google identity providers (no password).
//
// Multi-identity model (Round 6): one internal account (`users.id`) can have a
// GitHub identity and/or a Google identity linked. On sign-in we find-or-create-
// or-link by VERIFIED email (`resolveUserForIdentity`). The session carries the
// internal user id + which providers are linked; GitHub-specific actions
// (starring) are gated on a linked GitHub identity.
//
// Provider client secrets are read from env (AUTH_GITHUB_*/AUTH_GOOGLE_*),
// server-only — never in any client bundle.
import NextAuth from "next-auth";
import GitHub from "next-auth/providers/github";
// FUTURE: Google sign-in is intentionally disabled for now (kept out of sight so
// users don't pick it + hit errors). To re-enable: uncomment this import + the
// Google provider below, and set AUTH_GOOGLE_ID / AUTH_GOOGLE_SECRET in Vercel.
// import Google from "next-auth/providers/google";
import { resolveUserForIdentity, getLinkedProviders } from "@/lib/db";
import { githubPrimaryVerifiedEmail } from "@/lib/oauth";
import { audit } from "@/lib/audit";

export const { handlers, auth, signIn, signOut } = NextAuth({
  providers: [
    GitHub({
      // Read-only identity scopes (the star write scope is requested separately,
      // only on opt-in — see Round 5).
      authorization: { params: { scope: "read:user user:email" } },
    }),
    // FUTURE: Google provider disabled for now (see import note above). Restore:
    // Google({ authorization: { params: { scope: "openid email profile" } } }),
  ],
  session: { strategy: "jwt", maxAge: 30 * 24 * 60 * 60 },
  callbacks: {
    // SECURITY — load-bearing: copy ONLY identity fields into the token. Never
    // store a raw provider access token (it would leak into the session cookie).
    async jwt({ token, profile, account }) {
      if (account && profile) {
        const provider = account.provider; // 'github' | 'google'
        const providerAccountId = String(account.providerAccountId ?? profile.sub ?? profile.id ?? "");
        let email = (profile.email as string | undefined) ?? null;
        // SECURITY (account-hijack prevention): only ever treat an email as
        // verified on a provider-asserted signal. Google asserts email_verified
        // in the ID token. For GitHub we must NOT infer verification from the
        // presence of a (user-settable) profile email — we read the real
        // `verified` flag from /user/emails using this sign-in's access token.
        // An unverified GitHub email is kept but flagged unverified, so it can
        // never auto-link into someone else's account.
        let emailVerified = false;
        if (provider === "google") {
          emailVerified = Boolean((profile as Record<string, unknown>).email_verified);
        } else if (provider === "github") {
          const ghToken = (account.access_token as string | undefined) ?? "";
          if (ghToken) {
            const ve = await githubPrimaryVerifiedEmail(ghToken);
            email = ve.email ?? email;
            emailVerified = ve.verified;
          }
        }
        const name = (profile.name as string | undefined) ?? null;
        const avatarUrl =
          ((profile as Record<string, unknown>).avatar_url as string | undefined) ??
          ((profile as Record<string, unknown>).picture as string | undefined) ??
          null;
        const login =
          ((profile as Record<string, unknown>).login as string | undefined) ??
          ((profile as Record<string, unknown>).given_name as string | undefined) ??
          null;

        if (providerAccountId) {
          const { userId } = await resolveUserForIdentity({
            provider,
            providerAccountId,
            email,
            emailVerified,
            name,
            avatarUrl,
            login,
          });
          token.uid = userId;
          token.name = name;
          token.email = email;
          token.avatar = avatarUrl;
          token.providers = await getLinkedProviders(userId);
        }
      }
      return token;
    },
    async session({ session, token }) {
      if (session.user) {
        session.user.accountId = Number(token.uid ?? 0);
        session.user.providers = Array.isArray(token.providers) ? (token.providers as string[]) : [];
        session.user.login = (token.login as string | undefined) ?? null;
        if (token.avatar) session.user.image = token.avatar as string;
      }
      return session;
    },
  },
  events: {
    async signIn({ account, profile }) {
      audit("signin", { provider: account?.provider, email: profile?.email });
    },
    async signOut() {
      audit("signout");
    },
  },
  trustHost: true,
});

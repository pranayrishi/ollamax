// Module augmentation: the fields we attach to the session and JWT.
// Keyed on the internal account id (`users.id`); `providers` lists linked
// identity providers (github/google).
import type { DefaultSession } from "next-auth";

declare module "next-auth" {
  interface Session {
    user: {
      // `accountId` is our internal numeric users.id. (Auth.js already defines
      // `user.id` as string, so we use a distinct field to avoid a type clash.)
      accountId?: number;
      providers?: string[];
      login?: string | null;
    } & DefaultSession["user"];
  }
}

declare module "next-auth/jwt" {
  interface JWT {
    uid?: number;
    providers?: string[];
    login?: string | null;
    avatar?: string | null;
  }
}

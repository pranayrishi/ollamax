// Small crypto helpers used by the desktop auth flows. Node runtime only.
import "server-only";
import { createHash, randomBytes, timingSafeEqual } from "crypto";

/** URL-safe base64 (no padding). */
export function base64url(buf: Buffer): string {
  return buf.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** SHA-256 → base64url. Used to hash codes/tokens at rest and for PKCE S256. */
export function sha256(input: string): string {
  return base64url(createHash("sha256").update(input).digest());
}

/** Cryptographically-random URL-safe token of `bytes` entropy. */
export function randomToken(bytes = 32): string {
  return base64url(randomBytes(bytes));
}

/** Constant-time string compare to avoid leaking via timing. */
export function safeEqual(a: string, b: string): boolean {
  const ba = Buffer.from(a);
  const bb = Buffer.from(b);
  if (ba.length !== bb.length) return false;
  return timingSafeEqual(ba, bb);
}

/**
 * Verify a PKCE S256 challenge: the challenge the app sent at /start must equal
 * base64url(sha256(code_verifier)) the app presents at /token. Constant-time.
 */
export function verifyPkceS256(codeVerifier: string, codeChallenge: string): boolean {
  return safeEqual(sha256(codeVerifier), codeChallenge);
}

/**
 * Human-friendly device user-code, e.g. "WDJB-MJHT". Avoids ambiguous
 * characters (no 0/O/1/I) so it's easy to read aloud / type.
 */
export function userCode(): string {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const pick = (n: number) => {
    const bytes = randomBytes(n);
    let out = "";
    for (let i = 0; i < n; i++) out += alphabet[bytes[i] % alphabet.length];
    return out;
  };
  return `${pick(4)}-${pick(4)}`;
}

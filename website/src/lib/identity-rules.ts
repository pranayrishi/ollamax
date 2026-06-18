// Pure account-resolution rule (unit-tested). Keeping the decision separate from
// the DB lets us test the security-critical linking logic directly.
//
//  - If this provider identity already exists → it's that account.
//  - Else if the incoming email is VERIFIED and matches an existing account's
//    VERIFIED identity → LINK (account linking by verified email).
//  - Else → CREATE a new account.
//
// NEVER link on an unverified email — that would let an attacker hijack an
// account by signing up a provider with someone else's address.
export type Resolution = "existing" | "link" | "create";

export function resolutionDecision(p: {
  hasExistingIdentity: boolean;
  emailVerified: boolean;
  hasVerifiedEmailMatch: boolean;
}): Resolution {
  if (p.hasExistingIdentity) return "existing";
  if (p.emailVerified && p.hasVerifiedEmailMatch) return "link";
  return "create";
}

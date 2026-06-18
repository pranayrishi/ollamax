import { describe, it, expect } from "vitest";
import { resolutionDecision } from "./identity-rules";

describe("account resolution / merge rules", () => {
  it("returns existing when the provider identity already exists", () => {
    expect(
      resolutionDecision({ hasExistingIdentity: true, emailVerified: true, hasVerifiedEmailMatch: true })
    ).toBe("existing");
  });

  it("links by VERIFIED email match", () => {
    expect(
      resolutionDecision({ hasExistingIdentity: false, emailVerified: true, hasVerifiedEmailMatch: true })
    ).toBe("link");
  });

  it("NEVER links on an unverified email (hijack prevention) — creates instead", () => {
    expect(
      resolutionDecision({ hasExistingIdentity: false, emailVerified: false, hasVerifiedEmailMatch: true })
    ).toBe("create");
  });

  it("creates a new account when there's no match", () => {
    expect(
      resolutionDecision({ hasExistingIdentity: false, emailVerified: true, hasVerifiedEmailMatch: false })
    ).toBe("create");
  });
});

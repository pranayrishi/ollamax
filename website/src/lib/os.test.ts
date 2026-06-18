import { describe, it, expect } from "vitest";
import { detectOS } from "./os";

describe("OS / architecture detection", () => {
  it("detects Windows x64 from UA", () => {
    const r = detectOS("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120");
    expect(r.os).toBe("windows");
    expect(r.arch).toBe("x64");
  });

  it("detects Linux", () => {
    const r = detectOS("Mozilla/5.0 (X11; Linux x86_64) Firefox/120");
    expect(r.os).toBe("linux");
    expect(r.arch).toBe("x64");
  });

  it("detects Apple Silicon only from the high-entropy arch hint", () => {
    // Mac UA always claims Intel; arm must come from the structured hint.
    const ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120";
    expect(detectOS(ua).os).toBe("macos");
    expect(detectOS(ua).arch).toBe("unknown"); // can't tell from UA alone
    expect(detectOS(ua, "macOS", "arm").arch).toBe("arm64"); // hint reveals it
    expect(detectOS(ua, "macOS", "arm").label).toBe("macOS — Apple Silicon");
  });

  it("prefers the platform hint over the UA string", () => {
    expect(detectOS("weird-ua", "Windows", "x86").os).toBe("windows");
  });

  it("degrades gracefully to unknown", () => {
    const r = detectOS("");
    expect(r.os).toBe("unknown");
    expect(r.label).toBe("your platform");
  });
});

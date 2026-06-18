import { describe, it, expect } from "vitest";
import { validateUsageEvent, validateBatch, MAX_BATCH } from "./analytics";

describe("analytics content firewall", () => {
  it("accepts a clean metadata event", () => {
    const r = validateUsageEvent({ type: "chat", provider: "ollama", model: "qwen2.5-coder:7b", tokensIn: 100, tokensOut: 50, language: "rust" });
    expect(r.ok).toBe(true);
  });

  it("REJECTS any unexpected field (the content guard)", () => {
    for (const field of ["prompt", "code", "path", "repo", "filePath", "content", "message"]) {
      const r = validateUsageEvent({ type: "chat", [field]: "secret stuff" });
      expect(r.ok, `should reject field ${field}`).toBe(false);
    }
  });

  it("rejects content-shaped strings (whitespace / prose / too long)", () => {
    expect(validateUsageEvent({ type: "chat", model: "this is a sentence" }).ok).toBe(false); // whitespace
    expect(validateUsageEvent({ type: "chat", language: "function foo() {}" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "x".repeat(200) }).ok).toBe(false);
  });

  it("rejects a path or repo name smuggled into model", () => {
    expect(validateUsageEvent({ type: "chat", model: "/Users/me/secret/project/file.ts" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "../../etc/passwd" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "owner/repo/extra/deep" }).ok).toBe(false);
    // Tightened: repo-name + single-segment path + filename shapes are rejected.
    expect(validateUsageEvent({ type: "chat", model: "facebook/react" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "myorg/secret-internal-repo" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "src/PaymentService.java" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", model: "config.production.env" }).ok).toBe(false);
  });

  it("allows a normal Ollama model tag incl. one slash namespace", () => {
    expect(validateUsageEvent({ type: "chat", model: "library/llama3:8b" }).ok).toBe(true);
    expect(validateUsageEvent({ type: "chat", model: "qwen2.5-coder:7b" }).ok).toBe(true);
    expect(validateUsageEvent({ type: "chat", model: "llama3.1:8b-instruct-q8_0" }).ok).toBe(true);
    expect(validateUsageEvent({ type: "chat", model: "phi3" }).ok).toBe(true);
  });

  it("rejects unknown event types and bad token counts", () => {
    expect(validateUsageEvent({ type: "exfiltrate" }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", tokensIn: -5 }).ok).toBe(false);
    expect(validateUsageEvent({ type: "chat", tokensIn: 1.5 }).ok).toBe(false);
  });

  it("validateBatch keeps valid, counts rejected, flags oversize", () => {
    const { events, rejected } = validateBatch([
      { type: "chat" },
      { type: "build", prompt: "leak" }, // rejected (unknown field)
      { type: "agent", language: "go" },
    ]);
    expect(events.length).toBe(2);
    expect(rejected).toBe(1);
    expect(validateBatch(new Array(MAX_BATCH + 1).fill({ type: "chat" })).tooLarge).toBe(true);
  });
});

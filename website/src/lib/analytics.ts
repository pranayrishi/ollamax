// Usage-analytics validation. THE content firewall: every incoming event is
// validated to be metadata-only. Anything that could carry content — an unknown
// field (e.g. `prompt`, `code`, `path`, `repo`), an over-long string, or a
// string with whitespace/newlines (prose-shaped) — is REJECTED. So prompt text,
// generated code, file contents, file paths, and repo names can never be stored.
//
// Pure functions, unit-tested. (Importable without "server-only" so tests run.)

export type ValidUsageEvent = {
  type: string;
  provider: string | null;
  model: string | null;
  tokensIn: number | null;
  tokensOut: number | null;
  language: string | null;
  accepted: boolean | null;
  ts: Date;
};

const TYPES = new Set(["chat", "agent", "build", "route", "hub_activate", "suggestion"]);
const ALLOWED_KEYS = new Set([
  "type",
  "provider",
  "model",
  "tokensIn",
  "tokensOut",
  "language",
  "accepted",
  "ts",
]);

export const MAX_BATCH = 500;

// A metadata string: no whitespace (kills prose), bounded length, tight charset.
function metaString(v: unknown, maxLen: number, re: RegExp): string | null | undefined {
  if (v === undefined || v === null) return null;
  if (typeof v !== "string") return undefined; // invalid
  if (v.length === 0) return null;
  if (v.length > maxLen) return undefined;
  if (/\s/.test(v)) return undefined; // whitespace ⇒ likely content
  if (!re.test(v)) return undefined;
  return v;
}

function intOrNull(v: unknown, max: number): number | null | undefined {
  if (v === undefined || v === null) return null;
  if (typeof v !== "number" || !Number.isInteger(v) || v < 0 || v > max) return undefined;
  return v;
}

/** Validate one raw event. Returns the clean event, or a rejection reason. */
export function validateUsageEvent(raw: unknown): { ok: true; event: ValidUsageEvent } | { ok: false; reason: string } {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return { ok: false, reason: "not_an_object" };
  }
  const obj = raw as Record<string, unknown>;

  // Reject ANY unexpected field — this is the core content guard.
  for (const k of Object.keys(obj)) {
    if (!ALLOWED_KEYS.has(k)) return { ok: false, reason: `unexpected_field:${k}` };
  }

  if (typeof obj.type !== "string" || !TYPES.has(obj.type)) {
    return { ok: false, reason: "bad_type" };
  }

  const provider = metaString(obj.provider, 40, /^[\w.-]+$/);
  if (provider === undefined) return { ok: false, reason: "bad_provider" };

  // Model TAG: an Ollama-style id — `name`, `name:tag`, or `namespace/name:tag`.
  // Tight enough to reject repo-name and file-path shapes that the old "≤1
  // slash" rule let through (e.g. `facebook/react`, `src/Foo.java`,
  // `config.prod.env`): a slash REQUIRES a `:tag`, and a trailing `.ext`
  // filename shape is rejected unless it carries a `:tag`.
  let model: string | null = null;
  if (obj.model !== undefined && obj.model !== null) {
    if (typeof obj.model !== "string") return { ok: false, reason: "bad_model" };
    const m = obj.model;
    const hasSlash = m.includes("/");
    const hasColon = m.includes(":");
    const looksLikeFile = /\.[A-Za-z]{1,6}$/.test(m); // trailing .ext (e.g. .java, .env)
    if (
      m.length > 80 ||
      /\s/.test(m) ||
      m.includes("..") ||
      (m.match(/\//g) || []).length > 1 ||
      !/^[A-Za-z0-9._:/_-]+$/.test(m) ||
      (hasSlash && !hasColon) || // repo-name / path shape
      (looksLikeFile && !hasColon) // filename shape
    ) {
      return { ok: false, reason: "bad_model" };
    }
    model = m;
  }

  let language: string | null = null;
  if (obj.language !== undefined && obj.language !== null) {
    if (typeof obj.language !== "string") return { ok: false, reason: "bad_language" };
    const lang = obj.language.toLowerCase();
    const v = metaString(lang, 30, /^[a-z0-9+#.-]+$/);
    if (v === undefined) return { ok: false, reason: "bad_language" };
    language = v;
  }

  const tokensIn = intOrNull(obj.tokensIn, 10_000_000);
  if (tokensIn === undefined) return { ok: false, reason: "bad_tokensIn" };
  const tokensOut = intOrNull(obj.tokensOut, 10_000_000);
  if (tokensOut === undefined) return { ok: false, reason: "bad_tokensOut" };

  let accepted: boolean | null = null;
  if (obj.accepted !== undefined && obj.accepted !== null) {
    if (typeof obj.accepted !== "boolean") return { ok: false, reason: "bad_accepted" };
    accepted = obj.accepted;
  }

  // Timestamp: accept client ts but clamp to a sane window; default to now.
  let ts = new Date();
  if (typeof obj.ts === "string") {
    const parsed = new Date(obj.ts);
    const t = parsed.getTime();
    const now = Date.now();
    if (!Number.isNaN(t) && t > now - 90 * 86_400_000 && t < now + 5 * 60_000) ts = parsed;
  }

  return {
    ok: true,
    event: { type: obj.type, provider: provider ?? null, model, tokensIn, tokensOut, language, accepted, ts },
  };
}

/** Validate a batch. Returns the valid events and the count rejected. */
export function validateBatch(raw: unknown): { events: ValidUsageEvent[]; rejected: number; tooLarge: boolean } {
  if (!Array.isArray(raw)) return { events: [], rejected: 0, tooLarge: false };
  if (raw.length > MAX_BATCH) return { events: [], rejected: raw.length, tooLarge: true };
  const events: ValidUsageEvent[] = [];
  let rejected = 0;
  for (const r of raw) {
    const v = validateUsageEvent(r);
    if (v.ok) events.push(v.event);
    else rejected++;
  }
  return { events, rejected, tooLarge: false };
}

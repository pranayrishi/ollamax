// Security audit log. Emits structured JSON lines for security-relevant events
// (sign-in/out, token issuance/refresh/reuse/revoke, device approval, starring
// scope grants + stars applied). On Vercel these go to the platform log drain;
// in production, forward them to a SIEM / log sink (see the report).
//
// IDENTITY/SECURITY ONLY — never prompts, code, or inference. A defensive
// `redact()` pass strips anything token-shaped so a secret can't land in logs
// even if a caller passes one by mistake.
import "server-only";

export type AuditEvent =
  | "signin"
  | "signout"
  | "desktop_token_issued"
  | "desktop_token_refreshed"
  | "desktop_token_reuse_detected"
  | "desktop_revoke"
  | "device_approved"
  | "star_intent_created"
  | "star_scope_authorized"
  | "stars_applied"
  | "star_failed"
  | "hub_refresh";

// Unanchored so e.g. `access_token_value` or `x_authorization` are also caught.
const SECRETISH = /token|secret|code|verifier|authorization|password|key/i;
// Value-side scrub (defense-in-depth): a secret passed under a benign KEY would
// otherwise slip through. Catch JWT-shaped strings, long base64url/hex blobs,
// and `code=`/`access_token=`-style query fragments.
const SECRET_VALUE =
  /eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}|(?:access_token|code|client_secret|refresh_token)=|[A-Za-z0-9_-]{40,}/;

function scrubValue(v: unknown): unknown {
  if (typeof v === "string" && SECRET_VALUE.test(v)) return "[redacted]";
  return v;
}

function redact(fields: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fields)) {
    out[k] = SECRETISH.test(k) ? "[redacted]" : scrubValue(v);
  }
  return out;
}

export function audit(event: AuditEvent, fields: Record<string, unknown> = {}): void {
  try {
    console.log(
      JSON.stringify({
        ts: new Date().toISOString(),
        kind: "security_audit",
        event,
        ...redact(fields),
      })
    );
  } catch {
    /* logging must never throw into a request path */
  }
}

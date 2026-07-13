"use strict";

// URLs handed to the operating system are a capability boundary. Accept normal
// HTTPS links and local-development/OAuth loopback HTTP only. In particular,
// never pass server-provided command:, file:, data:, or credential-bearing URLs
// through vscode.env.openExternal.
const { URL } = require("url");

function isLiteralLoopbackHostname(hostname) {
  const host = String(hostname || "").toLowerCase();
  return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]";
}

function safeExternalUrl(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > 4_096) return null;
  try {
    const parsed = new URL(value);
    if (parsed.username || parsed.password) return null;
    if (parsed.protocol === "https:") return parsed.toString();
    if (parsed.protocol === "http:" && isLiteralLoopbackHostname(parsed.hostname)) {
      return parsed.toString();
    }
  } catch (_) {}
  return null;
}

module.exports = { isLiteralLoopbackHostname, safeExternalUrl };

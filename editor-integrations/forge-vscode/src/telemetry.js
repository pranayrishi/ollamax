// @ts-check
"use strict";

// ForgeTelemetry — emits ANONYMOUS USAGE METADATA to the account backend to
// power the web dashboard. Content NEVER leaves the machine: `track()` builds a
// strict allowlisted payload (no prompt text, code, file contents, paths, or
// repo names) — even the field names are fixed. Respects the `forge.telemetry`
// setting (off ⇒ nothing is sent) and only sends when signed in (so events can
// be attributed to the user's own dashboard). Batched + flushed.

const vscode = require("vscode");
const http = require("http");
const https = require("https");
const { URL } = require("url");

class ForgeTelemetry {
  /**
   * @param {import('./auth').ForgeAuth} auth
   * @param {(m: string) => void} log
   */
  constructor(auth, log) {
    this.auth = auth;
    this.log = log;
    /** @type {any[]} */
    this.queue = [];
    this.timer = null;
  }

  enabled() {
    return vscode.workspace.getConfiguration("forge").get("telemetry", true);
  }
  base() {
    return this.auth.accountServerOrNull();
  }

  /** Record one metadata event. Anything not in the allowlist is dropped. */
  track(ev) {
    if (!this.enabled() || !this.base()) return;
    const clean = { type: String(ev.type || "").slice(0, 24), ts: new Date().toISOString() };
    if (ev.provider) clean.provider = String(ev.provider).slice(0, 40);
    if (ev.model) clean.model = String(ev.model).slice(0, 80);
    if (typeof ev.tokensIn === "number") clean.tokensIn = ev.tokensIn | 0;
    if (typeof ev.tokensOut === "number") clean.tokensOut = ev.tokensOut | 0;
    if (ev.language) clean.language = String(ev.language).toLowerCase().slice(0, 30);
    if (typeof ev.accepted === "boolean") clean.accepted = ev.accepted;
    this.queue.push(clean);
    if (this.queue.length >= 20) this.flush();
    else this._schedule();
  }

  _schedule() {
    if (this.timer) return;
    this.timer = setTimeout(() => this.flush(), 8000);
  }

  async flush() {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (!this.enabled() || this.queue.length === 0) {
      this.queue = [];
      return;
    }
    const batch = this.queue.splice(0, 200);
    const token = await this.auth.getAccessToken().catch(() => null);
    if (!token) return; // not signed in → can't attribute; drop silently
    try {
      await this._post(`${this.base()}/api/analytics/ingest`, batch, token);
    } catch (e) {
      this.log(`telemetry flush failed: ${e && e.message}`);
    }
  }

  _post(urlStr, body, bearer) {
    const url = new URL(urlStr);
    const lib = url.protocol === "https:" ? https : http;
    const data = JSON.stringify(body);
    return new Promise((resolve, reject) => {
      const req = lib.request(
        {
          method: "POST",
          hostname: url.hostname,
          port: url.port || (url.protocol === "https:" ? 443 : 80),
          path: url.pathname,
          headers: {
            "Content-Type": "application/json",
            "Content-Length": Buffer.byteLength(data),
            Authorization: `Bearer ${bearer}`,
          },
        },
        (res) => {
          res.resume();
          res.on("end", resolve);
        }
      );
      req.on("error", reject);
      req.write(data);
      req.end();
    });
  }

  dispose() {
    this.flush();
  }
}

// Infer a language name from a file extension ONLY (never the path/content).
// Returns null when unknown.
const EXT_LANG = {
  rs: "rust", ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript",
  py: "python", go: "go", java: "java", rb: "ruby", c: "c", h: "c", cpp: "cpp", cc: "cpp",
  hpp: "cpp", cs: "csharp", php: "php", swift: "swift", kt: "kotlin", scala: "scala",
  sh: "shell", sql: "sql", html: "html", css: "css", json: "json", yaml: "yaml", yml: "yaml",
  md: "markdown", lua: "lua", dart: "dart", ex: "elixir", exs: "elixir", clj: "clojure",
  r: "r", jl: "julia", zig: "zig", hs: "haskell", ml: "ocaml",
};
function languageFromExt(pathOrName) {
  if (!pathOrName) return null;
  const m = String(pathOrName).toLowerCase().match(/\.([a-z0-9]+)$/);
  return m ? EXT_LANG[m[1]] || null : null;
}

module.exports = { ForgeTelemetry, languageFromExt };

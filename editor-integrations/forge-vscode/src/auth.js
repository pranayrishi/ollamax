// @ts-check
"use strict";

// ForgeAuth — desktop "Sign in with GitHub" for the extension. Resolves to the
// SAME account as the website (both key on the GitHub identity via our backend).
//
// Security model (matches the backend):
//  - The app is a PUBLIC client: it holds NO GitHub client secret.
//  - Primary flow: OAuth Authorization Code + PKCE over a loopback listener.
//    The app opens the system browser to <backend>/api/desktop/start with a
//    PKCE code_challenge + a loopback redirect_uri, receives a short-lived
//    single-use code on 127.0.0.1, then exchanges it (+ the PKCE verifier) at
//    <backend>/api/desktop/token for OUR app tokens. The app never sees the
//    GitHub secret or the raw GitHub token.
//  - Fallback: device-authorization flow (show a code, approve in browser).
//  - Tokens live in VSCode SecretStorage (OS-keychain-backed). "Sign out"
//    revokes server-side and clears the keychain entry.
//  - This is identity only. It never gates local inference.

const vscode = require("vscode");
const http = require("http");
const https = require("https");
const crypto = require("crypto");
const { URL } = require("url");

const SECRET_KEY = "forge.account.tokens.v1";

function b64url(buf) {
  return buf.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

class ForgeAuth {
  /**
   * @param {import('vscode').ExtensionContext} context
   * @param {(m: string) => void} log
   */
  constructor(context, log) {
    this.context = context;
    this.log = log;
    /** in-memory cache of the current user (null = signed out / unknown) */
    this.user = null;
  }

  /** Backend base URL from settings; throws a clear error if unset/insecure. */
  baseUrl() {
    const cfg = vscode.workspace.getConfiguration("forge");
    const url = (cfg.get("accountServer", "") || "").trim().replace(/\/+$/, "");
    if (!url) {
      throw new Error(
        "No account server configured. Set `forge.accountServer` to your deployed site " +
          "(e.g. https://your-app.vercel.app) or http://localhost:3000 for local dev."
      );
    }
    // Require HTTPS for anything but loopback — identity tokens + profile must
    // never transit in cleartext to a remote host.
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      throw new Error(`Invalid forge.accountServer URL: ${url}`);
    }
    const isLoopback =
      parsed.hostname === "127.0.0.1" ||
      parsed.hostname === "localhost" ||
      parsed.hostname === "[::1]" ||
      parsed.hostname === "::1";
    if (parsed.protocol !== "https:" && !isLoopback) {
      throw new Error(
        `forge.accountServer must use https:// (got ${parsed.protocol}//${parsed.hostname}). ` +
          "Plain http is only allowed for localhost during development."
      );
    }
    return url;
  }

  // ----- token storage (OS keychain via SecretStorage) -----

  async _store(tokens) {
    await this.context.secrets.store(SECRET_KEY, JSON.stringify(tokens));
    this.user = tokens.user || null;
  }

  async _load() {
    const raw = await this.context.secrets.get(SECRET_KEY);
    if (!raw) return null;
    try {
      return JSON.parse(raw);
    } catch {
      return null;
    }
  }

  async _clear() {
    await this.context.secrets.delete(SECRET_KEY);
    this.user = null;
  }

  // ----- public API -----

  /**
   * #6 login-gate: does the user have an account session? OFFLINE-GRACEFUL by
   * design — it does NOT touch the network. Having signed in before (a stored
   * token) is enough to pass the gate; token validity is enforced lazily when
   * the token is actually used (getAccessToken refreshes/clears as needed). This
   * means a signed-in user is not locked out of the app just because they're
   * temporarily offline.
   */
  async isSignedIn() {
    const tokens = await this._load();
    return !!(tokens && tokens.access_token);
  }

  /** The stored user profile without a network round-trip (for the gate UI). */
  async cachedUser() {
    const tokens = await this._load();
    return (tokens && tokens.user) || this.user || null;
  }

  // A transport failure (offline / connection refused / DNS) or a server-side
  // error (5xx) must NEVER delete the stored session — only a DEFINITIVE auth
  // rejection from the server does. Otherwise a momentary outage permanently
  // signs the user out (review finding #1). `clearedRejection` returns true only
  // when the server affirmatively says the session is dead.
  static _isAuthRejection(res) {
    return (
      res.status === 401 ||
      res.status === 403 ||
      (res.body && (res.body.error === "invalid_grant" || res.body.error === "invalid_token"))
    );
  }

  /** Current user if signed in. OFFLINE-GRACEFUL: keeps the cached session on
   *  any network/transport/5xx failure; clears only on a definitive rejection. */
  async getUser() {
    const tokens = await this._load();
    if (!tokens) {
      this.user = null;
      return null;
    }
    let meRes;
    try {
      meRes = await this._request("GET", "/api/me", null, tokens.access_token);
    } catch {
      // transport error → unreachable, not unauthorized. Keep the session.
      this.user = tokens.user || this.user;
      return this.user;
    }
    if (meRes.status === 200) {
      this.user = (meRes.body && meRes.body.user) || tokens.user || null;
      return this.user;
    }
    // Access token not accepted. Refresh — but only a definitive rejection clears.
    if (!tokens.refresh_token) {
      if (ForgeAuth._isAuthRejection(meRes)) {
        await this._clear();
        return null;
      }
      this.user = tokens.user || this.user;
      return this.user;
    }
    let rRes;
    try {
      rRes = await this._request("POST", "/api/desktop/refresh", {
        refresh_token: tokens.refresh_token,
      });
    } catch {
      this.user = tokens.user || this.user;
      return this.user;
    }
    if (rRes.status === 200) {
      await this._store(rRes.body);
      this.user = (rRes.body && rRes.body.user) || null;
      return this.user;
    }
    if (ForgeAuth._isAuthRejection(rRes)) {
      await this._clear();
      return null;
    }
    // 5xx / unknown — keep the session and retry later.
    this.user = tokens.user || this.user;
    return this.user;
  }

  /** A valid access token, or null if definitively signed out. Same offline
   *  rule: a transport/5xx failure keeps the token rather than wiping it. */
  async getAccessToken() {
    const tokens = await this._load();
    if (!tokens) return null;
    let meRes;
    try {
      meRes = await this._request("GET", "/api/me", null, tokens.access_token);
    } catch {
      return tokens.access_token; // offline — assume still valid; the real call surfaces errors
    }
    if (meRes.status === 200) return tokens.access_token;
    if (!tokens.refresh_token) {
      if (ForgeAuth._isAuthRejection(meRes)) {
        await this._clear();
        return null;
      }
      return tokens.access_token;
    }
    let rRes;
    try {
      rRes = await this._request("POST", "/api/desktop/refresh", {
        refresh_token: tokens.refresh_token,
      });
    } catch {
      return tokens.access_token;
    }
    if (rRes.status === 200) {
      await this._store(rRes.body);
      return rRes.body.access_token;
    }
    if (ForgeAuth._isAuthRejection(rRes)) {
      await this._clear();
      return null;
    }
    return tokens.access_token;
  }

  /** The configured account server base URL, or null if unset. */
  accountServerOrNull() {
    try {
      return this.baseUrl();
    } catch {
      return null;
    }
  }

  /** Primary sign-in: loopback + PKCE. Returns the user. */
  async signIn() {
    const base = this.baseUrl();
    const verifier = b64url(crypto.randomBytes(32));
    const challenge = b64url(crypto.createHash("sha256").update(verifier).digest());
    const state = b64url(crypto.randomBytes(16));

    const { port, waitForCode, close } = await this._startLoopback(state);
    const redirectUri = `http://127.0.0.1:${port}/callback`;
    const authUrl =
      `${base}/api/desktop/start?code_challenge=${challenge}` +
      `&code_challenge_method=S256&redirect_uri=${encodeURIComponent(redirectUri)}` +
      `&state=${encodeURIComponent(state)}`;

    this.log(`auth: opening browser for sign-in (loopback :${port})`);
    await vscode.env.openExternal(vscode.Uri.parse(authUrl));

    let code;
    try {
      code = await waitForCode(5 * 60 * 1000);
    } finally {
      close();
    }

    const tokens = await this._postJson("/api/desktop/token", {
      code,
      code_verifier: verifier,
      redirect_uri: redirectUri,
    });
    await this._store(tokens);
    this.log(`auth: signed in as @${tokens.user && tokens.user.login}`);
    return tokens.user;
  }

  /** Fallback sign-in: device authorization flow. Returns the user. */
  async signInDevice() {
    const start = await this._postJson("/api/desktop/device/start", {});
    // Open the verification page WITHOUT the code in the URL (anti-phishing).
    // The user must type the code they see below, proving THEY initiated this.
    await vscode.env.openExternal(vscode.Uri.parse(start.verification_uri));
    await vscode.window.showInformationMessage(
      `Ollamax sign-in: type this code in the browser to finish:\n\n${start.user_code}`,
      { modal: true }
    );

    const deadline = Date.now() + (start.expires_in || 600) * 1000;
    const interval = Math.max(2, start.interval || 5) * 1000;
    while (Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, interval));
      const res = await this._request("POST", "/api/desktop/device/token", {
        device_code: start.device_code,
      });
      if (res.status === 200) {
        await this._store(res.body);
        return res.body.user;
      }
      const err = res.body && res.body.error;
      if (err === "authorization_pending" || err === "slow_down") continue;
      throw new Error(`device sign-in failed: ${err || res.status}`);
    }
    throw new Error("device code expired before approval");
  }

  /** Sign out: revoke server-side (best-effort) and clear the keychain. */
  async signOut() {
    const tokens = await this._load();
    if (tokens && tokens.refresh_token) {
      await this._request("POST", "/api/desktop/revoke", {
        refresh_token: tokens.refresh_token,
        all: true,
      })
        .catch(() => {})
        .then(() => {});
    }
    await this._clear();
    this.log("auth: signed out");
  }

  // ----- internals -----

  async _me(accessToken) {
    const res = await this._request("GET", "/api/me", null, accessToken);
    return res.status === 200 ? res.body.user : null;
  }

  async _refresh(refreshToken) {
    if (!refreshToken) return null;
    const res = await this._request("POST", "/api/desktop/refresh", {
      refresh_token: refreshToken,
    });
    return res.status === 200 ? res.body : null;
  }

  async _postJson(path, body) {
    const res = await this._request("POST", path, body);
    if (res.status !== 200) {
      throw new Error(`${path} failed: ${(res.body && res.body.error) || res.status}`);
    }
    return res.body;
  }

  /**
   * HTTP(S) request to the backend. Returns { status, body }.
   * @param {"GET"|"POST"} method
   * @param {string} path
   * @param {any} body
   * @param {string=} bearer
   */
  _request(method, path, body, bearer) {
    const url = new URL(this.baseUrl() + path);
    const lib = url.protocol === "https:" ? https : http;
    const data = body ? JSON.stringify(body) : null;
    const headers = {};
    if (data) {
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(data);
    }
    if (bearer) headers["Authorization"] = `Bearer ${bearer}`;

    return new Promise((resolve, reject) => {
      const req = lib.request(
        {
          method,
          hostname: url.hostname,
          port: url.port || (url.protocol === "https:" ? 443 : 80),
          path: url.pathname + url.search,
          headers,
        },
        (res) => {
          let raw = "";
          res.setEncoding("utf8");
          res.on("data", (c) => (raw += c));
          res.on("end", () => {
            let parsed = {};
            try {
              parsed = raw ? JSON.parse(raw) : {};
            } catch {
              parsed = { error: "bad_json", raw };
            }
            resolve({ status: res.statusCode || 0, body: parsed });
          });
        }
      );
      req.on("error", reject);
      if (data) req.write(data);
      req.end();
    });
  }

  /**
   * Start a one-shot loopback HTTP server on a random 127.0.0.1 port to receive
   * the authorization code. Binds loopback only.
   * @param {string} expectedState
   */
  _startLoopback(expectedState) {
    return new Promise((resolve, reject) => {
      /** @type {(code: string) => void} */
      let resolveCode;
      /** @type {(e: Error) => void} */
      let rejectCode;
      const codePromise = new Promise((res, rej) => {
        resolveCode = res;
        rejectCode = rej;
      });

      const server = http.createServer((req, res) => {
        try {
          const u = new URL(req.url || "/", "http://127.0.0.1");
          if (u.pathname !== "/callback") {
            res.writeHead(404).end("not found");
            return;
          }
          const code = u.searchParams.get("code");
          const state = u.searchParams.get("state");
          // no-referrer so the code in this URL can never escape via Referer.
          res.writeHead(200, {
            "Content-Type": "text/html",
            "Referrer-Policy": "no-referrer",
            "Cache-Control": "no-store",
          });
          res.end(
            "<html><body style='font-family:system-ui;background:#0e0e16;color:#e5e7eb;text-align:center;padding-top:18vh'>" +
              "<h2>✓ Signed in to Ollamax</h2><p>You can close this tab and return to your editor.</p></body></html>"
          );
          if (!code) {
            rejectCode(new Error("no code in callback"));
          } else if (state !== expectedState) {
            // CSRF guard: the state must match what we generated.
            rejectCode(new Error("state mismatch (possible CSRF) — sign-in aborted"));
          } else {
            resolveCode(code);
          }
        } catch (e) {
          rejectCode(e instanceof Error ? e : new Error(String(e)));
        }
      });

      server.on("error", reject);
      // Port 0 → OS-assigned. Bind loopback only.
      server.listen(0, "127.0.0.1", () => {
        const addr = server.address();
        const port = typeof addr === "object" && addr ? addr.port : 0;
        resolve({
          port,
          waitForCode: (timeoutMs) =>
            Promise.race([
              codePromise,
              new Promise((_r, rej) =>
                setTimeout(() => rej(new Error("sign-in timed out")), timeoutMs)
              ),
            ]),
          close: () => {
            try {
              server.close();
            } catch {
              /* already closed */
            }
          },
        });
      });
    });
  }
}

module.exports = { ForgeAuth };

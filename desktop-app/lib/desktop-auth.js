"use strict";

// Main-process-only account client for the Electron app.  This deliberately
// has no Electron import so it can be exercised with node --test.  The caller
// supplies Electron's `safeStorage` only after app.whenReady().
//
// Security boundary:
// - OAuth codes, access tokens, and refresh tokens never leave this module.
// - Persistent storage is encrypted with Electron safeStorage or not used at
//   all.  If OS-backed encryption is unavailable, the signed-in session exists
//   only in memory and ends when the app exits.
// - The public API returns a deliberately small user profile, never a token.

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const https = require("https");
const path = require("path");
const { URL } = require("url");
const { safeExternalUrl } = require("./desktop-security");

const TOKEN_FILE = "ollamax-account-tokens-v1.bin";
const MAX_TOKEN_FILE_BYTES = 64 * 1024;
const MAX_HTTP_RESPONSE_BYTES = 512 * 1024;
const DEFAULT_REQUEST_TIMEOUT_MS = 15_000;
const DEFAULT_SIGN_IN_TIMEOUT_MS = 5 * 60 * 1000;

class DesktopAuthError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "DesktopAuthError";
    this.code = code;
  }
}

function b64url(value) {
  return Buffer.from(value)
    .toString("base64")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/g, "");
}

function constantTimeEqual(left, right, cryptoImpl = crypto) {
  if (typeof left !== "string" || typeof right !== "string") return false;
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  return a.length === b.length && cryptoImpl.timingSafeEqual(a, b);
}

// The account endpoint is a trusted deployment configuration, but it still
// must not be able to turn the app into a remote-cleartext token client.  A
// deployment root is intentionally required: internal API paths are composed
// here, never accepted from a renderer or server response.
function normalizeAccountServer(value) {
  if (typeof value !== "string" || !value.trim()) {
    throw new DesktopAuthError("no_account_server", "No account server is configured.");
  }
  const approved = safeExternalUrl(value.trim());
  if (!approved) {
    throw new DesktopAuthError(
      "invalid_account_server",
      "The account server must use HTTPS, or literal-loopback HTTP for local development."
    );
  }
  let parsed;
  try {
    parsed = new URL(approved);
  } catch (_) {
    throw new DesktopAuthError("invalid_account_server", "The account server URL is invalid.");
  }
  if (parsed.protocol === "http:" && parsed.hostname !== "127.0.0.1" && parsed.hostname !== "[::1]") {
    throw new DesktopAuthError(
      "invalid_account_server",
      "A development account server must use a literal 127.0.0.1 or [::1] loopback address."
    );
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash || parsed.pathname !== "/") {
    throw new DesktopAuthError(
      "invalid_account_server",
      "The account server must be an origin URL without credentials, a path, query, or fragment."
    );
  }
  return parsed.origin;
}

function boundedText(value, max = 160) {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (!text || text.length > max) return null;
  return text;
}

function safeAvatarUrl(value) {
  if (typeof value !== "string" || value.length > 2_048) return null;
  try {
    const parsed = new URL(value);
    return parsed.protocol === "https:" && !parsed.username && !parsed.password ? parsed.toString() : null;
  } catch (_) {
    return null;
  }
}

// Public account data is intentionally smaller than the website's response:
// desktop UI only needs a display label and avatar, so do not hand email or
// provider metadata to the renderer unnecessarily.
function sanitizeUser(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const login =
    boundedText(value.login) ||
    boundedText(value.name) ||
    (boundedText(value.email) || "").split("@")[0] ||
    "signed-in";
  const rawId = value.id;
  const id =
    (typeof rawId === "number" && Number.isSafeInteger(rawId) && rawId >= 0 && rawId) ||
    boundedText(rawId == null ? null : String(rawId), 80) ||
    null;
  return {
    ...(id == null ? {} : { id }),
    login,
    ...(safeAvatarUrl(value.avatarUrl) ? { avatarUrl: safeAvatarUrl(value.avatarUrl) } : {}),
  };
}

function validateTokenSet(value, now) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new DesktopAuthError("invalid_token_response", "The account server returned an invalid token response.");
  }
  const access = typeof value.access_token === "string" ? value.access_token : "";
  const refresh = typeof value.refresh_token === "string" ? value.refresh_token : "";
  if (!access || access.length > 16_384 || (refresh && refresh.length > 16_384)) {
    throw new DesktopAuthError("invalid_token_response", "The account server returned an invalid token response.");
  }
  const user = sanitizeUser(value.user);
  if (!user) {
    throw new DesktopAuthError("invalid_token_response", "The account server did not return an account profile.");
  }
  const expiresIn = Number(value.expires_in);
  const boundedExpiresIn = Number.isFinite(expiresIn) ? Math.max(1, Math.min(expiresIn, 86_400)) : 15 * 60;
  return {
    version: 1,
    access_token: access,
    ...(refresh ? { refresh_token: refresh } : {}),
    expires_at: Math.floor(now + boundedExpiresIn * 1000),
    user,
  };
}

function isAuthRejection(result) {
  const error = result && result.body && result.body.error;
  return (
    !!result &&
    (result.status === 401 ||
      result.status === 403 ||
      error === "invalid_grant" ||
      error === "invalid_token" ||
      error === "token_reuse_detected")
  );
}

function deviceVerificationUrl(value, accountOrigin) {
  if (typeof value !== "string" || !safeExternalUrl(value)) {
    throw new DesktopAuthError("invalid_device_response", "The account server returned an unsafe device verification URL.");
  }
  let parsed;
  try {
    parsed = new URL(value);
  } catch (_) {
    throw new DesktopAuthError("invalid_device_response", "The account server returned an unsafe device verification URL.");
  }
  if (
    parsed.origin !== accountOrigin ||
    parsed.pathname !== "/desktop/activate" ||
    parsed.search ||
    parsed.hash
  ) {
    throw new DesktopAuthError("invalid_device_response", "The account server returned an unsafe device verification URL.");
  }
  return parsed.toString();
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

class EncryptedTokenStore {
  constructor({ storageDir, safeStorage, fsImpl = fs, pathImpl = path, cryptoImpl = crypto, platform = process.platform }) {
    this.fs = fsImpl;
    this.path = pathImpl;
    this.crypto = cryptoImpl;
    this.storageDir = typeof storageDir === "string" && storageDir ? storageDir : null;
    this.safeStorage = safeStorage || null;
    this.platform = platform;
    this.disabled = false;
  }

  get available() {
    if (this.disabled || !this.storageDir || !this.safeStorage) return false;
    try {
      // Electron can select Linux's `basic_text` backend when no secret-store
      // service is available. It is not an OS-protected credential store, so
      // do not mistake its reversible representation for encrypted-at-rest
      // persistence. The session remains memory-only instead.
      if (
        this.platform === "linux" &&
        typeof this.safeStorage.getSelectedStorageBackend === "function" &&
        ["basic_text", "unknown"].includes(this.safeStorage.getSelectedStorageBackend())
      ) {
        return false;
      }
      return (
        typeof this.safeStorage.isEncryptionAvailable === "function" &&
        this.safeStorage.isEncryptionAvailable() === true &&
        typeof this.safeStorage.encryptString === "function" &&
        typeof this.safeStorage.decryptString === "function"
      );
    } catch (_) {
      return false;
    }
  }

  disable() {
    this.disabled = true;
  }

  get filePath() {
    return this.storageDir ? this.path.join(this.storageDir, TOKEN_FILE) : null;
  }

  load(now) {
    if (!this.available || !this.filePath) return null;
    let stat;
    try {
      stat = this.fs.lstatSync(this.filePath);
    } catch (error) {
      if (error && error.code === "ENOENT") return null;
      throw new DesktopAuthError("storage_unavailable", "Encrypted account storage could not be read.");
    }
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 1 || stat.size > MAX_TOKEN_FILE_BYTES) {
      throw new DesktopAuthError("storage_unavailable", "Encrypted account storage is not a regular credential file.");
    }
    try {
      const encrypted = this.fs.readFileSync(this.filePath);
      const raw = this.safeStorage.decryptString(encrypted);
      return validateTokenSet(JSON.parse(raw), now);
    } catch (error) {
      // A corrupt or undecryptable token record is not a reason to keep an
      // unknown session around. Remove only this record; no plaintext fallback.
      this.clear();
      if (error instanceof DesktopAuthError) throw error;
      return null;
    }
  }

  save(tokens) {
    if (!this.available || !this.filePath || !this.storageDir) return false;
    this.fs.mkdirSync(this.storageDir, { recursive: true, mode: 0o700 });
    const finalPath = this.filePath;
    try {
      const existing = this.fs.lstatSync(finalPath);
      if (!existing.isFile() || existing.isSymbolicLink()) {
        throw new DesktopAuthError("storage_unavailable", "Encrypted account storage is not a regular credential file.");
      }
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }

    const encrypted = this.safeStorage.encryptString(JSON.stringify(tokens));
    if (!Buffer.isBuffer(encrypted) || encrypted.length < 1 || encrypted.length > MAX_TOKEN_FILE_BYTES) {
      throw new DesktopAuthError("storage_unavailable", "Encrypted account storage rejected the credential record.");
    }
    const suffix = b64url(this.crypto.randomBytes(12));
    const tempPath = this.path.join(this.storageDir, `.${TOKEN_FILE}.${process.pid}.${suffix}.tmp`);
    const noFollow = this.fs.constants && this.fs.constants.O_NOFOLLOW ? this.fs.constants.O_NOFOLLOW : 0;
    let fd = null;
    try {
      fd = this.fs.openSync(
        tempPath,
        this.fs.constants.O_WRONLY | this.fs.constants.O_CREAT | this.fs.constants.O_EXCL | noFollow,
        0o600
      );
      this.fs.writeFileSync(fd, encrypted);
      try {
        this.fs.fsyncSync(fd);
      } catch (_) {
        // Some platforms/filesystems do not support fsync for this file. The
        // atomic rename still prevents a partially-written credential record.
      }
      this.fs.closeSync(fd);
      fd = null;
      this.fs.renameSync(tempPath, finalPath);
      try {
        this.fs.chmodSync(finalPath, 0o600);
      } catch (_) {}
      return true;
    } finally {
      if (fd !== null) {
        try {
          this.fs.closeSync(fd);
        } catch (_) {}
      }
      try {
        this.fs.unlinkSync(tempPath);
      } catch (_) {}
    }
  }

  clear() {
    if (!this.filePath) return;
    try {
      this.fs.unlinkSync(this.filePath);
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }
  }
}

class DesktopAuth {
  constructor(options = {}) {
    this.crypto = options.cryptoImpl || crypto;
    this.http = options.httpImpl || http;
    this.https = options.httpsImpl || https;
    this.now = typeof options.now === "function" ? options.now : () => Date.now();
    this.sleep = typeof options.sleep === "function" ? options.sleep : delay;
    this.openExternal = options.openExternal;
    this.log = typeof options.log === "function" ? options.log : () => {};
    this.requestJson = typeof options.requestJson === "function" ? options.requestJson : null;
    this.requestTimeoutMs = Math.max(1_000, Number(options.requestTimeoutMs) || DEFAULT_REQUEST_TIMEOUT_MS);
    this.signInTimeoutMs = Math.max(10_000, Number(options.signInTimeoutMs) || DEFAULT_SIGN_IN_TIMEOUT_MS);
    this.baseUrl = normalizeAccountServer(options.accountServer);
    this.accountOrigin = new URL(this.baseUrl).origin;
    if (typeof this.openExternal !== "function") {
      throw new DesktopAuthError("auth_unavailable", "The desktop browser integration is unavailable.");
    }
    this.store = new EncryptedTokenStore({
      storageDir: options.storageDir,
      safeStorage: options.safeStorage,
      fsImpl: options.fsImpl || fs,
      pathImpl: options.pathImpl || path,
      cryptoImpl: this.crypto,
      platform: options.platform || process.platform,
    });
    this.tokens = undefined; // undefined = not loaded; null = no signed-in session
    this.persisted = false;
    this.validationInFlight = null;
    this.signInInFlight = null;
  }

  get persistenceMode() {
    return this.persisted ? "encrypted" : "memory";
  }

  async status() {
    const tokens = await this._loadTokens();
    return {
      enabled: true,
      user: tokens ? tokens.user : null,
      sessionPersistence: tokens ? this.persistenceMode : this.store.available ? "encrypted" : "memory",
    };
  }

  async getUser() {
    const tokens = await this._validatedTokens();
    return tokens ? tokens.user : null;
  }

  // This is only called by Electron main-process integrations such as Hub
  // support. It is intentionally absent from preload and renderer-facing IPC.
  async authenticatedRequest(method, apiPath, body) {
    const tokens = await this._validatedTokens();
    if (!tokens) return { authenticated: false, status: 0, body: { error: "not_signed_in" } };
    const result = await this._request(method, apiPath, body, tokens.access_token);
    if (isAuthRejection(result)) {
      await this._clearTokens();
      return { authenticated: false, status: result.status, body: { error: "not_signed_in" } };
    }
    return { authenticated: true, ...result };
  }

  async requestPublic(method, apiPath, body) {
    return this._request(method, apiPath, body);
  }

  async signIn() {
    return this._withSignInLock(async () => {
      const verifier = b64url(this.crypto.randomBytes(32));
      const challenge = b64url(this.crypto.createHash("sha256").update(verifier).digest());
      const state = b64url(this.crypto.randomBytes(32));
      const loopback = await this._startLoopback(state);
      const redirectUri = `http://127.0.0.1:${loopback.port}/callback`;
      const start = new URL("/api/desktop/start", `${this.baseUrl}/`);
      start.searchParams.set("code_challenge", challenge);
      start.searchParams.set("code_challenge_method", "S256");
      start.searchParams.set("redirect_uri", redirectUri);
      start.searchParams.set("state", state);

      let code;
      try {
        await this.openExternal(start.toString());
        code = await loopback.waitForCode(this.signInTimeoutMs);
      } finally {
        await loopback.close();
      }

      const response = await this._request("POST", "/api/desktop/token", {
        code,
        code_verifier: verifier,
        redirect_uri: redirectUri,
      });
      if (response.status !== 200) {
        throw new DesktopAuthError("token_exchange_failed", "The account sign-in could not be completed.");
      }
      const tokens = validateTokenSet(response.body, this.now());
      this._commitTokens(tokens);
      this.log("account sign-in completed");
      return { user: tokens.user, sessionPersistence: this.persistenceMode };
    });
  }

  async signInDevice(options = {}) {
    return this._withSignInLock(async () => {
      const start = await this._request("POST", "/api/desktop/device/start", {});
      if (start.status !== 200 || !start.body || typeof start.body !== "object") {
        throw new DesktopAuthError("device_start_failed", "The device sign-in could not be started.");
      }
      const deviceCode = typeof start.body.device_code === "string" ? start.body.device_code : "";
      const userCode = typeof start.body.user_code === "string" ? start.body.user_code : "";
      const verificationUri = deviceVerificationUrl(start.body.verification_uri, this.accountOrigin);
      if (!deviceCode || deviceCode.length > 512 || !userCode || userCode.length > 80) {
        throw new DesktopAuthError("invalid_device_response", "The account server returned an invalid device sign-in response.");
      }
      const expiresIn = Math.max(1, Math.min(Number(start.body.expires_in) || 600, 60 * 60));
      let intervalMs = Math.max(2, Math.min(Number(start.body.interval) || 5, 30)) * 1000;
      await this.openExternal(verificationUri);
      if (typeof options.onDeviceCode === "function") {
        // The device code is displayed by a trusted native UI, not a renderer
        // message. It is never added to the browser URL.
        await options.onDeviceCode({ userCode, verificationUri, expiresIn });
      }

      const deadline = this.now() + expiresIn * 1000;
      while (this.now() < deadline) {
        if (options.signal && options.signal.aborted) {
          throw new DesktopAuthError("sign_in_cancelled", "Device sign-in was cancelled.");
        }
        await this.sleep(intervalMs);
        if (options.signal && options.signal.aborted) {
          throw new DesktopAuthError("sign_in_cancelled", "Device sign-in was cancelled.");
        }
        const poll = await this._request("POST", "/api/desktop/device/token", { device_code: deviceCode });
        if (poll.status === 200) {
          const tokens = validateTokenSet(poll.body, this.now());
          this._commitTokens(tokens);
          this.log("device account sign-in completed");
          return { user: tokens.user, sessionPersistence: this.persistenceMode };
        }
        const error = poll.body && poll.body.error;
        if (error === "authorization_pending") continue;
        if (error === "slow_down") {
          intervalMs = Math.min(intervalMs + 5_000, 30_000);
          continue;
        }
        throw new DesktopAuthError("device_sign_in_failed", "The device sign-in could not be completed.");
      }
      throw new DesktopAuthError("device_code_expired", "The device sign-in code expired before approval.");
    });
  }

  async signOut() {
    const tokens = await this._loadTokens();
    if (tokens && tokens.refresh_token) {
      try {
        await this._request(
          "POST",
          "/api/desktop/revoke",
          { refresh_token: tokens.refresh_token, all: true },
          tokens.access_token
        );
      } catch (_) {
        // Sign-out remains local-first: clear local credentials even when the
        // network is unavailable, and let the short access token expire.
      }
    }
    await this._clearTokens();
    this.log("account sign-out completed");
    return { user: null };
  }

  async _withSignInLock(operation) {
    if (this.signInInFlight) {
      throw new DesktopAuthError("sign_in_in_progress", "A sign-in is already in progress.");
    }
    const current = Promise.resolve().then(operation);
    this.signInInFlight = current;
    try {
      return await current;
    } finally {
      if (this.signInInFlight === current) this.signInInFlight = null;
    }
  }

  async _loadTokens() {
    if (this.tokens !== undefined) return this.tokens;
    this.tokens = null;
    if (!this.store.available) return this.tokens;
    try {
      const tokens = this.store.load(this.now());
      this.tokens = tokens;
      this.persisted = !!tokens;
    } catch (_) {
      // Do not surface filesystem/decrypt details to a renderer. The app
      // simply continues signed out rather than trying plaintext storage.
      this.store.disable();
      this.persisted = false;
      this.log("encrypted account storage was unavailable; using memory sessions only");
    }
    return this.tokens;
  }

  _commitTokens(tokens) {
    this.tokens = tokens;
    this.persisted = false;
    if (!this.store.available) return;
    try {
      this.store.save(tokens);
      this.persisted = true;
    } catch (_) {
      // Fail closed: never retry by serializing tokens in plaintext. The live
      // memory session remains usable and is explicitly reported as such.
      this.store.disable();
      this.log("encrypted account storage failed; keeping this account session in memory only");
    }
  }

  async _clearTokens() {
    this.tokens = null;
    this.persisted = false;
    try {
      this.store.clear();
    } catch (_) {
      this.log("could not remove encrypted account storage");
    }
  }

  async _validatedTokens() {
    if (this.validationInFlight) return this.validationInFlight;
    const current = this._validateTokens();
    this.validationInFlight = current;
    try {
      return await current;
    } finally {
      if (this.validationInFlight === current) this.validationInFlight = null;
    }
  }

  async _validateTokens() {
    const tokens = await this._loadTokens();
    if (!tokens) return null;
    let me;
    try {
      me = await this._request("GET", "/api/me", undefined, tokens.access_token);
    } catch (_) {
      // Offline or 5xx/transport failures do not erase a credential. A later
      // authenticated request will report its own network error.
      return tokens;
    }
    if (me.status === 200) {
      const user = sanitizeUser(me.body && me.body.user);
      if (user) {
        this.tokens = { ...tokens, user };
        this._commitTokens(this.tokens);
      }
      return this.tokens;
    }
    if (!isAuthRejection(me)) return tokens;
    if (!tokens.refresh_token) {
      await this._clearTokens();
      return null;
    }
    let refreshed;
    try {
      refreshed = await this._request("POST", "/api/desktop/refresh", { refresh_token: tokens.refresh_token });
    } catch (_) {
      return tokens;
    }
    if (refreshed.status === 200) {
      const next = validateTokenSet(refreshed.body, this.now());
      this._commitTokens(next);
      return next;
    }
    if (isAuthRejection(refreshed)) {
      await this._clearTokens();
      return null;
    }
    return tokens;
  }

  _endpoint(apiPath) {
    if (typeof apiPath !== "string" || !apiPath.startsWith("/api/")) {
      throw new DesktopAuthError("invalid_request", "Invalid account API path.");
    }
    const endpoint = new URL(apiPath, `${this.baseUrl}/`);
    if (endpoint.origin !== this.accountOrigin) {
      throw new DesktopAuthError("invalid_request", "Invalid account API origin.");
    }
    return endpoint;
  }

  async _request(method, apiPath, body, bearer) {
    const endpoint = this._endpoint(apiPath);
    if (this.requestJson) {
      const result = await this.requestJson({
        method,
        url: endpoint.toString(),
        path: endpoint.pathname + endpoint.search,
        body,
        bearer: bearer || null,
      });
      return {
        status: Number(result && result.status) || 0,
        body: result && result.body && typeof result.body === "object" ? result.body : {},
      };
    }
    const lib = endpoint.protocol === "https:" ? this.https : this.http;
    const data = body === undefined ? null : JSON.stringify(body);
    const headers = { Accept: "application/json", "User-Agent": "Ollamax-Desktop" };
    if (data !== null) {
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(data);
    }
    if (bearer) headers.Authorization = `Bearer ${bearer}`;

    return new Promise((resolve, reject) => {
      let settled = false;
      const finish = (callback, value) => {
        if (settled) return;
        settled = true;
        callback(value);
      };
      const req = lib.request(
        {
          method,
          hostname: endpoint.hostname,
          port: endpoint.port || (endpoint.protocol === "https:" ? 443 : 80),
          path: endpoint.pathname + endpoint.search,
          headers,
        },
        (res) => {
          const chunks = [];
          let received = 0;
          res.on("data", (chunk) => {
            const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
            received += buffer.length;
            if (received > MAX_HTTP_RESPONSE_BYTES) {
              res.destroy();
              finish(reject, new DesktopAuthError("response_too_large", "The account server response was too large."));
              return;
            }
            chunks.push(buffer);
          });
          res.on("error", (error) => finish(reject, error));
          res.on("end", () => {
            if (settled) return;
            let parsed = {};
            const raw = Buffer.concat(chunks).toString("utf8");
            if (raw) {
              try {
                parsed = JSON.parse(raw);
              } catch (_) {
                parsed = { error: "invalid_json" };
              }
            }
            finish(resolve, { status: res.statusCode || 0, body: parsed && typeof parsed === "object" ? parsed : {} });
          });
        }
      );
      req.setTimeout(this.requestTimeoutMs, () => {
        req.destroy(new DesktopAuthError("request_timeout", "The account server request timed out."));
      });
      req.on("error", (error) => finish(reject, error));
      if (data !== null) req.write(data);
      req.end();
    });
  }

  _startLoopback(expectedState) {
    return new Promise((resolve, reject) => {
      let settled = false;
      let waiting = false;
      let codeResolve;
      let codeReject;
      let timeout = null;
      const codePromise = new Promise((resolveCode, rejectCode) => {
        codeResolve = resolveCode;
        codeReject = rejectCode;
      });
      const settleCode = (error, code) => {
        if (settled) return;
        settled = true;
        if (timeout) clearTimeout(timeout);
        if (error) codeReject(error);
        else codeResolve(code);
      };
      const server = this.http.createServer((req, res) => {
        let target;
        try {
          target = new URL(req.url || "/", "http://127.0.0.1");
        } catch (_) {
          res
            .writeHead(400, { "Cache-Control": "no-store", "Referrer-Policy": "no-referrer", Connection: "close" })
            .end("invalid callback");
          settleCode(new DesktopAuthError("invalid_callback", "The sign-in callback was invalid."));
          return;
        }
        if (req.method !== "GET") {
          res
            .writeHead(405, { Allow: "GET", "Cache-Control": "no-store", "Referrer-Policy": "no-referrer", Connection: "close" })
            .end("method not allowed");
          return;
        }
        if (target.pathname !== "/callback") {
          res
            .writeHead(404, { "Cache-Control": "no-store", "Referrer-Policy": "no-referrer", Connection: "close" })
            .end("not found");
          return;
        }
        const code = target.searchParams.get("code");
        const state = target.searchParams.get("state");
        const valid = !!code && !!state && constantTimeEqual(state, expectedState, this.crypto);
        const headers = {
          "Content-Type": "text/html; charset=utf-8",
          "Cache-Control": "no-store",
          "Referrer-Policy": "no-referrer",
          // The callback listener is one-shot. Closing this browser connection
          // avoids retaining an idle local socket after a completed sign-in.
          Connection: "close",
          "X-Content-Type-Options": "nosniff",
          "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'",
        };
        if (!valid) {
          res.once("finish", () => {
            settleCode(new DesktopAuthError("state_mismatch", "The sign-in callback did not match this sign-in request."));
          });
          res.writeHead(400, headers).end("<p>Sign-in could not be completed. Return to Ollamax and try again.</p>");
          return;
        }
        res.once("finish", () => settleCode(null, code));
        res.writeHead(200, headers).end(
          "<html><body style=\"font-family:system-ui;background:#0e0e16;color:#e5e7eb;text-align:center;padding-top:18vh\">" +
            "<h2>✓ Signed in to Ollamax</h2><p>You can close this tab and return to the app.</p></body></html>"
        );
      });
      server.once("error", (error) => reject(error));
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        const port = address && typeof address === "object" ? address.port : 0;
        if (!Number.isInteger(port) || port < 1) {
          try {
            server.close();
          } catch (_) {}
          reject(new DesktopAuthError("loopback_unavailable", "A local sign-in listener could not be started."));
          return;
        }
        resolve({
          port,
          waitForCode: (timeoutMs) => {
            waiting = true;
            // A very fast browser/local test callback can settle before the
            // caller begins awaiting this promise. In that case, do not leave
            // an unnecessary timeout handle alive after a successful sign-in.
            if (!settled) {
              timeout = setTimeout(() => {
                settleCode(new DesktopAuthError("sign_in_timed_out", "Sign-in timed out before approval."));
              }, timeoutMs);
            }
            return codePromise;
          },
          close: () =>
            new Promise((resolveClose) => {
              // If opening the browser itself failed, no consumer has attached
              // to codePromise yet. Do not create an unhandled rejection while
              // still shutting down the listener.
              if (!settled && waiting) {
                settleCode(new DesktopAuthError("sign_in_cancelled", "Sign-in was cancelled."));
              }
              const done = () => resolveClose();
              try {
                server.close(done);
                // Node's HTTP server otherwise keeps an idle browser connection
                // alive for its default timeout after this one-shot callback.
                if (typeof server.closeAllConnections === "function") server.closeAllConnections();
                else if (typeof server.closeIdleConnections === "function") server.closeIdleConnections();
              } catch (_) {
                done();
              }
            }),
        });
      });
    });
  }
}

module.exports = {
  DesktopAuth,
  DesktopAuthError,
  EncryptedTokenStore,
  b64url,
  constantTimeEqual,
  deviceVerificationUrl,
  isAuthRejection,
  normalizeAccountServer,
  sanitizeUser,
};

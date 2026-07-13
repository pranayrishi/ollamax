"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { URL } = require("node:url");

const {
  DesktopAuth,
  DesktopAuthError,
  b64url,
  normalizeAccountServer,
} = require("../lib/desktop-auth");

function safeStorage(available = true, backend = "gnome_libsecret") {
  return {
    isEncryptionAvailable: () => available,
    getSelectedStorageBackend: () => backend,
    encryptString: (value) => Buffer.from(`encrypted:${value}`, "utf8"),
    decryptString: (value) => {
      const raw = Buffer.from(value).toString("utf8");
      if (!raw.startsWith("encrypted:")) throw new Error("not encrypted");
      return raw.slice("encrypted:".length);
    },
  };
}

function tokenSet(label = "Ada") {
  return {
    access_token: `access-${label}`,
    refresh_token: `refresh-${label}`,
    expires_in: 900,
    user: {
      id: 7,
      name: label,
      email: `${label.toLowerCase()}@example.test`,
      avatarUrl: "https://avatars.example.test/ada.png",
      providers: ["github"],
    },
  };
}

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-desktop-auth-"));
}

function makeAuth(options = {}) {
  return new DesktopAuth({
    accountServer: "https://accounts.example.test",
    storageDir: options.storageDir || tempDir(),
    safeStorage: options.safeStorage || safeStorage(),
    openExternal: options.openExternal || (async () => {}),
    requestJson: options.requestJson || (async () => ({ status: 500, body: {} })),
    sleep: options.sleep || (async () => {}),
    now: options.now || (() => Date.now()),
    signInTimeoutMs: options.signInTimeoutMs || 2_000,
    platform: options.platform,
  });
}

test("account server policy accepts only a secure origin or literal loopback development origin", () => {
  assert.equal(normalizeAccountServer("https://accounts.example.test/"), "https://accounts.example.test");
  assert.equal(normalizeAccountServer("http://127.0.0.1:3000/"), "http://127.0.0.1:3000");
  for (const value of [
    "http://accounts.example.test/",
    "http://localhost:3000/",
    "https://user:secret@accounts.example.test/",
    "https://accounts.example.test/path",
    "https://accounts.example.test/?next=x",
    "file:///tmp/account",
  ]) {
    assert.throws(() => normalizeAccountServer(value), DesktopAuthError, value);
  }
});

test("PKCE loopback binds a random 127.0.0.1 callback and never exposes tokens in status", async () => {
  const storageDir = tempDir();
  let opened;
  let exchange;
  const auth = makeAuth({
    storageDir,
    openExternal: async (candidate) => {
      opened = new URL(candidate);
      const redirect = new URL(opened.searchParams.get("redirect_uri"));
      await new Promise((resolve, reject) => {
        const req = http.get(
          `${redirect.toString()}?code=single-use-code&state=${encodeURIComponent(opened.searchParams.get("state"))}`,
          (res) => {
            res.resume();
            res.on("end", resolve);
          }
        );
        req.on("error", reject);
      });
    },
    requestJson: async (request) => {
      exchange = request;
      return { status: 200, body: tokenSet() };
    },
  });

  const signedIn = await auth.signIn();
  assert.equal(signedIn.user.login, "Ada");
  assert.equal(opened.pathname, "/api/desktop/start");
  assert.equal(opened.searchParams.get("code_challenge_method"), "S256");
  assert.match(opened.searchParams.get("code_challenge"), /^[A-Za-z0-9_-]{43}$/);
  assert.match(opened.searchParams.get("state"), /^[A-Za-z0-9_-]{43}$/);
  assert.equal(new URL(opened.searchParams.get("redirect_uri")).hostname, "127.0.0.1");
  assert.equal(exchange.path, "/api/desktop/token");
  assert.equal(exchange.body.code, "single-use-code");
  assert.equal(
    b64url(crypto.createHash("sha256").update(exchange.body.code_verifier).digest()),
    opened.searchParams.get("code_challenge")
  );

  const status = await auth.status();
  assert.deepEqual(status.user, { id: 7, login: "Ada", avatarUrl: "https://avatars.example.test/ada.png" });
  assert.equal(status.sessionPersistence, "encrypted");
  assert.equal(Object.prototype.hasOwnProperty.call(status, "access_token"), false);
  assert.equal(JSON.stringify(status).includes("access-Ada"), false);
  assert.equal(JSON.stringify(status).includes("refresh-Ada"), false);
});

test("loopback callback rejects non-GET, wrong paths, and a mismatched CSRF state", async () => {
  const auth = makeAuth();
  const listener = await auth._startLoopback("expected-state");
  const request = (method, suffix) =>
    new Promise((resolve, reject) => {
      const req = http.request(
        `http://127.0.0.1:${listener.port}${suffix}`,
        { method },
        (res) => {
          res.resume();
          res.on("end", () => resolve(res.statusCode));
        }
      );
      req.on("error", reject);
      req.end();
    });
  const pending = listener.waitForCode(1_000);
  const rejected = assert.rejects(pending, (error) => error && error.code === "state_mismatch");
  assert.equal(await request("POST", "/callback"), 405);
  assert.equal(await request("GET", "/not-callback"), 404);
  assert.equal(await request("GET", "/callback?code=code&state=wrong-state"), 400);
  await rejected;
  await listener.close();
});

test("device flow opens a code-free same-origin verification page and stores an encrypted session", async () => {
  const opened = [];
  const codes = [];
  let polls = 0;
  const auth = makeAuth({
    openExternal: async (url) => opened.push(url),
    requestJson: async (request) => {
      if (request.path === "/api/desktop/device/start") {
        return {
          status: 200,
          body: {
            device_code: "opaque-device-code",
            user_code: "ABCD-1234",
            verification_uri: "https://accounts.example.test/desktop/activate",
            interval: 2,
            expires_in: 600,
          },
        };
      }
      assert.equal(request.path, "/api/desktop/device/token");
      assert.equal(request.body.device_code, "opaque-device-code");
      polls += 1;
      return polls === 1
        ? { status: 400, body: { error: "authorization_pending" } }
        : { status: 200, body: tokenSet("Device") };
    },
  });

  const result = await auth.signInDevice({ onDeviceCode: (value) => codes.push(value) });
  assert.equal(result.user.login, "Device");
  assert.deepEqual(opened, ["https://accounts.example.test/desktop/activate"]);
  assert.deepEqual(codes, [
    { userCode: "ABCD-1234", verificationUri: "https://accounts.example.test/desktop/activate", expiresIn: 600 },
  ]);
  assert.equal(polls, 2);
  assert.equal((await auth.status()).user.login, "Device");
});

test("no encrypted OS storage means a documented memory-only session and no credential file", async () => {
  const storageDir = tempDir();
  const auth = makeAuth({
    storageDir,
    safeStorage: safeStorage(false),
    requestJson: async (request) => {
      if (request.path === "/api/desktop/device/start") {
        return {
          status: 200,
          body: {
            device_code: "opaque-device-code",
            user_code: "ABCD-1234",
            verification_uri: "https://accounts.example.test/desktop/activate",
          },
        };
      }
      return { status: 200, body: tokenSet("Memory") };
    },
  });
  const result = await auth.signInDevice();
  assert.equal(result.sessionPersistence, "memory");
  assert.equal((await auth.status()).sessionPersistence, "memory");
  assert.deepEqual(fs.readdirSync(storageDir), []);
});

test("Linux basic_text safeStorage is never treated as encrypted persistence", async () => {
  const storageDir = tempDir();
  const auth = makeAuth({
    storageDir,
    platform: "linux",
    safeStorage: safeStorage(true, "basic_text"),
    requestJson: async (request) => {
      if (request.path === "/api/desktop/device/start") {
        return {
          status: 200,
          body: {
            device_code: "opaque-device-code",
            user_code: "ABCD-1234",
            verification_uri: "https://accounts.example.test/desktop/activate",
          },
        };
      }
      return { status: 200, body: tokenSet("Linux") };
    },
  });
  const result = await auth.signInDevice();
  assert.equal(result.sessionPersistence, "memory");
  assert.deepEqual(fs.readdirSync(storageDir), []);
});

test("refresh is single-flight, preserves an offline session, and clears only definitive invalid grants", async () => {
  const auth = makeAuth();
  auth.tokens = {
    version: 1,
    access_token: "old-access",
    refresh_token: "old-refresh",
    expires_at: Date.now() + 1_000,
    user: { id: 7, login: "Old" },
  };
  let refreshes = 0;
  auth.requestJson = async (request) => {
    if (request.path === "/api/me") return { status: 401, body: { error: "invalid_token" } };
    if (request.path === "/api/desktop/refresh") {
      refreshes += 1;
      await new Promise((resolve) => setTimeout(resolve, 15));
      return { status: 200, body: tokenSet("Fresh") };
    }
    throw new Error(`unexpected ${request.path}`);
  };
  const [one, two] = await Promise.all([auth.getUser(), auth.getUser()]);
  assert.equal(one.login, "Fresh");
  assert.equal(two.login, "Fresh");
  assert.equal(refreshes, 1, "a rotating refresh token must not be used concurrently");

  auth.requestJson = async (request) => {
    if (request.path === "/api/me") return { status: 401, body: { error: "invalid_token" } };
    return { status: 401, body: { error: "invalid_grant" } };
  };
  assert.equal(await auth.getUser(), null);
  assert.equal((await auth.status()).user, null);
});

test("sign-out revokes internally and only returns public null state", async () => {
  const auth = makeAuth();
  auth.tokens = {
    version: 1,
    access_token: "access-private",
    refresh_token: "refresh-private",
    expires_at: Date.now() + 1_000,
    user: { id: 7, login: "Ada" },
  };
  let revoke;
  auth.requestJson = async (request) => {
    revoke = request;
    return { status: 200, body: { ok: true } };
  };
  assert.deepEqual(await auth.signOut(), { user: null });
  assert.equal(revoke.path, "/api/desktop/revoke");
  assert.equal(revoke.bearer, "access-private");
  assert.equal(revoke.body.refresh_token, "refresh-private");
  assert.deepEqual(await auth.status(), { enabled: true, user: null, sessionPersistence: "encrypted" });
});

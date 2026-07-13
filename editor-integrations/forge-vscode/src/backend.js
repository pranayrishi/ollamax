// @ts-check
"use strict";

// ForgeBackend — owns the lifecycle of the `forge serve` process and talks to
// it over HTTP/SSE. This runs in the *extension host* (Node.js), which has
// full network access; the webview never touches the network (see the CSP in
// chatViewProvider.js). Communication is therefore:
//
//   webview  <-- postMessage -->  extension host (this layer)  <-- HTTP/SSE -->  forge serve
//
// We use Node's built-in `http` and `child_process` only — no npm
// dependencies, so the extension runs with zero `npm install`.

const vscode = require("vscode");
const http = require("http");
const cp = require("child_process");
const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

class ForgeBackend {
  /** @param {(msg: string) => void} log */
  constructor(log) {
    this.log = log;
    /** @type {import('child_process').ChildProcess | null} */
    this.child = null;
    /** @type {string | null} */
    this.baseUrl = null;
    /** @type {string | null} Canonical root the backend has been verified for. */
    this.workspaceRoot = null;
    /** @type {boolean} Whether this extension owns the current server process. */
    this.managed = false;
    /** @type {number | null} Configured external server port, if any. */
    this.externalPort = null;
    /** @type {string | null} In-memory local-server bearer token. Never logged. */
    this.serverToken = null;
    /** @type {string | null} Optional capability advertised by newer engines. */
    this.capability = null;
    /** Serialize lifecycle changes so a workspace switch cannot race a launch. */
    this.lifecycle = Promise.resolve();
    /** Invalidates ready/exit callbacks from a server that has been replaced. */
    this.generation = 0;
    this.disposed = false;
  }

  isReady() {
    return !!this.baseUrl;
  }

  /** @param {() => Promise<any>} operation */
  _serial(operation) {
    const next = this.lifecycle.then(operation, operation);
    // Keep the queue usable after a failed launch while preserving the error to
    // the caller that requested it.
    this.lifecycle = next.catch(() => undefined);
    return next;
  }

  /** @param {string | null | undefined} root */
  _normalizeWorkspaceRoot(root) {
    if (!root) return null;
    let normalized = path.resolve(String(root));
    // The server canonicalizes its workspace before allowing filesystem tools.
    // Match that behavior here, including a linked VS Code workspace folder.
    try {
      normalized = fs.realpathSync.native
        ? fs.realpathSync.native(normalized)
        : fs.realpathSync(normalized);
    } catch {
      // A folder can disappear during a workspace-change event. The following
      // server verification still prevents the Agent from using a stale root.
    }
    return process.platform === "win32" ? normalized.toLowerCase() : normalized;
  }

  /** The one explicit root the Agent is allowed to work in (first VS Code folder). */
  currentWorkspaceRoot() {
    const folders = vscode.workspace.workspaceFolders;
    return folders && folders[0]
      ? this._normalizeWorkspaceRoot(folders[0].uri.fsPath)
      : null;
  }

  /** @param {string | null | undefined} a @param {string | null | undefined} b */
  _sameWorkspace(a, b) {
    return !!a && !!b && this._normalizeWorkspaceRoot(a) === this._normalizeWorkspaceRoot(b);
  }

  /** @param {string | null | undefined} root */
  isWorkspaceBound(root) {
    return this._sameWorkspace(this.workspaceRoot, root);
  }

  /** @param {any} cfg */
  _configuredToken(cfg) {
    const token = cfg.get("serverToken", "");
    return typeof token === "string" && token.trim() ? token.trim() : null;
  }

  /**
   * Ensure the backend is reachable. Either connects to a user-specified
   * already-running server (`forge.serverPort`) or launches and manages one.
   * A managed server is always launched with the current workspace as its cwd.
   * @param {string | null | undefined} workspaceRoot
   */
  async ensureStarted(workspaceRoot = this.currentWorkspaceRoot()) {
    const desiredRoot = this._normalizeWorkspaceRoot(workspaceRoot);
    return this._serial(async () => {
      if (this.disposed) throw new Error("Ollamax backend has been disposed");
      await this._ensureStarted(desiredRoot);
    });
  }

  /** @param {string | null} desiredRoot */
  async _ensureStarted(desiredRoot) {
    const cfg = vscode.workspace.getConfiguration("forge");
    const configuredPort = Number(cfg.get("serverPort", 0)) || 0;

    if (this.baseUrl) {
      const needsManagedRestart = this.managed && !this._sameWorkspace(this.workspaceRoot, desiredRoot);
      const externalChanged = !this.managed && this.externalPort !== configuredPort;
      if (!needsManagedRestart && !externalChanged) {
        // Pick up a changed configured-server token without requiring a restart.
        if (!this.managed) this.serverToken = this._configuredToken(cfg);
        return;
      }
      this._stopNow();
    }

    if (configuredPort > 0) {
      this.baseUrl = `http://127.0.0.1:${configuredPort}`;
      this.managed = false;
      this.externalPort = configuredPort;
      this.workspaceRoot = null; // verified just before an Agent run
      this.serverToken = this._configuredToken(cfg);
      this.capability = null;
      this.log(`connecting to existing forge serve at ${this.baseUrl}`);
      try {
        await this._waitHealthy(10);
      } catch (error) {
        this._clearConnection();
        throw error;
      }
      return;
    }

    await this._spawn(this._resolveBin(cfg.get("serverPath", "forge")), desiredRoot);
  }

  /**
   * Resolve the `forge` binary to launch. Zero-config in a SHIPPED build: when
   * the user hasn't overridden `forge.serverPath`, prefer a binary bundled INSIDE
   * this extension at `<ext>/bin/forge[.exe]` (staged there by the fork's
   * bundle-forge.sh). This travels with a built-in extension regardless of the
   * app layout, so a packaged app finds the engine with no PATH/config. Falls
   * back to the configured value (or bare `forge` on PATH) for dev installs.
   * @param {string} configured
   */
  _resolveBin(configured) {
    // An explicit, non-default override always wins.
    if (configured && configured !== "forge") return configured;
    const binName = process.platform === "win32" ? "forge.exe" : "forge";
    const bundled = path.join(__dirname, "..", "bin", binName);
    try {
      if (fs.existsSync(bundled)) {
        this.log(`using bundled forge engine: ${bundled}`);
        return bundled;
      }
    } catch {
      /* fall through to PATH */
    }
    return configured || "forge";
  }

  /** @param {string} bin @param {string | null} workspaceRoot */
  _spawn(bin, workspaceRoot) {
    const generation = ++this.generation;
    // A distinct token for every extension-managed server means a different
    // local process cannot silently issue agent actions against this one.
    const token = crypto.randomBytes(32).toString("hex");
    const cwd = workspaceRoot || undefined;
    this.log(
      `launching: ${bin} serve --port 0${workspaceRoot ? ` in ${workspaceRoot}` : ""}`
    );
    const child = cp.spawn(bin, ["serve", "--port", "0"], {
      cwd,
      // Do not mutate process.env: that would leak this server's token to
      // other extension-host children. The token stays only in this backend
      // instance and the spawned engine process.
      env: { ...process.env, FORGE_SERVER_TOKEN: token },
    });
    this.child = child;

    return new Promise((resolve, reject) => {
      let settled = false;
      let stdoutBuf = "";
      let timeout;

      const stale = () => generation !== this.generation || child !== this.child;
      const settle = (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (error) {
          // A timed-out or malformed launch must not leave a child running
          // long enough to emit a late ready line and resurrect stale state.
          if (this.child === child) {
            this.generation += 1;
            this._clearConnection();
            try {
              child.kill();
            } catch {
              /* process already exited */
            }
          }
          reject(error);
        } else {
          resolve(undefined);
        }
      };

      const onReady = (line) => {
        const m = line.match(/FORGE_SERVE_READY\s+(\{.*\})/);
        if (!m) {
          return;
        }
        try {
          const info = JSON.parse(m[1]);
          if (stale()) {
            settle(new Error("forge serve launch was superseded"));
            return;
          }
          if (typeof info.token === "string" && info.token !== token) {
            throw new Error("ready line token did not match the managed server token");
          }
          const port = Number(info.port);
          if (!Number.isInteger(port) || port <= 0 || port > 65535) {
            throw new Error("ready line did not include a valid port");
          }
          this.baseUrl = `http://127.0.0.1:${info.port}`;
          this.workspaceRoot = workspaceRoot;
          this.managed = true;
          this.externalPort = null;
          this.serverToken = token;
          // Keep compatibility with engines that advertise a separate
          // capability, without assuming it is the bearer token.
          this.capability =
            typeof info.capability === "string" && info.capability.trim()
              ? info.capability.trim()
              : null;
          this.log(`forge serve ready on ${this.baseUrl} (v${info.version})`);
          settle();
        } catch (e) {
          this.log(`failed to parse ready line: ${e}`);
          settle(e instanceof Error ? e : new Error(String(e)));
        }
      };

      child.stdout.on("data", (d) => {
        stdoutBuf += d.toString();
        let idx;
        while ((idx = stdoutBuf.indexOf("\n")) !== -1) {
          const line = stdoutBuf.slice(0, idx).trimEnd();
          stdoutBuf = stdoutBuf.slice(idx + 1);
          if (line) {
            // A future ready record may contain authentication metadata. Do
            // not echo it into the ordinary VS Code output channel.
            if (line.includes("FORGE_SERVE_READY")) {
              this.log("[forge:out] Forge server reported ready");
            } else {
              this.log(`[forge:out] ${line}`);
            }
            onReady(line);
          }
        }
      });

      child.stderr.on("data", (d) => {
        this.log(`[forge:err] ${d.toString().trimEnd()}`);
      });

      child.on("exit", (code) => {
        if (this.child === child) {
          this._clearConnection();
        }
        if (!settled) {
          settle(new Error(`forge serve exited (code ${code}) before reporting ready`));
        }
      });

      child.on("error", (err) => {
        if (!settled) {
          settle(
            new Error(
              `could not launch \`${bin}\`: ${err.message}. ` +
                `Set "forge.serverPath" to your built binary (e.g. target/release/forge).`
            )
          );
        }
      });

      timeout = setTimeout(() => {
        if (!settled) {
          settle(new Error("forge serve did not report ready within 20s"));
        }
      }, 20000);
    });
  }

  /** @param {number} attempts */
  async _waitHealthy(attempts) {
    for (let i = 0; i < attempts; i++) {
      try {
        await this.getJson("/health");
        return;
      } catch (_e) {
        await new Promise((r) => setTimeout(r, 300));
      }
    }
    throw new Error(`backend at ${this.baseUrl} did not respond to /health`);
  }

  _clearConnection() {
    this.child = null;
    this.baseUrl = null;
    this.workspaceRoot = null;
    this.managed = false;
    this.externalPort = null;
    this.serverToken = null;
    this.capability = null;
  }

  _stopNow() {
    this.generation += 1;
    const child = this.child;
    this._clearConnection();
    if (child) {
      this.log("stopping forge serve");
      try {
        child.kill();
      } catch {
        /* process already exited */
      }
    }
  }

  stop() {
    this._stopNow();
  }

  dispose() {
    this.disposed = true;
    this._stopNow();
  }

  async restart() {
    const workspaceRoot = this.currentWorkspaceRoot();
    return this._serial(async () => {
      if (this.disposed) throw new Error("Ollamax backend has been disposed");
      this._stopNow();
      await this._ensureStarted(workspaceRoot);
    });
  }

  /**
   * Verify that the server which will receive Agent requests was launched for
   * exactly the open VS Code workspace. External servers are never restarted
   * by the extension; they must expose the same root and token explicitly.
   * @param {string | null | undefined} workspaceRoot
   */
  async ensureWorkspace(workspaceRoot = this.currentWorkspaceRoot()) {
    const expectedRoot = this._normalizeWorkspaceRoot(workspaceRoot);
    if (!expectedRoot) {
      throw new Error("Open a workspace folder before running the Ollamax Agent.");
    }
    const cfg = vscode.workspace.getConfiguration("forge");
    if ((Number(cfg.get("serverPort", 0)) || 0) > 0 && !this._configuredToken(cfg)) {
      throw new Error(
        "Ollamax Agent cannot verify the configured forge.serverPort without \"forge.serverToken\". " +
          "Set it to the same value as FORGE_SERVER_TOKEN on that server, then try again."
      );
    }
    await this.ensureStarted(expectedRoot);
    return this._serial(async () => {
      // A workspace switch between the user's click and this verification must
      // never leave a short window for an Agent to run in the old folder.
      if (!this._sameWorkspace(expectedRoot, this.currentWorkspaceRoot())) {
        throw new Error("The workspace changed while Ollamax was starting. Send the Agent request again.");
      }
      if (!this.baseUrl) await this._ensureStarted(expectedRoot);
      if (!this.managed && !this.serverToken) {
        throw new Error(
          "Ollamax Agent cannot verify the configured forge.serverPort without \"forge.serverToken\". " +
            "Set it to the same value as FORGE_SERVER_TOKEN on that server, then try again."
        );
      }

      let actualRoot;
      try {
        actualRoot = await this._backendWorkspaceRoot();
      } catch (error) {
        if (!this.managed) {
          throw new Error(
            "Ollamax Agent could not authenticate or verify the configured forge.serverPort. " +
              "Check that \"forge.serverToken\" matches that server's FORGE_SERVER_TOKEN. " +
              `Details: ${String(error && error.message ? error.message : error)}`
          );
        }
        throw error;
      }

      if (!this._sameWorkspace(actualRoot, expectedRoot) && this.managed) {
        // The child should already be bound to expectedRoot. If it is not,
        // replace it rather than accepting an ambiguous filesystem boundary.
        this.log("managed backend reported a different workspace; restarting it safely");
        this._stopNow();
        await this._ensureStarted(expectedRoot);
        actualRoot = await this._backendWorkspaceRoot();
      }
      if (!this._sameWorkspace(actualRoot, expectedRoot)) {
        throw new Error(
          `Ollamax Agent blocked this request: backend workspace ${actualRoot || "(unknown)"} ` +
            `does not match the folder you opened (${expectedRoot}). ` +
            "Restart the backend from this workspace, or correct forge.serverPort/serverToken."
        );
      }
      this.workspaceRoot = actualRoot;
      return actualRoot;
    });
  }

  /** Rebind an owned server after VS Code changes workspace folders. */
  async rebindWorkspace() {
    return this._serial(async () => {
      if (this.disposed || !this.baseUrl) return { restarted: false };
      // Read inside the serialized operation so back-to-back folder events
      // cannot resurrect an intermediate workspace after a newer one opened.
      const nextRoot = this.currentWorkspaceRoot();
      if (!this.managed) {
        // The external process is not ours to kill. Invalidate its previous
        // verification so ensureWorkspace must prove the root again.
        this.workspaceRoot = null;
        this.log("workspace folders changed; configured backend will be re-verified before Agent use");
        return { restarted: false, external: true };
      }
      this.log("workspace folders changed; restarting the local backend in the new workspace");
      this._stopNow();
      // Do not start a filesystem-capable server with no opened workspace.
      if (nextRoot) await this._ensureStarted(nextRoot);
      return { restarted: true, workspaceRoot: nextRoot };
    });
  }

  async _backendWorkspaceRoot() {
    const info = await this.getJson("/api/workspace");
    if (!info || typeof info.root !== "string" || !info.root.trim()) {
      throw new Error("backend did not return a workspace root");
    }
    return this._normalizeWorkspaceRoot(info.root);
  }

  // ----- simple JSON calls -----

  /** @param {string} path */
  getJson(path) {
    return this._json("GET", path, null);
  }

  /** @param {string} path @param {any} body */
  postJson(path, body) {
    return this._json("POST", path, body);
  }

  /** @param {string | null} data @param {string | null} accept */
  _headers(data, accept = null) {
    const headers = {};
    if (data !== null) {
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(data);
    }
    if (accept) headers.Accept = accept;
    // The token protects every request, including the GET verification calls.
    // It is intentionally never copied into a log or error string.
    if (this.serverToken) headers["X-Ollamax-Token"] = this.serverToken;
    // Older/newer engines may additionally advertise a capability in the
    // ready record. Retain and forward it without treating it as a token.
    if (this.capability) headers["X-Ollamax-Capability"] = this.capability;
    return headers;
  }

  /** @param {number | undefined} status @param {string} path @param {string} raw */
  _httpError(status, path, raw) {
    let detail = raw.trim();
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed.error === "string") detail = parsed.error;
    } catch {
      /* leave non-JSON response as a short detail */
    }
    if (detail.length > 300) detail = `${detail.slice(0, 300)}…`;
    return new Error(
      `backend request ${path} failed (HTTP ${status || "unknown"})${detail ? `: ${detail}` : ""}`
    );
  }

  /** @param {string} method @param {string} path @param {any} body */
  _json(method, path, body) {
    return new Promise((resolve, reject) => {
      if (!this.baseUrl) {
        reject(new Error("backend not started"));
        return;
      }
      const url = new URL(this.baseUrl + path);
      const data = body ? JSON.stringify(body) : null;
      const req = http.request(
        {
          hostname: url.hostname,
          port: url.port,
          path: url.pathname + url.search,
          method,
          headers: this._headers(data),
        },
        (res) => {
          let raw = "";
          res.setEncoding("utf8");
          res.on("data", (c) => (raw += c));
          res.on("end", () => {
            if (!res.statusCode || res.statusCode < 200 || res.statusCode >= 300) {
              reject(this._httpError(res.statusCode, path, raw));
              return;
            }
            try {
              resolve(JSON.parse(raw || "{}"));
            } catch (e) {
              reject(new Error(`bad JSON from ${path}: ${e}`));
            }
          });
        }
      );
      req.on("error", reject);
      if (data) {
        req.write(data);
      }
      req.end();
    });
  }

  // ----- streaming (SSE) -----

  /**
   * POST `body` to a streaming endpoint and call `onEvent` for each parsed SSE
   * event object. Returns a handle with `.abort()` that destroys the HTTP
   * request (the server detects the dropped connection and stops generating).
   *
   * @param {string} path
   * @param {any} body
   * @param {(ev: any) => void} onEvent
   */
  stream(path, body, onEvent) {
    if (!this.baseUrl) {
      onEvent({ type: "error", message: "backend not started" });
      return { abort() {} };
    }
    const url = new URL(this.baseUrl + path);
    const data = JSON.stringify(body);
    const req = http.request(
      {
        hostname: url.hostname,
        port: url.port,
        path: url.pathname + url.search,
        method: "POST",
        headers: this._headers(data, "text/event-stream"),
      },
      (res) => {
        res.setEncoding("utf8");
        let buf = "";
        if (!res.statusCode || res.statusCode < 200 || res.statusCode >= 300) {
          res.on("data", (chunk) => (buf += chunk));
          res.on("end", () => {
            onEvent({ type: "error", message: this._httpError(res.statusCode, path, buf).message });
            onEvent({ type: "_end" });
          });
          return;
        }
        res.on("data", (chunk) => {
          buf += chunk;
          let idx;
          // SSE frames are separated by a blank line.
          while ((idx = buf.indexOf("\n\n")) !== -1) {
            const frame = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            const dataLine = frame
              .split("\n")
              .find((l) => l.startsWith("data:"));
            if (dataLine) {
              const json = dataLine.slice(5).trim();
              try {
                onEvent(JSON.parse(json));
              } catch (_e) {
                /* ignore malformed frame */
              }
            }
          }
        });
        res.on("end", () => onEvent({ type: "_end" }));
      }
    );
    req.on("error", (err) =>
      onEvent({ type: "error", message: String(err && err.message) })
    );
    req.write(data);
    req.end();

    return {
      abort: () => {
        try {
          req.destroy();
        } catch (_e) {
          /* already gone */
        }
      },
    };
  }

  /** Best-effort server-side cancel for an in-flight request id. */
  async cancel(id) {
    try {
      await this.postJson("/api/cancel", { id });
    } catch (_e) {
      /* server already done or unreachable */
    }
  }
}

module.exports = { ForgeBackend };

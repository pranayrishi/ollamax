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
  }

  isReady() {
    return !!this.baseUrl;
  }

  /**
   * Ensure the backend is reachable. Either connects to a user-specified
   * already-running server (`forge.serverPort`) or launches and manages one.
   * Idempotent: returns immediately if already started.
   */
  async ensureStarted() {
    if (this.baseUrl) {
      return;
    }
    const cfg = vscode.workspace.getConfiguration("forge");
    const port = cfg.get("serverPort", 0);

    if (port && port > 0) {
      this.baseUrl = `http://127.0.0.1:${port}`;
      this.log(`connecting to existing forge serve at ${this.baseUrl}`);
      await this._waitHealthy(10);
      return;
    }

    await this._spawn(this._resolveBin(cfg.get("serverPath", "forge")));
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

  /** @param {string} bin */
  _spawn(bin) {
    this.log(`launching: ${bin} serve --port 0`);
    const child = cp.spawn(bin, ["serve", "--port", "0"], {
      cwd:
        vscode.workspace.workspaceFolders &&
        vscode.workspace.workspaceFolders[0]
          ? vscode.workspace.workspaceFolders[0].uri.fsPath
          : undefined,
      env: process.env,
    });
    this.child = child;

    return new Promise((resolve, reject) => {
      let settled = false;
      let stdoutBuf = "";

      const onReady = (line) => {
        const m = line.match(/FORGE_SERVE_READY\s+(\{.*\})/);
        if (!m) {
          return;
        }
        try {
          const info = JSON.parse(m[1]);
          this.baseUrl = `http://127.0.0.1:${info.port}`;
          this.log(`forge serve ready on ${this.baseUrl} (v${info.version})`);
          settled = true;
          resolve(undefined);
        } catch (e) {
          this.log(`failed to parse ready line: ${e}`);
        }
      };

      child.stdout.on("data", (d) => {
        stdoutBuf += d.toString();
        let idx;
        while ((idx = stdoutBuf.indexOf("\n")) !== -1) {
          const line = stdoutBuf.slice(0, idx).trimEnd();
          stdoutBuf = stdoutBuf.slice(idx + 1);
          if (line) {
            this.log(`[forge:out] ${line}`);
            onReady(line);
          }
        }
      });

      child.stderr.on("data", (d) => {
        this.log(`[forge:err] ${d.toString().trimEnd()}`);
      });

      child.on("exit", (code) => {
        this.child = null;
        this.baseUrl = null;
        if (!settled) {
          settled = true;
          reject(new Error(`forge serve exited (code ${code}) before reporting ready`));
        }
      });

      child.on("error", (err) => {
        if (!settled) {
          settled = true;
          reject(
            new Error(
              `could not launch \`${bin}\`: ${err.message}. ` +
                `Set "forge.serverPath" to your built binary (e.g. target/release/forge).`
            )
          );
        }
      });

      setTimeout(() => {
        if (!settled) {
          settled = true;
          reject(new Error("forge serve did not report ready within 20s"));
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

  stop() {
    if (this.child) {
      this.log("stopping forge serve");
      this.child.kill();
      this.child = null;
    }
    this.baseUrl = null;
  }

  async restart() {
    this.stop();
    await this.ensureStarted();
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
          headers: data
            ? {
                "Content-Type": "application/json",
                "Content-Length": Buffer.byteLength(data),
              }
            : {},
        },
        (res) => {
          let raw = "";
          res.setEncoding("utf8");
          res.on("data", (c) => (raw += c));
          res.on("end", () => {
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
        path: url.pathname,
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Content-Length": Buffer.byteLength(data),
          Accept: "text/event-stream",
        },
      },
      (res) => {
        res.setEncoding("utf8");
        let buf = "";
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

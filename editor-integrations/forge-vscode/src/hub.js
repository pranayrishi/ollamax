// @ts-check
"use strict";

// The Central Hub panel — a SEPARATE Activity Bar container from the chat panel.
// Category cards with "+" that activate a domain "package": the backend compiles
// the category into rules + skills + curated references, and we write those into
// the user's config dirs so the EXISTING steering mechanisms pick them up. This
// is transparent steering, not magic — the panel shows exactly what each package
// injects. "Support these maintainers" is an explicit, opt-in star action.
//
// The Hub reads the PUBLIC catalog from the account server; it never sends
// prompts/code anywhere, and applying a package only writes local config files.

const vscode = require("vscode");
const http = require("http");
const https = require("https");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { URL } = require("url");

// Mirror Rust's `dirs::config_dir()` so we write to the same rules/skills dirs
// the `forge` binary reads (RuleSet::default_dir / SkillsEngine).
function configDir() {
  const home = os.homedir();
  if (process.platform === "darwin") return path.join(home, "Library", "Application Support");
  if (process.platform === "win32") return process.env.APPDATA || path.join(home, "AppData", "Roaming");
  return process.env.XDG_CONFIG_HOME || path.join(home, ".config");
}
const rulesDir = () => path.join(configDir(), "ollama-forge", "rules");
const skillsDir = () => path.join(configDir(), "ollama-forge", "skills");

/** @implements {vscode.WebviewViewProvider} */
class HubViewProvider {
  /**
   * @param {vscode.ExtensionContext} context
   * @param {import('./auth').ForgeAuth} auth
   * @param {(m: string) => void} log
   */
  constructor(context, auth, log, telemetry, backend) {
    this.context = context;
    this.auth = auth;
    this.log = log;
    this.telemetry = telemetry;
    // #7: the catalog is now served by the LOCAL engine (forge serve), so the
    // Hub loads with NO account-server config. The account server is only used
    // for the opt-in starring flow.
    this.backend = backend;
    /** @type {vscode.WebviewView | undefined} */
    this.view = undefined;
  }

  /** @param {vscode.WebviewView} webviewView */
  resolveWebviewView(webviewView) {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.context.extensionUri, "media")],
    };
    webviewView.webview.html = this._html(webviewView.webview);
    webviewView.webview.onDidReceiveMessage((m) => this.onMessage(m));
  }

  post(msg) {
    if (this.view) this.view.webview.postMessage(msg);
  }

  base() {
    return this.auth.accountServerOrNull();
  }

  async onMessage(msg) {
    switch (msg.type) {
      case "ready":
        await this._loadCategories();
        break;
      case "search":
        await this._search(msg.q);
        break;
      case "openPackage":
        await this._openPackage(msg.slug);
        break;
      case "activate":
        await this._activate(msg.slug);
        break;
      case "support":
        await this._support(msg.slug, msg.repos);
        break;
      default:
        break;
    }
  }

  // Catalog comes from the LOCAL engine now — auto-loads, no account server.
  async _loadCategories() {
    try {
      await this.backend.ensureStarted();
      const data = await this.backend.getJson("/api/hub/categories");
      this.post({ type: "categories", categories: data.categories || [] });
    } catch (e) {
      this.post({ type: "error", message: `Could not load Hub catalog: ${e && e.message}` });
    }
  }

  // #7: intent-aware search via the engine — loose queries return sensible hits.
  async _search(q) {
    if (!q || !q.trim()) {
      await this._loadCategories();
      return;
    }
    try {
      await this.backend.ensureStarted();
      const data = await this.backend.getJson(`/api/hub/search?q=${encodeURIComponent(q)}`);
      this.post({ type: "categories", categories: data.categories || [] });
    } catch (e) {
      this.post({ type: "error", message: `Search failed: ${e && e.message}` });
    }
  }

  async _openPackage(slug) {
    try {
      await this.backend.ensureStarted();
      const pkg = await this.backend.getJson(`/api/hub/package/${encodeURIComponent(slug)}`);
      this.post({ type: "package", pkg });
    } catch (e) {
      this.post({ type: "error", message: `Could not load package: ${e && e.message}` });
    }
  }

  // Apply a package: write its rules + skills into the local config dirs. This
  // is the entirety of "activation" — transparent, inspectable, reversible.
  async _activate(slug) {
    let pkg;
    try {
      await this.backend.ensureStarted();
      pkg = await this.backend.getJson(`/api/hub/package/${encodeURIComponent(slug)}`);
    } catch (e) {
      this.post({ type: "error", message: `Activate failed: ${e && e.message}` });
      return;
    }
    try {
      fs.mkdirSync(rulesDir(), { recursive: true });
      fs.mkdirSync(skillsDir(), { recursive: true });
      // Never trust a server-supplied string as a filesystem path: take the
      // basename, allow only [\w.-], and confirm the resolved path stays inside
      // the target dir (defense-in-depth even though the server hard-codes names).
      const safeBase = (s) => {
        const b = path.basename(String(s || ""));
        return /^[\w.-]+$/.test(b) && b !== "." && b !== ".." ? b : null;
      };
      const within = (dir, file) =>
        path.resolve(dir, file).startsWith(path.resolve(dir) + path.sep);

      // Rules → one markdown file in the rules dir (picked up by rules_suffix).
      const rulesFile = `hub-${safeBase(slug) || "package"}.md`;
      if (within(rulesDir(), rulesFile)) {
        fs.writeFileSync(path.join(rulesDir(), rulesFile), pkg.rules || "", "utf8");
      }
      // Skills → one JSON per scaffold recipe (loaded by SkillsEngine).
      let skillCount = 0;
      for (const skill of pkg.skills || []) {
        const base = safeBase(skill && skill.name);
        if (!base) continue;
        const file = `${base}.json`;
        if (!within(skillsDir(), file)) continue;
        fs.writeFileSync(path.join(skillsDir(), file), JSON.stringify(skill, null, 2), "utf8");
        skillCount++;
      }
      if (this.telemetry) this.telemetry.track({ type: "hub_activate" });
      this.post({
        type: "activated",
        slug,
        name: pkg.name,
        counts: { rules: (pkg.counts && pkg.counts.rules) || 0, skills: skillCount, references: (pkg.references || []).length },
      });
      vscode.window.showInformationMessage(
        `Hub: activated "${pkg.name}" — ${pkg.counts.rules} rules + ${skillCount} skills now steer the agent.`
      );
    } catch (e) {
      this.post({ type: "error", message: `Writing package failed: ${e && e.message}` });
    }
  }

  // Opt-in "Support these maintainers": create a star intent (needs the user's
  // app token), then open the review/consent page in the browser. We NEVER star
  // automatically — the user reviews and confirms in the browser.
  async _support(slug, repos) {
    const base = this.base();
    if (!base) return;
    const token = await this.auth.getAccessToken().catch(() => null);
    if (!token) {
      vscode.window.showWarningMessage("Sign in with GitHub first to support maintainers.");
      this.post({ type: "needsSignIn" });
      return;
    }
    try {
      const res = await this._post(`${base}/api/star/intent`, { repos, category: slug }, token);
      if (res.url) {
        await vscode.env.openExternal(vscode.Uri.parse(res.url));
        vscode.window.showInformationMessage(
          "Hub: opened the browser to review and star the repos (optional, opt-in)."
        );
      } else {
        this.post({ type: "error", message: "Could not create the support request." });
      }
    } catch (e) {
      this.post({ type: "error", message: `Support failed: ${e && e.message}` });
    }
  }

  // ---- http helpers ----

  _get(urlStr) {
    return this._request("GET", urlStr, null, null);
  }
  _post(urlStr, body, bearer) {
    return this._request("POST", urlStr, body, bearer);
  }
  _request(method, urlStr, body, bearer) {
    const url = new URL(urlStr);
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
            if ((res.statusCode || 0) >= 400) {
              reject(new Error(`HTTP ${res.statusCode}`));
              return;
            }
            try {
              resolve(raw ? JSON.parse(raw) : {});
            } catch (e) {
              reject(new Error("bad JSON"));
            }
          });
        }
      );
      req.on("error", reject);
      if (data) req.write(data);
      req.end();
    });
  }

  _html(webview) {
    const nonce = nonce32();
    const mediaUri = (f) =>
      webview.asWebviewUri(vscode.Uri.joinPath(this.context.extensionUri, "media", f));
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} https: data:`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
      "connect-src 'none'",
    ].join("; ");
    return `<!DOCTYPE html>
<html lang="en"><head>
<meta charset="UTF-8" />
<meta http-equiv="Content-Security-Policy" content="${csp}" />
<link href="${mediaUri("hub.css")}" rel="stylesheet" />
<title>Hub</title></head>
<body>
  <header class="hub-top">
    <input id="search" type="search" placeholder="Search domains…" />
  </header>
  <div id="status" class="hub-status"></div>
  <div id="grid" class="hub-grid"></div>
  <div id="detail" class="hub-detail" hidden></div>
  <script nonce="${nonce}" src="${mediaUri("hub.js")}"></script>
</body></html>`;
  }
}

function nonce32() {
  const c = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += c.charAt(Math.floor(Math.random() * c.length));
  return s;
}

module.exports = { HubViewProvider };

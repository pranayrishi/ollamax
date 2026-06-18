// @ts-check
"use strict";

const vscode = require("vscode");
const path = require("path");
const { languageFromExt } = require("./telemetry");

// Infer language from the first attached file's EXTENSION only (metadata).
function langFromContext(context) {
  if (!Array.isArray(context)) return null;
  for (const c of context) {
    const lang = languageFromExt(c && c.path);
    if (lang) return lang;
  }
  return null;
}

// Max characters of a single attached file we forward to the backend. Keeps
// the model's context from being blown by one giant file; the backend also
// budgets/truncates downstream.
const MAX_ATTACH_CHARS = 16000;

/**
 * Provides the side-docked chat webview and wires it to the ForgeBackend.
 *
 * Message routing:
 *   webview  --postMessage-->  this.onMessage()  --HTTP/SSE-->  backend
 *   backend  --SSE events-->   this.post()       --postMessage-->  webview
 *
 * @implements {vscode.WebviewViewProvider}
 */
class ChatViewProvider {
  /**
   * @param {vscode.ExtensionContext} context
   * @param {import('./backend').ForgeBackend} backend
   * @param {(m: string) => void} log
   */
  constructor(context, backend, log, auth, telemetry) {
    this.context = context;
    this.backend = backend;
    this.log = log;
    this.auth = auth;
    this.telemetry = telemetry;
    /** @type {vscode.WebviewView | undefined} */
    this.view = undefined;
    /** @type {{ id: string, handle: { abort: () => void } } | null} */
    this.current = null;
  }

  /** @param {vscode.WebviewView} webviewView */
  resolveWebviewView(webviewView) {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.context.extensionUri, "media"),
      ],
    };
    webviewView.webview.html = this._html(webviewView.webview);
    webviewView.webview.onDidReceiveMessage((m) => this.onMessage(m));
  }

  /** @param {any} msg */
  post(msg) {
    if (this.view) {
      this.view.webview.postMessage(msg);
    }
  }

  newChat() {
    if (this.current) {
      this.current.handle.abort();
      this.backend.cancel(this.current.id);
      this.current = null;
    }
    this.post({ type: "newChat" });
  }

  async restartBackend() {
    try {
      await this.backend.restart();
      await this._sendStatusAndModels();
      vscode.window.showInformationMessage("Ollama-Forge backend restarted.");
    } catch (e) {
      this.post({ type: "backendError", message: String(e && e.message) });
    }
  }

  // ----- webview -> extension -----

  /** @param {any} msg */
  async onMessage(msg) {
    switch (msg.type) {
      case "ready":
        await this._boot();
        break;
      case "send":
        await this._handleSend(msg);
        break;
      case "cancel":
        this._handleCancel();
        break;
      case "refresh":
        await this._sendStatusAndModels();
        break;
      case "modelInfo":
        await this._sendModelInfo(msg.name);
        break;
      case "signIn":
        await this.signIn(!!msg.device);
        break;
      case "signOut":
        await this.signOut();
        break;
      case "attachFile":
        await this.attachActiveFile();
        break;
      case "attachSelection":
        await this.attachSelection();
        break;
      case "pickFiles":
        await this._pickFiles();
        break;
      default:
        break;
    }
  }

  async _boot() {
    // Push UI config (read from VSCode settings) before booting the backend so
    // the webview renders correctly from the first frame.
    const cfg = vscode.workspace.getConfiguration("forge");
    this.post({
      type: "config",
      whimsy: cfg.get("statusWhimsy", true),
      accountEnabled: !!(cfg.get("accountServer", "") || "").trim(),
    });
    // Account state is independent of the inference backend — surface it even
    // if Ollama isn't running, and never block on it.
    this._sendAccount();
    // #6: gate the app behind website sign-in (deliberate owner choice — reverses
    // the earlier logged-out-usable default). Offline-graceful: a stored session
    // passes the gate without a network call.
    await this._sendGate();
    try {
      await this.backend.ensureStarted();
      await this._sendStatusAndModels();
    } catch (e) {
      this.post({ type: "backendError", message: String(e && e.message) });
    }
  }

  /** Tell the webview whether to show the chat or the sign-in screen. */
  async _sendGate() {
    // Gating requires an account server to authenticate against. With none
    // configured, gating is impossible — don't brick a local/dev run. The owner
    // turns the gate ON for production by setting `forge.accountServer` (baked
    // into the fork's product/defaults). When set, sign-in is REQUIRED.
    const cfg = vscode.workspace.getConfiguration("forge");
    const accountConfigured = !!(cfg.get("accountServer", "") || "").trim();
    if (!this.auth || !accountConfigured) {
      this.post({ type: "gate", signedIn: true });
      return;
    }
    const signedIn = await this.auth.isSignedIn().catch(() => false);
    this.post({ type: "gate", signedIn, user: signedIn ? await this.auth.cachedUser() : null });
  }

  /** Whether the action path must enforce sign-in (account server present). */
  async _gateBlocks() {
    const cfg = vscode.workspace.getConfiguration("forge");
    const accountConfigured = !!(cfg.get("accountServer", "") || "").trim();
    if (!this.auth || !accountConfigured) return false;
    return !(await this.auth.isSignedIn().catch(() => false));
  }

  // ----- account (identity only; never gates local inference) -----

  async _sendAccount() {
    if (!this.auth) return;
    let user = null;
    const cfg = vscode.workspace.getConfiguration("forge");
    const accountConfigured = !!(cfg.get("accountServer", "") || "").trim();
    try {
      if (accountConfigured) {
        user = await this.auth.getUser();
      }
    } catch (e) {
      this.log(`account check failed: ${e}`);
    }
    this.post({ type: "account", user });
    // #10: getUser() now clears the session only on a DEFINITIVE server sign-out
    // (offline/5xx keep it). If that happened, re-raise the gate immediately
    // rather than waiting for the next action or reboot.
    if (accountConfigured && !user) {
      this.post({ type: "gate", signedIn: false });
    }
  }

  /** @param {boolean} device use the device-code fallback flow */
  async signIn(device) {
    if (!this.auth) return;
    try {
      const user = device ? await this.auth.signInDevice() : await this.auth.signIn();
      this.post({ type: "account", user });
      // #6: opening the gate the moment sign-in succeeds.
      this.post({ type: "gate", signedIn: !!user, user: user || null });
      if (user) {
        vscode.window.showInformationMessage(`Ollama-Forge: signed in as @${user.login}.`);
      }
    } catch (e) {
      const msg = String(e && e.message);
      this.post({ type: "account", user: null });
      vscode.window.showErrorMessage(`Ollama-Forge sign-in failed: ${msg}`);
    }
  }

  async signOut() {
    if (!this.auth) return;
    try {
      await this.auth.signOut();
    } catch (e) {
      this.log(`sign-out error: ${e}`);
    }
    this.post({ type: "account", user: null });
    // #6: signing out drops back to the gate.
    this.post({ type: "gate", signedIn: false, user: null });
  }

  async _sendStatusAndModels() {
    try {
      const models = await this.backend.getJson("/api/models");
      this.post({ type: "models", models: models.models || [], default: models.default });
    } catch (e) {
      this.log(`models fetch failed: ${e}`);
    }
    try {
      const status = await this.backend.getJson("/api/status");
      this.post({ type: "status", status });
    } catch (e) {
      this.log(`status fetch failed: ${e}`);
    }
  }

  /** Fetch local context-window + capability metadata for one model. */
  async _sendModelInfo(name) {
    if (!name) return;
    try {
      const info = await this.backend.getJson(
        `/api/model_info?name=${encodeURIComponent(name)}`
      );
      this.post({ type: "modelInfo", info });
    } catch (e) {
      this.log(`model_info fetch failed: ${e}`);
    }
  }

  /** @param {any} msg */
  async _handleSend(msg) {
    // #6: enforce the gate on the action path too — not just at boot — so the
    // app can't be driven without an account session even if the UI is bypassed.
    if (await this._gateBlocks()) {
      this.post({ type: "gate", signedIn: false });
      this.post({ type: "backendError", message: "Sign in with your Ollama-Forge account to use the app." });
      return;
    }
    try {
      await this.backend.ensureStarted();
    } catch (e) {
      this.post({ type: "backendError", message: String(e && e.message) });
      return;
    }

    const id = `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    const context = Array.isArray(msg.context) ? msg.context : [];
    let path0;
    let body;

    if (msg.mode === "agent") {
      path0 = "/api/research";
      body = { id, question: msg.text, model: msg.model, context };
    } else if (msg.mode === "build") {
      path0 = "/api/build";
      body = { id, task: msg.text, output_dir: msg.outputDir || null };
    } else {
      path0 = "/api/chat";
      // 3-TAB REFRAME: Chat is now PURE-LOCAL, zero-egress, single-model quick
      // Q&A — tools are always OFF here. Tool-using / autonomous work lives in the
      // Agent tab (/api/research → the full run_agent_streamed with memory,
      // skills, file/shell/MCP tools, delegation, and the scheduler). This makes
      // the three tabs genuinely distinct (talk / act-autonomously / build).
      body = { id, model: msg.model, messages: msg.messages || [], context, tools: false };
    }

    // Telemetry context (metadata only): the language is inferred from an
    // attached file's EXTENSION — never its path or contents.
    const lang = langFromContext(context);
    let lastModel = msg.model === "auto" ? null : msg.model;

    const handle = this.backend.stream(path0, body, (ev) => {
      if (ev.type === "_end") {
        // Stream socket closed; the explicit done/error/cancelled event has
        // already been forwarded. Clear the in-flight handle.
        if (this.current && this.current.id === id) {
          this.current = null;
        }
        this.post({ type: "streamEnd", id });
        return;
      }
      // Usage metadata (respects the telemetry toggle inside track()).
      if (ev.type === "meta") {
        if (ev.routing && ev.routing.model) lastModel = ev.routing.model;
        else if (ev.model) lastModel = ev.model;
        if (ev.routing && ev.routing.auto && this.telemetry) {
          this.telemetry.track({ type: "route", model: lastModel, provider: "ollama", language: lang });
        }
      } else if (ev.type === "result" && this.telemetry) {
        this.telemetry.track({
          type: "build",
          model: ev.model || lastModel,
          provider: "ollama",
          tokensOut: typeof ev.tokens === "number" ? ev.tokens : undefined,
          language: lang,
        });
      } else if (ev.type === "done" && msg.mode !== "build" && this.telemetry) {
        this.telemetry.track({
          type: msg.mode === "agent" ? "agent" : "chat",
          model: lastModel,
          provider: "ollama",
          language: lang,
        });
      }
      this.post({ type: "stream", id, ev });
      if (ev.type === "done" || ev.type === "error" || ev.type === "cancelled") {
        if (this.current && this.current.id === id) {
          this.current = null;
        }
      }
    });

    this.current = { id, handle };
  }

  _handleCancel() {
    if (!this.current) {
      return;
    }
    const { id, handle } = this.current;
    this.backend.cancel(id); // server-side stop
    handle.abort(); // drop the socket
    this.current = null;
    this.post({ type: "stream", id, ev: { type: "cancelled" } });
  }

  // ----- editor context -----

  async attachActiveFile() {
    const ed = vscode.window.activeTextEditor;
    if (!ed) {
      vscode.window.showWarningMessage("Ollama-Forge: no active editor to attach.");
      return;
    }
    const item = this._fileItem(ed.document.uri, ed.document.getText());
    this.post({ type: "context", items: [item] });
    await this._focus();
  }

  async attachSelection() {
    const ed = vscode.window.activeTextEditor;
    if (!ed || ed.selection.isEmpty) {
      vscode.window.showWarningMessage("Ollama-Forge: no selection to attach.");
      return;
    }
    const text = ed.document.getText(ed.selection);
    const rel = this._rel(ed.document.uri);
    const start = ed.selection.start.line + 1;
    const end = ed.selection.end.line + 1;
    this.post({
      type: "context",
      items: [
        {
          path: `${rel}:${start}-${end}`,
          content: text.slice(0, MAX_ATTACH_CHARS),
          label: `${path.basename(rel)}:${start}-${end}`,
        },
      ],
    });
    await this._focus();
  }

  async _pickFiles() {
    const uris = await vscode.workspace.findFiles(
      "**/*",
      "**/{node_modules,target,.git,dist,build,.venv,venv}/**",
      2000
    );
    if (uris.length === 0) {
      return;
    }
    const picks = uris.map((u) => ({
      label: this._rel(u),
      uri: u,
    }));
    const chosen = await vscode.window.showQuickPick(picks, {
      canPickMany: true,
      placeHolder: "Select files/folders to attach as @-context",
    });
    if (!chosen || chosen.length === 0) {
      return;
    }
    const items = [];
    for (const c of chosen) {
      try {
        const bytes = await vscode.workspace.fs.readFile(c.uri);
        const text = Buffer.from(bytes).toString("utf8");
        items.push(this._fileItem(c.uri, text));
      } catch (e) {
        this.log(`could not read ${c.label}: ${e}`);
      }
    }
    if (items.length > 0) {
      this.post({ type: "context", items });
    }
  }

  /** @param {vscode.Uri} uri @param {string} text */
  _fileItem(uri, text) {
    const rel = this._rel(uri);
    return {
      path: rel,
      content: text.slice(0, MAX_ATTACH_CHARS),
      label: path.basename(rel),
    };
  }

  /** @param {vscode.Uri} uri */
  _rel(uri) {
    const folders = vscode.workspace.workspaceFolders;
    if (folders && folders.length > 0) {
      return path.relative(folders[0].uri.fsPath, uri.fsPath) || path.basename(uri.fsPath);
    }
    return path.basename(uri.fsPath);
  }

  async _focus() {
    try {
      await vscode.commands.executeCommand("forge.chatView.focus");
    } catch (_e) {
      /* view not registered yet */
    }
  }

  // ----- html -----

  /** @param {vscode.Webview} webview */
  _html(webview) {
    const nonce = nonce32();
    const mediaUri = (f) =>
      webview.asWebviewUri(
        vscode.Uri.joinPath(this.context.extensionUri, "media", f)
      );
    const cssUri = mediaUri("main.css");
    const jsUri = mediaUri("main.js");
    // Strict CSP. `connect-src 'none'` is deliberate: the webview never makes
    // network calls — the extension host does. This is a hard guarantee that
    // the chat UI itself cannot phone home, reinforcing the zero-telemetry
    // promise.
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} https: data:`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
      "connect-src 'none'",
    ].join("; ");

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <link href="${cssUri}" rel="stylesheet" />
  <title>Ollama-Forge</title>
</head>
<body>
  <!-- #6 login gate: covers the whole panel until the user signs in. -->
  <div id="gate" class="gate" hidden>
    <div class="gate-card">
      <div class="gate-logo">⚒</div>
      <h1>Sign in to Ollama-Forge</h1>
      <p class="gate-sub">An Ollama-Forge account is required to use the app. Your code and
        prompts stay on your device — only anonymous usage metadata syncs to your dashboard.</p>
      <button id="gate-signin" class="primary">Sign in with GitHub or Google</button>
      <button id="gate-signin-device" class="linkbtn">Use a device code instead</button>
      <p id="gate-error" class="gate-error" hidden></p>
    </div>
  </div>

  <header id="topbar">
    <div class="modes" role="tablist">
      <button class="mode active" data-mode="chat" title="Quick local Q&A — single model, no tools, nothing leaves your machine">Chat</button>
      <button class="mode" data-mode="agent" title="Autonomous agent — uses tools, memory and skills, can delegate sub-agents and run scheduled tasks (all local)">Agent</button>
      <button class="mode" data-mode="build" title="Parallel multi-model code build/orchestration">Build</button>
    </div>
    <div class="picker">
      <select id="model" title="Installed Ollama model (auto-selected for your hardware)"></select>
      <button id="refresh" class="iconbtn" title="Refresh installed Ollama models (after ollama pull)">⟳</button>
    </div>
  </header>

  <div id="statusline" class="statusline">starting local backend…</div>
  <div id="modelhint" class="modelhint" hidden></div>
  <div id="account" class="account" hidden></div>

  <div id="messages" class="messages" aria-live="polite"></div>

  <div id="queue" class="queue" hidden></div>

  <div id="context" class="context" hidden></div>

  <footer id="composer">
    <div class="attachrow">
      <button class="attach" data-attach="file" title="Attach the active editor file">+ file</button>
      <button class="attach" data-attach="selection" title="Attach the current selection">+ selection</button>
      <button class="attach" data-attach="pick" title="@-mention files from the workspace">@ files</button>
    </div>
    <div class="inputrow">
      <textarea id="input" rows="3" placeholder="Ask anything — runs locally on your hardware. ⏎ to send, ⇧⏎ for newline."></textarea>
      <div class="btns">
        <button id="send" class="send">Send</button>
        <button id="stop" class="stop" hidden>Stop</button>
      </div>
    </div>
  </footer>

  <script nonce="${nonce}" src="${jsUri}"></script>
</body>
</html>`;
  }
}

function nonce32() {
  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) {
    s += chars.charAt(Math.floor(Math.random() * chars.length));
  }
  return s;
}

module.exports = { ChatViewProvider };

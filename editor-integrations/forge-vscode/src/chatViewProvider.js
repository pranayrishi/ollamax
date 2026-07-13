// @ts-check
"use strict";

const vscode = require("vscode");
const path = require("path");
const fs = require("fs");
const os = require("os");
const { languageFromExt } = require("./telemetry");
const { resolveWorkspacePreviewPath } = require("./workspace-preview-paths");

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
    this._saveHistory([]); // #3: a new chat clears this project's persisted history
    this.post({ type: "newChat" });
  }

  // #3 Per-project chat persistence — on-device, workspace-scoped (workspaceState
  // is automatically per-folder), never sent to the backend.
  static get _HISTORY_KEY() {
    return "ollamax.chat.v1";
  }
  _loadHistory() {
    const h = this.context.workspaceState.get(ChatViewProvider._HISTORY_KEY, []);
    return Array.isArray(h) ? h : [];
  }
  _saveHistory(messages) {
    const arr = Array.isArray(messages) ? messages : [];
    // Token-ish budget: keep the most recent turns within ~32k chars (~8k tokens)
    // so a long-lived project doesn't grow unbounded.
    const BUDGET = 32000;
    let total = 0;
    const kept = [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const len = ((arr[i] && arr[i].content) || "").length + 16;
      if (total + len > BUDGET && kept.length) break;
      total += len;
      kept.unshift(arr[i]);
    }
    this.context.workspaceState.update(ChatViewProvider._HISTORY_KEY, kept);
  }

  async restartBackend() {
    try {
      await this.backend.restart();
      await this._sendStatusAndModels();
      vscode.window.showInformationMessage("Ollamax backend restarted.");
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
      case "approve":
        await this._approve(!!msg.decision, msg.approvalId);
        break;
      case "previewEdit":
        await this._previewEdit(msg.tool, msg.args, msg.approvalId);
        break;
      case "persistHistory":
        this._saveHistory(msg.messages);
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
    // #3 Per-project persistence: restore this workspace's saved chat history
    // (on-device, workspaceState — never sent to the backend).
    this.post({ type: "restoreHistory", messages: this._loadHistory() });
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

  /** Relay the user's Autonomy-Dial decision to the waiting agent run. */
  // #1 Agentic file edit with SAFE diff/preview. Called when the agent's approval
  // request is for fs_write/fs_edit: path-guard, show the change as a real VS Code
  // diff + a modal BEFORE anything is written, then relay the decision to the
  // agent run (the sandboxed fs tool does the actual write on Approve).
  async _previewEdit(tool, args, approvalId) {
    const folders = vscode.workspace.workspaceFolders;
    const root = folders && folders[0] ? folders[0].uri.fsPath : null;
    if (!root) {
      await this._approve(false, approvalId);
      vscode.window.showWarningMessage("Open a folder before letting the Agent edit files.");
      return;
    }
    const requestedPath = String((args && args.path) || "");
    const resolved = resolveWorkspacePreviewPath(root, requestedPath, {
      // fs_write creates missing parent directories through the engine's
      // descriptor-relative workspace capability. Allow its preview to show
      // a new nested file after we verified the existing path prefix.
      allowMissing: tool === "fs_write",
    });
    if (resolved.error) {
      await this._approve(false, approvalId);
      vscode.window.showWarningMessage(
        `Ollamax blocked a proposed file preview: ${requestedPath || "(empty)"} (${resolved.error}).`
      );
      return;
    }
    const abs = resolved.target;
    const rel = resolved.relative;

    let current = "";
    const isNew = !resolved.exists;
    if (!isNew) {
      try {
        current = fs.readFileSync(abs, "utf8");
      } catch (error) {
        await this._approve(false, approvalId);
        vscode.window.showWarningMessage(
          `Ollamax could not read the proposed edit target ${rel}: ${error && error.message ? error.message : error}`
        );
        return;
      }
    }

    let proposed;
    if (tool === "fs_edit") {
      const oldS = String((args && args.old_string) || "");
      const newS = String((args && args.new_string) || "");
      const occurrences = oldS ? current.split(oldS).length - 1 : 0;
      if (occurrences !== 1) {
        await this._approve(false, approvalId);
        vscode.window.showWarningMessage(
          `Ollamax edit not applied: target text ${occurrences === 0 ? "not found" : "not unique"} in ${rel}.`
        );
        return;
      }
      proposed = current.replace(oldS, newS);
    } else {
      proposed = String((args && args.content) || "");
    }

    // Diff: live file (or an empty temp for new files) vs the proposed content.
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-diff-"));
    const proposedTmp = path.join(tmp, "proposed__" + (path.basename(rel) || "file"));
    fs.writeFileSync(proposedTmp, proposed);
    let leftUri;
    if (isNew) {
      const emptyTmp = path.join(tmp, "empty__" + (path.basename(rel) || "file"));
      fs.writeFileSync(emptyTmp, "");
      leftUri = vscode.Uri.file(emptyTmp);
    } else {
      leftUri = vscode.Uri.file(abs);
    }
    try {
      await vscode.commands.executeCommand(
        "vscode.diff",
        leftUri,
        vscode.Uri.file(proposedTmp),
        `Ollamax ${isNew ? "create" : "edit"}: ${rel}`,
        { preview: true }
      );
    } catch {
      /* diff is best-effort; the modal still gates */
    }

    const choice = await vscode.window.showInformationMessage(
      `Apply Ollamax's ${isNew ? "new file" : "change"} to ${rel}?`,
      {
        modal: true,
        detail: "Review the diff that just opened. Applying writes the file; undo with ⌘Z or VS Code Local History.",
      },
      "Apply",
      "Discard"
    );
    setTimeout(() => {
      try {
        fs.rmSync(tmp, { recursive: true, force: true });
      } catch {}
    }, 20000);

    const apply = choice === "Apply";
    await this._approve(apply, approvalId); // relay to the engine; the fs tool writes on Allow
    if (apply) {
      setTimeout(async () => {
        try {
          const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(abs));
          await vscode.window.showTextDocument(doc, { preview: false });
        } catch {}
      }, 400);
    }
  }

  async _approve(decision, approvalId) {
    if (!this.currentAgentId || typeof approvalId !== "string" || !approvalId) {
      this.log("approval ignored: no active Agent run or approval nonce");
      return;
    }
    try {
      await this.backend.postJson("/api/agent/approve", {
        id: this.currentAgentId,
        approvalId,
        decision,
      });
    } catch (e) {
      this.log(`approve failed: ${e && e.message}`);
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
        vscode.window.showInformationMessage(`Ollamax: signed in as @${user.login}.`);
      }
    } catch (e) {
      const msg = String(e && e.message);
      this.post({ type: "account", user: null });
      vscode.window.showErrorMessage(`Ollamax sign-in failed: ${msg}`);
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
      if (models.error) {
        this.post({
          type: "backendError",
          message:
            `Ollama could not be reached at its configured endpoint. ${models.error} ` +
            "Explicitly configured loopback local endpoints remain selectable. On Windows, verify the Ollama app/service is running and run `Invoke-RestMethod http://127.0.0.1:11434/api/tags`.",
        });
      }
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
      this.post({ type: "backendError", message: "Sign in with your Ollamax account to use the app." });
      return;
    }
    const isWorkspaceMode = msg.mode === "agent" || msg.mode === "team";
    const folders = vscode.workspace.workspaceFolders;
    const workspaceRoot = folders && folders[0] ? folders[0].uri.fsPath : null;
    if (isWorkspaceMode && !workspaceRoot) {
      const message = "Open a workspace folder before running an Ollamax Agent or Team task. It only edits the folder you explicitly opened.";
      this.post({ type: "backendError", message });
      vscode.window.showWarningMessage(message);
      return;
    }
    try {
      // Agent runs get a stronger preflight than Ask: the server must prove it
      // was launched for this exact VS Code folder before it sees the prompt.
      if (isWorkspaceMode) {
        await this.backend.ensureWorkspace(workspaceRoot);
        if (!this.backend.isWorkspaceBound(workspaceRoot)) {
          throw new Error("Ollamax Agent blocked this request because the backend is not bound to the open workspace.");
        }
      } else {
        await this.backend.ensureStarted();
      }
    } catch (e) {
      const message = String(e && e.message);
      this.post({ type: "backendError", message });
      if (isWorkspaceMode) vscode.window.showWarningMessage(message);
      return;
    }

    const id = `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    const context = Array.isArray(msg.context) ? msg.context : [];
    let path0;
    let body;

    if (msg.mode === "agent") {
      path0 = "/api/research";
      // Autonomy Dial: "auto" | "confirm" | "readonly" (default confirm-each so a
      // first-timer is asked before the agent writes/executes anything).
      this.currentAgentId = id;
      body = {
        id,
        question: msg.text,
        model: msg.model,
        context,
        autonomy: msg.autonomy || "confirm",
      };
    } else if (msg.mode === "team") {
      path0 = "/api/team";
      this.currentAgentId = id;
      body = {
        id,
        task: msg.text,
        model: msg.model,
        context,
        autonomy: msg.autonomy || "confirm",
        parallel_scouts: !!msg.parallelScouts,
      };
    } else {
      // TWO TABS (Build removed): Chat = "Ask" — PURE-LOCAL, zero-egress,
      // single-model, tools OFF, READ-ONLY (never edits files). Autonomous,
      // tool-using, file-editing work lives in the Agent tab (/api/research →
      // run_agent_streamed with memory, skills, file/shell/MCP tools, delegation,
      // scheduler). Build's multi-model orchestration is folding into Agent.
      path0 = "/api/chat";
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
        if (this.currentAgentId === id) this.currentAgentId = null;
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
          type: msg.mode === "agent" ? "agent" : msg.mode === "team" ? "team" : "chat",
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
        if (this.currentAgentId === id) this.currentAgentId = null;
      }
    });

    this.current = { id, handle };
  }

  /** Stop an in-flight response before the backend is rebound to another folder. */
  workspaceChanged() {
    if (!this.current) return;
    const { id, handle } = this.current;
    // Ask the old server to stop before the extension tears down or invalidates
    // its workspace binding. Dropping the stream alone is not enough for an
    // external configured server that the extension does not own.
    this.backend.cancel(id);
    try {
      handle.abort();
    } catch {
      /* request is already closed */
    }
    this.current = null;
    this.currentAgentId = null;
    this.post({ type: "stream", id, ev: { type: "cancelled", reason: "workspaceChanged" } });
  }

  _handleCancel() {
    if (!this.current) {
      return;
    }
    const { id, handle } = this.current;
    this.backend.cancel(id); // server-side stop
    handle.abort(); // drop the socket
    this.current = null;
    this.currentAgentId = null;
    this.post({ type: "stream", id, ev: { type: "cancelled" } });
  }

  // ----- editor context -----

  async attachActiveFile() {
    const ed = vscode.window.activeTextEditor;
    if (!ed) {
      vscode.window.showWarningMessage("Ollamax: no active editor to attach.");
      return;
    }
    const item = this._fileItem(ed.document.uri, ed.document.getText());
    this.post({ type: "context", items: [item] });
    await this._focus();
  }

  async attachSelection() {
    const ed = vscode.window.activeTextEditor;
    if (!ed || ed.selection.isEmpty) {
      vscode.window.showWarningMessage("Ollamax: no selection to attach.");
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
  <title>Ollamax</title>
</head>
<body>
  <!-- #6 login gate: covers the whole panel until the user signs in. -->
  <div id="gate" class="gate" hidden>
    <div class="gate-card">
      <div class="gate-logo">⚒</div>
      <h1>Sign in to Ollamax</h1>
      <p class="gate-sub">An Ollamax account is required to use the app. Your code and
        prompts stay on your device — only anonymous usage metadata syncs to your dashboard.</p>
      <button id="gate-signin" class="primary">Sign in with GitHub</button>
      <button id="gate-signin-device" class="linkbtn" hidden>Use a device code instead</button>
      <p id="gate-error" class="gate-error" hidden></p>
    </div>
  </div>

  <header id="topbar">
    <div class="modes" role="tablist">
      <button class="mode active" data-mode="agent" title="Agent — inspects your workspace, runs multi-step tasks, and edits files with your approval.">Agent</button>
      <button class="mode" data-mode="team" title="Team — read-only scouts, one controlled writer, deterministic verification, and review.">Team</button>
      <button class="mode" data-mode="chat" title="Ask — conversational Q&A. Read-only: never changes your files.">Ask</button>
    </div>
    <div class="picker">
      <select id="autonomy" title="Autonomy Dial — how much the agent does before asking you">
        <option value="confirm">Confirm each action</option>
        <option value="auto">Act autonomously</option>
        <option value="readonly">Read-only (no writes)</option>
      </select>
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
    <textarea id="input" rows="3" placeholder="Tell the agent what to change — it works locally and asks before edits. ⏎ to send, ⇧⏎ for newline."></textarea>
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

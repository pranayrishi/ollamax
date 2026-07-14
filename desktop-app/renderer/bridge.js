// Bridge: the "host" side of the existing panel's message protocol, but talking
// to `forge serve` directly over HTTP/SSE instead of the VS Code extension host.
// This is the SAME logic chatViewProvider.js used — reused, not rebuilt — so the
// existing media/main.js runs unchanged on top of the vscode-shim.
//
// The UI posts via acquireVsCodeApi().postMessage(); the shim routes that to
// __forgeBridge.handle(). We reply with window.postMessage({...}) so the UI's
// `message` listener receives it exactly as it did from the extension.

(function () {
  let baseUrl = null;
  let accountServer = "";
  let apiToken = "";
  let workspaceReady = false;
  let current = null; // { id, ctrl, baseUrl }
  let reviewInFlight = null;

  const post = (m) => window.postMessage(m, "*");
  const newId = () => `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

  function clearCurrent(run) {
    if (current === run) current = null;
  }

  function applyConfig(cfg) {
    const previousBaseUrl = baseUrl;
    baseUrl = (cfg && cfg.baseUrl) || null;
    accountServer = (cfg && cfg.accountServer) || "";
    apiToken = (cfg && cfg.apiToken) || "";
    workspaceReady = !!(cfg && cfg.workspaceReady);

    // A folder switch replaces the server process/port. End the old stream
    // locally rather than allowing its late events to affect the new session.
    if (previousBaseUrl && previousBaseUrl !== baseUrl && current) {
      const run = current;
      current = null;
      run.ctrl.abort();
      post({ type: "stream", ev: { type: "cancelled" } });
    }
  }

  async function refreshConfig() {
    const cfg = await window.forgeNative.config();
    applyConfig(cfg);
    return cfg;
  }

  async function init() {
    try {
      await refreshConfig();
    } catch (_) {
      post({ type: "backendError", message: "local engine not running" });
    }
  }
  const ready = init();

  if (window.forgeNative && typeof window.forgeNative.onConfigChanged === "function") {
    window.forgeNative.onConfigChanged(applyConfig);
  }

  // Companion [TASK:...] handoff → prefill the chat input and attach the
  // relevant screenshot. Deliberately NOT auto-sent: the user reviews it.
  if (
    window.forgeNative &&
    window.forgeNative.companion &&
    typeof window.forgeNative.companion.onTask === "function"
  ) {
    window.forgeNative.companion.onTask(({ text, items } = {}) => {
      post({ type: "prefill", text: text || "", items: items || [] });
    });
  }

  async function getJson(path) {
    const target = baseUrl;
    if (!target) throw new Error("local engine not running");
    const r = await fetch(target + path, {
      headers: { "X-Ollamax-Token": apiToken },
    });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  }

  async function streamPost(path, body, id) {
    const ctrl = new AbortController();
    const run = { id, ctrl, baseUrl };
    if (!run.baseUrl) {
      post({ type: "stream", ev: { type: "error", message: "local engine not running" } });
      return;
    }
    current = run;
    try {
      const res = await fetch(run.baseUrl + path, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Ollamax-Token": apiToken },
        body: JSON.stringify(body),
        signal: ctrl.signal,
      });
      if (!res.ok || !res.body) throw new Error(`request failed (HTTP ${res.status})`);
      const reader = res.body.getReader();
      const dec = new TextDecoder();
      let buf = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (current !== run) return;
        buf += dec.decode(value, { stream: true });
        let idx;
        while ((idx = buf.indexOf("\n\n")) !== -1) {
          const block = buf.slice(0, idx);
          buf = buf.slice(idx + 2);
          for (const line of block.split("\n")) {
            const t = line.trimStart();
            if (!t.startsWith("data:")) continue;
            const json = t.slice(5).trim();
            if (!json) continue;
            try {
              if (current === run) post({ type: "stream", ev: JSON.parse(json) });
            } catch (_) {}
          }
        }
      }
    } catch (e) {
      if (!ctrl.signal.aborted && current === run) {
        post({ type: "stream", ev: { type: "error", message: String(e) } });
      }
    } finally {
      clearCurrent(run);
    }
  }

  async function relayApproval(run, approvalId, decision) {
    if (!run || current !== run || !run.baseUrl || typeof approvalId !== "string" || !approvalId) return;
    try {
      await fetch(run.baseUrl + "/api/agent/approve", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Ollamax-Token": apiToken },
        body: JSON.stringify({ id: run.id, approvalId, decision: !!decision }),
      });
    } catch (_) {}
  }

  async function reviewEdit(msg) {
    const run = current;
    if (!run || reviewInFlight) return;
    reviewInFlight = run;
    let decision = false;
    try {
      const review = await window.forgeNative.ide.previewEdit(msg.tool, msg.args);
      decision = !!(review && review.decision);
    } catch (_) {
      // A failed/closed native dialog is a denial, preserving the safe default.
      decision = false;
    }
    await relayApproval(run, msg.approvalId, decision);
    if (reviewInFlight === run) reviewInFlight = null;
  }

  async function handle(msg) {
    await ready;
    switch (msg.type) {
      case "ready":
      case "refresh": {
        try {
          await refreshConfig();
        } catch (_) {}
        post({ type: "config", whimsy: true, accountEnabled: !!accountServer });
        if (msg.type === "ready") post({ type: "account", user: null });
        if (!baseUrl) {
          post({ type: "backendError", message: "local engine not running" });
          break;
        }
        try {
          const m = await getJson("/api/models");
          post({ type: "models", models: m.models || [], default: m.default });
          // The local server returns provider details (including its endpoint)
          // in-band, so do not collapse an Ollama failure into a generic error.
          if (m.error) {
            post({ type: "backendError", message: `Failed to list models from Ollama: ${m.error}` });
          }
        } catch (_) {
          post({ type: "backendError", message: "could not reach the local engine" });
        }
        try {
          const s = await getJson("/api/status");
          post({ type: "status", status: s });
        } catch (_) {}
        break;
      }
      case "modelInfo": {
        if (!msg.name) break;
        try {
          await refreshConfig();
          const info = await getJson(`/api/model_info?name=${encodeURIComponent(msg.name)}`);
          post({ type: "modelInfo", info });
        } catch (_) {}
        break;
      }
      case "send": {
        try {
          await refreshConfig();
        } catch (_) {
          post({ type: "stream", ev: { type: "error", message: "local engine not running" } });
          break;
        }
        if (!baseUrl) {
          post({ type: "stream", ev: { type: "error", message: "local engine not running" } });
          break;
        }
        // Chat remains pure/read-only and usable without a folder. Agent tools
        // are never started until a folder has explicitly restarted the server.
        if ((msg.mode === "agent" || msg.mode === "team") && !workspaceReady) {
          post({
            type: "stream",
            ev: { type: "error", message: "Open a folder in the Editor before running an Agent or Team task." },
          });
          break;
        }
        const id = newId();
        const context = msg.context || [];
        if (msg.mode === "agent") {
          // Autonomy Dial: carry the mode so the engine gates consequential tools
          // + the Plan card (confirm = pause for Run/Approve).
          streamPost(
            "/api/research",
            { id, question: msg.text, model: msg.model, context, autonomy: msg.autonomy || "confirm" },
            id
          );
        } else if (msg.mode === "team") {
          streamPost(
            "/api/team",
            { id, task: msg.text, model: msg.model, context, autonomy: msg.autonomy || "confirm" },
            id
          );
        } else if (msg.mode === "build") {
          streamPost("/api/build", { id, task: msg.text, output_dir: null }, id);
        } else {
          streamPost(
            "/api/chat",
            { id, model: msg.model, messages: msg.messages || [], context, tools: false },
            id
          );
        }
        break;
      }
      case "cancel": {
        const run = current;
        if (run) {
          current = null;
          run.ctrl.abort();
          try {
            await fetch(run.baseUrl + "/api/cancel", {
              method: "POST",
              headers: { "Content-Type": "application/json", "X-Ollamax-Token": apiToken },
              body: JSON.stringify({ id: run.id }),
            });
          } catch (_) {}
          post({ type: "stream", ev: { type: "cancelled" } });
        }
        break;
      }
      case "approve": {
        // Autonomy Dial / Plan card decision → relay to the waiting agent run.
        await relayApproval(current, msg.approvalId, !!msg.decision);
        break;
      }
      case "previewEdit": {
        // The shared chat UI already emits this for fs_write/fs_edit approvals.
        // Main validates/calculates the proposal and owns the native dialog.
        await reviewEdit(msg);
        break;
      }
      case "attachFile":
      case "pickFiles":
      case "attachSelection": {
        // No editor in the standalone app — all three resolve to a file picker.
        const items = await window.forgeNative.pickFiles();
        if (items && items.length) post({ type: "context", items });
        break;
      }
      case "signIn": {
        const r = await window.forgeNative.signIn({ device: !!msg.device });
        if (!r.ok && r.error === "no_account_server") {
          post({ type: "backendError", message: "sign-in needs an account server (FORGE_ACCOUNT_SERVER)" });
        }
        break;
      }
      case "signOut": {
        post({ type: "account", user: null });
        break;
      }
      default:
        break;
    }
  }

  // Register with the shim's multi-bridge dispatcher (chat + hub coexist).
  window.__forgeRegisterBridge(handle);
})();

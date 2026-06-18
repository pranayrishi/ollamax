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
  let current = null; // { id, ctrl }

  const post = (m) => window.postMessage(m, "*");
  const newId = () => `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

  async function init() {
    const cfg = await window.forgeNative.config();
    baseUrl = cfg.baseUrl;
    accountServer = cfg.accountServer || "";
    if (!baseUrl) {
      post({ type: "backendError", message: "local engine not running" });
    }
  }
  const ready = init();

  async function getJson(path) {
    const r = await fetch(baseUrl + path);
    return r.json();
  }

  async function streamPost(path, body, id) {
    const ctrl = new AbortController();
    current = { id, ctrl };
    let res;
    try {
      res = await fetch(baseUrl + path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: ctrl.signal,
      });
    } catch (e) {
      post({ type: "stream", ev: { type: "error", message: String(e) } });
      current = null;
      return;
    }
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
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
              post({ type: "stream", ev: JSON.parse(json) });
            } catch (_) {}
          }
        }
      }
    } catch (_) {
      /* aborted or socket closed; the UI already saw cancelled/done */
    }
    current = null;
  }

  async function handle(msg) {
    await ready;
    switch (msg.type) {
      case "ready":
      case "refresh": {
        post({ type: "config", whimsy: true, accountEnabled: !!accountServer });
        if (msg.type === "ready") post({ type: "account", user: null });
        try {
          const m = await getJson("/api/models");
          post({ type: "models", models: m.models || [], default: m.default });
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
          const info = await getJson(`/api/model_info?name=${encodeURIComponent(msg.name)}`);
          post({ type: "modelInfo", info });
        } catch (_) {}
        break;
      }
      case "send": {
        const id = newId();
        const context = msg.context || [];
        if (msg.mode === "agent") {
          streamPost("/api/research", { id, question: msg.text, model: msg.model, context }, id);
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
        if (current) {
          const id = current.id;
          current.ctrl.abort();
          try {
            await fetch(baseUrl + "/api/cancel", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ id }),
            });
          } catch (_) {}
          post({ type: "stream", ev: { type: "cancelled" } });
        }
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

  window.__forgeBridge = { handle };
})();

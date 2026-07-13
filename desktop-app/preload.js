// Preload — the ONLY privileged surface exposed to the renderer (contextIsolation
// is on, nodeIntegration off). Everything the chat UI needs that requires Node/
// Electron goes through these narrow IPC calls; the rest (HTTP/SSE to the local
// engine) the renderer does itself with fetch.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("forgeNative", {
  // { baseUrl, accountServer, apiToken, workspace, workspaceReady }
  config: () => ipcRenderer.invoke("forge:config"),
  // A workspace switch restarts the local engine on a new ephemeral port.
  onConfigChanged: (cb) => {
    if (typeof cb !== "function") return () => {};
    const listener = (_e, config) => cb(config);
    ipcRenderer.on("forge:configChanged", listener);
    return () => ipcRenderer.removeListener("forge:configChanged", listener);
  },
  // Returns [{ path, label, content }] from a native file picker.
  pickFiles: () => ipcRenderer.invoke("forge:pickFiles"),
  openExternal: (url) => ipcRenderer.invoke("forge:openExternal", url),
  signIn: (opts) => ipcRenderer.invoke("forge:signIn", opts),
  // Explicit, one-shot local voice operations. The renderer never gains a
  // shell, microphone recording is initiated in the renderer after a gesture,
  // and neither operation falls back to a hosted speech service.
  voice: {
    status: () => ipcRenderer.invoke("voice:status"),
    transcribe: (wavBase64) => ipcRenderer.invoke("voice:transcribe", wavBase64),
    speak: (text) => ipcRenderer.invoke("voice:speak", text),
  },
  // Starts a transient, explicit lasso overlay. It returns only a capped crop
  // and textual selection metadata; full-screen captures never reach chat.
  spatial: {
    select: () => ipcRenderer.invoke("spatial:select"),
  },
  // A display-only cursor companion. It can request neither screen capture
  // nor OS actions. `point` accepts a bounded normalized visual cue only; the
  // privileged process independently validates it before it reaches the
  // click-through overlay.
  buddy: {
    setState: (state) => ipcRenderer.invoke("buddy:setState", state),
    point: (directive) => ipcRenderer.invoke("buddy:point", directive),
    onVoiceToggle: (callback) => {
      if (typeof callback !== "function") return () => {};
      const listener = (_event, payload) => callback(payload);
      ipcRenderer.on("buddy:voiceToggle", listener);
      return () => ipcRenderer.removeListener("buddy:voiceToggle", listener);
    },
    onShortcutStatus: (callback) => {
      if (typeof callback !== "function") return () => {};
      const listener = (_event, payload) => callback(payload);
      ipcRenderer.on("buddy:shortcutStatus", listener);
      return () => ipcRenderer.removeListener("buddy:shortcutStatus", listener);
    },
  },
  // Central Hub (#2): catalog read from the account server; activation writes
  // local rules/skills; starring is opt-in (browser review), never automatic.
  hub: {
    categories: () => ipcRenderer.invoke("hub:categories"),
    package: (slug) => ipcRenderer.invoke("hub:package", slug),
    activate: (slug) => ipcRenderer.invoke("hub:activate", slug),
    support: (args) => ipcRenderer.invoke("hub:support", args),
  },
  // IDE workspace (#3): folder/file access scoped to the opened root by the
  // main process + a node-pty-backed integrated terminal. This IPC boundary is
  // not an OS-level filesystem sandbox; symlink-safe agent edits use the Rust
  // engine's descriptor-relative workspace tools instead.
  ide: {
    openFolder: () => ipcRenderer.invoke("ide:openFolder"),
    readDir: (dir) => ipcRenderer.invoke("ide:readDir", dir),
    readFile: (p) => ipcRenderer.invoke("ide:readFile", p),
    writeFile: (p, content) => ipcRenderer.invoke("ide:writeFile", { path: p, content }),
    // Computes and displays a native diff review in the main process; it never
    // writes the proposed content. The bridge relays its decision to the agent.
    previewEdit: (tool, args) => ipcRenderer.invoke("ide:previewEdit", { tool, args }),
  },
  pty: {
    start: (size) => ipcRenderer.invoke("pty:start", size),
    write: (data) => ipcRenderer.send("pty:write", data),
    resize: (cols, rows) => ipcRenderer.send("pty:resize", { cols, rows }),
    kill: () => ipcRenderer.invoke("pty:kill"),
    onData: (cb) => ipcRenderer.on("pty:data", (_e, d) => cb(d)),
    onExit: (cb) => ipcRenderer.on("pty:exit", () => cb()),
  },
});

// Preload — the ONLY privileged surface exposed to the renderer (contextIsolation
// is on, nodeIntegration off). Everything the chat UI needs that requires Node/
// Electron goes through these narrow IPC calls; the rest (HTTP/SSE to the local
// engine) the renderer does itself with fetch.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("forgeNative", {
  // { baseUrl: "http://127.0.0.1:<port>", accountServer: "" }
  config: () => ipcRenderer.invoke("forge:config"),
  // Returns [{ path, label, content }] from a native file picker.
  pickFiles: () => ipcRenderer.invoke("forge:pickFiles"),
  openExternal: (url) => ipcRenderer.invoke("forge:openExternal", url),
  signIn: (opts) => ipcRenderer.invoke("forge:signIn", opts),
});

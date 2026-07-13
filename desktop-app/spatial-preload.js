"use strict";

// The lasso overlay gets exactly three narrow operations. It cannot call the
// normal app IPC surface, inspect the filesystem, or request a screen capture.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("spatialNative", {
  onInit(callback) {
    if (typeof callback !== "function") return () => {};
    const listener = (_event, payload) => callback(payload);
    ipcRenderer.on("spatial:init", listener);
    return () => ipcRenderer.removeListener("spatial:init", listener);
  },
  complete(payload) {
    ipcRenderer.send("spatial:complete", payload);
  },
  cancel(payload) {
    ipcRenderer.send("spatial:cancel", payload);
  },
});

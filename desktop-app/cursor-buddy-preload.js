"use strict";

// The companion is display-only. Its renderer can receive one already
// validated local state from the main process and has no invoke/send surface.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("cursorBuddyNative", {
  onState(callback) {
    if (typeof callback !== "function") return () => {};
    const listener = (_event, payload) => callback(payload);
    ipcRenderer.on("cursor-buddy:state", listener);
    return () => ipcRenderer.removeListener("cursor-buddy:state", listener);
  },
});

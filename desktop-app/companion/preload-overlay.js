// Preload for the companion overlay windows — the ONLY privileged surface the
// transparent overlay renderer gets (contextIsolation on, nodeIntegration
// off). Mic audio is recorded in the renderer and shipped to the main process
// as a WAV ArrayBuffer; everything else is display/state events.
"use strict";

const { contextBridge, ipcRenderer } = require("electron");

function subscribe(channel, cb) {
  if (typeof cb !== "function") return () => {};
  const listener = (_e, payload) => cb(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

contextBridge.exposeInMainWorld("companionOverlay", {
  // ---- events from the companion (main process) ----
  onState: (cb) => subscribe("companion:state", cb),
  onStartRecord: (cb) => subscribe("companion:start-record", cb),
  onStopRecord: (cb) => subscribe("companion:stop-record", cb),
  onDrawMode: (cb) => subscribe("companion:draw-mode", cb),
  onPartial: (cb) => subscribe("companion:partial", cb),
  onFinal: (cb) => subscribe("companion:final", cb),
  onTranscript: (cb) => subscribe("companion:transcript", cb),
  onPoint: (cb) => subscribe("companion:point", cb),
  onTaskOffer: (cb) => subscribe("companion:task-offer", cb),
  onToast: (cb) => subscribe("companion:toast", cb),
  onTurnDone: (cb) => subscribe("companion:turn-done", cb),
  onRegionArmed: (cb) => subscribe("companion:region-armed", cb),
  onSpeakFallback: (cb) => subscribe("companion:speak-fallback", cb),

  // ---- messages to the companion ----
  sendAudio: (wavArrayBuffer) =>
    ipcRenderer.send("companion:audio", { wav: new Uint8Array(wavArrayBuffer) }),
  recordError: (message) => ipcRenderer.send("companion:record-error", { message }),
  sendStroke: (displayId, points) =>
    ipcRenderer.send("companion:stroke", { displayId, points }),
  cancelDraw: () => ipcRenderer.send("companion:draw-cancel"),
  speechDone: () => ipcRenderer.send("companion:speech-done"),
  acceptTask: () => ipcRenderer.send("companion:accept-task"),
  dismissTask: () => ipcRenderer.send("companion:dismiss-task"),
  // Hover-interactivity for the task chip: the overlay stays click-through
  // except while the pointer is over a clickable element.
  setInteractive: (interactive) =>
    ipcRenderer.send("companion:set-interactive", { interactive: !!interactive }),
});

// Ollamax Companion — a voice + screen-context helper that lives in a
// transparent overlay beside the cursor. Fully local and free:
//   speech-to-text  → whisper.cpp (bundled or user-installed)
//   understanding   → the local vision model via `forge serve` → Ollama
//   text-to-speech  → Piper or the OS speech engine (see tts.js)
//
// Pipeline (toggle hotkey): record mic → whisper transcript → screenshot all
// displays → POST /api/chat (SSE) with the companion persona + images →
// stream reply into the overlay bubble → parse [POINT]/[TASK] tags → fly the
// overlay cursor to the target and/or hand the task to the main window →
// speak the reply aloud.
//
// Spatial context (draw hotkey): the overlay becomes interactive, the user
// circles a region with the mouse, the region is cropped from a fresh
// screenshot and attached alongside the full screen; the next voice turn is
// about exactly that region ("replicate this search bar…").
//
// Privacy: audio and screenshots go ONLY to 127.0.0.1 (`forge serve` →
// Ollama). Nothing leaves the machine.
"use strict";

const {
  BrowserWindow,
  desktopCapturer,
  globalShortcut,
  ipcMain,
  screen,
  app,
} = require("electron");
const fs = require("fs");
const http = require("http");
const path = require("path");

const protocol = require("./protocol");
const stt = require("./stt");
const tts = require("./tts");

const DEFAULT_SETTINGS = {
  enabled: true,
  talkShortcut: "Control+Shift+Space",
  drawShortcut: "Control+Shift+D",
  // "" = let the server auto-pick an installed vision model.
  model: "",
  whisperPath: "",
  whisperModel: "",
  ttsEngine: "auto", // auto | piper | system | none
  piperPath: "",
  piperVoice: "",
  systemVoice: "",
  maxHistoryTurns: 12,
  // Longest capture; the toggle hotkey normally ends recording sooner.
  maxRecordSeconds: 60,
};

class Companion {
  /**
   * @param {object} host — accessors into main.js state:
   *   getBaseUrl(): string|null      — the running forge server
   *   getApiToken(): string          — its capability token
   *   getMainWindow(): BrowserWindow — for [TASK] handoffs
   *   resourcesBinDir: string        — bundled bin dir (whisper/piper live here)
   */
  constructor(host) {
    this.host = host;
    this.settingsPath = path.join(app.getPath("userData"), "companion-settings.json");
    this.userModelsDir = path.join(app.getPath("userData"), "companion", "models");
    this.settings = this.loadSettings();
    this.overlays = new Map(); // display.id -> BrowserWindow
    this.state = "idle"; // idle|listening|transcribing|thinking|speaking|drawing
    this.history = []; // [{role, content}]
    this.pendingRegion = null; // {displayId, bbox, pngBase64}
    this.pendingTask = null; // {task, imageBase64}
    this.activeTts = null;
    this.activeRequest = null;
    this.shortcutsRegistered = [];
    this.started = false;
  }

  // ------------------------------------------------------------------
  // Settings
  // ------------------------------------------------------------------
  loadSettings() {
    try {
      const raw = JSON.parse(fs.readFileSync(this.settingsPath, "utf8"));
      return { ...DEFAULT_SETTINGS, ...raw };
    } catch (_) {
      return { ...DEFAULT_SETTINGS };
    }
  }

  saveSettings(next) {
    this.settings = { ...this.settings, ...next };
    try {
      fs.mkdirSync(path.dirname(this.settingsPath), { recursive: true });
      fs.writeFileSync(this.settingsPath, JSON.stringify(this.settings, null, 2), "utf8");
    } catch (_) {}
    this.registerShortcuts();
    this.broadcastState();
  }

  /** Health report for the settings UI: what works, what needs setup. */
  doctor() {
    const whisper = stt.discoverWhisper(this.settings, {
      resourcesBinDir: this.host.resourcesBinDir,
      userModelsDir: this.userModelsDir,
    });
    const ttsInfo = tts.discoverTts(this.settings, {
      resourcesBinDir: this.host.resourcesBinDir,
    });
    return {
      enabled: this.settings.enabled,
      serverReady: !!this.host.getBaseUrl(),
      stt: { ready: !!(whisper.bin && whisper.model), issues: whisper.issues },
      tts: ttsInfo,
      shortcuts: {
        talk: this.settings.talkShortcut,
        draw: this.settings.drawShortcut,
      },
      modelsDir: this.userModelsDir,
    };
  }

  // ------------------------------------------------------------------
  // Lifecycle
  // ------------------------------------------------------------------
  start() {
    if (this.started) return;
    this.started = true;
    try {
      fs.mkdirSync(this.userModelsDir, { recursive: true });
    } catch (_) {}
    this.registerIpc();
    this.registerShortcuts();
    this.createOverlays();
    screen.on("display-added", () => this.recreateOverlays());
    screen.on("display-removed", () => this.recreateOverlays());
    screen.on("display-metrics-changed", () => this.recreateOverlays());
  }

  stop() {
    for (const accel of this.shortcutsRegistered) {
      try {
        globalShortcut.unregister(accel);
      } catch (_) {}
    }
    this.shortcutsRegistered = [];
    for (const w of this.overlays.values()) {
      try {
        w.destroy();
      } catch (_) {}
    }
    this.overlays.clear();
    this.started = false;
  }

  registerShortcuts() {
    for (const accel of this.shortcutsRegistered) {
      try {
        globalShortcut.unregister(accel);
      } catch (_) {}
    }
    this.shortcutsRegistered = [];
    if (!this.settings.enabled) return;
    const bind = (accel, fn) => {
      if (!accel) return;
      try {
        if (globalShortcut.register(accel, fn)) this.shortcutsRegistered.push(accel);
      } catch (_) {}
    };
    bind(this.settings.talkShortcut, () => this.onTalkHotkey());
    bind(this.settings.drawShortcut, () => this.onDrawHotkey());
  }

  // ------------------------------------------------------------------
  // Overlay windows (one transparent click-through window per display)
  // ------------------------------------------------------------------
  createOverlays() {
    for (const display of screen.getAllDisplays()) {
      if (this.overlays.has(display.id)) continue;
      const w = new BrowserWindow({
        x: display.bounds.x,
        y: display.bounds.y,
        width: display.bounds.width,
        height: display.bounds.height,
        frame: false,
        transparent: true,
        hasShadow: false,
        resizable: false,
        movable: false,
        minimizable: false,
        maximizable: false,
        focusable: false,
        skipTaskbar: true,
        alwaysOnTop: true,
        show: this.settings.enabled,
        webPreferences: {
          preload: path.join(__dirname, "preload-overlay.js"),
          contextIsolation: true,
          nodeIntegration: false,
          backgroundThrottling: false,
        },
      });
      w.setAlwaysOnTop(true, "screen-saver");
      w.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
      w.setIgnoreMouseEvents(true, { forward: true });
      w.loadFile(path.join(__dirname, "..", "renderer", "companion-overlay.html"), {
        query: { displayId: String(display.id) },
      });
      w.on("closed", () => this.overlays.delete(display.id));
      this.overlays.set(display.id, w);
    }
    this.broadcastState();
  }

  recreateOverlays() {
    for (const w of this.overlays.values()) {
      try {
        w.destroy();
      } catch (_) {}
    }
    this.overlays.clear();
    if (this.settings.enabled) this.createOverlays();
  }

  cursorDisplay() {
    return screen.getDisplayNearestPoint(screen.getCursorScreenPoint());
  }

  overlayFor(displayId) {
    return this.overlays.get(displayId) || null;
  }

  sendToOverlay(displayId, channel, payload) {
    const w = this.overlayFor(displayId);
    if (w && !w.isDestroyed()) w.webContents.send(channel, payload);
  }

  sendToAllOverlays(channel, payload) {
    for (const w of this.overlays.values()) {
      if (!w.isDestroyed()) w.webContents.send(channel, payload);
    }
  }

  setState(state, extra = {}) {
    this.state = state;
    this.broadcastState(extra);
  }

  broadcastState(extra = {}) {
    this.sendToAllOverlays("companion:state", {
      state: this.state,
      enabled: this.settings.enabled,
      cursorDisplayId: this.cursorDisplay().id,
      hasRegion: !!this.pendingRegion,
      talkShortcut: this.settings.talkShortcut,
      drawShortcut: this.settings.drawShortcut,
      ...extra,
    });
  }

  // ------------------------------------------------------------------
  // Hotkeys
  // ------------------------------------------------------------------
  onTalkHotkey() {
    if (!this.settings.enabled) return;
    if (this.state === "listening") {
      // Second press ends the take.
      this.sendToAllOverlays("companion:stop-record", {});
      return;
    }
    if (this.state === "speaking" && this.activeTts) {
      // Barge-in: cut speech, start listening immediately.
      this.activeTts.stop();
      this.activeTts = null;
    }
    if (this.state === "thinking" && this.activeRequest) {
      this.activeRequest.abort();
      this.activeRequest = null;
    }
    this.beginListening();
  }

  onDrawHotkey() {
    if (!this.settings.enabled) return;
    if (this.state === "drawing") {
      this.exitDrawMode(false);
      return;
    }
    this.enterDrawMode();
  }

  beginListening() {
    const display = this.cursorDisplay();
    this.setState("listening", { cursorDisplayId: display.id });
    this.sendToOverlay(display.id, "companion:start-record", {
      maxSeconds: this.settings.maxRecordSeconds,
    });
  }

  enterDrawMode() {
    const display = this.cursorDisplay();
    this.setState("drawing", { cursorDisplayId: display.id });
    const w = this.overlayFor(display.id);
    if (w && !w.isDestroyed()) {
      w.setIgnoreMouseEvents(false);
      this.sendToOverlay(display.id, "companion:draw-mode", { active: true });
    }
  }

  exitDrawMode(keepRegion) {
    for (const w of this.overlays.values()) {
      if (!w.isDestroyed()) w.setIgnoreMouseEvents(true, { forward: true });
    }
    this.sendToAllOverlays("companion:draw-mode", { active: false });
    if (!keepRegion) this.pendingRegion = null;
    if (this.state === "drawing") this.setState("idle");
  }

  // ------------------------------------------------------------------
  // Screen capture
  // ------------------------------------------------------------------
  /**
   * Capture every display as PNG (base64, no data: prefix), largest edge
   * capped so local VLM inference stays fast. Returns
   * [{displayId, base64, widthPx, heightPx, isCursorScreen, screenNumber}].
   */
  async captureScreens() {
    const displays = screen.getAllDisplays();
    const cursorDisplayId = this.cursorDisplay().id;
    const MAX_EDGE = 1568; // plenty for UI-reading VLMs; keeps latency sane
    const sources = await desktopCapturer.getSources({
      types: ["screen"],
      thumbnailSize: { width: MAX_EDGE, height: MAX_EDGE },
    });
    const shots = [];
    displays.forEach((display, i) => {
      const source =
        sources.find((s) => String(s.display_id) === String(display.id)) || sources[i];
      if (!source || source.thumbnail.isEmpty()) return;
      const img = source.thumbnail;
      const size = img.getSize();
      shots.push({
        displayId: display.id,
        base64: img.toPNG().toString("base64"),
        widthPx: size.width,
        heightPx: size.height,
        isCursorScreen: display.id === cursorDisplayId,
        screenNumber: i + 1,
        displayBounds: display.bounds,
      });
    });
    return shots;
  }

  /** Capture one display at full native resolution and crop a DIP bbox. */
  async captureRegion(displayId, bbox) {
    const display = screen.getAllDisplays().find((d) => d.id === displayId);
    if (!display) return null;
    const scale = display.scaleFactor || 1;
    const sources = await desktopCapturer.getSources({
      types: ["screen"],
      thumbnailSize: {
        width: Math.round(display.bounds.width * scale),
        height: Math.round(display.bounds.height * scale),
      },
    });
    const source =
      sources.find((s) => String(s.display_id) === String(display.id)) || null;
    if (!source || source.thumbnail.isEmpty()) return null;
    const img = source.thumbnail;
    const actual = img.getSize();
    // The capturer may not honor the exact size; derive the real scale.
    const sx = actual.width / display.bounds.width;
    const sy = actual.height / display.bounds.height;
    const rect = {
      x: Math.max(0, Math.round(bbox.x * sx)),
      y: Math.max(0, Math.round(bbox.y * sy)),
      width: Math.min(actual.width, Math.round(bbox.width * sx)),
      height: Math.min(actual.height, Math.round(bbox.height * sy)),
    };
    if (rect.width < 8 || rect.height < 8) return null;
    const crop = img.crop(rect);
    return {
      base64: crop.toPNG().toString("base64"),
      widthPx: rect.width,
      heightPx: rect.height,
    };
  }

  // ------------------------------------------------------------------
  // IPC from overlay renderers
  // ------------------------------------------------------------------
  registerIpc() {
    ipcMain.handle("companion:settings:get", () => ({
      settings: this.settings,
      doctor: this.doctor(),
    }));
    ipcMain.handle("companion:settings:set", (_e, next) => {
      const wasEnabled = this.settings.enabled;
      this.saveSettings(next || {});
      if (this.settings.enabled && !wasEnabled) this.recreateOverlays();
      if (!this.settings.enabled && wasEnabled) {
        for (const w of this.overlays.values()) if (!w.isDestroyed()) w.hide();
      } else if (this.settings.enabled) {
        for (const w of this.overlays.values()) if (!w.isDestroyed()) w.show();
      }
      return { settings: this.settings, doctor: this.doctor() };
    });

    // Finished freehand stroke from the drawing overlay (display-local DIP).
    ipcMain.on("companion:stroke", async (_e, { displayId, points }) => {
      const display = screen.getAllDisplays().find((d) => d.id === displayId);
      if (!display) return this.exitDrawMode(false);
      const bbox = protocol.strokeBoundingBox(
        points,
        display.bounds.width,
        display.bounds.height
      );
      if (!bbox) {
        this.sendToOverlay(displayId, "companion:toast", {
          text: "circle a bit bigger and i'll catch it",
        });
        return this.exitDrawMode(false);
      }
      this.exitDrawMode(true);
      const crop = await this.captureRegion(displayId, bbox).catch(() => null);
      if (!crop) return this.setState("idle");
      this.pendingRegion = { displayId, bbox, ...crop };
      this.sendToOverlay(displayId, "companion:region-armed", { bbox });
      // Region armed → immediately listen for the instruction about it.
      this.beginListening();
    });

    ipcMain.on("companion:draw-cancel", () => this.exitDrawMode(false));

    // Hover-interactivity: the overlay is click-through except while the
    // pointer is over its one clickable element (the task chip). Draw mode
    // owns interactivity outright, so ignore these while drawing.
    ipcMain.on("companion:set-interactive", (e, { interactive } = {}) => {
      if (this.state === "drawing") return;
      const w = BrowserWindow.fromWebContents(e.sender);
      if (!w || w.isDestroyed()) return;
      if (interactive) w.setIgnoreMouseEvents(false);
      else w.setIgnoreMouseEvents(true, { forward: true });
    });

    // Recorded WAV (16 kHz mono PCM16) from the overlay renderer.
    ipcMain.on("companion:audio", async (_e, { wav }) => {
      if (this.state !== "listening") return;
      try {
        await this.handleVoiceTurn(Buffer.from(wav));
      } catch (e) {
        this.setState("idle");
        this.sendToAllOverlays("companion:toast", { text: String(e.message || e) });
      }
    });

    ipcMain.on("companion:record-error", (_e, { message }) => {
      this.setState("idle");
      this.sendToAllOverlays("companion:toast", {
        text: `microphone problem: ${message}`,
      });
    });

    // Renderer finished a speechSynthesis fallback utterance.
    ipcMain.on("companion:speech-done", () => {
      if (this.state === "speaking") this.finishTurn();
    });

    // User clicked the "send to agent" chip after a [TASK:...] reply.
    ipcMain.on("companion:accept-task", () => this.deliverPendingTask());
    ipcMain.on("companion:dismiss-task", () => {
      this.pendingTask = null;
      this.broadcastState();
    });
  }

  // ------------------------------------------------------------------
  // The voice turn pipeline
  // ------------------------------------------------------------------
  async handleVoiceTurn(wavBuffer) {
    const baseUrl = this.host.getBaseUrl();
    if (!baseUrl) {
      this.setState("idle");
      this.sendToAllOverlays("companion:toast", {
        text: "the local engine isn't running yet",
      });
      return;
    }

    // 1. Local STT.
    this.setState("transcribing");
    const whisper = stt.discoverWhisper(this.settings, {
      resourcesBinDir: this.host.resourcesBinDir,
      userModelsDir: this.userModelsDir,
    });
    if (!whisper.bin || !whisper.model) {
      this.setState("idle");
      this.sendToAllOverlays("companion:toast", {
        text: whisper.issues[0] || "speech-to-text is not set up",
      });
      return;
    }
    const rawTranscript = await stt.transcribeWavBuffer(wavBuffer, whisper);
    const transcript = protocol.sanitizeTranscript(rawTranscript);
    if (!transcript) {
      this.setState("idle");
      return;
    }
    this.sendToAllOverlays("companion:transcript", { text: transcript });

    // 2. Screenshots (+ armed spatial region, if any).
    const shots = await this.captureScreens();
    const cursorShot = shots.find((s) => s.isCursorScreen) || shots[0];
    const context = shots.map((s) => ({
      path: `screen-${s.screenNumber}${s.isCursorScreen ? " (primary focus)" : ""} — ${s.widthPx}x${s.heightPx} pixels`,
      content: "",
      image: s.base64,
    }));
    let userText = transcript;
    const region = this.pendingRegion;
    if (region) {
      const label = "user-circled region";
      context.push({ path: label, content: "", image: region.base64 });
      userText = `${transcript}\n\n${protocol.describeCircledRegion(label, region.bbox)}`;
      this.pendingRegion = null;
      this.sendToAllOverlays("companion:region-armed", { bbox: null });
    }

    // 3. Stream the reply from the local model.
    this.setState("thinking");
    const messages = this.history
      .slice(-2 * this.settings.maxHistoryTurns)
      .concat([{ role: "user", content: userText }]);
    const body = {
      id: `companion-${Date.now()}`,
      model: this.settings.model || "auto",
      messages,
      context,
      system: protocol.buildCompanionSystemPrompt({ screens: shots.length }),
      temperature: 0.7,
    };

    const fullText = await this.streamChat(baseUrl, body, (delta, sofar) => {
      // Stream the text into the bubble as it arrives (tags stripped live).
      const preview = protocol.parseCompanionReply(sofar).spokenText;
      this.sendToOverlay(cursorShot.displayId, "companion:partial", { text: preview });
    });
    if (fullText === null) return; // aborted (barge-in)

    const reply = protocol.parseCompanionReply(fullText);
    this.history.push({ role: "user", content: userText });
    this.history.push({ role: "assistant", content: reply.spokenText });
    if (this.history.length > 4 * this.settings.maxHistoryTurns) {
      this.history = this.history.slice(-2 * this.settings.maxHistoryTurns);
    }

    // 4. Pointing.
    if (reply.point) {
      const targetShot =
        (reply.point.screenNumber &&
          shots.find((s) => s.screenNumber === reply.point.screenNumber)) ||
        cursorShot;
      const local = protocol.scalePointToDisplay(
        reply.point,
        targetShot.widthPx,
        targetShot.heightPx,
        targetShot.displayBounds
      );
      if (local) {
        this.sendToOverlay(targetShot.displayId, "companion:point", {
          x: local.x,
          y: local.y,
          label: reply.point.label,
        });
      }
    }

    // 5. Task handoff chip.
    if (reply.task) {
      this.pendingTask = {
        task: reply.task,
        imageBase64: (region && region.base64) || cursorShot.base64,
      };
      this.sendToOverlay(cursorShot.displayId, "companion:task-offer", {
        task: reply.task,
      });
    }

    // 6. Speak.
    this.setState("speaking", { text: reply.spokenText });
    this.sendToOverlay(cursorShot.displayId, "companion:final", {
      text: reply.spokenText,
    });
    const engineChoice = tts.speak(reply.spokenText, this.settings, {
      resourcesBinDir: this.host.resourcesBinDir,
    });
    this.activeTts = engineChoice;
    if (engineChoice.engine === "renderer") {
      this.sendToOverlay(cursorShot.displayId, "companion:speak-fallback", {
        text: reply.spokenText,
      });
      // finishTurn() arrives via companion:speech-done.
    } else {
      engineChoice.done.then(() => {
        if (this.activeTts === engineChoice) this.finishTurn();
      });
    }
  }

  finishTurn() {
    this.activeTts = null;
    this.setState("idle");
    // Bubble/pointer fade is handled renderer-side after a short linger.
    this.sendToAllOverlays("companion:turn-done", {});
  }

  /**
   * POST an SSE request to the local engine and accumulate `text` events.
   * Resolves the full text, or null if aborted. onDelta(delta, sofar) fires
   * per chunk for live rendering.
   */
  streamChat(baseUrl, body, onDelta) {
    return new Promise((resolve, reject) => {
      const url = new URL("/api/chat", baseUrl);
      const payload = JSON.stringify(body);
      const req = http.request(
        {
          hostname: url.hostname,
          port: url.port,
          path: url.pathname,
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Content-Length": Buffer.byteLength(payload),
            "X-Ollamax-Token": this.host.getApiToken(),
          },
        },
        (res) => {
          if (res.statusCode !== 200) {
            res.resume();
            reject(new Error(`engine returned HTTP ${res.statusCode}`));
            return;
          }
          let buf = "";
          let text = "";
          res.setEncoding("utf8");
          res.on("data", (chunk) => {
            buf += chunk;
            let idx;
            while ((idx = buf.indexOf("\n\n")) !== -1) {
              const block = buf.slice(0, idx);
              buf = buf.slice(idx + 2);
              for (const line of block.split("\n")) {
                const trimmed = line.trimStart();
                if (!trimmed.startsWith("data:")) continue;
                try {
                  const ev = JSON.parse(trimmed.slice(5).trim());
                  // The engine streams {"type":"token","text":"…"} chunks,
                  // then {"type":"done"}. "thinking" chunks are dropped —
                  // they'd be nonsense spoken aloud.
                  if (ev.type === "token" && typeof ev.text === "string") {
                    text += ev.text;
                    onDelta(ev.text, text);
                  } else if (ev.type === "done") {
                    resolve(text);
                    req.destroy();
                    return;
                  } else if (ev.type === "error") {
                    reject(new Error(ev.message || "engine error"));
                    req.destroy();
                    return;
                  }
                } catch (_) {}
              }
            }
          });
          res.on("end", () => resolve(text));
          res.on("error", (e) => reject(e));
        }
      );
      req.on("error", (e) => {
        if (this.activeRequest && this.activeRequest.aborted) resolve(null);
        else reject(e);
      });
      this.activeRequest = {
        aborted: false,
        abort: () => {
          this.activeRequest.aborted = true;
          req.destroy();
          resolve(null);
        },
      };
      req.end(payload);
    });
  }

  /** Hand the accepted [TASK:...] to the main window's chat as a prefill. */
  deliverPendingTask() {
    const pending = this.pendingTask;
    this.pendingTask = null;
    if (!pending) return;
    const win = this.host.getMainWindow();
    if (!win || win.isDestroyed()) return;
    win.show();
    win.focus();
    win.webContents.send("companion:task", {
      text: pending.task,
      items: [
        {
          path: `companion-region-${Date.now()}.png`,
          label: "companion screenshot",
          content: "",
          image: pending.imageBase64,
          isImage: true,
        },
      ],
    });
    this.sendToAllOverlays("companion:toast", {
      text: "sent to ollamax — review it there and hit send",
    });
    this.broadcastState();
  }
}

module.exports = { Companion, DEFAULT_SETTINGS };

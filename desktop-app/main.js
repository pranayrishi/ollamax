// Ollamax desktop app — Electron main process.
//
// The app is a thin SHELL around the existing engine. On launch it spawns the
// bundled `forge` binary as `forge serve --port 0` (a HIDDEN local HTTP+SSE
// backend — no terminal, never on PATH), reads its ephemeral port from the
// `FORGE_SERVE_READY {json}` line, then opens a window hosting the existing chat
// + Hub UI, which talks to that local server. Inference stays local
// (app → forge serve → Ollama). Modeled on the Sattva AI desktop app.

const {
  app,
  BrowserWindow,
  desktopCapturer,
  dialog,
  globalShortcut,
  ipcMain,
  safeStorage,
  screen,
  session,
  shell,
} = require("electron");
const path = require("path");
const fs = require("fs");
const os = require("os");
const cp = require("child_process");
const crypto = require("crypto");
const { URL } = require("url");
const {
  resolveTtsRuntime,
  resolveWhisperRuntime,
  speakText,
  transcribeWavBase64,
} = require("./lib/voice-runtime");
const {
  assertBoundedLassoInput,
  buildPaddedSelectionBounds,
  mapDipRectToScreenshotPixels,
  normalizeLassoSamples,
  planCappedCrop,
} = require("./spatial-selection");
const {
  cursorBuddyBounds,
  cursorBuddyCueBounds,
  cursorBuddyState,
  normalizedPointToDisplayPoint,
} = require("./cursor-buddy-state");
const { validatePointDirective } = require("./renderer/point-directives");
const {
  isAudioOnlyPermissionCheck,
  isAudioOnlyPermissionRequest,
  isExactFileUrl,
  safeExternalUrl,
  isTrustedMainWebContents,
} = require("./lib/desktop-security");
const { DesktopAuth, DesktopAuthError, normalizeAccountServer } = require("./lib/desktop-auth");
const { readAttachmentPreview } = require("./lib/attachment-preview");
const { resolveWorkspacePath } = require("./lib/workspace-paths");

const isDev = !app.isPackaged;
let mainWindow = null;
let forgeProc = null;
let baseUrl = null;
// A chat-only server may run before a folder is chosen. Once a workspace is
// open, the managed server is restarted in that folder so agent tools use it.
let workspaceRoot = null;
let forgeWorkspaceRoot = null;
let forgeRestartQueue = Promise.resolve();
let forgeStartAbort = null;
let spatialSession = null;
let cursorBuddyWindow = null;
let cursorBuddyReady = false;
let cursorBuddyStateKey = "idle";
let cursorBuddyPoint = null;
let cursorBuddyTimer = null;
let cursorBuddyCue = null;
let cursorBuddyCueQueue = [];
const MAX_CURSOR_BUDDY_CUE_QUEUE = 2;
let voiceShortcutRegistered = false;
let lastVoiceShortcutAt = 0;
// A Whisper process loads a non-trivial local model.  One trusted renderer is
// enough to request transcription, but it must not be able to start a pile of
// concurrent native processes if a button is double-clicked or a renderer
// script misbehaves.
let voiceTranscriptionInFlight = false;
const VOICE_TOGGLE_ACCELERATOR = "CommandOrControl+Alt+Space";
// Private capability shared only with the bundled engine process and this
// trusted Electron shell. It prevents arbitrary web content from driving the
// loopback agent API.
const forgeApiToken = crypto.randomBytes(32).toString("hex");

// Account server for sign-in + the Central Hub (identity only; never inference).
// Configurable; empty disables account features gracefully. The raw environment
// value is never used for requests: it is canonicalized once after Electron's
// OS credential service is available.
const ACCOUNT_SERVER = process.env.FORGE_ACCOUNT_SERVER || "";
let accountServer = "";
let desktopAuth = null;
const MAIN_RENDERER_PATH = path.join(__dirname, "renderer", "index.html");
const CURSOR_BUDDY_RENDERER_PATH = path.join(__dirname, "renderer", "cursor-buddy.html");
const SPATIAL_OVERLAY_RENDERER_PATH = path.join(__dirname, "renderer", "spatial-overlay.html");

function logFile(msg) {
  try {
    fs.appendFileSync(
      path.join(app.getPath("userData"), "forge-app.log"),
      `${new Date().toISOString()} ${msg}\n`
    );
  } catch (_) {}
}

// Every window in the app is a fixed local document. A renderer must never
// keep its preload bridge after navigating to web content or spawn a second
// BrowserWindow with inherited privileges. External links use the explicit,
// separately validated shell bridge instead.
function lockWindowToLocalFile(win, allowedFile) {
  const contents = win && win.webContents;
  if (!contents) return;
  contents.setWindowOpenHandler(() => ({ action: "deny" }));
  const rejectUnexpectedNavigation = (event, details) => {
    const url = details && typeof details === "object" ? details.url : details;
    const isMainFrame = !details || typeof details !== "object" || details.isMainFrame !== false;
    if (!isMainFrame || !isExactFileUrl(url, allowedFile)) event.preventDefault();
  };
  contents.on("will-navigate", rejectUnexpectedNavigation);
  contents.on("will-frame-navigate", rejectUnexpectedNavigation);
  contents.on("will-attach-webview", (event) => event.preventDefault());
}

// Electron's default permission behaviour is intentionally not an application
// policy. Deny every renderer-granted capability except primary-frame audio
// capture in the exact bundled main document. Screen selection uses
// main-process desktopCapturer after an explicit IPC request, so renderer
// display capture remains denied.
function installDesktopSecurityPolicy() {
  const desktopSession = session.defaultSession;
  desktopSession.setPermissionCheckHandler((contents, permission, _origin, details) => {
    return (
      isTrustedMainWebContents(contents, mainWindow, MAIN_RENDERER_PATH) &&
      isAudioOnlyPermissionCheck(permission, details)
    );
  });
  desktopSession.setPermissionRequestHandler((contents, permission, callback, details) => {
    const allowed =
      isTrustedMainWebContents(contents, mainWindow, MAIN_RENDERER_PATH) &&
      isAudioOnlyPermissionRequest(permission, details);
    callback(allowed);
  });
  // No HID/USB/serial device gets an implicit in-memory grant. The app has no
  // device feature, so a future renderer change must add an explicit policy.
  desktopSession.setDevicePermissionHandler(() => false);
  // A compromised page must not reach the system picker through
  // getDisplayMedia. The approved lasso path is the only screen-capture flow.
  desktopSession.setDisplayMediaRequestHandler((_request, callback) => callback({}));
}

async function openValidatedExternal(candidate) {
  const url = safeExternalUrl(candidate);
  if (!url) throw new Error("only HTTPS or literal-loopback HTTP links may be opened externally");
  await shell.openExternal(url);
  return url;
}

// Construct this only after app.whenReady(): Electron safeStorage may not be
// initialized before then. A malformed account URL simply disables optional
// account/Hub features; it never affects local inference.
function initializeDesktopAuth() {
  desktopAuth = null;
  accountServer = "";
  if (!ACCOUNT_SERVER.trim()) return;
  try {
    accountServer = normalizeAccountServer(ACCOUNT_SERVER);
    desktopAuth = new DesktopAuth({
      accountServer,
      storageDir: app.getPath("userData"),
      safeStorage,
      openExternal: openValidatedExternal,
      log: (message) => logFile(`account: ${message}`),
    });
  } catch (error) {
    accountServer = "";
    const message = error instanceof Error ? error.message : "invalid account configuration";
    logFile(`account server disabled: ${message}`);
  }
}

function accountError(error) {
  if (error instanceof DesktopAuthError) {
    return { ok: false, error: error.code, message: error.message };
  }
  logFile(`account operation failed: ${String((error && error.message) || error)}`);
  return { ok: false, error: "account_error", message: "The account operation could not be completed." };
}

function accountReady() {
  return !!desktopAuth && !!accountServer;
}

// ---------------------------------------------------------------------
// Local cursor companion
// ---------------------------------------------------------------------
// This is intentionally a visual status surface, not an agent-control layer.
// It receives only a closed set of short state labels (defined in
// cursor-buddy-state.js), is transparent/click-through, and has no OS-control
// API. The sole model-derived datum it can receive is an independently
// validated, capped POINT label for a temporary visual cue.
function clearCursorBuddyTimer() {
  if (cursorBuddyTimer) clearTimeout(cursorBuddyTimer);
  cursorBuddyTimer = null;
}

function currentCursorPoint() {
  try {
    return screen.getCursorScreenPoint();
  } catch (_) {
    return { x: 0, y: 0 };
  }
}

function positionCursorBuddy(win, point, cue = null) {
  if (!win || win.isDestroyed()) return null;
  const cursor = point && Number.isFinite(point.x) && Number.isFinite(point.y)
    ? point
    : currentCursorPoint();
  try {
    const display = screen.getDisplayNearestPoint(cursor) || screen.getPrimaryDisplay();
    const displayBounds = display && display.bounds;
    const workArea = (display && display.workArea) || displayBounds;
    if (!workArea || !displayBounds) return null;
    const bounds = cue
      ? cursorBuddyCueBounds(cursor, displayBounds)
      : cursorBuddyBounds(cursor, workArea);
    win.setBounds(bounds);
    if (!cue) return null;
    return {
      x: Math.max(0, Math.min(bounds.width - 1, Math.round(cursor.x - bounds.x))),
      y: Math.max(0, Math.min(bounds.height - 1, Math.round(cursor.y - bounds.y))),
    };
  } catch (error) {
    logFile(`cursor buddy position failed: ${String((error && error.message) || error)}`);
    return null;
  }
}

function publishCursorBuddyState() {
  const win = cursorBuddyWindow;
  const state = cursorBuddyState(cursorBuddyStateKey);
  if (!win || win.isDestroyed() || !cursorBuddyReady || !state) return;
  if (state.key === "idle") {
    win.webContents.send("cursor-buddy:state", { label: "", detail: "", cue: null });
    win.hide();
    return;
  }
  const cue = state.key === "pointing" ? cursorBuddyCue : null;
  const cuePosition = positionCursorBuddy(win, cursorBuddyPoint, cue);
  win.webContents.send("cursor-buddy:state", {
    label: state.label,
    detail: cue ? cue.label : state.detail,
    cue: cue && cuePosition ? cuePosition : null,
  });
  win.showInactive();
  try {
    win.moveTop();
  } catch (_) {}
}

function createCursorBuddyWindow() {
  if (cursorBuddyWindow && !cursorBuddyWindow.isDestroyed()) return cursorBuddyWindow;
  if (!app.isReady()) return null;
  const win = new BrowserWindow({
    width: 264,
    height: 84,
    frame: false,
    transparent: true,
    backgroundColor: "#00000000",
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    closable: false,
    focusable: false,
    skipTaskbar: true,
    alwaysOnTop: true,
    hasShadow: false,
    show: false,
    webPreferences: {
      preload: path.join(__dirname, "cursor-buddy-preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      // This window has no need for Node-capable renderer primitives. Its
      // preload uses only Electron's contextBridge/ipcRenderer APIs, which are
      // supported by Electron's sandboxed preload environment.
      sandbox: true,
    },
  });
  lockWindowToLocalFile(win, CURSOR_BUDDY_RENDERER_PATH);
  cursorBuddyWindow = win;
  cursorBuddyReady = false;
  try {
    win.setIgnoreMouseEvents(true, { forward: true });
  } catch (error) {
    // Never leave a visual-only overlay able to intercept input on a platform
    // that cannot provide click-through behavior.
    logFile(`cursor buddy click-through is unavailable: ${String((error && error.message) || error)}`);
    if (cursorBuddyWindow === win) cursorBuddyWindow = null;
    cursorBuddyReady = false;
    win.destroy();
    return null;
  }
  try {
    win.setAlwaysOnTop(true, "screen-saver");
    win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
  } catch (_) {}
  win.once("closed", () => {
    if (cursorBuddyWindow === win) {
      cursorBuddyWindow = null;
      cursorBuddyReady = false;
    }
  });
  win.webContents.once("did-finish-load", () => {
    if (cursorBuddyWindow !== win || win.isDestroyed()) return;
    cursorBuddyReady = true;
    publishCursorBuddyState();
  });
  win.loadFile(CURSOR_BUDDY_RENDERER_PATH).catch((error) => {
    logFile(`cursor buddy failed to load: ${String((error && error.message) || error)}`);
    if (!win.isDestroyed()) win.destroy();
  });
  return win;
}

function setCursorBuddyState(name, point, cue = null) {
  const state = cursorBuddyState(name);
  if (!state) return false;
  if (
    state.key === "pointing" &&
    (!cue || typeof cue.label !== "string" || !Number.isFinite(cue.point && cue.point.x) || !Number.isFinite(cue.point && cue.point.y))
  ) {
    return false;
  }
  clearCursorBuddyTimer();
  cursorBuddyStateKey = state.key;
  cursorBuddyPoint = point && Number.isFinite(point.x) && Number.isFinite(point.y)
    ? { x: point.x, y: point.y }
    : currentCursorPoint();
  if (state.key === "pointing") {
    cursorBuddyCue = { label: cue.label, point: { ...cursorBuddyPoint } };
  } else {
    cursorBuddyCue = null;
    cursorBuddyCueQueue = [];
  }
  if (state.key === "idle") {
    publishCursorBuddyState();
    return true;
  }
  if (!createCursorBuddyWindow()) {
    // If Electron cannot guarantee a click-through surface, do not retain a
    // hidden pointer state or claim that a visual cue was shown.
    cursorBuddyStateKey = "idle";
    cursorBuddyPoint = null;
    cursorBuddyCue = null;
    cursorBuddyCueQueue = [];
    return false;
  }
  publishCursorBuddyState();
  if (state.durationMs > 0) {
    cursorBuddyTimer = setTimeout(() => {
      if (cursorBuddyStateKey !== state.key) return;
      if (state.key === "pointing" && cursorBuddyCueQueue.length > 0) {
        const nextCue = cursorBuddyCueQueue.shift();
        setCursorBuddyState("pointing", nextCue.point, nextCue);
        return;
      }
      setCursorBuddyState("idle");
    }, state.durationMs);
  }
  return true;
}

function destroyCursorBuddy() {
  clearCursorBuddyTimer();
  cursorBuddyStateKey = "idle";
  cursorBuddyPoint = null;
  cursorBuddyCue = null;
  cursorBuddyCueQueue = [];
  const win = cursorBuddyWindow;
  cursorBuddyWindow = null;
  cursorBuddyReady = false;
  if (win && !win.isDestroyed()) win.destroy();
}

// Convert a renderer-supplied, already-parsed directive into a display point.
// This revalidates every field in the privileged process and deliberately has
// no `setCursorPos`, accessibility, click, keyboard, or window-control call.
function localPointCueFromDirective(value) {
  const directive = validatePointDirective(value);
  if (!directive) return null;
  try {
    const displays = screen.getAllDisplays();
    if (!Array.isArray(displays) || displays.length === 0) return null;
    const display = directive.screenIndex === null
      ? (screen.getDisplayNearestPoint(currentCursorPoint()) || screen.getPrimaryDisplay())
      : displays[directive.screenIndex];
    if (!display || !display.bounds) return null;
    const point = normalizedPointToDisplayPoint(directive, display.bounds);
    if (!point) return null;
    return { point, label: directive.label };
  } catch (error) {
    logFile(`local point cue could not resolve a display: ${String((error && error.message) || error)}`);
    return null;
  }
}

function enqueueCursorBuddyCue(cue) {
  if (!cue) return false;
  if (cursorBuddyStateKey === "pointing" && cursorBuddyCue) {
    // A final response can carry a few ordered cues. Bound the queue so an
    // untrusted renderer cannot turn the visual overlay into a long-running
    // attention-grabbing loop.
    if (cursorBuddyCueQueue.length >= MAX_CURSOR_BUDDY_CUE_QUEUE) return false;
    cursorBuddyCueQueue.push(cue);
    return true;
  }
  cursorBuddyCueQueue = [];
  return setCursorBuddyState("pointing", cue.point, cue);
}

function isTrustedMainRenderer(event) {
  return !!(event && isTrustedMainWebContents(event.sender, mainWindow, MAIN_RENDERER_PATH));
}

function publishVoiceShortcutStatus() {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send("buddy:shortcutStatus", {
      registered: voiceShortcutRegistered,
      accelerator: VOICE_TOGGLE_ACCELERATOR,
    });
  }
}

function requestVoiceToggleFromShortcut() {
  // Some platforms repeat a global shortcut while keys are held. Treat it as
  // one explicit toggle rather than starting and immediately stopping capture.
  const now = Date.now();
  if (now - lastVoiceShortcutAt < 350) return;
  lastVoiceShortcutAt = now;
  if (!mainWindow || mainWindow.isDestroyed()) {
    setCursorBuddyState("voice_window_unavailable");
    return;
  }
  setCursorBuddyState("voice_starting");
  // The renderer owns microphone permission and the actual push-to-talk
  // session. This event asks it to start/stop; it grants no new OS authority.
  mainWindow.webContents.send("buddy:voiceToggle", { accelerator: VOICE_TOGGLE_ACCELERATOR });
}

function registerVoiceToggleShortcut() {
  if (voiceShortcutRegistered) return true;
  try {
    voiceShortcutRegistered = globalShortcut.register(VOICE_TOGGLE_ACCELERATOR, requestVoiceToggleFromShortcut);
  } catch (error) {
    voiceShortcutRegistered = false;
    logFile(`could not register ${VOICE_TOGGLE_ACCELERATOR}: ${String((error && error.message) || error)}`);
  }
  if (!voiceShortcutRegistered) {
    logFile(`could not register ${VOICE_TOGGLE_ACCELERATOR}; another application may own it`);
    setCursorBuddyState("shortcut_unavailable");
  }
  publishVoiceShortcutStatus();
  return voiceShortcutRegistered;
}

function unregisterVoiceToggleShortcut() {
  try {
    globalShortcut.unregister(VOICE_TOGGLE_ACCELERATOR);
  } catch (_) {}
  voiceShortcutRegistered = false;
}

// Resolve the bundled engine binary: prod = inside the .app Resources/bin;
// dev = the repo's release build.
function forgeBinaryPath() {
  const exe = process.platform === "win32" ? "forge.exe" : "forge";
  const candidates = isDev
    ? [
        path.resolve(__dirname, "..", "target", "release", exe),
        path.resolve(__dirname, "bin", exe),
      ]
    : [
        path.join(process.resourcesPath, "bin", exe),
      ];
  return candidates.find((p) => fs.existsSync(p)) || candidates[0];
}

function forgeConfig() {
  const workspaceReady = !!baseUrl && !!workspaceRoot && forgeWorkspaceRoot === workspaceRoot;
  return {
    baseUrl,
    accountEnabled: accountReady(),
    apiToken: forgeApiToken,
    workspace: workspaceRoot ? { root: workspaceRoot, ready: workspaceReady } : null,
    workspaceReady,
  };
}

// The renderer keeps no privileged process state. Push a fresh endpoint after
// a workspace restart so its next request cannot target the retired server.
function notifyForgeConfig() {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send("forge:configChanged", forgeConfig());
  }
}

function stopForgeServe() {
  const child = forgeProc;
  baseUrl = null;
  if (!child) return Promise.resolve();

  // Detach this child from global state before killing it: a late exit/ready
  // event from the old process must never clear a newly started server.
  if (forgeProc === child) forgeProc = null;
  notifyForgeConfig();
  return new Promise((resolve) => {
    let done = false;
    let forceKill = null;
    const finish = () => {
      if (done) return;
      done = true;
      if (forceKill) clearTimeout(forceKill);
      resolve();
    };
    forceKill = setTimeout(() => {
      try {
        child.kill("SIGKILL");
      } catch (_) {}
      finish();
    }, 5000);
    child.once("exit", finish);
    child.once("error", finish);
    const startup = forgeStartAbort;
    if (startup && startup.child === child) {
      startup.abort(new Error("forge serve stopped for workspace switch"));
    } else {
      try {
        child.kill();
      } catch (_) {
        finish();
      }
    }
  });
}

function startForgeServe(cwd) {
  return new Promise((resolve, reject) => {
    const bin = forgeBinaryPath();
    if (!fs.existsSync(bin)) {
      reject(new Error(`engine not found at ${bin}`));
      return;
    }
    const launchCwd = path.resolve(cwd || app.getPath("home"));
    logFile(`launching ${bin} serve --port 0 in ${launchCwd}`);
    const child = cp.spawn(bin, ["serve", "--port", "0"], {
      cwd: launchCwd,
      env: { ...process.env, FORGE_SERVER_TOKEN: forgeApiToken },
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });

    forgeProc = child;
    baseUrl = null;
    let settled = false;
    let readyTimer = null;
    const clearReadyTimer = () => {
      if (readyTimer) clearTimeout(readyTimer);
      readyTimer = null;
    };
    const fail = (error) => {
      if (settled) return;
      settled = true;
      clearReadyTimer();
      if (forgeStartAbort && forgeStartAbort.child === child) forgeStartAbort = null;
      if (forgeProc === child) {
        forgeProc = null;
        baseUrl = null;
        notifyForgeConfig();
      }
      try {
        child.kill();
      } catch (_) {}
      reject(error);
    };
    forgeStartAbort = { child, abort: fail };

    let buf = "";
    const onData = (d) => {
      if (forgeProc !== child) return;
      buf += d.toString();
      let i;
      while ((i = buf.indexOf("\n")) !== -1) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        const m = line.match(/FORGE_SERVE_READY\s+(\{.*\})/);
        if (m) {
          try {
            const info = JSON.parse(m[1]);
            if (info.token !== forgeApiToken) {
              fail(new Error("local engine did not confirm its private API capability"));
              return;
            }
            baseUrl = `http://127.0.0.1:${info.port}`;
            logFile(`forge serve ready on ${baseUrl} (v${info.version})`);
            if (!settled) {
              settled = true;
              clearReadyTimer();
              if (forgeStartAbort && forgeStartAbort.child === child) forgeStartAbort = null;
            }
            notifyForgeConfig();
            resolve(baseUrl);
          } catch (e) {
            fail(new Error(`bad ready line: ${e}`));
          }
        }
      }
    };
    child.stdout.on("data", onData);
    child.stderr.on("data", (d) => logFile(`[forge] ${d.toString().trim()}`));
    child.on("error", (error) => fail(error));
    child.on("exit", (code) => {
      logFile(`forge serve exited (${code})`);
      if (forgeProc === child) {
        forgeProc = null;
        baseUrl = null;
        notifyForgeConfig();
      }
      if (!settled) fail(new Error(`forge serve exited (${code}) before reporting ready`));
    });
    readyTimer = setTimeout(() => fail(new Error("forge serve did not become ready in 15s")), 15000);
  });
}

// Serialize restarts so replacing folders cannot leave two engines racing to
// update the renderer's endpoint.
function restartForgeServe(cwd) {
  const launchCwd = path.resolve(cwd || app.getPath("home"));
  const restart = forgeRestartQueue.then(async () => {
    await stopForgeServe();
    return startForgeServe(launchCwd);
  });
  forgeRestartQueue = restart.catch(() => undefined);
  return restart;
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1100,
    height: 760,
    minWidth: 720,
    minHeight: 480,
    title: "Ollamax",
    backgroundColor: "#0b0d12",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      // Keep a Chromium renderer compromise from gaining unsandboxed process
      // access. All legitimate native operations flow through the narrow,
      // validated preload bridge below.
      sandbox: true,
    },
  });
  lockWindowToLocalFile(mainWindow, MAIN_RENDERER_PATH);
  mainWindow.loadFile(MAIN_RENDERER_PATH);
  if (isDev) mainWindow.webContents.openDevTools({ mode: "detach" });
  mainWindow.webContents.once("did-finish-load", publishVoiceShortcutStatus);
  mainWindow.on("closed", () => {
    // A hidden companion is still an Electron BrowserWindow. Destroy it (do
    // not merely hide it) so it cannot keep the app alive or block macOS's
    // normal activate-to-reopen behavior. The same applies to every lasso
    // overlay and its in-memory capture if the main app is closed mid-select.
    if (spatialSession) {
      finishSpatialSession(
        spatialSession,
        null,
        new Error("Screen-region selection cancelled because Ollamax was closed.")
      );
    }
    destroyCursorBuddy();
    mainWindow = null;
  });
}

// --- IPC: things the renderer can't do itself (Node/Electron only) --------

ipcMain.handle("forge:config", (event) => {
  if (!isTrustedMainRenderer(event)) return null;
  return forgeConfig();
});

ipcMain.handle("forge:pickFiles", async (event) => {
  if (!isTrustedMainRenderer(event)) return [];
  const r = await dialog.showOpenDialog(mainWindow, {
    properties: ["openFile", "multiSelections"],
  });
  if (r.canceled) return [];
  return r.filePaths.map((p) => ({ path: p, label: path.basename(p), content: readAttachmentPreview(p) }));
});

ipcMain.handle("forge:openExternal", async (event, url) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted external-link request" };
  }
  try {
    await openValidatedExternal(url);
    return { ok: true };
  } catch (error) {
    return { ok: false, error: String((error && error.message) || error) };
  }
});

// The main renderer may update only an enumerated, display-only companion
// state. Reject calls from every other webContents and reject arbitrary text.
// `pointing` is internal-only; a pointer cue must go through the separately
// validated `buddy:point` path below.
ipcMain.handle("buddy:setState", (event, state) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted cursor companion request" };
  }
  if (state === "pointing") return { ok: false, error: "pointing requires a validated local directive" };
  return setCursorBuddyState(state) ? { ok: true } : { ok: false, error: "invalid local state" };
});

// The desktop POINT cue is strictly visual. The renderer can supply only
// normalized coordinates, an optional indexed display, and a short validated
// label. The main process maps it to a transient click-through overlay; it
// never moves the pointer or performs an OS/UI action.
ipcMain.handle("buddy:point", (event, directive) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted local point request" };
  }
  const cue = localPointCueFromDirective(directive);
  return enqueueCursorBuddyCue(cue)
    ? { ok: true }
    : { ok: false, error: "invalid or unavailable local point cue" };
});

// ---------------------------------------------------------------------
// Local voice (explicit push-to-talk only)
// ---------------------------------------------------------------------
// The renderer can ask for a one-shot transcription or optional spoken answer,
// but it never receives filesystem/process access.  No captured audio or text
// is logged, persisted, or sent to an API from this shell.
function localVoiceOptions() {
  return {
    resourcesPath: process.resourcesPath,
    appPath: app.getAppPath(),
    env: process.env,
  };
}

function publicVoiceStatus() {
  const options = localVoiceOptions();
  const whisper = resolveWhisperRuntime(options);
  const tts = resolveTtsRuntime(options);
  return {
    whisper: { available: whisper.available, reason: whisper.reason || null, source: whisper.source || null },
    tts: { available: tts.available, reason: tts.reason || null, kind: tts.kind || null },
    // This is an important product contract: local voice has no cloud fallback.
    localOnly: true,
  };
}

ipcMain.handle("voice:status", (event) => {
  if (!isTrustedMainRenderer(event)) {
    return {
      whisper: { available: false, reason: "untrusted local voice request", source: null },
      tts: { available: false, reason: "untrusted local voice request", kind: null },
      localOnly: true,
    };
  }
  return publicVoiceStatus();
});

ipcMain.handle("voice:transcribe", async (event, wavBase64) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted local voice request" };
  }
  if (voiceTranscriptionInFlight) {
    return { ok: false, error: "A local transcription is already in progress." };
  }
  voiceTranscriptionInFlight = true;
  try {
    const transcript = await transcribeWavBase64(wavBase64, localVoiceOptions());
    return { ok: true, transcript };
  } catch (error) {
    return { ok: false, error: String((error && error.message) || error) };
  } finally {
    voiceTranscriptionInFlight = false;
  }
});

ipcMain.handle("voice:speak", async (event, text) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted local voice request" };
  }
  try {
    await speakText(text, localVoiceOptions());
    return { ok: true };
  } catch (error) {
    return { ok: false, error: String((error && error.message) || error) };
  }
});

// ---------------------------------------------------------------------
// Spatial context (explicit lasso selection only)
// ---------------------------------------------------------------------
// A screen image is captured only after the user presses the region button. It
// stays in memory, is cropped before it reaches the local engine, and is
// discarded as soon as the selection resolves or is cancelled. The overlay is
// not a click-through controller: it describes visual context, never grants
// mouse, accessibility, shell, or filesystem authority.
// Leaves room below the engine's 4 MiB JSON request cap for the base64
// expansion, user prompt, and any ordinary attached context.
const SPATIAL_MAX_ENCODED_BYTES = 2_000_000;
// The lasso crop is capped below 1.4 MP, so a 2048-pixel thumbnail retains
// more than enough source detail without holding one 4K-sized image per
// monitor in RAM. Only the user-chosen display is retained or serialized to an
// overlay.
const SPATIAL_THUMBNAIL_BOUND = 2_048;

function desktopCaptureDisplayId(source) {
  const direct = source && (source.display_id || source.displayId);
  if (direct != null && String(direct)) return String(direct);
  const match = source && /^screen:([^:]+):/i.exec(String(source.id || ""));
  return match ? match[1] : null;
}

async function chooseDisplayForSpatialSelection() {
  const displays = screen.getAllDisplays();
  if (!displays.length) throw new Error("No display is available for a spatial selection.");
  if (displays.length === 1) return displays[0];

  const nearest = screen.getDisplayNearestPoint(currentCursorPoint());
  const defaultId = nearest ? String(nearest.id) : "";
  const buttons = displays.map((display, index) => {
    const selected = String(display.id) === defaultId ? " (current)" : "";
    return `Display ${index + 1} · ${Math.round(display.bounds.width)} × ${Math.round(display.bounds.height)}${selected}`;
  });
  buttons.push("Cancel");
  const options = {
    type: "question",
    title: "Choose a display",
    message: "Choose the display containing the region you want to attach.",
    detail: "Only the display you choose is captured, and only your lassoed crop is sent to the local model.",
    buttons,
    defaultId: Math.max(0, displays.findIndex((display) => String(display.id) === defaultId)),
    cancelId: displays.length,
    noLink: true,
  };
  const parent = mainWindow && !mainWindow.isDestroyed() ? mainWindow : null;
  const selection = parent
    ? await dialog.showMessageBox(parent, options)
    : await dialog.showMessageBox(options);
  if (selection.response === displays.length) return null;
  return displays[selection.response] || null;
}

async function captureDisplayForSpatialSelection(display) {
  if (!display || display.id == null) throw new Error("No display was selected for a spatial selection.");
  let sources;
  try {
    sources = await desktopCapturer.getSources({
      types: ["screen"],
      fetchWindowIcons: false,
      thumbnailSize: { width: SPATIAL_THUMBNAIL_BOUND, height: SPATIAL_THUMBNAIL_BOUND },
    });
  } catch (error) {
    throw new Error(`Screen capture is unavailable. Grant screen-recording permission and try again. (${String((error && error.message) || error)})`);
  }
  const displays = screen.getAllDisplays();
  let source = sources.find((candidate) => desktopCaptureDisplayId(candidate) === String(display.id));
  // Electron versions that do not expose display_id are unambiguous only on a
  // one-display machine. Do not guess a source on a multi-display desktop.
  if (!source && displays.length === 1 && sources.length) source = sources[0];
  if (!source || !source.thumbnail || source.thumbnail.isEmpty()) {
    throw new Error("No readable capture was returned for the selected display. Grant screen-recording permission and try again.");
  }
  const imageSize = source.thumbnail.getSize();
  if (!imageSize.width || !imageSize.height) {
    throw new Error("The selected display capture has no usable image size.");
  }
  return { display, thumbnail: source.thumbnail, imageSize };
}

function finishSpatialSession(session, result, error) {
  if (!session || spatialSession !== session || session.finished) return;
  session.finished = true;
  spatialSession = null;
  for (const overlay of session.overlays.values()) {
    if (!overlay.isDestroyed()) overlay.destroy();
  }
  session.overlays.clear();
  // Drop screenshots and their base64 data immediately after the only selected
  // crop has been encoded. The result carries the crop, not the full display.
  session.captures.clear();
  if (error) {
    const message = String((error && error.message) || error).toLowerCase();
    setCursorBuddyState(message.includes("cancel") ? "spatial_cancelled" : "spatial_error");
    session.reject(error instanceof Error ? error : new Error(String(error)));
  } else {
    session.resolve(result);
  }
}

function compactSpatialImage(image) {
  let current = image;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const quality = Math.max(52, 84 - attempt * 8);
    const bytes = current.toJPEG(quality);
    if (bytes.length <= SPATIAL_MAX_ENCODED_BYTES) {
      const base64 = bytes.toString("base64");
      return { image: base64, thumb: `data:image/jpeg;base64,${base64}` };
    }
    const size = current.getSize();
    const factor = Math.max(0.45, Math.sqrt(SPATIAL_MAX_ENCODED_BYTES / bytes.length) * 0.9);
    const width = Math.max(1, Math.floor(size.width * factor));
    const height = Math.max(1, Math.floor(size.height * factor));
    if (width === size.width && height === size.height) break;
    current = current.resize({ width, height, quality: "best" });
  }
  throw new Error("The selected screen region is too detailed to attach safely. Select a smaller region.");
}

function makeSpatialContextItem(capture, samples) {
  const localPoints = normalizeLassoSamples(assertBoundedLassoInput(samples), {
    minDistance: 0.5,
    maxSamples: 512,
  });
  const points = localPoints.map((point) => ({
    x: point.x + capture.display.bounds.x,
    y: point.y + capture.display.bounds.y,
  }));
  const selection = buildPaddedSelectionBounds(points, capture.display.bounds, {
    padding: 12,
    minWidth: 28,
    minHeight: 28,
  });
  if (!selection) throw new Error("Draw a region on the displayed screen to attach it.");
  const pixelRect = mapDipRectToScreenshotPixels(selection, {
    ...capture.display.bounds,
    scaleFactor: capture.display.scaleFactor,
  }, capture.imageSize);
  if (!pixelRect) throw new Error("That region is outside the captured display.");
  const plan = planCappedCrop(pixelRect, capture.imageSize, {
    maxWidth: 1_400,
    maxHeight: 1_400,
    maxPixels: 1_400_000,
  });
  if (!plan) throw new Error("That screen region could not be cropped.");
  const cropped = capture.thumbnail.crop(plan.sourceRect);
  const scaled = plan.downscaled
    ? cropped.resize({ width: plan.outputSize.width, height: plan.outputSize.height, quality: "best" })
    : cropped;
  const encoded = compactSpatialImage(scaled);
  const normalized = {
    x: Number(((selection.x - capture.display.bounds.x) / capture.display.bounds.width).toFixed(4)),
    y: Number(((selection.y - capture.display.bounds.y) / capture.display.bounds.height).toFixed(4)),
    width: Number((selection.width / capture.display.bounds.width).toFixed(4)),
    height: Number((selection.height / capture.display.bounds.height).toFixed(4)),
  };
  return {
    path: `screen-region-${Date.now()}.jpg`,
    label: "Selected screen region",
    image: encoded.image,
    thumb: encoded.thumb,
    isImage: true,
    spatial: true,
    content: [
      "Spatial visual context: the user explicitly lasso-selected this visible screen region.",
      `Normalized region: x=${normalized.x}, y=${normalized.y}, width=${normalized.width}, height=${normalized.height}.`,
      "Treat the image as a visual reference only. Do not assume it authorizes controlling the operating system, clicking UI, reading outside this crop, or editing files. If asked to replicate it, identify observable components and then propose or make only the workspace changes authorized by the current autonomy setting.",
    ].join("\n"),
  };
}

async function startSpatialSelection() {
  if (spatialSession) throw new Error("A screen-region selection is already active.");
  // Choosing the display happens before capture. We retain and serialize only
  // that display's thumbnail, not one full image per monitor.
  const display = await chooseDisplayForSpatialSelection();
  if (!display) throw new Error("Screen-region selection cancelled.");
  const capture = await captureDisplayForSpatialSelection(display);

  return new Promise((resolve, reject) => {
    const displayId = String(capture.display.id);
    const session = {
      id: crypto.randomUUID ? crypto.randomUUID() : crypto.randomBytes(16).toString("hex"),
      captures: new Map([[displayId, capture]]),
      overlays: new Map(),
      resolve,
      reject,
      finished: false,
    };
    spatialSession = session;
    const bounds = capture.display.bounds;
    const overlay = new BrowserWindow({
      x: bounds.x,
      y: bounds.y,
      width: bounds.width,
      height: bounds.height,
      frame: false,
      transparent: false,
      resizable: false,
      movable: false,
      minimizable: false,
      maximizable: false,
      closable: true,
      skipTaskbar: true,
      alwaysOnTop: true,
      fullscreenable: false,
      hasShadow: false,
      show: false,
      webPreferences: {
        preload: path.join(__dirname, "spatial-preload.js"),
        contextIsolation: true,
        nodeIntegration: false,
        // The overlay receives an in-memory screenshot and must remain a
        // sandboxed, one-purpose drawing surface.
        sandbox: true,
      },
    });
    lockWindowToLocalFile(overlay, SPATIAL_OVERLAY_RENDERER_PATH);
    session.overlays.set(displayId, overlay);
    overlay.setAlwaysOnTop(true, "screen-saver");
    overlay.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
    overlay.on("closed", () => {
      if (!session.finished) finishSpatialSession(session, null, new Error("Screen-region selection cancelled."));
    });
    overlay.webContents.once("did-finish-load", () => {
      if (session.finished || overlay.isDestroyed()) return;
      // This base64 image is sent only to the one local overlay rendering the
      // display the user explicitly chose; it is cleared with the session.
      overlay.webContents.send("spatial:init", {
        sessionId: session.id,
        displayId,
        image: capture.thumbnail.toDataURL(),
      });
      overlay.show();
      overlay.focus();
      // Keep the visual-only local-state bubble above the selector without
      // taking focus or intercepting the lasso. Its click-through surface
      // makes this safe even while the user is drawing a region.
      if (cursorBuddyStateKey === "spatial_selecting") publishCursorBuddyState();
    });
    overlay.loadFile(SPATIAL_OVERLAY_RENDERER_PATH).catch((error) => {
      finishSpatialSession(session, null, new Error(`Could not open screen selector: ${error.message}`));
    });
  });
}

ipcMain.handle("spatial:select", async (event) => {
  if (!isTrustedMainRenderer(event)) {
    return { ok: false, error: "untrusted spatial selection request" };
  }
  setCursorBuddyState("spatial_selecting");
  try {
    const item = await startSpatialSelection();
    return { ok: true, item };
  } catch (error) {
    setCursorBuddyState("spatial_error");
    return { ok: false, error: String((error && error.message) || error) };
  }
});

ipcMain.on("spatial:cancel", (event, payload) => {
  const session = spatialSession;
  if (!session || !payload || payload.sessionId !== session.id) return;
  const overlay = session.overlays.get(String(payload.displayId));
  if (!overlay || overlay.isDestroyed() || overlay.webContents.id !== event.sender.id) return;
  finishSpatialSession(session, null, new Error("Screen-region selection cancelled."));
});

ipcMain.on("spatial:complete", (event, payload) => {
  const session = spatialSession;
  if (!session || !payload || payload.sessionId !== session.id) return;
  const displayId = String(payload.displayId || "");
  const overlay = session.overlays.get(displayId);
  const capture = session.captures.get(displayId);
  if (!overlay || !capture || overlay.isDestroyed() || overlay.webContents.id !== event.sender.id) return;
  try {
    const item = makeSpatialContextItem(capture, payload.points);
    finishSpatialSession(session, item, null);
    // The pointer-up position is within the region the user just drew, so it
    // is a safe local anchor for a short confirmation bubble. No crop details
    // or screen text are exposed to that bubble.
    setCursorBuddyState("spatial_attached", currentCursorPoint());
  } catch (error) {
    finishSpatialSession(session, null, error);
  }
});

// Account IPC is intentionally separate from the forge engine bridge. It is
// callable only from the exact bundled renderer, and returns public identity
// state only—never OAuth codes, access tokens, refresh tokens, or device codes.
ipcMain.handle("account:status", async (event) => {
  if (!isTrustedMainRenderer(event)) return { enabled: false, user: null, error: "untrusted account request" };
  if (!accountReady()) return { enabled: false, user: null, sessionPersistence: "none" };
  try {
    return await desktopAuth.status();
  } catch (error) {
    logFile(`account status failed: ${String((error && error.message) || error)}`);
    return { enabled: true, user: null, sessionPersistence: "memory" };
  }
});

function showDeviceCode({ userCode, verificationUri, expiresIn }) {
  const parent = mainWindow && !mainWindow.isDestroyed() ? mainWindow : undefined;
  // Do not wait for this dialog to close: device polling must continue while
  // the user reads the code and completes the approval in their browser.
  void dialog
    .showMessageBox(parent, {
      type: "info",
      title: "Complete Ollamax sign-in",
      message: "Enter this code in the browser Ollamax opened:",
      detail:
        `${userCode}\n\nFor your security, type the code yourself; it is not included in the browser link. ` +
        `The code expires in about ${Math.ceil(expiresIn / 60)} minute(s).`,
      buttons: ["Continue"],
      defaultId: 0,
      noLink: true,
    })
    .catch((error) => logFile(`could not show device sign-in code: ${String((error && error.message) || error)}`));
  // verificationUri is validated by DesktopAuth before the browser opens. Keep
  // this reference only for diagnostic clarity; never log the code or tokens.
  void verificationUri;
}

ipcMain.handle("account:signIn", async (event, options = {}) => {
  if (!isTrustedMainRenderer(event)) return { ok: false, error: "untrusted_account_request", message: "Untrusted account request." };
  if (!accountReady()) return { ok: false, error: "no_account_server", message: "Sign-in needs a valid account server." };
  try {
    const result = options && options.device === true
      ? await desktopAuth.signInDevice({ onDeviceCode: showDeviceCode })
      : await desktopAuth.signIn();
    return { ok: true, user: result.user, sessionPersistence: result.sessionPersistence };
  } catch (error) {
    return accountError(error);
  }
});

ipcMain.handle("account:signOut", async (event) => {
  if (!isTrustedMainRenderer(event)) return { ok: false, error: "untrusted_account_request", message: "Untrusted account request." };
  if (!accountReady()) return { ok: true, user: null };
  try {
    return { ok: true, ...(await desktopAuth.signOut()) };
  } catch (error) {
    // A local sign-out should still clear memory/persistent credentials even
    // if the optional server revoke request had a transient failure.
    return accountError(error);
  }
});

// =====================================================================
// Central Hub (#2) — ported from the VS Code extension's HubViewProvider so the
// app surfaces the SAME server-side catalog. Categories/packages are read from
// the account server (public). "Activation" only writes local rules/skills the
// engine already reads (transparent steering). Starring is OPT-IN ONLY: we
// create a star intent and open the browser for the user to consciously review
// and confirm — NEVER automatic (GitHub AUP). Done in the main process (Node).
// =====================================================================
function configDir() {
  const home = os.homedir();
  if (process.platform === "darwin") return path.join(home, "Library", "Application Support");
  if (process.platform === "win32") return process.env.APPDATA || path.join(home, "AppData", "Roaming");
  return process.env.XDG_CONFIG_HOME || path.join(home, ".config");
}
const rulesDir = () => path.join(configDir(), "ollama-forge", "rules");
const skillsDir = () => path.join(configDir(), "ollama-forge", "skills");
const safeBase = (s) => {
  const b = path.basename(String(s || ""));
  return /^[\w.-]+$/.test(b) && b !== "." && b !== ".." ? b : null;
};
const within = (dir, file) => path.resolve(dir, file).startsWith(path.resolve(dir) + path.sep);
const safeHubSlug = (value) => {
  const slug = typeof value === "string" ? value.trim() : "";
  return /^[a-z0-9][a-z0-9_-]{0,80}$/i.test(slug) ? slug : null;
};

async function hubHttp(method, apiPath, body) {
  if (!accountReady()) throw new Error("account server is unavailable");
  const response = await desktopAuth.requestPublic(method, apiPath, body);
  if (response.status < 200 || response.status >= 300) throw new Error(`HTTP ${response.status || 0}`);
  return response.body;
}

function supportReviewUrl(value) {
  const approved = safeExternalUrl(value);
  if (!approved || !accountServer) return null;
  try {
    const candidate = new URL(approved);
    const origin = new URL(accountServer).origin;
    return candidate.origin === origin && /^\/star\/[A-Za-z0-9_-]{12,}$/.test(candidate.pathname) && !candidate.search && !candidate.hash
      ? candidate.toString()
      : null;
  } catch (_) {
    return null;
  }
}

ipcMain.handle("hub:categories", async (event) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted Hub request" };
  if (!accountReady()) return { needsServer: true };
  try {
    const d = await hubHttp("GET", "/api/hub/categories");
    return { categories: d.categories || [] };
  } catch (e) {
    return { error: `Could not load Hub catalog: ${e.message}` };
  }
});

ipcMain.handle("hub:package", async (event, slug) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted Hub request" };
  slug = safeHubSlug(slug);
  if (!slug) return { error: "invalid Hub package" };
  if (!accountReady()) return { needsServer: true };
  try {
    return { pkg: await hubHttp("GET", `/api/hub/package/${encodeURIComponent(slug)}`) };
  } catch (e) {
    return { error: `Could not load package: ${e.message}` };
  }
});

// Apply a package = write its rules + skills into the local config dirs the
// engine reads. Transparent, inspectable, reversible (delete the files).
ipcMain.handle("hub:activate", async (event, slug) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted Hub request" };
  slug = safeHubSlug(slug);
  if (!slug) return { error: "invalid Hub package" };
  if (!accountReady()) return { needsServer: true };
  let pkg;
  try {
    pkg = await hubHttp("GET", `/api/hub/package/${encodeURIComponent(slug)}`);
  } catch (e) {
    return { error: `Activate failed: ${e.message}` };
  }
  try {
    fs.mkdirSync(rulesDir(), { recursive: true });
    fs.mkdirSync(skillsDir(), { recursive: true });
    const rulesFile = `hub-${slug}.md`;
    if (within(rulesDir(), rulesFile)) {
      fs.writeFileSync(path.join(rulesDir(), rulesFile), pkg.rules || "", "utf8");
    }
    let skillCount = 0;
    for (const skill of pkg.skills || []) {
      const b = safeBase(skill && skill.name);
      if (!b) continue;
      const file = `${b}.json`;
      if (!within(skillsDir(), file)) continue;
      fs.writeFileSync(path.join(skillsDir(), file), JSON.stringify(skill, null, 2), "utf8");
      skillCount++;
    }
    return {
      activated: true,
      slug,
      name: pkg.name,
      counts: { rules: (pkg.counts && pkg.counts.rules) || 0, skills: skillCount, references: (pkg.references || []).length },
    };
  } catch (e) {
    return { error: `Writing package failed: ${e.message}` };
  }
});

// Opt-in "support maintainers": create a star intent, open the browser for the
// user to review + consciously star. NEVER automatic. Its bearer token is
// retrieved only inside the main process; a renderer cannot supply or read it.
ipcMain.handle("hub:support", async (event, { slug, repos } = {}) => {
  if (!isTrustedMainRenderer(event)) return { ok: false, error: "untrusted support request" };
  slug = safeHubSlug(slug);
  let encodedRepos = "";
  try {
    encodedRepos = JSON.stringify(repos);
  } catch (_) {}
  if (!slug || !Array.isArray(repos) || repos.length > 100 || Buffer.byteLength(encodedRepos, "utf8") > 256 * 1024) {
    return { ok: false, error: "invalid support request" };
  }
  if (!accountReady()) return { needsServer: true };
  try {
    const res = await desktopAuth.authenticatedRequest("POST", "/api/star/intent", { repos, category: slug });
    if (!res.authenticated) return { needsSignIn: true };
    const reviewUrl = res.body && supportReviewUrl(res.body.url);
    if (res.status >= 200 && res.status < 300 && reviewUrl) {
      await openValidatedExternal(reviewUrl);
      return { ok: true };
    }
    return { error: "Could not create the support request." };
  } catch (e) {
    return { error: `Support failed: ${e.message}` };
  }
});

// =====================================================================
// IDE workspace (#3) — open a folder, browse files, edit (Monaco in the
// renderer), and an integrated terminal (xterm.js ↔ node-pty here). All file
// file access is scoped to the opened workspace root. This is a normal in-app IDE
// terminal — it does NOT contradict "no user-facing CLI install" (that was about
// distribution, not having an editor terminal).
// =====================================================================
const IGNORE_DIRS = new Set([".git", "node_modules", "target", "dist", ".next", "release", ".cache"]);
const MAX_FILE_BYTES = 2 * 1024 * 1024;

function canonicalWorkspaceRoot(p) {
  const resolved = path.resolve(p);
  const realpath = fs.realpathSync.native || fs.realpathSync;
  const root = realpath(resolved);
  if (!fs.statSync(root).isDirectory()) throw new Error("selected path is not a folder");
  return root;
}

// Keep folder changes serialized: stopping a process, waiting for it to exit,
// and then starting its replacement must be one indivisible workspace change.
let workspaceSwitchQueue = Promise.resolve();
async function switchWorkspace(nextRoot) {
  const previousRoot = workspaceRoot;
  workspaceRoot = nextRoot;
  forgeWorkspaceRoot = null;
  notifyForgeConfig();
  try {
    await restartForgeServe(nextRoot);
    forgeWorkspaceRoot = nextRoot;
    notifyForgeConfig();
    return { root: workspaceRoot, name: path.basename(workspaceRoot) || workspaceRoot };
  } catch (e) {
    // Restore the prior usable session (or the chat-only home server) rather
    // than leaving a failed workspace switch with no backend at all.
    workspaceRoot = previousRoot;
    forgeWorkspaceRoot = null;
    let restoreError = null;
    try {
      await restartForgeServe(previousRoot || app.getPath("home"));
      forgeWorkspaceRoot = previousRoot || null;
    } catch (restoreFailure) {
      restoreError = restoreFailure;
    }
    notifyForgeConfig();
    const detail = restoreError
      ? `\n\nThe previous local engine could not be restored: ${restoreError.message}`
      : "\n\nYour previous workspace/session was restored.";
    logFile(`workspace switch failed: ${e.message}`);
    dialog.showErrorBox("Ollamax", `Could not start the local engine for this folder:\n${e.message}${detail}`);
    return null;
  }
}

ipcMain.handle("ide:openFolder", async (event) => {
  if (!isTrustedMainRenderer(event)) return null;
  const r = await dialog.showOpenDialog(mainWindow, { properties: ["openDirectory"] });
  if (r.canceled || !r.filePaths[0]) return null;
  let nextRoot;
  try {
    nextRoot = canonicalWorkspaceRoot(r.filePaths[0]);
  } catch (e) {
    dialog.showErrorBox("Ollamax", `Could not open that folder:\n${e.message}`);
    return null;
  }
  const change = workspaceSwitchQueue.then(() => switchWorkspace(nextRoot));
  workspaceSwitchQueue = change.catch(() => undefined);
  return change;
});

// One directory level (lazy tree expansion). Dirs first, then files, sorted.
// Every direct-IDE path rejects symlink components just like the agent preview
// path, so an in-root symlink cannot become a bridge to another location.
ipcMain.handle("ide:readDir", async (event, dir) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted IDE request" };
  const resolved = resolveWorkspacePath(workspaceRoot, dir || workspaceRoot, { allowRoot: true });
  if (resolved.error) return { error: resolved.error };
  try {
    const stat = fs.lstatSync(resolved.target);
    if (!stat.isDirectory()) return { error: "path is not a directory" };
    const ents = fs.readdirSync(resolved.target, { withFileTypes: true });
    const out = ents
      .filter((e) => !e.isSymbolicLink() && !(e.isDirectory() && IGNORE_DIRS.has(e.name)) && !e.name.startsWith("."))
      .map((e) => ({ name: e.name, path: path.join(resolved.target, e.name), dir: e.isDirectory() }))
      .sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
    return { entries: out };
  } catch (e) {
    return { error: e.message };
  }
});

ipcMain.handle("ide:readFile", async (event, p) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted IDE request" };
  const resolved = resolveWorkspacePath(workspaceRoot, p);
  if (resolved.error) return { error: resolved.error };
  try {
    const st = fs.lstatSync(resolved.target);
    if (!st.isFile()) return { error: "path is not a regular file" };
    if (st.size > MAX_FILE_BYTES) return { error: "file too large to open in-editor" };
    return { content: fs.readFileSync(resolved.target, "utf8"), path: resolved.target };
  } catch (e) {
    return { error: e.message };
  }
});

ipcMain.handle("ide:writeFile", async (event, { path: p, content } = {}) => {
  if (!isTrustedMainRenderer(event)) return { error: "untrusted IDE request" };
  if (typeof content !== "string" || Buffer.byteLength(content, "utf8") > MAX_FILE_BYTES) {
    return { error: "file content must be text no larger than 2 MiB" };
  }
  const resolved = resolveWorkspacePath(workspaceRoot, p, { allowMissingFinal: true });
  if (resolved.error) return { error: resolved.error };
  try {
    try {
      const stat = fs.lstatSync(resolved.target);
      if (!stat.isFile()) return { error: "path is not a regular file" };
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }
    const flags =
      fs.constants.O_WRONLY |
      fs.constants.O_CREAT |
      fs.constants.O_TRUNC |
      (fs.constants.O_NOFOLLOW || 0);
    const fd = fs.openSync(resolved.target, flags, 0o666);
    try {
      if (!fs.fstatSync(fd).isFile()) return { error: "path is not a regular file" };
      fs.writeFileSync(fd, content, "utf8");
    } finally {
      fs.closeSync(fd);
    }
    return { ok: true };
  } catch (e) {
    return { error: e.message };
  }
});

// Return an in-root, non-symlink target for the *preview* path. The Rust file
// tools repeat these checks before writing; this prevents the review dialog
// from being tricked into showing an unrelated file in the meantime.
function previewWorkspacePath(rel) {
  if (!workspaceRoot || forgeWorkspaceRoot !== workspaceRoot || !baseUrl) {
    return { error: "opened workspace is not ready for agent edits" };
  }
  return resolveWorkspacePath(workspaceRoot, rel, {
    allowMissingFinal: true,
    requireRelative: true,
  });
}

function lineCount(text) {
  return text ? text.split(/\r\n|\n|\r/).length : 0;
}

function diffLine(text) {
  const clean = text.replace(/\0/g, "�").replace(/\t/g, "  ");
  return clean.length > 180 ? `${clean.slice(0, 177)}…` : clean;
}

// This deliberately summarizes only the changed region so a native dialog is
// reviewable; it never writes a temp file or the workspace file.
function conciseDiff(current, proposed, isNew) {
  const before = current ? current.split(/\r\n|\n|\r/) : [];
  const after = proposed ? proposed.split(/\r\n|\n|\r/) : [];
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let suffix = 0;
  while (
    suffix < before.length - start &&
    suffix < after.length - start &&
    before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
  ) {
    suffix++;
  }
  const removed = before.slice(start, before.length - suffix);
  const added = after.slice(start, after.length - suffix);
  const lines = [
    `${isNew ? "New file" : "Existing file"} · ${lineCount(current)} → ${lineCount(proposed)} lines`,
    `${Buffer.byteLength(current, "utf8")} → ${Buffer.byteLength(proposed, "utf8")} bytes`,
    "--- current",
    "+++ proposed",
    `@@ changed near line ${start + 1} @@`,
  ];
  const append = (prefix, values) => {
    values.slice(0, 8).forEach((line) => lines.push(`${prefix} ${diffLine(line)}`));
    if (values.length > 8) lines.push(`${prefix} … ${values.length - 8} more line(s)`);
  };
  append("-", removed);
  append("+", added);
  return lines.join("\n").slice(0, 3500);
}

function prepareEditPreview(tool, args) {
  if (!args || typeof args !== "object") return { error: "missing tool arguments" };
  if (tool !== "fs_write" && tool !== "fs_edit") return { error: "unsupported edit tool" };

  const pathResult = previewWorkspacePath(args.path);
  if (pathResult.error) return pathResult;
  let current = "";
  let isNew = false;
  try {
    const stat = fs.lstatSync(pathResult.target);
    if (!stat.isFile()) return { error: "target is not a regular file" };
    if (stat.size > MAX_FILE_BYTES) return { error: "target is too large to review" };
    current = fs.readFileSync(pathResult.target, "utf8");
  } catch (e) {
    if (!e || e.code !== "ENOENT") return { error: `could not read target: ${e.message}` };
    isNew = true;
  }

  let proposed;
  if (tool === "fs_edit") {
    if (typeof args.old_string !== "string" || typeof args.new_string !== "string" || !args.old_string) {
      return { error: "fs_edit needs non-empty old_string and string new_string" };
    }
    const first = current.indexOf(args.old_string);
    const second = first < 0 ? -1 : current.indexOf(args.old_string, first + args.old_string.length);
    if (first < 0 || second >= 0) {
      return { error: first < 0 ? "edit target text was not found" : "edit target text is not unique" };
    }
    proposed = current.slice(0, first) + args.new_string + current.slice(first + args.old_string.length);
  } else {
    if (typeof args.content !== "string") return { error: "fs_write needs string content" };
    proposed = args.content;
  }
  if (Buffer.byteLength(proposed, "utf8") > MAX_FILE_BYTES) return { error: "proposed content is too large to review" };
  if (proposed === current) return { error: "proposal does not change the file" };

  return {
    target: pathResult.target,
    relative: pathResult.relative,
    isNew,
    detail: conciseDiff(current, proposed, isNew),
  };
}

ipcMain.handle("ide:previewEdit", async (event, { tool, args } = {}) => {
  if (!isTrustedMainRenderer(event)) return { decision: false, reason: "untrusted IDE request" };
  const preview = prepareEditPreview(tool, args);
  if (preview.error) {
    logFile(`blocked agent preview: ${preview.error}`);
    return { decision: false, reason: preview.error };
  }
  const options = {
    type: "question",
    title: "Review Ollamax change",
    message: `Apply Ollamax's ${preview.isNew ? "new file" : "change"} to ${preview.relative}?`,
    detail: `Nothing has been written yet. Review this concise diff:\n\n${preview.detail}`,
    buttons: ["Apply change", "Discard"],
    defaultId: 1,
    cancelId: 1,
    noLink: true,
  };
  try {
    const parent = mainWindow && !mainWindow.isDestroyed() ? mainWindow : null;
    const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
    return { decision: result.response === 0 };
  } catch (e) {
    logFile(`agent preview dialog failed: ${e.message}`);
    return { decision: false, reason: "review dialog failed" };
  }
});

// Integrated terminal via node-pty (a NATIVE module — guarded so the app still
// runs if it isn't rebuilt; the terminal panel then shows an install hint).
let ptyProc = null;
function ptyDimension(value, fallback, minimum, maximum) {
  const rounded = Number.isFinite(value) ? Math.round(value) : fallback;
  return Math.max(minimum, Math.min(maximum, rounded));
}

ipcMain.handle("pty:start", async (event, size = {}) => {
  if (!isTrustedMainRenderer(event)) return { ok: false, error: "untrusted terminal request" };
  const cols = ptyDimension(size && size.cols, 80, 20, 500);
  const rows = ptyDimension(size && size.rows, 24, 5, 200);
  let nodePty;
  try {
    nodePty = require("node-pty");
  } catch (_) {
    return { ok: false, error: "node-pty not built (run npm install + electron-rebuild)" };
  }
  try {
    if (ptyProc) {
      try { ptyProc.kill(); } catch (_) {}
    }
    const shell = process.platform === "win32" ? "powershell.exe" : process.env.SHELL || "/bin/zsh";
    ptyProc = nodePty.spawn(shell, [], {
      name: "xterm-color",
      cols,
      rows,
      cwd: workspaceRoot || app.getPath("home"),
      env: process.env,
    });
    ptyProc.onData((d) => mainWindow && mainWindow.webContents.send("pty:data", d));
    ptyProc.onExit(() => {
      mainWindow && mainWindow.webContents.send("pty:exit");
      ptyProc = null;
    });
    return { ok: true };
  } catch (e) {
    return { ok: false, error: e.message };
  }
});
ipcMain.on("pty:write", (event, data) => {
  if (!isTrustedMainRenderer(event) || typeof data !== "string" || Buffer.byteLength(data, "utf8") > 64 * 1024) return;
  if (ptyProc) ptyProc.write(data);
});
ipcMain.on("pty:resize", (event, size = {}) => {
  if (!isTrustedMainRenderer(event)) return;
  try {
    ptyProc && ptyProc.resize(ptyDimension(size.cols, 80, 20, 500), ptyDimension(size.rows, 24, 5, 200));
  } catch (_) {}
});
ipcMain.handle("pty:kill", async (event) => {
  if (!isTrustedMainRenderer(event)) return { ok: false, error: "untrusted terminal request" };
  if (ptyProc) { try { ptyProc.kill(); } catch (_) {} ptyProc = null; }
  return { ok: true };
});

app.whenReady().then(async () => {
  installDesktopSecurityPolicy();
  initializeDesktopAuth();
  try {
    await startForgeServe();
  } catch (e) {
    logFile(`startup error: ${e.message}`);
    dialog.showErrorBox("Ollamax", `Could not start the local engine:\n${e.message}`);
  }
  createWindow();
  registerVoiceToggleShortcut();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
      registerVoiceToggleShortcut();
    }
  });
});

function shutdown() {
  if (spatialSession) {
    finishSpatialSession(
      spatialSession,
      null,
      new Error("Screen-region selection cancelled because Ollamax is shutting down.")
    );
  }
  unregisterVoiceToggleShortcut();
  destroyCursorBuddy();
  if (forgeProc) {
    try {
      forgeProc.kill();
    } catch (_) {}
    forgeProc = null;
  }
  if (ptyProc) {
    try {
      ptyProc.kill();
    } catch (_) {}
    ptyProc = null;
  }
}
app.on("window-all-closed", () => {
  shutdown();
  if (process.platform !== "darwin") app.quit();
});
app.on("before-quit", shutdown);
process.on("exit", shutdown);

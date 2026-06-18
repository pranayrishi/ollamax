// Ollama-Forge desktop app — Electron main process.
//
// The app is a thin SHELL around the existing engine. On launch it spawns the
// bundled `forge` binary as `forge serve --port 0` (a HIDDEN local HTTP+SSE
// backend — no terminal, never on PATH), reads its ephemeral port from the
// `FORGE_SERVE_READY {json}` line, then opens a window hosting the existing chat
// + Hub UI, which talks to that local server. Inference stays local
// (app → forge serve → Ollama). Modeled on the Sattva AI desktop app.

const { app, BrowserWindow, ipcMain, dialog, shell } = require("electron");
const path = require("path");
const fs = require("fs");
const cp = require("child_process");
const http = require("http");

const isDev = !app.isPackaged;
let mainWindow = null;
let forgeProc = null;
let baseUrl = null;

// Account server for sign-in + the Central Hub (identity only; never inference).
// Configurable; empty disables account features gracefully.
const ACCOUNT_SERVER = process.env.FORGE_ACCOUNT_SERVER || "";

function logFile(msg) {
  try {
    fs.appendFileSync(
      path.join(app.getPath("userData"), "forge-app.log"),
      `${new Date().toISOString()} ${msg}\n`
    );
  } catch (_) {}
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

function startForgeServe() {
  return new Promise((resolve, reject) => {
    const bin = forgeBinaryPath();
    if (!fs.existsSync(bin)) {
      reject(new Error(`engine not found at ${bin}`));
      return;
    }
    logFile(`launching ${bin} serve --port 0`);
    forgeProc = cp.spawn(bin, ["serve", "--port", "0"], {
      cwd: app.getPath("home"),
      env: { ...process.env },
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });

    let buf = "";
    const onData = (d) => {
      buf += d.toString();
      let i;
      while ((i = buf.indexOf("\n")) !== -1) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        const m = line.match(/FORGE_SERVE_READY\s+(\{.*\})/);
        if (m) {
          try {
            const info = JSON.parse(m[1]);
            baseUrl = `http://127.0.0.1:${info.port}`;
            logFile(`forge serve ready on ${baseUrl} (v${info.version})`);
            resolve(baseUrl);
          } catch (e) {
            reject(new Error(`bad ready line: ${e}`));
          }
        }
      }
    };
    forgeProc.stdout.on("data", onData);
    forgeProc.stderr.on("data", (d) => logFile(`[forge] ${d.toString().trim()}`));
    forgeProc.on("error", reject);
    forgeProc.on("exit", (code) => {
      logFile(`forge serve exited (${code})`);
      forgeProc = null;
    });
    setTimeout(() => baseUrl || reject(new Error("forge serve did not become ready in 15s")), 15000);
  });
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1100,
    height: 760,
    minWidth: 720,
    minHeight: 480,
    title: "Ollama-Forge",
    backgroundColor: "#0b0d12",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  mainWindow.loadFile(path.join(__dirname, "renderer", "index.html"));
  if (isDev) mainWindow.webContents.openDevTools({ mode: "detach" });
  mainWindow.on("closed", () => (mainWindow = null));
}

// --- IPC: things the renderer can't do itself (Node/Electron only) --------

ipcMain.handle("forge:config", () => ({ baseUrl, accountServer: ACCOUNT_SERVER }));

ipcMain.handle("forge:pickFiles", async () => {
  const r = await dialog.showOpenDialog(mainWindow, {
    properties: ["openFile", "multiSelections"],
  });
  if (r.canceled) return [];
  return r.filePaths.map((p) => {
    let content = "";
    try {
      content = fs.readFileSync(p, "utf8").slice(0, 200_000);
    } catch (_) {}
    return { path: p, label: path.basename(p), content };
  });
});

ipcMain.handle("forge:openExternal", (_e, url) => shell.openExternal(url));

// Desktop sign-in: open the account server's desktop auth in the system browser
// (the existing GitHub/Google OAuth loopback + PKCE flow). The loopback server
// that receives the token is started by the engine; here we just open the URL.
ipcMain.handle("forge:signIn", async (_e, { device } = {}) => {
  if (!ACCOUNT_SERVER) return { ok: false, error: "no_account_server" };
  const startUrl = device
    ? `${ACCOUNT_SERVER}/desktop/activate`
    : `${ACCOUNT_SERVER}/api/desktop/start?redirect_uri=http://127.0.0.1:0`;
  await shell.openExternal(startUrl);
  return { ok: true };
});

app.whenReady().then(async () => {
  try {
    await startForgeServe();
  } catch (e) {
    logFile(`startup error: ${e.message}`);
    dialog.showErrorBox("Ollama-Forge", `Could not start the local engine:\n${e.message}`);
  }
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

function shutdown() {
  if (forgeProc) {
    try {
      forgeProc.kill();
    } catch (_) {}
    forgeProc = null;
  }
}
app.on("window-all-closed", () => {
  shutdown();
  if (process.platform !== "darwin") app.quit();
});
app.on("before-quit", shutdown);
process.on("exit", shutdown);

// Keep a reference so http isn't tree-shaken (used by the health probe path).
void http;

// Ollamax desktop app — Electron main process.
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
const os = require("os");
const cp = require("child_process");
const http = require("http");
const https = require("https");
const { URL } = require("url");

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
    title: "Ollamax",
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

function hubHttp(method, urlStr, body, bearer) {
  return new Promise((resolve, reject) => {
    const url = new URL(urlStr);
    const lib = url.protocol === "https:" ? https : http;
    const data = body ? JSON.stringify(body) : null;
    const headers = {};
    if (data) {
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(data);
    }
    if (bearer) headers["Authorization"] = `Bearer ${bearer}`;
    const req = lib.request(
      { method, hostname: url.hostname, port: url.port || (url.protocol === "https:" ? 443 : 80), path: url.pathname + url.search, headers },
      (res) => {
        let raw = "";
        res.setEncoding("utf8");
        res.on("data", (c) => (raw += c));
        res.on("end", () => {
          if ((res.statusCode || 0) >= 400) return reject(new Error(`HTTP ${res.statusCode}`));
          try {
            resolve(raw ? JSON.parse(raw) : {});
          } catch (_) {
            reject(new Error("bad JSON"));
          }
        });
      }
    );
    req.on("error", reject);
    if (data) req.write(data);
    req.end();
  });
}

ipcMain.handle("hub:categories", async () => {
  if (!ACCOUNT_SERVER) return { needsServer: true };
  try {
    const d = await hubHttp("GET", `${ACCOUNT_SERVER}/api/hub/categories`);
    return { categories: d.categories || [] };
  } catch (e) {
    return { error: `Could not load Hub catalog: ${e.message}` };
  }
});

ipcMain.handle("hub:package", async (_e, slug) => {
  if (!ACCOUNT_SERVER) return { needsServer: true };
  try {
    return { pkg: await hubHttp("GET", `${ACCOUNT_SERVER}/api/hub/package/${encodeURIComponent(slug)}`) };
  } catch (e) {
    return { error: `Could not load package: ${e.message}` };
  }
});

// Apply a package = write its rules + skills into the local config dirs the
// engine reads. Transparent, inspectable, reversible (delete the files).
ipcMain.handle("hub:activate", async (_e, slug) => {
  if (!ACCOUNT_SERVER) return { needsServer: true };
  let pkg;
  try {
    pkg = await hubHttp("GET", `${ACCOUNT_SERVER}/api/hub/package/${encodeURIComponent(slug)}`);
  } catch (e) {
    return { error: `Activate failed: ${e.message}` };
  }
  try {
    fs.mkdirSync(rulesDir(), { recursive: true });
    fs.mkdirSync(skillsDir(), { recursive: true });
    const rulesFile = `hub-${safeBase(slug) || "package"}.md`;
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
// user to review + consciously star. NEVER automatic. Needs the app token; if
// sign-in isn't wired/available yet, ask the user to sign in.
ipcMain.handle("hub:support", async (_e, { slug, repos, token } = {}) => {
  if (!ACCOUNT_SERVER) return { needsServer: true };
  if (!token) return { needsSignIn: true };
  try {
    const res = await hubHttp("POST", `${ACCOUNT_SERVER}/api/star/intent`, { repos, category: slug }, token);
    if (res.url) {
      await shell.openExternal(res.url);
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
// access is sandboxed to the opened workspace root. This is a normal in-app IDE
// terminal — it does NOT contradict "no user-facing CLI install" (that was about
// distribution, not having an editor terminal).
// =====================================================================
let workspaceRoot = null;
const IGNORE_DIRS = new Set([".git", "node_modules", "target", "dist", ".next", "release", ".cache"]);
const MAX_FILE_BYTES = 2 * 1024 * 1024;

function inWorkspace(p) {
  if (!workspaceRoot) return false;
  const r = path.resolve(p);
  return r === workspaceRoot || r.startsWith(workspaceRoot + path.sep);
}

ipcMain.handle("ide:openFolder", async () => {
  const r = await dialog.showOpenDialog(mainWindow, { properties: ["openDirectory"] });
  if (r.canceled || !r.filePaths[0]) return null;
  workspaceRoot = path.resolve(r.filePaths[0]);
  return { root: workspaceRoot, name: path.basename(workspaceRoot) };
});

// One directory level (lazy tree expansion). Dirs first, then files, sorted.
ipcMain.handle("ide:readDir", async (_e, dir) => {
  const target = dir ? path.resolve(dir) : workspaceRoot;
  if (!target || !inWorkspace(target)) return { error: "outside workspace" };
  try {
    const ents = fs.readdirSync(target, { withFileTypes: true });
    const out = ents
      .filter((e) => !(e.isDirectory() && IGNORE_DIRS.has(e.name)) && !e.name.startsWith("."))
      .map((e) => ({ name: e.name, path: path.join(target, e.name), dir: e.isDirectory() }))
      .sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
    return { entries: out };
  } catch (e) {
    return { error: e.message };
  }
});

ipcMain.handle("ide:readFile", async (_e, p) => {
  if (!inWorkspace(p)) return { error: "outside workspace" };
  try {
    const st = fs.statSync(p);
    if (st.size > MAX_FILE_BYTES) return { error: "file too large to open in-editor" };
    return { content: fs.readFileSync(p, "utf8"), path: p };
  } catch (e) {
    return { error: e.message };
  }
});

ipcMain.handle("ide:writeFile", async (_e, { path: p, content }) => {
  if (!inWorkspace(p)) return { error: "outside workspace" };
  try {
    fs.writeFileSync(p, content, "utf8");
    return { ok: true };
  } catch (e) {
    return { error: e.message };
  }
});

// Integrated terminal via node-pty (a NATIVE module — guarded so the app still
// runs if it isn't rebuilt; the terminal panel then shows an install hint).
let ptyProc = null;
ipcMain.handle("pty:start", async (_e, { cols, rows } = {}) => {
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
      cols: cols || 80,
      rows: rows || 24,
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
ipcMain.on("pty:write", (_e, data) => ptyProc && ptyProc.write(data));
ipcMain.on("pty:resize", (_e, { cols, rows }) => {
  try { ptyProc && ptyProc.resize(cols, rows); } catch (_) {}
});
ipcMain.handle("pty:kill", async () => {
  if (ptyProc) { try { ptyProc.kill(); } catch (_) {} ptyProc = null; }
  return { ok: true };
});

app.whenReady().then(async () => {
  try {
    await startForgeServe();
  } catch (e) {
    logFile(`startup error: ${e.message}`);
    dialog.showErrorBox("Ollamax", `Could not start the local engine:\n${e.message}`);
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

// Keep a reference so http isn't tree-shaken (used by the health probe path).
void http;

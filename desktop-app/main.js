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
const crypto = require("crypto");
const http = require("http");
const https = require("https");
const { URL } = require("url");

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
// Private capability shared only with the bundled engine process and this
// trusted Electron shell. It prevents arbitrary web content from driving the
// loopback agent API.
const forgeApiToken = crypto.randomBytes(32).toString("hex");

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

function forgeConfig() {
  const workspaceReady = !!baseUrl && !!workspaceRoot && forgeWorkspaceRoot === workspaceRoot;
  return {
    baseUrl,
    accountServer: ACCOUNT_SERVER,
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
    },
  });
  mainWindow.loadFile(path.join(__dirname, "renderer", "index.html"));
  if (isDev) mainWindow.webContents.openDevTools({ mode: "detach" });
  mainWindow.on("closed", () => (mainWindow = null));
}

// --- IPC: things the renderer can't do itself (Node/Electron only) --------

ipcMain.handle("forge:config", () => forgeConfig());

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
// file access is scoped to the opened workspace root. This is a normal in-app IDE
// terminal — it does NOT contradict "no user-facing CLI install" (that was about
// distribution, not having an editor terminal).
// =====================================================================
const IGNORE_DIRS = new Set([".git", "node_modules", "target", "dist", ".next", "release", ".cache"]);
const MAX_FILE_BYTES = 2 * 1024 * 1024;

function isWithinWorkspace(root, target) {
  const rel = path.relative(root, target);
  return rel === "" || (!rel.startsWith(`..${path.sep}`) && rel !== ".." && !path.isAbsolute(rel));
}

function inWorkspace(p) {
  return !!workspaceRoot && typeof p === "string" && isWithinWorkspace(workspaceRoot, path.resolve(p));
}

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

ipcMain.handle("ide:openFolder", async () => {
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

// Return an in-root, non-symlink target for the *preview* path. The Rust file
// tools repeat these checks before writing; this prevents the review dialog
// from being tricked into showing an unrelated file in the meantime.
function previewWorkspacePath(rel) {
  if (!workspaceRoot || forgeWorkspaceRoot !== workspaceRoot || !baseUrl) {
    return { error: "opened workspace is not ready for agent edits" };
  }
  if (typeof rel !== "string" || !rel.trim() || rel.includes("\0")) {
    return { error: "a non-empty relative path is required" };
  }
  if (
    path.isAbsolute(rel) ||
    path.win32.isAbsolute(rel) ||
    path.posix.isAbsolute(rel) ||
    /^[a-zA-Z]:/.test(rel) ||
    rel.split(/[\\/]+/).some((part) => part === "..")
  ) {
    return { error: "path must stay inside the opened workspace" };
  }
  const target = path.resolve(workspaceRoot, rel);
  if (target === workspaceRoot || !isWithinWorkspace(workspaceRoot, target)) {
    return { error: "path must name a file inside the opened workspace" };
  }

  // Match the engine's defense-in-depth symlink policy for both existing files
  // and newly proposed nested files (stop once the first new component appears).
  const relative = path.relative(workspaceRoot, target);
  let current = workspaceRoot;
  for (const part of relative.split(path.sep)) {
    if (!part) continue;
    current = path.join(current, part);
    try {
      if (fs.lstatSync(current).isSymbolicLink()) {
        return { error: "paths containing symlinks cannot be reviewed or changed" };
      }
    } catch (e) {
      if (e && e.code === "ENOENT") break;
      return { error: `could not inspect proposed path: ${e.message}` };
    }
  }
  return { target, relative: relative.replace(/\\/g, "/") };
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

ipcMain.handle("ide:previewEdit", async (_e, { tool, args } = {}) => {
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

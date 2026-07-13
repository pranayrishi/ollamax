// IDE workspace (#3): file explorer + editor (Monaco, with a textarea fallback)
// + integrated terminal (xterm.js ↔ node-pty). Reuses proven components rather
// than building an editor from scratch (Monaco/xterm). Access is scoped
// to the folder the user opens; file access goes through preload IPC.
(function () {
  const ide = window.forgeNative && window.forgeNative.ide;
  const ptyApi = window.forgeNative && window.forgeNative.pty;

  let monaco = null;        // set once Monaco loads (else null → textarea)
  let editor = null;        // Monaco editor instance
  let textarea = null;      // fallback editor
  let activePath = null;
  const openModels = new Map(); // path -> { content, model? }

  const $ = (id) => document.getElementById(id);

  // ---- Monaco (lazy; falls back to a styled textarea if unavailable) -------
  function loadMonaco() {
    return new Promise((resolve) => {
      if (monaco) return resolve(monaco);
      if (window.monaco) {
        monaco = window.monaco;
        return resolve(monaco);
      }
      // Monaco's AMD loader is staged at renderer/vs by prepare.mjs when the
      // `monaco-editor` dependency is installed. If it's absent, resolve null.
      const loaderEl = document.createElement("script");
      loaderEl.src = "vs/loader.js";
      loaderEl.onload = () => {
        try {
          window.MonacoEnvironment = {
            getWorkerUrl: () => "vs/base/worker/workerMain.js",
          };
          // eslint-disable-next-line no-undef
          require.config({ paths: { vs: "vs" } });
          // eslint-disable-next-line no-undef
          require(["vs/editor/editor.main"], () => {
            monaco = window.monaco;
            resolve(monaco);
          });
        } catch (_) {
          resolve(null);
        }
      };
      loaderEl.onerror = () => resolve(null); // not installed → fallback
      document.head.appendChild(loaderEl);
    });
  }

  function langFor(p) {
    const ext = (p.split(".").pop() || "").toLowerCase();
    const map = { rs: "rust", ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", py: "python", go: "go", json: "json", md: "markdown", css: "css", html: "html", sh: "shell", yml: "yaml", yaml: "yaml", toml: "ini", sql: "sql", c: "c", h: "c", cpp: "cpp" };
    return map[ext] || "plaintext";
  }

  async function ensureEditor() {
    if (editor || textarea) return;
    const host = $("editor-host");
    const m = await loadMonaco();
    if (m) {
      editor = m.editor.create(host, {
        value: "", language: "plaintext", theme: "vs-dark",
        automaticLayout: true, fontSize: 13, minimap: { enabled: false },
      });
      editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, saveActive);
    } else {
      // Fallback editor — always works without the Monaco dependency.
      textarea = document.createElement("textarea");
      textarea.id = "editor-fallback";
      textarea.spellcheck = false;
      textarea.addEventListener("keydown", (e) => {
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") { e.preventDefault(); saveActive(); }
      });
      host.appendChild(textarea);
      const hint = $("editor-hint");
      if (hint) hint.textContent = "Monaco not installed — using a basic editor. Run npm install in desktop-app/.";
    }
  }

  function setEditorContent(content, path) {
    if (editor) {
      const m = monaco;
      let model = openModels.get(path) && openModels.get(path).model;
      if (!model) {
        model = m.editor.createModel(content, langFor(path));
        openModels.set(path, { content, model });
      }
      editor.setModel(model);
    } else if (textarea) {
      textarea.value = content;
    }
  }

  function currentContent() {
    if (editor) return editor.getModel() ? editor.getModel().getValue() : "";
    if (textarea) return textarea.value;
    return "";
  }

  // ---- File tree (lazy directory expansion) --------------------------------
  async function expand(dir, ul) {
    const r = await ide.readDir(dir);
    if (r.error) return;
    ul.innerHTML = "";
    for (const e of r.entries) {
      const li = document.createElement("li");
      li.className = e.dir ? "tree-dir" : "tree-file";
      const row = document.createElement("div");
      row.className = "tree-row";
      row.textContent = (e.dir ? "▸ " : "  ") + e.name;
      row.title = e.path;
      li.appendChild(row);
      if (e.dir) {
        const child = document.createElement("ul");
        child.className = "tree-children";
        child.hidden = true;
        let loaded = false;
        row.addEventListener("click", async () => {
          child.hidden = !child.hidden;
          row.textContent = (child.hidden ? "▸ " : "▾ ") + e.name;
          if (!loaded) { await expand(e.path, child); loaded = true; }
        });
        li.appendChild(child);
      } else {
        row.addEventListener("click", () => openFile(e.path));
      }
      ul.appendChild(li);
    }
  }

  // ---- Tabs + open/save ----------------------------------------------------
  function renderTabs() {
    const tabs = $("editor-tabs");
    tabs.innerHTML = "";
    for (const path of openModels.keys()) {
      const t = document.createElement("button");
      t.className = "tab" + (path === activePath ? " active" : "");
      t.textContent = path.split("/").pop();
      t.title = path;
      t.addEventListener("click", () => activate(path));
      tabs.appendChild(t);
    }
  }

  async function openFile(path) {
    await ensureEditor();
    if (!openModels.has(path)) {
      const r = await ide.readFile(path);
      if (r.error) { setStatus(`Open failed: ${r.error}`); return; }
      openModels.set(path, { content: r.content, model: null });
    }
    activate(path);
  }

  function activate(path) {
    activePath = path;
    const entry = openModels.get(path);
    setEditorContent(entry.content, path);
    renderTabs();
    setStatus(path);
  }

  async function saveActive() {
    if (!activePath) return;
    const r = await ide.writeFile(activePath, currentContent());
    setStatus(r.ok ? `Saved ${activePath}` : `Save failed: ${r.error}`);
  }

  function setStatus(s) { const el = $("ide-status"); if (el) el.textContent = s; }

  // ---- Integrated terminal (xterm.js ↔ node-pty) ---------------------------
  let term = null;
  async function initTerminal() {
    if (term) return;
    const host = $("terminal-host");
    const X = window.Terminal; // global from @xterm/xterm (staged by prepare)
    if (!X || !ptyApi) {
      host.textContent = "Terminal needs `npm install` (xterm + node-pty) in desktop-app/, then a rebuild.";
      host.className = "terminal-missing";
      return;
    }
    term = new X({ fontSize: 12, theme: { background: "#0b0d12" }, cursorBlink: true });
    const Fit = window.FitAddon && window.FitAddon.FitAddon;
    const fit = Fit ? new Fit() : null;
    if (fit) term.loadAddon(fit);
    term.open(host);
    if (fit) fit.fit();
    const r = await ptyApi.start({ cols: term.cols, rows: term.rows });
    if (!r.ok) { setStatus(`Terminal: ${r.error}`); return; }
    ptyApi.onData((d) => term.write(d));
    ptyApi.onExit(() => term.write("\r\n[process exited]\r\n"));
    term.onData((d) => ptyApi.write(d));
    term.onResize(({ cols, rows }) => ptyApi.resize(cols, rows));
    window.addEventListener("resize", () => fit && fit.fit());
  }

  // ---- Wire up on first activation of the IDE view -------------------------
  let booted = false;
  async function boot() {
    if (booted) return;
    booted = true;
    await initTerminal();
  }

  window.addEventListener("DOMContentLoaded", () => {
    const openBtn = $("open-folder");
    if (openBtn) {
      openBtn.addEventListener("click", async () => {
        if (!ide) return;
        const r = await ide.openFolder();
        if (!r) return;
        $("workspace-name").textContent = r.name;
        await expand(r.root, $("file-tree"));
        boot();
      });
    }
    // Boot the terminal when the Editor rail tab is first opened.
    document.querySelectorAll('#rail .rail-btn[data-view="ide"]').forEach((b) =>
      b.addEventListener("click", boot)
    );
  });
})();

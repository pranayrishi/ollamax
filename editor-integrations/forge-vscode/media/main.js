// @ts-check
/* Webview UI for the Ollama-Forge chat panel. Runs in the sandboxed webview;
 * it never touches the network (CSP `connect-src 'none'`). All I/O goes through
 * postMessage to the extension host, which talks to `forge serve`.
 *
 * Features in this file:
 *  - Chat / Agent / Build modes, model picker, streaming, stop/cancel.
 *  - Message QUEUE (Feature 2): enqueue while streaming; strict FIFO; the next
 *    item starts only after the backend's terminal `done`; edit/remove/reorder;
 *    cancel = pause-and-confirm (does not auto-advance).
 *  - Thinking-style STATUS labels (Feature 3): cycling gerunds while in flight,
 *    with a plain "Working…" fallback (whimsy setting) and reduced-motion
 *    support. REAL reasoning (`<think>…</think>`) is rendered for real in a
 *    collapsible section and never fabricated. */
(function () {
  "use strict";
  const vscode = acquireVsCodeApi();

  // ----- config (from extension) -----
  let whimsy = true;
  let accountEnabled = false;
  let currentUser = null;
  const reduceMotion =
    !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);

  // Whimsical "thinking" words. Easily editable. "Discombobulating" included by
  // request. These are an ambient indicator only — never a fake thoughts log.
  const GERUNDS = [
    "Discombobulating",
    "Ruminating",
    "Percolating",
    "Untangling",
    "Conjuring",
    "Marinating",
    "Noodling",
    "Tinkering",
    "Wrangling",
    "Synthesizing",
    "Pondering",
    "Calibrating",
    "Spelunking",
  ];

  // ----- state -----
  let mode = "chat";
  let model = null;
  let streaming = false;
  let chatHistory = []; // {role, content} — chat mode only, for multi-turn
  let contextItems = []; // {path, content, label}
  let active = null; // the assistant message currently streaming
  let activeMode = "chat"; // mode of the in-flight turn (may differ from toggle)
  /** queued items: {text, mode, model, context} */
  let queue = [];
  let pausedByCancel = false;

  // ----- elements -----
  const $ = (sel) => document.querySelector(sel);
  const messagesEl = $("#messages");
  const inputEl = $("#input");
  const sendBtn = $("#send");
  const stopBtn = $("#stop");
  const modelSel = $("#model");
  const statusEl = $("#statusline");
  const contextEl = $("#context");
  const queueEl = $("#queue");
  const refreshBtn = $("#refresh");
  const modelHintEl = $("#modelhint");
  const accountEl = $("#account");

  // ----- helpers -----
  function escapeHtml(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // Minimal Markdown: fenced code blocks + inline code + line breaks.
  function renderMarkdown(text) {
    const parts = text.split(/```/);
    let html = "";
    for (let i = 0; i < parts.length; i++) {
      if (i % 2 === 1) {
        const nl = parts[i].indexOf("\n");
        const label = nl >= 0 ? parts[i].slice(0, nl).trim() : "";
        const code = nl >= 0 ? parts[i].slice(nl + 1) : parts[i];
        html +=
          `<pre class="code"${label ? ` data-label="${escapeHtml(label)}"` : ""}>` +
          `<code>${escapeHtml(code.replace(/\n$/, ""))}</code></pre>`;
      } else {
        let seg = escapeHtml(parts[i]);
        seg = seg.replace(/`([^`]+)`/g, "<code>$1</code>");
        seg = seg.replace(/\n/g, "<br>");
        html += seg;
      }
    }
    return html;
  }

  // Split out genuine model reasoning (<think>…</think> or <thinking>…</thinking>)
  // from the answer. Handles an unclosed trailing tag (still streaming). We only
  // ever surface reasoning the model actually emitted — never invented.
  function splitThinking(raw) {
    const norm = raw
      .replace(/<thinking>/g, "<think>")
      .replace(/<\/thinking>/g, "</think>");
    let thinking = "";
    let answer = "";
    let rest = norm;
    for (;;) {
      const o = rest.indexOf("<think>");
      if (o === -1) {
        answer += rest;
        break;
      }
      answer += rest.slice(0, o);
      const after = rest.slice(o + "<think>".length);
      const c = after.indexOf("</think>");
      if (c === -1) {
        thinking += after; // unclosed: reasoning still streaming
        rest = "";
        break;
      }
      thinking += after.slice(0, c) + "\n";
      rest = after.slice(c + "</think>".length);
    }
    return { thinking: thinking.trim(), answer: answer.trim() };
  }

  function scrollDown() {
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  // ----- a chat message (user or assistant) -----
  function addMessage(role) {
    const el = document.createElement("div");
    el.className = `msg ${role}`;

    const meta = document.createElement("div");
    meta.className = "msg-meta";
    meta.textContent = role === "user" ? "You" : "Assistant";
    el.appendChild(meta);

    const warn = document.createElement("div");
    warn.className = "warn";
    warn.hidden = true;
    el.appendChild(warn);

    const trimmed = document.createElement("div");
    trimmed.className = "trimmed";
    trimmed.hidden = true;
    el.appendChild(trimmed);

    const status = document.createElement("div");
    status.className = "status-label";
    status.hidden = true;
    el.appendChild(status);

    const thinking = document.createElement("details");
    thinking.className = "thinking";
    thinking.hidden = true;
    const tsum = document.createElement("summary");
    tsum.textContent = "Thinking";
    const tpre = document.createElement("pre");
    thinking.appendChild(tsum);
    thinking.appendChild(tpre);
    el.appendChild(thinking);

    const steps = document.createElement("div");
    steps.className = "steps";
    steps.hidden = true;
    el.appendChild(steps);

    const progress = document.createElement("div");
    progress.className = "progress";
    progress.hidden = true;
    el.appendChild(progress);

    const body = document.createElement("div");
    body.className = "body";
    el.appendChild(body);

    const files = document.createElement("div");
    files.className = "files";
    files.hidden = true;
    el.appendChild(files);

    messagesEl.appendChild(el);
    scrollDown();

    let raw = "";
    let thinkingRaw = "";
    let statusTimer = null;

    function renderContent() {
      const { thinking: think, answer } = splitThinking(raw);
      if (think) {
        thinking.hidden = false;
        if (!thinking.open) thinking.open = true;
        tpre.textContent = think;
      }
      body.innerHTML = renderMarkdown(answer);
      scrollDown();
    }

    return {
      el,
      startStatus() {
        status.hidden = false;
        if (!whimsy) {
          status.textContent = "Working…";
          return;
        }
        let i = Math.floor(Math.random() * GERUNDS.length);
        status.textContent = GERUNDS[i] + "…";
        if (reduceMotion) return; // no rotation when reduced motion is requested
        statusTimer = setInterval(() => {
          i = (i + 1) % GERUNDS.length;
          status.textContent = GERUNDS[i] + "…";
        }, 2500);
      },
      stopStatus() {
        if (statusTimer) {
          clearInterval(statusTimer);
          statusTimer = null;
        }
        status.hidden = true;
      },
      setMetaLabel(t) {
        meta.textContent = t;
      },
      appendToken(t) {
        this.stopStatus();
        raw += t;
        renderContent();
      },
      // #4: REAL reasoning tokens from a thinking-capable model (Ollama's
      // separate `thinking` stream), shown in the collapsible block — distinct
      // from the answer. We only ever render reasoning the model actually emits.
      appendThinking(t) {
        this.stopStatus(); // real reasoning is progress — drop the spinner
        thinkingRaw += t;
        thinking.hidden = false;
        if (!thinking.open) thinking.open = true;
        tpre.textContent = thinkingRaw;
        scrollDown();
      },
      setAnswerText(t) {
        this.stopStatus();
        raw = t;
        renderContent();
      },
      appendNote(t) {
        raw += t;
        renderContent();
      },
      getAnswer() {
        return splitThinking(raw).answer;
      },
      showWarnings(list) {
        if (!list || list.length === 0) return;
        warn.hidden = false;
        warn.textContent =
          `⚠ secret scan: ${list.length} finding(s) in attached context — ` +
          list.map((w) => `${w.rule} (${w.severity})`).join(", ");
      },
      showTrimmed(n) {
        if (!n || n <= 0) return;
        trimmed.hidden = false;
        trimmed.textContent = `↥ ${n} older message(s) trimmed to fit the model's context window`;
      },
      showNote(text) {
        if (!text) return;
        trimmed.hidden = false;
        trimmed.textContent = text;
      },
      addStep(ev) {
        this.stopStatus();
        steps.hidden = false;
        const row = document.createElement("div");
        row.className = "step " + (ev.ok ? "ok" : "fail");
        row.innerHTML =
          `<span class="badge">round ${ev.iteration}</span> ` +
          `<span class="tool">${escapeHtml(ev.tool)}</span> ` +
          `<span class="prev">${escapeHtml(ev.preview || "")}</span>`;
        steps.appendChild(row);
        scrollDown();
      },
      addProgress(ev) {
        this.stopStatus();
        progress.hidden = false;
        const row = document.createElement("div");
        row.className = "prow " + ev.kind;
        const label = ev.kind.replace(/_/g, " ");
        let detail = "";
        if (ev.kind.startsWith("preload")) {
          detail = ev.model + (ev.ok === false ? " (failed)" : "");
        } else {
          detail =
            (ev.subtask || "") +
            (ev.model ? ` · ${ev.model}` : "") +
            (ev.tokens != null ? ` · ${ev.tokens} tok` : "");
        }
        row.innerHTML = `<span class="pbadge">${label}</span> <span>${escapeHtml(detail)}</span>`;
        progress.appendChild(row);
        scrollDown();
      },
      showFiles(list) {
        if (!list || list.length === 0) return;
        files.hidden = false;
        files.innerHTML =
          `<div class="files-h">📦 wrote ${list.length} file(s):</div>` +
          list.map((f) => `<div class="file">${escapeHtml(f)}</div>`).join("");
      },
      setBodyText(t) {
        raw = t;
        renderContent();
      },
    };
  }

  function setStreaming(on) {
    streaming = on;
    stopBtn.hidden = !on;
    sendBtn.textContent = on ? "Queue" : "Send";
  }

  // ----- dispatch / queue -----
  function submit() {
    const text = inputEl.value.trim();
    if (!text) return;
    inputEl.value = "";
    const item = { text, mode, model, context: contextItems.slice() };
    contextItems = []; // consumed by this message
    renderContext();
    if (streaming) {
      queue.push(item);
      renderQueue();
    } else {
      dispatch(item);
    }
  }

  function dispatch(item) {
    activeMode = item.mode;
    const userMsg = addMessage("user");
    userMsg.setBodyText(item.text);
    if (item.context && item.context.length > 0) {
      const note = document.createElement("div");
      note.className = "attached-note";
      note.textContent =
        "↳ context: " + item.context.map((c) => c.label || c.path).join(", ");
      userMsg.el.appendChild(note);
    }

    active = addMessage("assistant");
    if (item.model) active.setMetaLabel(`Assistant · ${item.model}`);
    active.startStatus();
    setStreaming(true);

    const payload = {
      type: "send",
      mode: item.mode,
      model: item.model,
      text: item.text,
      // Strip UI-only fields (thumb/isImage) so we don't send the image twice;
      // the server reads `image` (base64) for vision + `content` for text. For
      // images, send the clean filename (not the name#size dedup key).
      context: (item.context || []).map((c) => ({
        path: c.isImage ? c.label || c.path : c.path,
        label: c.label,
        content: c.content,
        image: c.image,
      })),
    };
    if (item.mode === "chat") {
      chatHistory.push({ role: "user", content: item.text });
      payload.messages = chatHistory.slice();
    }
    vscode.postMessage(payload);
  }

  function stop() {
    if (!streaming) return;
    // Pause-and-confirm: a cancel must NOT silently fire the next queued prompt.
    pausedByCancel = true;
    vscode.postMessage({ type: "cancel" });
  }

  function finishTurn() {
    if (activeMode === "chat" && active) {
      const txt = active.getAnswer();
      if (txt) chatHistory.push({ role: "assistant", content: txt });
    }
    setStreaming(false);
    active = null;
    maybeAdvanceQueue();
  }

  function maybeAdvanceQueue() {
    if (pausedByCancel) {
      renderQueue(); // shows the paused banner so the user can resume
      return;
    }
    if (queue.length > 0) {
      dispatch(queue.shift());
    }
    renderQueue();
  }

  function resumeQueue() {
    pausedByCancel = false;
    maybeAdvanceQueue();
  }

  function clearQueue() {
    queue = [];
    pausedByCancel = false;
    renderQueue();
  }

  // ----- incoming stream events -----
  function onStreamEvent(ev) {
    if (!active) return;
    switch (ev.type) {
      case "meta":
        if (ev.routing && ev.routing.auto) {
          // Auto routing chose this model — show which + a one-line "why".
          active.setMetaLabel(`Assistant · ${ev.routing.model} · auto`);
          if (ev.routing.reasoning) active.showNote("🔀 " + ev.routing.reasoning);
        } else if (ev.model) {
          active.setMetaLabel(`Assistant · ${ev.model}`);
        }
        active.showWarnings(ev.warnings);
        if (!(ev.routing && ev.routing.auto)) active.showTrimmed(ev.trimmed);
        // Feature 2: when web tools are on, surface the egress disclosure IN the
        // conversation at use time — not just buried in Settings.
        if (ev.toolsEnabled && ev.disclosure) active.showNote("🌐 " + ev.disclosure);
        break;
      case "token":
        active.appendToken(ev.text);
        break;
      case "thinking":
        active.appendThinking(ev.text || "");
        break;
      case "step":
        active.addStep(ev);
        break;
      case "answer":
        active.setAnswerText(ev.text || "");
        if (ev.capped) active.appendNote("\n\n_(agent hit its iteration cap)_");
        break;
      case "progress":
        active.addProgress(ev);
        break;
      case "result":
        active.setAnswerText(ev.output || "");
        active.showFiles(ev.files);
        active.appendNote(
          `\n\n_built on ${ev.model} · ${ev.tokens} tok · ${ev.durationMs} ms_`
        );
        break;
      case "done":
        finishTurn();
        break;
      case "cancelled":
        active.stopStatus();
        active.appendNote("\n\n_(cancelled)_");
        finishTurn();
        break;
      case "error":
        active.stopStatus();
        active.setBodyText(`⚠ ${ev.message || "error"}`);
        active.el.classList.add("error");
        finishTurn();
        break;
      default:
        break;
    }
  }

  // ----- queue UI -----
  function renderQueue() {
    queueEl.innerHTML = "";
    if (queue.length === 0 && !pausedByCancel) {
      queueEl.hidden = true;
      return;
    }
    queueEl.hidden = false;

    if (pausedByCancel && queue.length > 0) {
      const banner = document.createElement("div");
      banner.className = "queue-paused";
      const txt = document.createElement("span");
      txt.textContent = `Queue paused after cancel · ${queue.length} pending`;
      const resume = document.createElement("button");
      resume.textContent = "Resume";
      resume.addEventListener("click", resumeQueue);
      const clear = document.createElement("button");
      clear.textContent = "Clear";
      clear.addEventListener("click", clearQueue);
      banner.appendChild(txt);
      banner.appendChild(resume);
      banner.appendChild(clear);
      queueEl.appendChild(banner);
    } else if (queue.length > 0) {
      const hdr = document.createElement("div");
      hdr.className = "queue-h";
      hdr.textContent = `Queued (${queue.length}) — runs after the current reply`;
      queueEl.appendChild(hdr);
    }

    queue.forEach((item, i) => {
      const row = document.createElement("div");
      row.className = "qitem";

      const badge = document.createElement("span");
      badge.className = "qbadge";
      badge.textContent = item.mode;
      row.appendChild(badge);

      const text = document.createElement("span");
      text.className = "qtext";
      text.textContent = item.text;
      row.appendChild(text);

      const ctrls = document.createElement("span");
      ctrls.className = "qctrls";

      const up = mkBtn("↑", "move up", () => {
        if (i > 0) {
          [queue[i - 1], queue[i]] = [queue[i], queue[i - 1]];
          renderQueue();
        }
      });
      const down = mkBtn("↓", "move down", () => {
        if (i < queue.length - 1) {
          [queue[i + 1], queue[i]] = [queue[i], queue[i + 1]];
          renderQueue();
        }
      });
      const edit = mkBtn("✎", "edit", () => beginEdit(row, text, item));
      const del = mkBtn("✕", "remove", () => {
        queue.splice(i, 1);
        if (queue.length === 0) pausedByCancel = false;
        renderQueue();
      });
      ctrls.appendChild(up);
      ctrls.appendChild(down);
      ctrls.appendChild(edit);
      ctrls.appendChild(del);
      row.appendChild(ctrls);

      queueEl.appendChild(row);
    });
  }

  function mkBtn(label, title, onClick) {
    const b = document.createElement("button");
    b.className = "qbtn";
    b.textContent = label;
    b.title = title;
    b.addEventListener("click", onClick);
    return b;
  }

  function beginEdit(row, textSpan, item) {
    const input = document.createElement("input");
    input.className = "qedit";
    input.value = item.text;
    row.replaceChild(input, textSpan);
    input.focus();
    const save = () => {
      const v = input.value.trim();
      if (v) item.text = v;
      renderQueue();
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        save();
      } else if (e.key === "Escape") {
        renderQueue();
      }
    });
    input.addEventListener("blur", save);
  }

  // ----- context chips -----
  function addContext(items) {
    for (const it of items) {
      if (!contextItems.some((c) => c.path === it.path)) contextItems.push(it);
    }
    renderContext();
  }

  function renderContext() {
    if (contextItems.length === 0) {
      contextEl.hidden = true;
      contextEl.innerHTML = "";
      return;
    }
    contextEl.hidden = false;
    contextEl.innerHTML = "";
    contextItems.forEach((c, i) => {
      const chip = document.createElement("span");
      chip.className = c.isImage ? "chip chip-img" : "chip";
      if (c.isImage && c.thumb) {
        const img = document.createElement("img");
        img.className = "chip-thumb";
        img.src = c.thumb; // data: URL — allowed by the webview CSP
        img.alt = c.label || c.path;
        chip.appendChild(img);
      }
      const label = document.createElement("span");
      label.textContent = c.label || c.path;
      chip.appendChild(label);
      const x = document.createElement("button");
      x.className = "x";
      x.textContent = "×";
      x.title = "remove";
      x.addEventListener("click", () => {
        contextItems.splice(i, 1);
        renderContext();
      });
      chip.appendChild(x);
      contextEl.appendChild(chip);
    });
  }

  // ----- #5 drag-and-drop files & images into the chat -----
  const isImageFile = (f) => !!f && /^image\//.test(f.type || "");
  const readFile = (file, asText) =>
    new Promise((res, rej) => {
      const r = new FileReader();
      r.onload = () => res(String(r.result || ""));
      r.onerror = () => rej(r.error || new Error("read failed"));
      asText ? r.readAsText(file) : r.readAsDataURL(file);
    });

  async function handleDroppedFiles(files) {
    let addedImage = false;
    for (const file of files) {
      try {
        if (isImageFile(file)) {
          if (file.size > 6 * 1024 * 1024) {
            setStatus(`"${file.name}" is too large to attach (${Math.round(file.size / 1048576)} MB; max 6 MB).`);
            continue;
          }
          const dataUrl = await readFile(file, false);
          const base64 = dataUrl.includes(",") ? dataUrl.slice(dataUrl.indexOf(",") + 1) : dataUrl;
          // Dedup by name+size so two different images sharing a filename don't
          // collide (review #16); re-dropping the exact same file still dedups.
          addContext([
            { path: `${file.name}#${file.size}`, label: file.name, image: base64, thumb: dataUrl, isImage: true },
          ]);
          addedImage = true;
        } else if (file.size > 512 * 1024) {
          setStatus(`"${file.name}" is too large to attach as text (${Math.round(file.size / 1024)} KB).`);
        } else {
          const content = await readFile(file, true);
          addContext([{ path: file.name, label: file.name, content }]);
        }
      } catch (e) {
        setStatus(`Could not attach ${file.name}: ${(e && e.message) || e}`);
      }
    }
    if (addedImage) maybeWarnVision();
  }

  // Route images to a vision-capable model: nudge to switch when the current
  // pick clearly isn't multimodal (Auto + the server's own check handle the rest).
  function maybeWarnVision() {
    // Images are only honored on the Chat path — Agent/Build can't analyze them,
    // so warn instead of silently dropping the image (review #7/#8).
    if (mode === "agent" || mode === "build") {
      setStatus(
        `🖼 Images only work in Chat mode — ${mode} mode can't analyze an image. Switch to Chat to use it.`
      );
      return;
    }
    const m = (model || "").toLowerCase();
    const looksVision = /llava|vision|bakllava|moondream|qwen2\.?5?-?vl|minicpm-v|gemma3/.test(m);
    if (!model) {
      // #17: no model resolved yet — still tell the user vision needs a vision model.
      setStatus("🖼 Image attached — pick a vision model (llava, llama3.2-vision) to analyze it.");
    } else if (model !== "auto" && !looksVision) {
      setStatus(
        "🖼 Image attached — your model may not support vision. Pick a vision model (llava, llama3.2-vision) or use Auto."
      );
    }
  }

  (function setupDropZone() {
    let depth = 0;
    // Only intercept drags that carry FILES — otherwise we'd swallow drops meant
    // for the editor/inputs and break native text drag-and-drop (review #19).
    const hasFiles = (e) => {
      const t = e.dataTransfer && e.dataTransfer.types;
      return !!t && Array.prototype.indexOf.call(t, "Files") !== -1;
    };
    document.body.addEventListener("dragover", (e) => {
      if (hasFiles(e)) e.preventDefault();
    });
    document.body.addEventListener("dragenter", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth++;
      document.body.classList.add("dropping");
    });
    document.body.addEventListener("dragleave", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = Math.max(0, depth - 1);
      if (!depth) document.body.classList.remove("dropping");
    });
    document.body.addEventListener("drop", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = 0;
      document.body.classList.remove("dropping");
      const files = e.dataTransfer && e.dataTransfer.files;
      if (files && files.length) handleDroppedFiles(Array.from(files));
    });
  })();

  // ----- models / status -----
  function setModels(models, def) {
    modelSel.innerHTML = "";
    if (!models || models.length === 0) {
      const o = document.createElement("option");
      o.textContent = "(no models — `ollama pull …`)";
      o.value = "";
      modelSel.appendChild(o);
      model = null;
      modelHintEl.hidden = true;
      return;
    }
    // Auto routing is the default (Feature 2): the router picks a local model
    // per task. A manual pick below always overrides it. `def` (hardware
    // recommendation) is the model Auto falls back to when it can't classify.
    const auto = document.createElement("option");
    auto.value = "auto";
    auto.textContent = "Auto (router picks per task)";
    modelSel.appendChild(auto);
    for (const m of models) {
      const o = document.createElement("option");
      o.value = m.name;
      o.textContent = `${m.name}  (${m.sizeHuman || ""})`;
      modelSel.appendChild(o);
    }
    model = "auto";
    modelSel.value = "auto";
    requestModelInfo("auto");
  }

  function requestModelInfo(name) {
    if (name === "auto") {
      modelHintEl.innerHTML =
        "🔀 Auto — the router picks a local model per task (simple → small, complex → large). Local only; never cloud.";
      modelHintEl.hidden = false;
      return;
    }
    if (name) vscode.postMessage({ type: "modelInfo", name });
  }

  // Render the local-only context-window + capability hint for the selected
  // model. Flags `thinking`-capable models (ties into the real reasoning view).
  function renderModelHint(info) {
    if (model === "auto") return; // auto hint is set in requestModelInfo
    if (!info || info.name !== model || info.error) {
      modelHintEl.hidden = true;
      return;
    }
    const bits = [];
    if (info.contextLength) {
      const n = info.contextLength;
      bits.push(`ctx ${n >= 1024 ? Math.round(n / 1024) + "K" : n}`);
    }
    const d = info.details || {};
    if (d.parameter_size) bits.push(escapeHtml(d.parameter_size));
    if (d.quantization_level) bits.push(escapeHtml(d.quantization_level));
    const caps = Array.isArray(info.capabilities) ? info.capabilities : [];
    const interesting = caps.filter((c) =>
      ["tools", "thinking", "vision", "insert", "embedding"].includes(c)
    );
    let html = "🔒 local · " + bits.join(" · ");
    for (const c of interesting) {
      const cls = c === "thinking" ? "cap thinking" : "cap";
      html += ` <span class="${cls}">${escapeHtml(c)}</span>`;
    }
    modelHintEl.innerHTML = html;
    modelHintEl.hidden = false;
  }

  // Account chip (identity only — never affects local inference availability).
  function renderAccount() {
    if (!accountEl) return;
    if (!accountEnabled) {
      accountEl.hidden = true;
      accountEl.innerHTML = "";
      return;
    }
    accountEl.hidden = false;
    accountEl.innerHTML = "";
    if (currentUser) {
      const wrap = document.createElement("div");
      wrap.className = "acct-in";
      if (currentUser.avatarUrl) {
        const img = document.createElement("img");
        img.src = currentUser.avatarUrl;
        img.alt = "";
        img.className = "acct-av";
        wrap.appendChild(img);
      }
      const name = document.createElement("span");
      name.className = "acct-name";
      name.textContent = "@" + (currentUser.login || "");
      wrap.appendChild(name);
      const out = document.createElement("button");
      out.className = "acct-link";
      out.textContent = "Sign out";
      out.addEventListener("click", () => vscode.postMessage({ type: "signOut" }));
      wrap.appendChild(out);
      accountEl.appendChild(wrap);
    } else {
      const btn = document.createElement("button");
      btn.className = "acct-signin";
      btn.textContent = "Sign in with GitHub";
      btn.addEventListener("click", () => vscode.postMessage({ type: "signIn" }));
      accountEl.appendChild(btn);
      const dev = document.createElement("button");
      dev.className = "acct-link";
      dev.textContent = "device code";
      dev.title = "Sign in with a device code (no loopback needed)";
      dev.addEventListener("click", () => vscode.postMessage({ type: "signIn", device: true }));
      accountEl.appendChild(dev);
    }
  }

  function setStatus(status) {
    if (!status) return;
    const hw = status.hardware || {};
    const ok = status.ollamaHealthy;
    statusEl.classList.toggle("bad", !ok);
    const free =
      hw.free_vram_mb != null ? `${(hw.free_vram_mb / 1024).toFixed(1)} GB free` : "";
    const rec = hw.recommended_model ? `rec ${hw.recommended_model}` : "";
    statusEl.textContent =
      `${ok ? "●" : "○"} Local · Ollama ${ok ? "✓" : "✗ (run `ollama serve`)"}` +
      (hw.gpu_kind ? ` · ${hw.gpu_kind}` : "") +
      (free ? ` · ${free}` : "") +
      (rec ? ` · ${rec}` : "");
  }

  // ----- wire up DOM -----
  document.querySelectorAll(".mode").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mode").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      mode = btn.getAttribute("data-mode");
      inputEl.placeholder =
        mode === "agent"
          ? "Research question — the agent will use web/wiki/arxiv tools…"
          : mode === "build"
          ? "Describe what to build — runs parallel workers across models…"
          : "Ask anything — runs locally on your hardware…";
    });
  });

  document.querySelectorAll(".attach").forEach((btn) => {
    btn.addEventListener("click", () => {
      const kind = btn.getAttribute("data-attach");
      if (kind === "file") vscode.postMessage({ type: "attachFile" });
      else if (kind === "selection") vscode.postMessage({ type: "attachSelection" });
      else vscode.postMessage({ type: "pickFiles" });
    });
  });

  modelSel.addEventListener("change", () => {
    model = modelSel.value;
    requestModelInfo(model);
  });

  refreshBtn.addEventListener("click", () => {
    modelHintEl.hidden = true;
    vscode.postMessage({ type: "refresh" });
  });

  sendBtn.addEventListener("click", submit);
  stopBtn.addEventListener("click", stop);
  inputEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  });

  // #6 login gate: show/hide the full-panel sign-in screen.
  function renderGate(signedIn) {
    const gate = $("#gate");
    if (!gate) return;
    gate.hidden = !!signedIn;
  }
  const gateSignIn = $("#gate-signin");
  if (gateSignIn) gateSignIn.addEventListener("click", () => vscode.postMessage({ type: "signIn" }));
  const gateSignInDev = $("#gate-signin-device");
  if (gateSignInDev)
    gateSignInDev.addEventListener("click", () => vscode.postMessage({ type: "signIn", device: true }));

  window.addEventListener("message", (event) => {
    const msg = event.data;
    switch (msg.type) {
      case "config":
        if (typeof msg.whimsy === "boolean") whimsy = msg.whimsy;
        if (typeof msg.accountEnabled === "boolean") accountEnabled = msg.accountEnabled;
        renderAccount();
        break;
      case "account":
        currentUser = msg.user || null;
        renderAccount();
        break;
      case "gate":
        renderGate(!!msg.signedIn);
        break;
      case "models":
        setModels(msg.models, msg.default);
        break;
      case "modelInfo":
        renderModelHint(msg.info);
        break;
      case "status":
        setStatus(msg.status);
        break;
      case "stream":
        onStreamEvent(msg.ev);
        break;
      case "context":
        addContext(msg.items || []);
        break;
      case "newChat":
        messagesEl.innerHTML = "";
        chatHistory = [];
        contextItems = [];
        queue = [];
        pausedByCancel = false;
        renderContext();
        renderQueue();
        setStreaming(false);
        active = null;
        break;
      case "backendError":
        statusEl.classList.add("bad");
        statusEl.textContent = "✗ backend: " + msg.message;
        setStreaming(false);
        break;
      default:
        break;
    }
  });

  // Tell the extension we're ready so it sends config, boots the backend, and
  // sends models/status.
  vscode.postMessage({ type: "ready" });
})();

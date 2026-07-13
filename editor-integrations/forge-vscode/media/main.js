// @ts-check
/* Webview UI for the Ollamax chat panel. Runs in the sandboxed webview;
 * it never touches the network (CSP `connect-src 'none'`). All I/O goes through
 * postMessage to the extension host, which talks to `forge serve`.
 *
 * Features in this file:
 *  - Chat / Agent / Team / Build modes, model picker, streaming, stop/cancel.
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
  // Start in Agent mode: Ollamax is a coding assistant, so a normal composer
  // prompt can inspect and change the opened workspace (subject to the visible
  // permission dial). Ask remains one click away for read-only discussion.
  let mode = "agent";
  let model = null;
  // Picker metadata from `/api/models`. Configured self-hosted local models
  // declare capabilities explicitly, so never guess from their served name.
  let modelEntries = new Map();
  // Ollama exposes capabilities from `/api/model_info` rather than its compact
  // installed-model list. Cache that authoritative response per model so an
  // image attachment does not fall back to a brittle model-name heuristic.
  let modelCapabilities = new Map();
  let streaming = false;
  let chatHistory = []; // {role, content} — chat mode only, for multi-turn
  let contextItems = []; // {path, content, label}
  let active = null; // the assistant message currently streaming
  let activeMode = "agent"; // mode of the in-flight turn (may differ from toggle)
  let lastAutonomy = "confirm"; // autonomy of the in-flight agent turn (gates the Plan card)
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
  const teamParallelScoutsEl = $("#team-parallel-scouts");
  const teamParallelScoutsOption = $("#team-parallel-option");

  // ----- helpers -----
  function escapeHtml(s) {
    return String(s == null ? "" : s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
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

    // Sub-agents lane — delegated child agents render here, OUT of the main
    // step stream. `subCurrent` tracks the open sub-agent's body container.
    const subagents = document.createElement("div");
    subagents.className = "subagents";
    subagents.hidden = true;
    el.appendChild(subagents);
    let subCurrent = null;

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
    // A desktop-only finalizer may remove local control markup from the answer
    // after the terminal stream event. Keep the original raw text for the
    // genuine thinking disclosure while rendering/persisting the cleaned body.
    let answerOverride = null;
    let statusTimer = null;

    function renderContent() {
      const { thinking: think, answer: rawAnswer } = splitThinking(raw);
      const answer = answerOverride === null ? rawAnswer : answerOverride;
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
        answerOverride = null;
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
        answerOverride = null;
        raw = t;
        renderContent();
      },
      appendNote(t) {
        answerOverride = null;
        raw += t;
        renderContent();
      },
      setFinalAnswerText(t) {
        this.stopStatus();
        answerOverride = typeof t === "string" ? t : "";
        renderContent();
      },
      getAnswer() {
        return answerOverride === null ? splitThinking(raw).answer : answerOverride;
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
        // Activity Timeline: status dot + tool + result preview + expandable args
        // (the agent's `args` were captured but never shown before).
        const argsStr = ev.args ? JSON.stringify(ev.args) : "";
        row.innerHTML =
          `<span class="sdot">${ev.ok ? "●" : "✕"}</span>` +
          `<span class="badge">round ${ev.iteration}</span> ` +
          `<span class="tool">${escapeHtml(ev.tool)}</span> ` +
          `<span class="prev">${escapeHtml(ev.preview || "")}</span>` +
          (argsStr
            ? `<details class="sargs"><summary>args</summary><pre>${escapeHtml(argsStr)}</pre></details>`
            : "");
        steps.appendChild(row);
        scrollDown();
      },
      // Surface that a skill was auto-applied (Hermes-class skills-in-the-loop).
      addSkill(name) {
        steps.hidden = false;
        const row = document.createElement("div");
        row.className = "step skill";
        row.innerHTML =
          `<span class="sdot">✦</span><span class="badge">skill</span> ` +
          `<span class="tool">${escapeHtml(name)}</span> ` +
          `<span class="prev">applied to this task</span>`;
        steps.appendChild(row);
        scrollDown();
      },
      // Surface recalled on-device memory (the Memory drawer).
      addMemory(preview) {
        steps.hidden = false;
        const row = document.createElement("div");
        row.className = "step memory";
        row.innerHTML =
          `<span class="sdot">⌘</span><span class="badge">memory</span> ` +
          `<details class="sargs"><summary>recalled context</summary><pre>${escapeHtml(preview)}</pre></details>`;
        steps.appendChild(row);
        scrollDown();
      },
      // Intent Preview: the agent's proposed plan before it executes. In confirm
      // mode it's gated with Run / Cancel; otherwise it's shown for transparency.
      addPlan(text, gated, approvalId) {
        this.stopStatus();
        steps.hidden = false;
        const card = document.createElement("div");
        card.className = "plan-card";
        const head = document.createElement("div");
        head.className = "plan-head";
        head.textContent = "📋 Plan";
        card.appendChild(head);
        const bodyc = document.createElement("pre");
        bodyc.className = "plan-body";
        bodyc.textContent = text;
        card.appendChild(bodyc);
        if (gated) {
          const actions = document.createElement("div");
          actions.className = "approw";
          const run = document.createElement("button");
          run.className = "primary";
          run.textContent = "Run plan";
          const cancel = document.createElement("button");
          cancel.className = "ghost";
          cancel.textContent = "Cancel (I'll do it)";
          const decide = (decision) => {
            vscode.postMessage({ type: "approve", approvalId, decision });
            actions.textContent = decision ? "▶ running…" : "✕ cancelled";
          };
          run.addEventListener("click", () => decide(true));
          cancel.addEventListener("click", () => decide(false));
          actions.appendChild(run);
          actions.appendChild(cancel);
          card.appendChild(actions);
        }
        steps.appendChild(card);
        scrollDown();
      },
      // Autonomy Dial: the agent paused for approval before a consequential tool.
      // Render an Approve / Deny prompt; the decision is relayed to the agent.
      addApprovalPrompt(ev) {
        this.stopStatus();
        steps.hidden = false;
        const row = document.createElement("div");
        row.className = "step approval";
        const argsStr = ev.args ? JSON.stringify(ev.args) : "";
        const head = document.createElement("div");
        head.innerHTML =
          `<span class="sdot">⏸</span><span class="badge">approve?</span> ` +
          `<span class="tool">${escapeHtml(ev.tool)}</span>` +
          (argsStr ? ` <span class="prev">${escapeHtml(argsStr.slice(0, 200))}</span>` : "");
        row.appendChild(head);
        const actions = document.createElement("div");
        actions.className = "approw";
        const allow = document.createElement("button");
        allow.className = "primary";
        allow.textContent = "Approve";
        const deny = document.createElement("button");
        deny.className = "ghost";
        deny.textContent = "Deny";
        const decide = (decision) => {
          vscode.postMessage({ type: "approve", approvalId: ev.approvalId, decision });
          actions.textContent = decision ? "✓ approved" : "✕ denied";
        };
        allow.addEventListener("click", () => decide(true));
        deny.addEventListener("click", () => decide(false));
        actions.appendChild(allow);
        actions.appendChild(deny);
        row.appendChild(actions);
        steps.appendChild(row);
        scrollDown();
      },
      // ----- Sub-agents lane (delegated child agents) -----
      addSubagentStart(task) {
        subagents.hidden = false;
        const det = document.createElement("details");
        det.className = "subagent";
        det.open = true;
        const sum = document.createElement("summary");
        sum.innerHTML = `🤖 <span class="sa-task">${escapeHtml(task || "sub-agent")}</span> <span class="sa-state">running…</span>`;
        det.appendChild(sum);
        const bodyc = document.createElement("div");
        bodyc.className = "sa-body";
        det.appendChild(bodyc);
        subagents.appendChild(det);
        subCurrent = { det, body: bodyc, sum };
        scrollDown();
      },
      addSubagentStep(ev) {
        if (!subCurrent) this.addSubagentStart("sub-agent");
        const row = document.createElement("div");
        row.className = "step " + (ev.ok ? "ok" : "fail");
        row.innerHTML =
          `<span class="sdot">${ev.ok ? "●" : "✕"}</span>` +
          `<span class="badge">round ${ev.iteration}</span> ` +
          `<span class="tool">${escapeHtml(ev.tool)}</span> ` +
          `<span class="prev">${escapeHtml(ev.preview || "")}</span>`;
        subCurrent.body.appendChild(row);
        scrollDown();
      },
      addSubagentEnd(ev) {
        if (subCurrent) {
          const st = subCurrent.sum.querySelector(".sa-state");
          if (st) st.textContent = ev && ev.ok === false ? "failed" : "done";
          subCurrent.det.open = false;
          subCurrent = null;
        }
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
    const autonomyEl = $("#autonomy");
    const item = {
      text,
      mode,
      model,
      context: contextItems.slice(),
      autonomy: autonomyEl ? autonomyEl.value : "confirm",
      // Parallelism is deliberately limited to the two read-only scout lanes.
      // The server still owns the hardware/configuration gate and retains one
      // writer, deterministic verifier, and bounded repair loop.
      parallelScouts: mode === "team" && !!(teamParallelScoutsEl && teamParallelScoutsEl.checked),
    };
    if (mode === "agent" || mode === "team") lastAutonomy = item.autonomy;
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
      autonomy: item.autonomy,
      parallelScouts: item.parallelScouts,
      text: item.text,
      // Strip UI-only fields (thumb/isImage) so we don't send the image twice;
      // the server reads `image` (base64) for vision + `content` for text. For
      // images, send the clean filename (not the name#size dedup key).
      context: (item.context || []).map((c) => ({
        path: c.isImage ? c.label || c.path : c.path,
        label: c.label,
        content: c.content,
        image: c.image,
        // Preserve the trusted desktop lasso marker so the server can offer
        // optional visual-only POINT cues for a real spatial selection, never
        // for an ordinary image attachment.
        spatial: c.spatial === true,
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
      // #3 persist this project's chat history (on-device, workspace-scoped).
      vscode.postMessage({ type: "persistHistory", messages: chatHistory });
    }
    setStreaming(false);
    active = null;
    maybeAdvanceQueue();
  }

  // POINT directives are desktop-only local visual cues. A host may install a
  // synchronous finalizer before this shared UI loads; invoke it only after a
  // genuine terminal `done`, never for partial streaming, cancellation, or an
  // error. The returned text may only remove content, so a host cannot inject
  // additional rendered output through this hook.
  function finalizeActiveAssistantResponse() {
    if (!active) return;
    let text = active.getAnswer();
    const finalizer = typeof window.__ollamaxFinalizeAssistantResponse === "function"
      ? window.__ollamaxFinalizeAssistantResponse
      : null;
    if (finalizer) {
      try {
        const cleaned = finalizer(text);
        if (typeof cleaned === "string" && cleaned.length <= text.length) {
          if (cleaned !== text) active.setFinalAnswerText(cleaned);
          text = cleaned;
        }
      } catch (_) {}
    }
    try {
      window.dispatchEvent(new CustomEvent("ollamax:assistant-final", { detail: { text } }));
    } catch (_) {}
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
      case "team_meta":
        active.setMetaLabel(
          `Team · writer ${ev.writerModel || "local"} · scouts ${ev.scoutModel || "local"} · planner ${ev.plannerModel || "local"}`
        );
        active.showNote(
          "👥 Controlled local team: read-only scouts → one writer → fixed verification → review." +
            " Confirm is selected by default; allowed test commands execute this workspace's code." +
            (ev.parallelRequestedButDisabled ? " Parallel scouts were disabled by this project's configuration." : "")
        );
        break;
      case "team_plan": {
        const p = ev.plan || {};
        const checks = Array.isArray(p.verification_commands) && p.verification_commands.length
          ? p.verification_commands.join("; ")
          : "no conventional verifier detected";
        active.showNote(`📋 Team plan · one writer · checks: ${checks}`);
        break;
      }
      case "team_scout_started":
        active.showNote(`🔎 ${String(ev.role || "scout").replace(/([A-Z])/g, " $1").trim()} started`);
        break;
      case "team_scout_finished":
        active.showNote(`✓ ${String(ev.role || "scout").replace(/([A-Z])/g, " $1").trim()} finished (${ev.steps || 0} tool steps)`);
        break;
      case "team_planner_started":
        active.showNote("🧭 Planner is synthesizing the read-only scout hand-offs");
        break;
      case "team_planner_finished":
        active.showNote("✓ Planner hand-off is ready");
        break;
      case "team_writer_started":
        active.showNote(`✎ Writer ${ev.repairRound ? "repair" : "implementation"} pass started`);
        break;
      case "team_writer_finished":
        active.showNote(`✓ Writer pass finished (${ev.steps || 0} tool steps)`);
        break;
      case "team_verification_started":
        active.showNote(`🧪 Verifying: ${ev.command || "check"}`);
        break;
      case "team_verification_finished": {
        const r = ev.result || {};
        active.showNote(
          `${r.passed ? "✓" : "✕"} Verification ${r.passed ? "passed" : r.skipped_by_user ? "declined" : "failed"}: ${r.command || "check"}`
        );
        break;
      }
      case "team_reviewer_finished":
        active.showNote(ev.available === false ? "⚠ Advisory reviewer was unavailable" : "✓ Advisory review finished");
        break;
      case "team_result": {
        const bits = [
          `team status: ${String(ev.status || "unknown")}`,
          ev.writerMutationSteps != null ? `${ev.writerMutationSteps} writer mutation step(s)` : "",
          ev.functionalVerificationPassed === true ? "functional check passed" : "",
          ev.modelCalls != null ? `${ev.modelCalls} local model calls` : "",
          ev.toolCalls != null ? `${ev.toolCalls} tool/check calls` : "",
          ev.elapsedMs != null ? `${ev.elapsedMs} ms` : "",
        ].filter(Boolean);
        active.appendNote(`\n\n_${bits.join(" · ")}_`);
        if (ev.review) active.appendNote(`\n\n**Review**\n${ev.review}`);
        break;
      }
      case "token":
        active.appendToken(ev.text);
        break;
      case "thinking":
        active.appendThinking(ev.text || "");
        break;
      case "step":
        active.addStep(ev);
        break;
      case "plan":
        active.addPlan(ev.text || "", lastAutonomy === "confirm", ev.approvalId);
        break;
      case "approval_request":
        // #1 File edits get a real diff/preview + modal in the editor (host-driven);
        // other consequential tools (shell) keep the inline Approve/Deny.
        if (ev.tool === "fs_write" || ev.tool === "fs_edit") {
          vscode.postMessage({ type: "previewEdit", tool: ev.tool, args: ev.args, approvalId: ev.approvalId });
          active.showNote(
            "📝 Proposed change to " + ((ev.args && ev.args.path) || "a file") +
              " — review the diff that opened, then Apply / Discard."
          );
        } else {
          active.addApprovalPrompt(ev);
        }
        break;
      case "subagent_start":
        active.addSubagentStart(ev.task);
        break;
      case "subagent_step":
        active.addSubagentStep(ev);
        break;
      case "subagent_end":
        active.addSubagentEnd(ev);
        break;
      case "skill_applied":
        active.addSkill(ev.name);
        break;
      case "memory_used":
        active.addMemory(ev.preview || "");
        break;
      case "knowledge_plugins_used": {
        const names = Array.isArray(ev.plugins)
          ? ev.plugins.map((plugin) => plugin.name || plugin.id).filter(Boolean).join(", ")
          : "";
        active.showNote(`🧩 Using installed GitHub knowledge reference${names ? `: ${names}` : ""} (untrusted documentation only).`);
        break;
      }
      case "knowledge_plugin_warning":
        active.showNote(`⚠ ${ev.message || "Installed knowledge plugin was not loaded."}`);
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
        finalizeActiveAssistantResponse();
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
    // Images are only honored on the Chat path — Agent can't analyze them,
    // so warn instead of silently dropping the image (review #7/#8).
    if (mode === "agent" || mode === "team") {
      const hasSpatialImage = contextItems.some((item) => item && item.isImage && item.spatial);
      setStatus(
        hasSpatialImage
          ? "⌁ Spatial reference attached — use a local vision model; the agent will turn it into a visual brief before workspace work."
          : "🖼 Images only work in Chat mode — switch to Chat to analyze an image."
      );
      return;
    }
    if (!model) {
      // #17: no model resolved yet — still tell the user vision needs a vision model.
      setStatus("🖼 Image attached — pick a vision model (llava, llama3.2-vision) to analyze it.");
      return;
    }
    if (model === "auto") return;

    const entry = modelEntries.get(model);
    const capabilities = modelCapabilities.get(model);
    const supportsVision = entry && typeof entry.vision === "boolean"
      ? entry.vision
      : capabilities
        ? capabilities.has("vision")
        : null;

    if (supportsVision === null) {
      // A model may have just been selected, so its capability request can
      // still be in flight. Ask the local engine rather than treating a family
      // name as evidence; the incoming modelInfo response populates the cache.
      requestModelInfo(model);
    } else if (!supportsVision) {
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
    modelEntries = new Map((models || []).filter((m) => m && m.name).map((m) => [m.name, m]));
    modelCapabilities = new Map();
    if (!models || models.length === 0) {
      const o = document.createElement("option");
      o.textContent = "(no local models — install Ollama or configure a local endpoint)";
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
      o.textContent = `${m.displayName || m.name}  (${m.sizeHuman || ""})`;
      if (m.runtime === "openai-compatible-local") {
        o.title = "Explicitly configured loopback self-hosted endpoint";
      }
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
    // Installed Ollama entries do not include capability flags in /api/models.
    // Remember a successful local model-info lookup even when the user changes
    // the selection before its response arrives.
    if (info && typeof info.name === "string" && !info.error && Array.isArray(info.capabilities)) {
      modelCapabilities.set(
        info.name,
        new Set(info.capabilities.filter((capability) => typeof capability === "string"))
      );
    }
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
    const entry = modelEntries.get(model);
    const locality = entry && entry.runtime === "openai-compatible-local"
      ? "🔒 local endpoint"
      : "🔒 local";
    let html = locality + " · " + bits.join(" · ");
    for (const c of interesting) {
      const cls = c === "thinking" ? "cap thinking" : "cap";
      html += ` <span class="${cls}">${escapeHtml(c)}</span>`;
    }
    modelHintEl.innerHTML = html;
    modelHintEl.hidden = false;
    if (contextItems.some((item) => item && item.isImage)) maybeWarnVision();
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
      // Use the device-code flow (reliable across deployments; the loopback flow
      // depends on the account server's redirect handling).
      btn.addEventListener("click", () => vscode.postMessage({ type: "signIn", device: true }));
      accountEl.appendChild(btn);
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
  function updateModeControls() {
    const teamMode = mode === "team";
    if (teamParallelScoutsOption) teamParallelScoutsOption.hidden = !teamMode;
    // A switch to another mode cannot accidentally queue a Team run with an
    // old parallel preference. Keep the user's Team choice while it is hidden.
  }

  document.querySelectorAll(".mode").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mode").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      mode = btn.getAttribute("data-mode");
      // The Autonomy Dial applies to both workspace-writing modes.
      const dial = $("#autonomy");
      if (dial) dial.hidden = mode !== "agent" && mode !== "team";
      updateModeControls();
      inputEl.placeholder =
        mode === "agent"
          ? "Tell the agent what to do — it uses tools, memory & skills and edits files (asks first)…"
          : mode === "team"
            ? "Describe a complex task — scouts inspect first, one writer edits, then Ollamax verifies the result…"
            : "Ask anything — conversational, read-only, runs locally on your hardware…";
    });
  });
  updateModeControls();

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
  // Primary sign-in uses the device-code flow (reliable across deployments;
  // the loopback flow needs the account server's redirect handling).
  if (gateSignIn) gateSignIn.addEventListener("click", () => vscode.postMessage({ type: "signIn", device: true }));
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
        if ((msg.items || []).some((item) => item && item.isImage)) maybeWarnVision();
        break;
      case "restoreHistory":
        // #3 Re-render this project's saved chat history on reopen.
        if (Array.isArray(msg.messages) && msg.messages.length) {
          chatHistory = msg.messages.slice();
          for (const m of msg.messages) {
            const el = addMessage(m.role === "assistant" ? "assistant" : "user");
            el.setBodyText(m.content || "");
          }
          const div = document.createElement("div");
          div.className = "trimmed";
          div.textContent = "↑ restored from this project";
          messagesEl.appendChild(div);
          scrollDown();
        }
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

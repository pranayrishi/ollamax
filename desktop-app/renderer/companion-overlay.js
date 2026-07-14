// Companion overlay renderer — pointer flight, response bubble, waveform,
// freehand region drawing, mic capture (16 kHz mono WAV), and the
// speechSynthesis TTS fallback. Runs in a transparent, normally
// click-through window covering one display. No network access: everything
// goes through the preload IPC bridge.
(function () {
  "use strict";
  const api = window.companionOverlay;
  if (!api) return;

  const displayId = Number(new URLSearchParams(location.search).get("displayId"));
  const body = document.body;
  const pointer = document.getElementById("pointer");
  const pointLabel = document.getElementById("point-label");
  const bubble = document.getElementById("bubble");
  const bubbleText = document.getElementById("bubble-text");
  const wave = document.getElementById("wave");
  const taskChip = document.getElementById("task-chip");
  const taskText = document.getElementById("task-text");
  const toast = document.getElementById("toast");
  const canvas = document.getElementById("draw-canvas");
  const ctx = canvas.getContext("2d");
  const regionRing = document.getElementById("region-ring");

  let isCursorDisplay = true;
  let lingerTimer = null;
  let toastTimer = null;

  // ------------------------------------------------------------------
  // Pointer position + bezier flight (the "flies to the target" motion)
  // ------------------------------------------------------------------
  const pos = { x: window.innerWidth / 2, y: window.innerHeight * 0.62 };
  let flight = null;

  function renderPointer() {
    pointer.style.transform = `translate(${pos.x - 4}px, ${pos.y - 2}px)`;
  }
  renderPointer();

  function flyTo(x, y, label) {
    const from = { x: pos.x, y: pos.y };
    const dx = x - from.x;
    const dy = y - from.y;
    const dist = Math.hypot(dx, dy);
    // Control point perpendicular to the path for a pleasing arc.
    const arc = Math.min(180, dist * 0.35);
    const cx = from.x + dx / 2 - (dy / (dist || 1)) * arc;
    const cy = from.y + dy / 2 + (dx / (dist || 1)) * arc;
    const duration = Math.min(1100, 350 + dist * 0.6);
    const start = performance.now();
    pointer.classList.add("visible");
    if (flight) cancelAnimationFrame(flight.raf);
    flight = {};
    const ease = (t) => 1 - Math.pow(1 - t, 3); // ease-out cubic
    const step = (now) => {
      const t = Math.min(1, (now - start) / duration);
      const e = ease(t);
      const inv = 1 - e;
      pos.x = inv * inv * from.x + 2 * inv * e * cx + e * e * x;
      pos.y = inv * inv * from.y + 2 * inv * e * cy + e * e * y;
      renderPointer();
      positionBubble();
      if (t < 1) {
        flight.raf = requestAnimationFrame(step);
      } else {
        flight = null;
        if (label) {
          pointLabel.textContent = label;
          pointLabel.style.left = `${x}px`;
          pointLabel.style.top = `${y + 22}px`;
          pointLabel.classList.add("visible");
        }
      }
    };
    flight.raf = requestAnimationFrame(step);
  }

  function parkPointer() {
    // Rest position: lower middle of the screen, out of the way.
    flyTo(window.innerWidth / 2, window.innerHeight * 0.62, null);
  }

  // ------------------------------------------------------------------
  // Bubble
  // ------------------------------------------------------------------
  function positionBubble() {
    const bw = bubble.offsetWidth || 320;
    const bh = bubble.offsetHeight || 60;
    let x = pos.x + 26;
    let y = pos.y - bh - 14;
    if (x + bw > window.innerWidth - 12) x = pos.x - bw - 26;
    if (y < 12) y = pos.y + 26;
    bubble.style.left = `${Math.max(12, x)}px`;
    bubble.style.top = `${Math.max(12, y)}px`;
  }

  function showBubble(mode, text) {
    bubble.classList.remove("listening", "thinking");
    if (mode) bubble.classList.add(mode);
    bubbleText.innerHTML = "";
    if (text) bubbleText.textContent = text;
    bubble.classList.add("visible");
    pointer.classList.add("visible");
    positionBubble();
    clearTimeout(lingerTimer);
  }

  function hideAllSoon(delayMs) {
    clearTimeout(lingerTimer);
    lingerTimer = setTimeout(() => {
      bubble.classList.remove("visible", "listening", "thinking");
      pointLabel.classList.remove("visible");
      pointer.classList.remove("visible");
      regionRing.style.display = "none";
    }, delayMs);
  }

  function showToast(text) {
    toast.textContent = text;
    toast.classList.add("visible");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("visible"), 3800);
  }

  // ------------------------------------------------------------------
  // Microphone capture → 16 kHz mono PCM16 WAV
  // ------------------------------------------------------------------
  let rec = null; // { ctx, stream, node, chunks, maxTimer, level }

  async function startRecording(maxSeconds) {
    if (rec) return;
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
      });
    } catch (e) {
      api.recordError(e.message || "microphone permission denied");
      return;
    }
    const ctxA = new AudioContext({ sampleRate: 16000 });
    const source = ctxA.createMediaStreamSource(stream);
    const node = ctxA.createScriptProcessor(4096, 1, 1);
    const chunks = [];
    node.onaudioprocess = (ev) => {
      const data = ev.inputBuffer.getChannelData(0);
      chunks.push(new Float32Array(data));
      // Live level for the waveform.
      let sum = 0;
      for (let i = 0; i < data.length; i += 8) sum += data[i] * data[i];
      const rms = Math.sqrt(sum / (data.length / 8));
      animateWave(rms);
    };
    source.connect(node);
    node.connect(ctxA.destination);
    rec = { ctx: ctxA, stream, node, chunks };
    rec.maxTimer = setTimeout(() => stopRecording(), (maxSeconds || 60) * 1000);
  }

  function animateWave(rms) {
    const bars = wave.children;
    const level = Math.min(1, rms * 9);
    for (let i = 0; i < bars.length; i++) {
      const jitter = 0.55 + 0.45 * Math.sin(Date.now() / 90 + i * 1.7);
      bars[i].style.height = `${Math.max(4, level * 22 * jitter)}px`;
    }
  }

  function stopRecording() {
    const r = rec;
    rec = null;
    if (!r) return;
    clearTimeout(r.maxTimer);
    try {
      r.node.disconnect();
      r.stream.getTracks().forEach((t) => t.stop());
      r.ctx.close();
    } catch (_) {}
    const sampleRate = 16000;
    let length = 0;
    for (const c of r.chunks) length += c.length;
    if (length < sampleRate / 4) {
      // Under a quarter second of audio — treat as an accidental tap.
      api.recordError("too short — hold the thought and try again");
      return;
    }
    const pcm = new Int16Array(length);
    let off = 0;
    for (const c of r.chunks) {
      for (let i = 0; i < c.length; i++) {
        const s = Math.max(-1, Math.min(1, c[i]));
        pcm[off++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      }
    }
    api.sendAudio(encodeWav(pcm, sampleRate));
  }

  function encodeWav(pcm, sampleRate) {
    const buf = new ArrayBuffer(44 + pcm.length * 2);
    const v = new DataView(buf);
    const writeStr = (o, s) => {
      for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
    };
    writeStr(0, "RIFF");
    v.setUint32(4, 36 + pcm.length * 2, true);
    writeStr(8, "WAVE");
    writeStr(12, "fmt ");
    v.setUint32(16, 16, true);
    v.setUint16(20, 1, true); // PCM
    v.setUint16(22, 1, true); // mono
    v.setUint32(24, sampleRate, true);
    v.setUint32(28, sampleRate * 2, true);
    v.setUint16(32, 2, true);
    v.setUint16(34, 16, true);
    writeStr(36, "data");
    v.setUint32(40, pcm.length * 2, true);
    new Int16Array(buf, 44).set(pcm);
    return buf;
  }

  // ------------------------------------------------------------------
  // Freehand region drawing (spatial context)
  // ------------------------------------------------------------------
  let stroke = null;

  function resizeCanvas() {
    canvas.width = window.innerWidth * devicePixelRatio;
    canvas.height = window.innerHeight * devicePixelRatio;
    canvas.style.width = `${window.innerWidth}px`;
    canvas.style.height = `${window.innerHeight}px`;
    ctx.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
  }
  window.addEventListener("resize", resizeCanvas);
  resizeCanvas();

  function clearCanvas() {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  }

  canvas.addEventListener("mousedown", (e) => {
    stroke = [{ x: e.clientX, y: e.clientY }];
    clearCanvas();
    ctx.strokeStyle = "#4f7cff";
    ctx.lineWidth = 2.5;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.setLineDash([7, 6]);
    ctx.beginPath();
    ctx.moveTo(e.clientX, e.clientY);
  });
  canvas.addEventListener("mousemove", (e) => {
    if (!stroke) return;
    stroke.push({ x: e.clientX, y: e.clientY });
    ctx.lineTo(e.clientX, e.clientY);
    ctx.stroke();
  });
  window.addEventListener("mouseup", () => {
    if (!stroke) return;
    const points = stroke;
    stroke = null;
    api.sendStroke(displayId, points);
    setTimeout(clearCanvas, 350);
  });
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && body.classList.contains("drawing")) {
      stroke = null;
      clearCanvas();
      api.cancelDraw();
    }
  });

  // ------------------------------------------------------------------
  // TTS fallback via speechSynthesis (offline OS voices)
  // ------------------------------------------------------------------
  function speakFallback(text) {
    try {
      const u = new SpeechSynthesisUtterance(text);
      u.rate = 1.04;
      u.onend = () => api.speechDone();
      u.onerror = () => api.speechDone();
      speechSynthesis.cancel();
      speechSynthesis.speak(u);
    } catch (_) {
      api.speechDone();
    }
  }

  // ------------------------------------------------------------------
  // Companion events
  // ------------------------------------------------------------------
  api.onState((s) => {
    isCursorDisplay = s.cursorDisplayId === displayId;
    if (s.state === "idle" && !rec) hideAllSoon(2600);
  });

  api.onStartRecord(({ maxSeconds }) => {
    parkPointer();
    showBubble("listening", "listening…");
    startRecording(maxSeconds);
  });

  api.onStopRecord(() => stopRecording());

  api.onTranscript(({ text }) => {
    showBubble(null, "");
    bubbleText.innerHTML = `<span class="heard">“${text}”</span>`;
    positionBubble();
  });

  api.onDrawMode(({ active }) => {
    body.classList.toggle("drawing", !!active);
    if (!active) clearCanvas();
  });

  api.onPartial(({ text }) => {
    if (!bubble.classList.contains("visible")) showBubble("thinking", "");
    bubble.classList.add("thinking");
    bubbleText.textContent = text || "";
    positionBubble();
  });

  api.onFinal(({ text }) => {
    bubble.classList.remove("thinking", "listening");
    bubbleText.textContent = text || "";
    bubble.classList.add("visible");
    positionBubble();
  });

  api.onPoint(({ x, y, label }) => {
    pointLabel.classList.remove("visible");
    flyTo(x, y, label);
  });

  api.onTaskOffer(({ task }) => {
    taskText.textContent = task.length > 90 ? `${task.slice(0, 87)}…` : task;
    taskChip.style.display = "flex";
    const bw = taskChip.offsetWidth || 280;
    taskChip.style.left = `${Math.min(window.innerWidth - bw - 16, Math.max(16, pos.x - bw / 2))}px`;
    taskChip.style.top = `${Math.min(window.innerHeight - 64, pos.y + 40)}px`;
  });

  api.onToast(({ text }) => showToast(text));

  api.onTurnDone(() => hideAllSoon(6000));

  api.onRegionArmed(({ bbox }) => {
    if (!bbox) {
      regionRing.style.display = "none";
      return;
    }
    regionRing.style.display = "block";
    regionRing.style.left = `${bbox.x}px`;
    regionRing.style.top = `${bbox.y}px`;
    regionRing.style.width = `${bbox.width}px`;
    regionRing.style.height = `${bbox.height}px`;
  });

  api.onSpeakFallback(({ text }) => speakFallback(text));

  // The overlay is click-through (mouse events forward to the app below),
  // but mousemove still reaches us thanks to {forward: true}. When the
  // pointer hovers the task chip we ask the main process to make the window
  // interactive so the click lands; on leave we return to click-through.
  taskChip.addEventListener("mouseenter", () => api.setInteractive(true));
  taskChip.addEventListener("mouseleave", () => api.setInteractive(false));
  taskChip.addEventListener("click", (e) => {
    if (e.target && e.target.id === "task-dismiss") {
      taskChip.style.display = "none";
      api.dismissTask();
      return;
    }
    taskChip.style.display = "none";
    api.setInteractive(false);
    api.acceptTask();
  });
  // Auto-expire an unanswered chip so the overlay never lingers.
  api.onTaskOffer(() => {
    setTimeout(() => {
      if (taskChip.style.display !== "none") {
        taskChip.style.display = "none";
        api.setInteractive(false);
        api.dismissTask();
      }
    }, 30_000);
  });
})();

// Desktop-only push-to-talk and optional local speech output.  This layer is
// deliberately separate from the shared VS Code webview so the extension does
// not accidentally gain Electron-only capabilities.
(function () {
  "use strict";

  const talk = document.querySelector("#voice-push-to-talk");
  const sendAfter = document.querySelector("#voice-send-after");
  const speakAnswers = document.querySelector("#voice-speak-answers");
  const status = document.querySelector("#voice-status");
  const input = document.querySelector("#input");
  const send = document.querySelector("#send");
  if (!talk || !status || !window.forgeNative || !window.forgeNative.voice) return;
  const buddy = window.forgeNative.buddy;

  let recording = null;
  let startInFlight = false;
  let statusTimer = null;
  let lastSpokenAnswer = "";
  let voiceAvailable = false;
  let shortcutAvailable = true;

  // The native companion accepts only a closed enum of short interaction
  // states. Never route a transcript or assistant response through it.
  function reportBuddy(state) {
    if (!buddy || typeof buddy.setState !== "function") return;
    Promise.resolve(buddy.setState(state)).catch(() => {});
  }

  function setStatus(text, bad) {
    status.textContent = text || "";
    status.classList.toggle("bad", !!bad);
  }

  function setTalkState(active) {
    talk.classList.toggle("active", !!active);
    talk.textContent = active ? "● Release to send" : "🎙 Hold to talk";
    talk.setAttribute("aria-pressed", active ? "true" : "false");
  }

  function flatten(parts) {
    const length = parts.reduce((total, part) => total + part.length, 0);
    const output = new Float32Array(length);
    let offset = 0;
    for (const part of parts) {
      output.set(part, offset);
      offset += part.length;
    }
    return output;
  }

  // whisper.cpp accepts PCM WAV. The browser gives float samples at the device
  // rate, so downsample locally and serialize a mono 16 kHz file in-memory.
  function encodeWav(samples, sourceRate, targetRate) {
    const ratio = sourceRate / targetRate;
    const length = Math.max(1, Math.floor(samples.length / ratio));
    const buffer = new ArrayBuffer(44 + length * 2);
    const view = new DataView(buffer);
    const writeAscii = (offset, value) => {
      for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i));
    };
    writeAscii(0, "RIFF");
    view.setUint32(4, 36 + length * 2, true);
    writeAscii(8, "WAVE");
    writeAscii(12, "fmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, targetRate, true);
    view.setUint32(28, targetRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeAscii(36, "data");
    view.setUint32(40, length * 2, true);
    for (let i = 0; i < length; i++) {
      // Averaging the source interval reduces aliasing relative to simple
      // nearest-neighbour sampling and has no network or worker dependency.
      const start = Math.floor(i * ratio);
      const end = Math.min(samples.length, Math.max(start + 1, Math.floor((i + 1) * ratio)));
      let sum = 0;
      for (let j = start; j < end; j++) sum += samples[j];
      const sample = Math.max(-1, Math.min(1, sum / (end - start)));
      view.setInt16(44 + i * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
    }
    const bytes = new Uint8Array(buffer);
    let binary = "";
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary);
  }

  async function startRecording(event) {
    if (recording || startInFlight) return;
    if (event) event.preventDefault();
    if (talk.disabled || !voiceAvailable) {
      talk.dataset.cancelPending = "false";
      setStatus("Local voice recognition is not configured.", true);
      reportBuddy("voice_unavailable");
      return;
    }
    // Clear a stale release from a previous failed attempt. A new release
    // during the permission sheet will set this back to true below.
    talk.dataset.cancelPending = "false";
    startInFlight = true;
    reportBuddy("voice_starting");
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1 } });
    } catch (error) {
      startInFlight = false;
      talk.dataset.cancelPending = "false";
      setStatus("Microphone permission is required for local voice commands.", true);
      reportBuddy("voice_error");
      return;
    }
    startInFlight = false;
    // A release can happen while permission is being granted. In that case,
    // immediately close the stream instead of starting an invisible recording.
    if (talk.dataset.cancelPending === "true") {
      talk.dataset.cancelPending = "false";
      stream.getTracks().forEach((track) => track.stop());
      reportBuddy("idle");
      return;
    }
    let context;
    try {
      const AudioContextImpl = window.AudioContext || window.webkitAudioContext;
      if (!AudioContextImpl) throw new Error("Web Audio is unavailable");
      context = new AudioContextImpl();
      const source = context.createMediaStreamSource(stream);
      const processor = context.createScriptProcessor(4096, 1, 1);
      const silent = context.createGain();
      silent.gain.value = 0;
      const chunks = [];
      processor.onaudioprocess = (audioEvent) => {
        if (recording && recording.processor === processor) {
          chunks.push(new Float32Array(audioEvent.inputBuffer.getChannelData(0)));
        }
      };
      source.connect(processor);
      processor.connect(silent);
      silent.connect(context.destination);
      recording = { stream, context, source, processor, silent, chunks, startedAt: Date.now() };
    } catch (_) {
      talk.dataset.cancelPending = "false";
      stream.getTracks().forEach((track) => track.stop());
      if (context) {
        try {
          await context.close();
        } catch (_) {}
      }
      setStatus("Local audio capture could not start on this device.", true);
      reportBuddy("voice_error");
      return;
    }
    setTalkState(true);
    setStatus("Listening locally… release to transcribe.");
    reportBuddy("voice_listening");
    statusTimer = setTimeout(() => stopRecording(), 60_000);
  }

  async function stopRecording(event) {
    if (event) event.preventDefault();
    if (!recording) {
      // Covers pointer-up while the permission sheet is visible.
      if (startInFlight) {
        talk.dataset.cancelPending = "true";
        reportBuddy("idle");
      }
      return;
    }
    const active = recording;
    recording = null;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = null;
    setTalkState(false);
    try {
      active.processor.disconnect();
      active.source.disconnect();
      active.silent.disconnect();
    } catch (_) {}
    active.stream.getTracks().forEach((track) => track.stop());
    const sampleRate = active.context.sampleRate;
    await active.context.close();
    const elapsed = Date.now() - active.startedAt;
    if (elapsed < 180 || active.chunks.length === 0) {
      setStatus("Voice command was too short to transcribe.", true);
      reportBuddy("voice_error");
      return;
    }
    setStatus("Transcribing locally…");
    reportBuddy("voice_transcribing");
    const wav = encodeWav(flatten(active.chunks), sampleRate, 16_000);
    const result = await window.forgeNative.voice.transcribe(wav);
    if (!result || !result.ok) {
      setStatus((result && result.error) || "Local transcription failed.", true);
      reportBuddy("voice_error");
      return;
    }
    const transcript = String(result.transcript || "").trim();
    if (!transcript) {
      setStatus("Nothing was recognized. Try again in a quieter space.", true);
      reportBuddy("voice_error");
      return;
    }
    setStatus(`Heard locally: ${transcript}`);
    reportBuddy("voice_done");
    input.value = transcript;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    // Holding and releasing this explicit control is a user command. The
    // normal Agent confirmation dial still governs any workspace edit/tool use.
    if (sendAfter && sendAfter.checked && send && !send.disabled) send.click();
  }

  function bindPressAndHold() {
    talk.addEventListener("pointerdown", (event) => {
      talk.setPointerCapture && talk.setPointerCapture(event.pointerId);
      startRecording(event);
    });
    talk.addEventListener("pointerup", stopRecording);
    talk.addEventListener("pointercancel", stopRecording);
    talk.addEventListener("lostpointercapture", () => {
      if (recording) stopRecording();
    });
    talk.addEventListener("keydown", (event) => {
      if ((event.key === " " || event.key === "Enter") && !event.repeat) startRecording(event);
    });
    talk.addEventListener("keyup", (event) => {
      if (event.key === " " || event.key === "Enter") stopRecording(event);
    });
  }

  function toggleFromGlobalShortcut() {
    // This arrives only from the registered Electron shortcut. It delegates to
    // the exact same local recorder as the visible button; no model call or
    // OS action is made here.
    if (recording || startInFlight) {
      void stopRecording();
      return;
    }
    setStatus("Global shortcut requested local voice capture…");
    void startRecording();
  }

  async function speakFinalAnswer(answer) {
    if (!speakAnswers || !speakAnswers.checked) return;
    // Defense in depth: the terminal finalizer normally removes POINT markup
    // before this event, but never pass a directive to local speech even if a
    // renderer script is loaded out of order during development.
    const parser = window.OllamaxPointDirectives;
    const cleaned = parser && typeof parser.parsePointDirectives === "function"
      ? parser.parsePointDirectives(answer).text
      : String(answer || "");
    const answers = document.querySelectorAll(".msg.assistant .body");
    const visible = answers.length ? String(answers[answers.length - 1].textContent || "").trim() : "";
    const latest = visible && !visible.includes("[POINT:") ? visible : cleaned.trim();
    if (!latest || latest === lastSpokenAnswer) return;
    lastSpokenAnswer = latest;
    const result = await window.forgeNative.voice.speak(latest);
    if (!result || !result.ok) setStatus((result && result.error) || "Local speech output failed.", true);
  }

  // The shared chat UI emits this only after its terminal `done` processing.
  // It carries the same cleaned final assistant text that is rendered and
  // persisted, never a partial stream token or a cancelled/error response.
  window.addEventListener("ollamax:assistant-final", (event) => {
    const answer = event && event.detail ? event.detail.text : "";
    setTimeout(() => { void speakFinalAnswer(answer); }, 0);
  });

  bindPressAndHold();
  if (buddy && typeof buddy.onVoiceToggle === "function") {
    buddy.onVoiceToggle(toggleFromGlobalShortcut);
  }
  if (buddy && typeof buddy.onShortcutStatus === "function") {
    buddy.onShortcutStatus((state) => {
      shortcutAvailable = !!(state && state.registered);
      if (!shortcutAvailable) {
        setStatus("Global voice shortcut is unavailable because another app is using it.", true);
      }
    });
  }
  window.forgeNative.voice.status().then((state) => {
    if (!state || !state.whisper || !state.whisper.available) {
      talk.disabled = true;
      setStatus((state && state.whisper && state.whisper.reason) || "Local voice recognition is not configured.", true);
      reportBuddy("voice_unavailable");
    } else if (!state.tts || !state.tts.available) {
      voiceAvailable = true;
      if (speakAnswers) speakAnswers.disabled = true;
      setStatus(
        shortcutAvailable
          ? "Local voice commands are ready. Hold the microphone button or press ⌘/Ctrl+Alt+Space. Speech output is unavailable on this system."
          : "Local voice commands are ready. Speech output is unavailable; the global shortcut is in use by another app."
      );
      reportBuddy("idle");
    } else {
      voiceAvailable = true;
      setStatus(
        shortcutAvailable
          ? "Local voice is ready. Hold the microphone button or press ⌘/Ctrl+Alt+Space."
          : "Local voice is ready. The global shortcut is in use by another app."
      );
      reportBuddy("idle");
    }
  }).catch(() => {
    setStatus("Could not check the local voice runtime.", true);
    reportBuddy("voice_unavailable");
  });
})();

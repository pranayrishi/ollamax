// Companion text-to-speech — free and fully offline, no API keys.
//
// Engine ladder (first available wins):
//   1. Piper (https://github.com/rhasspy/piper) when a binary + voice model
//      are configured/bundled — the best free neural voice, MIT licensed.
//      Output WAV is played with the OS's native player.
//   2. The OS speech engine: macOS `say`, Windows System.Speech via
//      PowerShell, Linux `espeak-ng`/`spd-say`.
//   3. Renderer speechSynthesis fallback (the caller is told to handle it) —
//      Chromium's offline OS voices, still no network.
//
// Everything is spawn-based with a stop() control so barge-in (starting a new
// push-to-talk while the companion is speaking) can cut speech instantly.
"use strict";

const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

function exists(p) {
  try {
    return !!p && fs.existsSync(p);
  } catch (_) {
    return false;
  }
}

function onPath(name) {
  const probe = process.platform === "win32" ? "where" : "which";
  try {
    const out = cp
      .execFileSync(probe, [name], { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .split(/\r?\n/)[0]
      .trim();
    return out || null;
  } catch (_) {
    return null;
  }
}

/** Resolve which engine speak() will use, for status/doctor reporting. */
function discoverTts(settings, { resourcesBinDir } = {}) {
  const piperBin =
    (settings.piperPath && settings.piperPath.trim()) ||
    (resourcesBinDir &&
      [path.join(resourcesBinDir, process.platform === "win32" ? "piper.exe" : "piper")].find(
        exists
      )) ||
    onPath("piper");
  const piperVoice = settings.piperVoice && settings.piperVoice.trim();
  if (piperBin && exists(piperVoice)) {
    return { engine: "piper", detail: `piper + ${path.basename(piperVoice)}` };
  }
  if (process.platform === "darwin") return { engine: "system", detail: "macOS say" };
  if (process.platform === "win32") return { engine: "system", detail: "Windows System.Speech" };
  const espeak = onPath("espeak-ng") || onPath("espeak") || onPath("spd-say");
  if (espeak) return { engine: "system", detail: path.basename(espeak) };
  return { engine: "renderer", detail: "browser speechSynthesis (OS voices)" };
}

/** Play a WAV file with the OS's native audio player. Returns a process. */
function playWav(wavPath) {
  if (process.platform === "darwin") {
    return cp.spawn("afplay", [wavPath], { stdio: "ignore" });
  }
  if (process.platform === "win32") {
    return cp.spawn(
      "powershell.exe",
      [
        "-NoProfile",
        "-Command",
        `(New-Object Media.SoundPlayer '${wavPath.replace(/'/g, "''")}').PlaySync()`,
      ],
      { stdio: "ignore", windowsHide: true }
    );
  }
  return cp.spawn("aplay", [wavPath], { stdio: "ignore" });
}

/**
 * Speak `text` aloud. Returns a controller:
 *   { engine, done: Promise<void>, stop() }
 * When the resolved engine is "renderer", `done` resolves immediately and the
 * caller must speak through the overlay renderer's speechSynthesis instead.
 */
function speak(text, settings, { resourcesBinDir } = {}) {
  const clean = String(text || "").trim();
  const resolved = discoverTts(settings, { resourcesBinDir });
  if (!clean || settings.ttsEngine === "none") {
    return { engine: "none", done: Promise.resolve(), stop() {} };
  }
  if (resolved.engine === "renderer") {
    return { engine: "renderer", done: Promise.resolve(), stop() {} };
  }

  let stopped = false;
  let currentProc = null;
  let tmpWav = null;
  const cleanup = () => {
    if (tmpWav) fs.unlink(tmpWav, () => {});
    tmpWav = null;
  };

  const done = (async () => {
    if (resolved.engine === "piper") {
      const piperBin =
        (settings.piperPath && settings.piperPath.trim()) ||
        (resourcesBinDir &&
          path.join(resourcesBinDir, process.platform === "win32" ? "piper.exe" : "piper")) ||
        "piper";
      tmpWav = path.join(os.tmpdir(), `ollamax-tts-${Date.now()}.wav`);
      await new Promise((resolve, reject) => {
        const proc = cp.spawn(
          piperBin,
          ["--model", settings.piperVoice, "--output_file", tmpWav],
          { stdio: ["pipe", "ignore", "ignore"], windowsHide: true }
        );
        currentProc = proc;
        proc.on("error", reject);
        proc.on("close", (code) =>
          code === 0 && !stopped ? resolve() : reject(new Error(`piper exited ${code}`))
        );
        proc.stdin.end(clean);
      });
      if (stopped) return;
      await new Promise((resolve) => {
        const player = playWav(tmpWav);
        currentProc = player;
        player.on("close", resolve);
        player.on("error", resolve);
      });
      cleanup();
      return;
    }

    // OS speech engines speak directly through the default output device.
    await new Promise((resolve) => {
      let proc;
      if (process.platform === "darwin") {
        const args = settings.systemVoice ? ["-v", settings.systemVoice, clean] : [clean];
        proc = cp.spawn("say", args, { stdio: "ignore" });
      } else if (process.platform === "win32") {
        proc = cp.spawn(
          "powershell.exe",
          [
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Speech; " +
              "$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; " +
              "$s.Speak([Console]::In.ReadToEnd())",
          ],
          { stdio: ["pipe", "ignore", "ignore"], windowsHide: true }
        );
        proc.stdin.end(clean);
      } else {
        const bin = onPath("espeak-ng") || onPath("espeak") || "spd-say";
        proc = cp.spawn(bin, [clean], { stdio: "ignore" });
      }
      currentProc = proc;
      proc.on("close", resolve);
      proc.on("error", resolve);
    });
  })().catch(() => cleanup());

  return {
    engine: resolved.engine,
    done,
    stop() {
      stopped = true;
      if (currentProc) {
        try {
          currentProc.kill();
        } catch (_) {}
      }
      cleanup();
    },
  };
}

module.exports = { discoverTts, speak };

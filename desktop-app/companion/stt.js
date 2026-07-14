// Companion speech-to-text — local whisper.cpp only. Audio NEVER leaves the
// machine: the overlay renderer records a 16 kHz mono WAV, this module runs
// the whisper.cpp CLI over it and returns the transcript. Same engine and
// arguments the VS Code voice feature already ships (`-m model -f wav -nt`),
// so one whisper install serves both.
"use strict";

const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const WHISPER_BIN_NAMES =
  process.platform === "win32"
    ? ["whisper-cli.exe", "whisper-cpp.exe", "main.exe"]
    : ["whisper-cli", "whisper-cpp"];

/** First existing path from a list, or null. */
function firstExisting(paths) {
  for (const p of paths) {
    try {
      if (p && fs.existsSync(p)) return p;
    } catch (_) {}
  }
  return null;
}

/** Look for a binary on PATH (cheap `which`/`where`). */
function onPath(names) {
  const probe = process.platform === "win32" ? "where" : "which";
  for (const name of names) {
    try {
      const out = cp
        .execFileSync(probe, [name], { stdio: ["ignore", "pipe", "ignore"] })
        .toString()
        .split(/\r?\n/)[0]
        .trim();
      if (out) return out;
    } catch (_) {}
  }
  return null;
}

/**
 * Resolve the whisper binary + ggml model. Order: explicit settings → files
 * bundled next to the app's engine (`resourcesBinDir`) → the user's model dir
 * (`userModelsDir`) → PATH / Homebrew defaults. Returns nulls plus
 * human-readable issues when something is missing so the overlay can show a
 * setup hint instead of failing silently.
 */
function discoverWhisper(settings, { resourcesBinDir, userModelsDir } = {}) {
  const issues = [];

  let bin =
    (settings.whisperPath && settings.whisperPath.trim()) ||
    firstExisting(
      (resourcesBinDir ? WHISPER_BIN_NAMES.map((n) => path.join(resourcesBinDir, n)) : []).concat(
        process.platform === "darwin"
          ? ["/opt/homebrew/bin/whisper-cli", "/usr/local/bin/whisper-cli"]
          : []
      )
    ) ||
    onPath(WHISPER_BIN_NAMES);
  if (bin && !fs.existsSync(bin) && !onPath([bin])) {
    issues.push(`whisper binary not found at "${bin}"`);
    bin = null;
  }

  let model = (settings.whisperModel && settings.whisperModel.trim()) || null;
  if (!model) {
    const candidates = [];
    if (resourcesBinDir) {
      candidates.push(path.join(resourcesBinDir, "ggml-base.en.bin"));
      candidates.push(path.join(resourcesBinDir, "ggml-base.bin"));
    }
    if (userModelsDir) {
      try {
        for (const f of fs.readdirSync(userModelsDir)) {
          if (/^ggml-.*\.bin$/.test(f)) candidates.push(path.join(userModelsDir, f));
        }
      } catch (_) {}
    }
    model = firstExisting(candidates);
  } else if (!fs.existsSync(model)) {
    issues.push(`whisper model not found at "${model}"`);
    model = null;
  }

  if (!bin) {
    issues.push(
      "install whisper.cpp (macOS: brew install whisper-cpp) or set the binary path in Companion settings"
    );
  }
  if (!model) {
    issues.push(
      "download a ggml Whisper model (e.g. ggml-base.en.bin from the whisper.cpp releases) into the Companion models folder or set its path in settings"
    );
  }
  return { bin, model, issues };
}

/**
 * Transcribe a WAV buffer with whisper.cpp. Writes a temp file, runs
 * `whisper -m model -f file -nt` (no timestamps), returns trimmed stdout.
 */
function transcribeWavBuffer(wavBuffer, { bin, model }, { timeoutMs = 60_000 } = {}) {
  return new Promise((resolve, reject) => {
    if (!bin || !model) {
      reject(new Error("local speech-to-text is not configured"));
      return;
    }
    const wavPath = path.join(os.tmpdir(), `ollamax-companion-${Date.now()}.wav`);
    try {
      fs.writeFileSync(wavPath, wavBuffer);
    } catch (e) {
      reject(e);
      return;
    }
    const cleanup = () => fs.unlink(wavPath, () => {});
    const proc = cp.spawn(bin, ["-m", model, "-f", wavPath, "-nt"], {
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let out = "";
    let err = "";
    const killTimer = setTimeout(() => {
      try {
        proc.kill("SIGKILL");
      } catch (_) {}
      cleanup();
      reject(new Error("whisper timed out"));
    }, timeoutMs);
    proc.stdout.on("data", (d) => (out += d.toString()));
    proc.stderr.on("data", (d) => (err += d.toString()));
    proc.on("error", (e) => {
      clearTimeout(killTimer);
      cleanup();
      reject(
        new Error(
          e.code === "ENOENT" ? `whisper binary "${bin}" not found` : e.message
        )
      );
    });
    proc.on("close", (code) => {
      clearTimeout(killTimer);
      cleanup();
      if (code === 0) {
        resolve(out.replace(/\[[0-9:.\s\->]+\]/g, "").trim());
      } else {
        reject(new Error(err.trim() || `whisper exited with code ${code}`));
      }
    });
  });
}

module.exports = { discoverWhisper, transcribeWavBuffer };

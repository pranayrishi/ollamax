"use strict";

// Local voice runtime used by the Electron shell.  It deliberately has no
// network client: speech recognition is whisper.cpp and speech output uses an
// operating-system voice (or an explicitly configured local executable).
// Keeping this module independent of Electron also makes its safety-critical
// resolution and argument handling straightforward to test.

const childProcess = require("child_process");
const fs = require("fs");
const fsp = fs.promises;
const os = require("os");
const path = require("path");

// The visible recorder stops at 60 seconds and always emits mono, 16 kHz,
// signed-16-bit PCM. Keep a small scheduling margin rather than accepting a
// multi-minute arbitrary RIFF payload from the renderer.
const MAX_WAV_BYTES = 2 * 1024 * 1024;
const PCM_SAMPLE_RATE = 16_000;
const PCM_CHANNELS = 1;
const PCM_BITS_PER_SAMPLE = 16;
const PCM_BLOCK_ALIGN = (PCM_CHANNELS * PCM_BITS_PER_SAMPLE) / 8;
const PCM_BYTE_RATE = PCM_SAMPLE_RATE * PCM_BLOCK_ALIGN;
const MAX_OUTPUT_BYTES = 512 * 1024;
const DEFAULT_TIMEOUT_MS = 120_000;

function executableName(name, platform = process.platform) {
  return platform === "win32" && !name.toLowerCase().endsWith(".exe") ? `${name}.exe` : name;
}

function fileExists(candidate) {
  try {
    return !!candidate && fs.statSync(candidate).isFile();
  } catch (_) {
    return false;
  }
}

function findOnPath(command, env = process.env, platform = process.platform) {
  if (!command) return null;
  if (path.isAbsolute(command) || command.includes(path.sep)) return fileExists(command) ? command : null;
  const separator = platform === "win32" ? ";" : ":";
  const suffixes = platform === "win32" ? (env.PATHEXT || ".EXE;.CMD;.BAT").split(";") : [""];
  for (const directory of String(env.PATH || "").split(separator)) {
    if (!directory) continue;
    for (const suffix of suffixes) {
      const alreadySuffixed = platform === "win32"
        && suffix
        && command.toLowerCase().endsWith(suffix.toLowerCase());
      const candidate = path.join(
        directory,
        platform === "win32" && !alreadySuffixed ? command + suffix.toLowerCase() : command
      );
      if (fileExists(candidate)) return candidate;
    }
  }
  return null;
}

function firstExisting(candidates) {
  return candidates.find(fileExists) || null;
}

function voiceRoots({ resourcesPath, appPath, moduleDir = __dirname } = {}) {
  // Production assets live beside the bundled engine.  Development candidates
  // are intentionally the same relative layout, so a release smoke test and a
  // local developer exercise identical resolution logic.
  return [
    resourcesPath && path.join(resourcesPath, "voice"),
    resourcesPath && path.join(resourcesPath, "bin", "voice"),
    appPath && path.join(appPath, "voice"),
    path.join(moduleDir, "..", "bin", "voice"),
    path.join(moduleDir, "..", "bin"),
  ].filter(Boolean);
}

function resolveWhisperRuntime(options = {}) {
  const env = options.env || process.env;
  const platform = options.platform || process.platform;
  const binaryName = executableName("whisper-cli", platform);
  const roots = voiceRoots(options);
  const explicitBinary = env.OLLAMAX_WHISPER_PATH && findOnPath(env.OLLAMAX_WHISPER_PATH, env, platform);
  const explicitModel = env.OLLAMAX_WHISPER_MODEL && firstExisting([env.OLLAMAX_WHISPER_MODEL]);
  const binary = explicitBinary || firstExisting(roots.map((root) => path.join(root, binaryName))) || findOnPath(binaryName, env, platform);
  const model = explicitModel || firstExisting(roots.map((root) => path.join(root, "ggml-base.en.bin")));

  if (!binary || !model) {
    return {
      available: false,
      binary: binary || null,
      model: model || null,
      reason: !binary
        ? "Local whisper.cpp was not found. Install whisper.cpp or set OLLAMAX_WHISPER_PATH."
        : "A local Whisper ggml model was not found. Set OLLAMAX_WHISPER_MODEL.",
    };
  }
  return { available: true, binary, model, source: explicitBinary || explicitModel ? "configured" : "local" };
}

function resolveTtsRuntime(options = {}) {
  const env = options.env || process.env;
  const platform = options.platform || process.platform;
  const configured = env.OLLAMAX_TTS_PATH && findOnPath(env.OLLAMAX_TTS_PATH, env, platform);
  if (configured) return { available: true, kind: "configured", command: configured };
  if (platform === "darwin") return { available: true, kind: "macos-say", command: findOnPath("say", env, platform) || "say" };
  if (platform === "win32") return { available: true, kind: "windows-sapi", command: findOnPath("powershell.exe", env, platform) || "powershell.exe" };
  const espeak = findOnPath("espeak-ng", env, platform) || findOnPath("espeak", env, platform);
  if (espeak) return { available: true, kind: "espeak", command: espeak };
  return {
    available: false,
    reason: "No local speech output was found. Install espeak-ng or set OLLAMAX_TTS_PATH to a local speech command.",
  };
}

function cleanTranscript(output) {
  return String(output || "")
    .replace(/\[[0-9:.\s\-\>]+\]/g, " ")
    .replace(/^\s*(output\s+transcription|system_info|main:).*$/gim, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function assertWav(buffer) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 46 || buffer.length > MAX_WAV_BYTES) {
    throw new Error(`Voice capture must be a WAV between 44 bytes and ${Math.floor(MAX_WAV_BYTES / 1024 / 1024)} MB.`);
  }
  if (
    buffer.subarray(0, 4).toString("ascii") !== "RIFF" ||
    buffer.readUInt32LE(4) !== buffer.length - 8 ||
    buffer.subarray(8, 12).toString("ascii") !== "WAVE" ||
    buffer.subarray(12, 16).toString("ascii") !== "fmt " ||
    buffer.readUInt32LE(16) !== 16 ||
    buffer.readUInt16LE(20) !== 1 ||
    buffer.readUInt16LE(22) !== PCM_CHANNELS ||
    buffer.readUInt32LE(24) !== PCM_SAMPLE_RATE ||
    buffer.readUInt32LE(28) !== PCM_BYTE_RATE ||
    buffer.readUInt16LE(32) !== PCM_BLOCK_ALIGN ||
    buffer.readUInt16LE(34) !== PCM_BITS_PER_SAMPLE ||
    buffer.subarray(36, 40).toString("ascii") !== "data" ||
    buffer.readUInt32LE(40) !== buffer.length - 44 ||
    buffer.length - 44 < PCM_BLOCK_ALIGN ||
    (buffer.length - 44) % PCM_BLOCK_ALIGN !== 0
  ) {
    throw new Error("Voice capture must be canonical 16 kHz mono PCM WAV audio.");
  }
}

function runLocal(command, args, { input, timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer = null;
    let stdout = "";
    let stderr = "";
    const finish = (error, result) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      error ? reject(error) : resolve(result);
    };
    let child;
    try {
      child = childProcess.spawn(command, args, {
        shell: false,
        windowsHide: true,
        stdio: [input == null ? "ignore" : "pipe", "pipe", "pipe"],
      });
    } catch (error) {
      finish(error);
      return;
    }
    timer = setTimeout(() => {
      try { child.kill(); } catch (_) {}
      finish(new Error("Local voice runtime timed out."));
    }, timeoutMs);
    const collect = (which) => (chunk) => {
      const next = which === "out" ? stdout + chunk.toString() : stderr + chunk.toString();
      if (next.length > MAX_OUTPUT_BYTES) {
        try { child.kill(); } catch (_) {}
        finish(new Error("Local voice runtime produced too much output."));
        return;
      }
      if (which === "out") stdout = next;
      else stderr = next;
    };
    child.stdout.on("data", collect("out"));
    child.stderr.on("data", collect("err"));
    // A short-lived local command can close stdin before the write below.
    // Always consume that EPIPE so Electron never treats it as an unhandled
    // stream error; the child's exit status remains the authoritative result.
    child.stdin.on("error", (error) => {
      if (error && error.code === "EPIPE") return;
      finish(error instanceof Error ? error : new Error(String(error)));
    });
    child.on("error", (error) => {
      const detail = error && error.code === "ENOENT" ? `Local executable '${command}' was not found.` : error.message;
      finish(new Error(detail));
    });
    child.on("close", (code) => {
      if (code === 0) finish(null, { stdout, stderr });
      else finish(new Error(stderr.trim() || `Local voice runtime exited with ${code}.`));
    });
    if (input != null) child.stdin.end(input);
  });
}

async function transcribeWavBase64(wavBase64, options = {}) {
  const runtime = options.runtime || resolveWhisperRuntime(options);
  if (!runtime.available) throw new Error(runtime.reason);
  const encoded = String(wavBase64 || "");
  // Check the encoded size before decoding so an untrusted renderer cannot make
  // the main process allocate an arbitrarily large buffer.
  if (encoded.length > Math.ceil((MAX_WAV_BYTES * 4) / 3) + 8) {
    throw new Error(`Voice capture exceeds ${Math.floor(MAX_WAV_BYTES / 1024 / 1024)} MB.`);
  }
  const wav = Buffer.from(encoded, "base64");
  assertWav(wav);
  const tempDir = await fsp.mkdtemp(path.join(os.tmpdir(), "ollamax-stt-"));
  const inputPath = path.join(tempDir, "speech.wav");
  try {
    await fsp.writeFile(inputPath, wav, { mode: 0o600 });
    const result = await runLocal(runtime.binary, ["-m", runtime.model, "-f", inputPath, "-nt"], options);
    return cleanTranscript(result.stdout);
  } finally {
    await fsp.rm(tempDir, { recursive: true, force: true });
  }
}

function windowsSpeakScript() {
  return "$voice=New-Object -ComObject SAPI.SpVoice;$voice.Speak([Console]::In.ReadToEnd())";
}

async function speakText(text, options = {}) {
  const safeText = String(text || "").replace(/\0/g, "").trim().slice(0, 12_000);
  if (!safeText) return;
  const runtime = options.runtime || resolveTtsRuntime(options);
  if (!runtime.available) throw new Error(runtime.reason);
  // Kept injectable for a contract test. Production always uses the lexical
  // local runner; neither branch invokes a shell.
  const localRunner = typeof options.runLocal === "function" ? options.runLocal : runLocal;
  if (runtime.kind === "macos-say") {
    // `say` accepts stdin when no message argument is supplied. Do not put a
    // full assistant response in argv where ordinary local process listings
    // can expose it while speech is in progress.
    await localRunner(runtime.command, ["-r", "190"], { ...options, input: safeText });
  } else if (runtime.kind === "windows-sapi") {
    const encoded = Buffer.from(windowsSpeakScript(), "utf16le").toString("base64");
    await localRunner(runtime.command, ["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], { ...options, input: safeText });
  } else if (runtime.kind === "espeak") {
    await localRunner(runtime.command, ["--stdin"], { ...options, input: safeText });
  } else {
    // A configured local executable receives plain text on stdin. No shell or
    // interpolated arguments are used, so spoken text cannot become a command.
    await localRunner(runtime.command, [], { ...options, input: safeText });
  }
}

module.exports = {
  MAX_WAV_BYTES,
  PCM_BITS_PER_SAMPLE,
  PCM_CHANNELS,
  PCM_SAMPLE_RATE,
  assertWav,
  cleanTranscript,
  executableName,
  findOnPath,
  runLocal,
  resolveTtsRuntime,
  resolveWhisperRuntime,
  speakText,
  transcribeWavBase64,
  windowsSpeakScript,
};

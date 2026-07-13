"use strict";

// One audited contract shared by CI staging and both source/package validators.
// The model hash and byte count are intentionally checked again after
// electron-builder copies Resources: validating a download once is not enough
// to prove the installer contains the reviewed asset.
const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const WHISPER_CPP_VERSION = "v1.9.1";
const WHISPER_CPP_COMMIT = "f049fff95a089aa9969deb009cdd4892b3e74916";
const WHISPER_MODEL_FILE = "ggml-base.en.bin";
const WHISPER_MODEL_SHA256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";
const WHISPER_MODEL_BYTES = 147_964_211;
const WHISPER_LICENSE_FILE = "WHISPER_CPP_LICENSE.txt";

function assertSafeFileName(value, label) {
  if (
    typeof value !== "string" ||
    !value ||
    value !== path.basename(value) ||
    value.includes("/") ||
    value.includes("\\") ||
    value === "." ||
    value === ".." ||
    value.includes("\0")
  ) {
    throw new Error(`${label} must be a simple file name`);
  }
  return value;
}

function voiceBinaryName(binary = "whisper-cli", platform = process.platform) {
  const safe = assertSafeFileName(binary, "voice recognition binary");
  if (platform === "win32") {
    return safe.toLowerCase().endsWith(".exe") ? safe : `${safe}.exe`;
  }
  if (safe.toLowerCase().endsWith(".exe")) {
    throw new Error("non-Windows voice recognition binary must not use a .exe suffix");
  }
  return safe;
}

function assertNonEmptyFile(file, label = file) {
  let stats;
  try {
    stats = fs.statSync(file);
  } catch (_) {
    throw new Error(`missing ${label}: ${file}`);
  }
  if (!stats.isFile() || stats.size === 0) {
    throw new Error(`missing or empty ${label}: ${file}`);
  }
  return stats;
}

function sha256File(file) {
  const descriptor = fs.openSync(file, "r");
  const hash = crypto.createHash("sha256");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let offset = 0;
    for (;;) {
      const bytes = fs.readSync(descriptor, buffer, 0, buffer.length, offset);
      if (bytes === 0) break;
      hash.update(buffer.subarray(0, bytes));
      offset += bytes;
    }
  } finally {
    fs.closeSync(descriptor);
  }
  return hash.digest("hex");
}

function assertPinnedWhisperModel(file) {
  const stats = assertNonEmptyFile(file, `pinned Whisper model (${WHISPER_MODEL_FILE})`);
  if (stats.size !== WHISPER_MODEL_BYTES) {
    throw new Error(
      `pinned Whisper model has ${stats.size} bytes; expected ${WHISPER_MODEL_BYTES} bytes`
    );
  }
  const actual = sha256File(file);
  if (actual !== WHISPER_MODEL_SHA256) {
    throw new Error(`pinned Whisper model SHA-256 mismatch: expected ${WHISPER_MODEL_SHA256}, got ${actual}`);
  }
  return { bytes: stats.size, sha256: actual };
}

function declaredWindowsRuntimeFiles(recognition) {
  const files = recognition?.windowsRuntimeFiles ?? [];
  if (!Array.isArray(files)) {
    throw new Error("voice manifest windowsRuntimeFiles must be an array");
  }
  const unique = new Set();
  for (const file of files) {
    const safe = assertSafeFileName(file, "Windows voice runtime dependency");
    if (!safe.toLowerCase().endsWith(".dll")) {
      throw new Error(`Windows voice runtime dependency must be a .dll file: ${safe}`);
    }
    if (unique.has(safe)) {
      throw new Error(`Windows voice runtime dependency is declared more than once: ${safe}`);
    }
    unique.add(safe);
  }
  return [...unique];
}

function assertBundledWhisperRuntime(voiceDir, recognition, platform = process.platform, options = {}) {
  if (!recognition || recognition.engine !== "whisper.cpp" || recognition.bundled !== true) {
    throw new Error("bundled voice runtime must declare whisper.cpp with bundled: true");
  }
  if (recognition.version !== WHISPER_CPP_VERSION) {
    throw new Error(`bundled whisper.cpp version must be ${WHISPER_CPP_VERSION}`);
  }
  if (recognition.defaultModel !== WHISPER_MODEL_FILE) {
    throw new Error(`bundled Whisper model must be ${WHISPER_MODEL_FILE}`);
  }
  if (recognition.binary !== "whisper-cli") {
    throw new Error("bundled Whisper binary must be declared as whisper-cli");
  }
  if (recognition.licenseFile !== WHISPER_LICENSE_FILE) {
    throw new Error(`bundled Whisper license file must be ${WHISPER_LICENSE_FILE}`);
  }
  if (recognition.modelSha256 !== WHISPER_MODEL_SHA256 || recognition.modelBytes !== WHISPER_MODEL_BYTES) {
    throw new Error("bundled voice manifest does not declare the reviewed Whisper model checksum and byte count");
  }

  const binary = path.join(voiceDir, voiceBinaryName(recognition.binary, platform));
  const binaryStats = assertNonEmptyFile(binary, "bundled whisper-cli executable");
  if (platform !== "win32" && (binaryStats.mode & 0o111) === 0) {
    throw new Error(`bundled whisper-cli executable is not marked executable: ${binary}`);
  }
  const model = path.join(voiceDir, WHISPER_MODEL_FILE);
  assertNonEmptyFile(model, "bundled Whisper model");
  (options.assertModel || assertPinnedWhisperModel)(model);
  const licenseFile = assertSafeFileName(recognition.licenseFile, "voice license file");
  assertNonEmptyFile(path.join(voiceDir, licenseFile), "bundled whisper.cpp license notice");

  const windowsRuntimeFiles = declaredWindowsRuntimeFiles(recognition);
  for (const dependency of windowsRuntimeFiles) {
    assertNonEmptyFile(path.join(voiceDir, dependency), `bundled Windows voice dependency (${dependency})`);
  }
  return { binary, model, licenseFile, windowsRuntimeFiles };
}

module.exports = {
  WHISPER_CPP_COMMIT,
  WHISPER_CPP_VERSION,
  WHISPER_LICENSE_FILE,
  WHISPER_MODEL_BYTES,
  WHISPER_MODEL_FILE,
  WHISPER_MODEL_SHA256,
  assertBundledWhisperRuntime,
  assertNonEmptyFile,
  assertPinnedWhisperModel,
  assertSafeFileName,
  declaredWindowsRuntimeFiles,
  sha256File,
  voiceBinaryName,
};

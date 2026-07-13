// Release-only staging for the bundled, fully local Whisper runtime. This
// script never downloads anything: release-app.yml supplies a native
// whisper-cli, the official model file, and the upstream license notice after
// it has checked out the pinned whisper.cpp source. Keeping acquisition in CI
// makes the source tree small and keeps `manifest.json` intentionally
// `bundled: false` for ordinary developer builds.
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const {
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
  voiceBinaryName,
} = require("./voice-contract.cjs");

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const voiceDir = resolve(appDir, "voice");
const manifestPath = resolve(voiceDir, "manifest.json");

function requiredPath(environmentName) {
  const value = process.env[environmentName];
  if (!value) {
    throw new Error(`${environmentName} is required to stage the release-only Whisper runtime`);
  }
  return resolve(value);
}

function copyChecked(source, destination, label) {
  assertNonEmptyFile(source, label);
  rmSync(destination, { force: true });
  copyFileSync(source, destination);
  assertNonEmptyFile(destination, label);
}

function collectAdjacentLibraries(sourceDirectory, destinationDirectory, extension, label) {
  // `BUILD_SHARED_LIBS=OFF` and the static MSVC runtime normally leave this
  // empty. If a future whisper.cpp build emits DLLs next to whisper-cli.exe,
  // copy every such DLL and record it in the manifest so package validation
  // can prove none was lost between CI staging and the installer.
  let entries;
  try {
    entries = readdirSync(sourceDirectory, { withFileTypes: true });
  } catch (error) {
    throw new Error(`cannot inspect ${label} whisper runtime directory ${sourceDirectory}: ${error.message}`);
  }

  const files = entries
    .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith(extension))
    .map((entry) => assertSafeFileName(entry.name, `${label} dependency`))
    .sort((left, right) => left.localeCompare(right));

  for (const file of files) {
    copyChecked(resolve(sourceDirectory, file), resolve(destinationDirectory, file), `${label} dependency ${file}`);
  }
  return files;
}

function stage() {
  if (process.env.OLLAMAX_RELEASE_STAGE_VOICE !== "1") {
    throw new Error(
      "Refusing to stage a bundled voice runtime outside release CI. Set OLLAMAX_RELEASE_STAGE_VOICE=1 only in the reviewed release workflow."
    );
  }
  if (
    process.env.WHISPER_CPP_VERSION !== WHISPER_CPP_VERSION ||
    process.env.WHISPER_CPP_COMMIT !== WHISPER_CPP_COMMIT
  ) {
    throw new Error(
      `release staging requires whisper.cpp ${WHISPER_CPP_VERSION} at ${WHISPER_CPP_COMMIT}`
    );
  }

  const targetPlatform = process.env.OLLAMAX_WHISPER_STAGE_PLATFORM || process.platform;
  if (!new Set(["darwin", "linux", "win32"]).has(targetPlatform)) {
    throw new Error(`unsupported Whisper staging platform: ${targetPlatform}`);
  }
  if (targetPlatform !== process.platform) {
    throw new Error(
      `Whisper staging target (${targetPlatform}) must match the native CI host (${process.platform})`
    );
  }

  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (error) {
    throw new Error(`cannot read source voice manifest: ${error.message}`);
  }
  const recognition = manifest?.recognition;
  if (manifest?.privacy !== "on-device-only" || recognition?.engine !== "whisper.cpp") {
    throw new Error("source voice manifest must declare on-device whisper.cpp recognition");
  }
  if (recognition.bundled !== false) {
    throw new Error("release staging requires the checked-out source voice manifest to remain bundled: false");
  }
  if (recognition.binary !== "whisper-cli" || recognition.defaultModel !== WHISPER_MODEL_FILE) {
    throw new Error("source voice manifest must reserve the canonical whisper-cli and ggml-base.en.bin names");
  }

  const binarySource = requiredPath("OLLAMAX_WHISPER_CLI_PATH");
  const modelSource = requiredPath("OLLAMAX_WHISPER_MODEL_PATH");
  const licenseSource = requiredPath("OLLAMAX_WHISPER_LICENSE_PATH");
  const binaryName = voiceBinaryName(recognition.binary, targetPlatform);
  const windowsDirectory = process.env.OLLAMAX_WHISPER_DLL_DIR
    ? resolve(process.env.OLLAMAX_WHISPER_DLL_DIR)
    : dirname(binarySource);

  // Verify the official payload before it reaches the package directory, then
  // verify it again after copying through the shared contract below.
  assertPinnedWhisperModel(modelSource);
  mkdirSync(voiceDir, { recursive: true });
  copyChecked(binarySource, resolve(voiceDir, binaryName), "native whisper-cli executable");
  if (targetPlatform !== "win32") chmodSync(resolve(voiceDir, binaryName), 0o755);
  copyChecked(modelSource, resolve(voiceDir, WHISPER_MODEL_FILE), "pinned Whisper model");
  copyChecked(licenseSource, resolve(voiceDir, WHISPER_LICENSE_FILE), "whisper.cpp license notice");
  const windowsRuntimeFiles = targetPlatform === "win32"
    ? collectAdjacentLibraries(windowsDirectory, voiceDir, ".dll", "Windows")
    : [];
  // A static build should not produce these, but copy any adjacent dylibs when
  // it does. afterPack signs every staged voice dylib before signing the app.
  const macRuntimeFiles = targetPlatform === "darwin"
    ? collectAdjacentLibraries(dirname(binarySource), voiceDir, ".dylib", "macOS")
    : [];

  const stagedRecognition = {
    ...recognition,
    binary: recognition.binary,
    defaultModel: WHISPER_MODEL_FILE,
    bundled: true,
    version: WHISPER_CPP_VERSION,
    modelSha256: WHISPER_MODEL_SHA256,
    modelBytes: WHISPER_MODEL_BYTES,
    licenseFile: WHISPER_LICENSE_FILE,
    windowsRuntimeFiles,
    notes: "Staged only by release-app CI from pinned whisper.cpp v1.9.1 and the reviewed official ggml-base.en.bin payload. Recognition remains fully local; no hosted transcription API is included.",
  };
  const stagedManifest = { ...manifest, recognition: stagedRecognition };
  writeFileSync(manifestPath, `${JSON.stringify(stagedManifest, null, 2)}\n`);

  const verified = assertBundledWhisperRuntime(voiceDir, stagedRecognition, targetPlatform);
  console.log(
    `staged verified local Whisper runtime (${targetPlatform}): ${binaryName}, ${WHISPER_MODEL_FILE}, ` +
      `${verified.windowsRuntimeFiles.length} declared Windows DLL dependency(s), ` +
      `${macRuntimeFiles.length} staged macOS dylib(s)`
  );
}

stage();

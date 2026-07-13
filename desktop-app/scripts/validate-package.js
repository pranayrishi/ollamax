"use strict";

// Validate the *staged* application rather than only its source tree. This
// runs from Electron Builder's afterPack hook, after `files` and
// `extraResources` have been copied, so a release cannot silently omit the
// local voice or spatial-context implementation.
const fs = require("fs");
const path = require("path");
const { assertBundledWhisperRuntime } = require("./voice-contract.cjs");

const REQUIRED_APP_FILES = Object.freeze([
  "main.js",
  "preload.js",
  "cursor-buddy-preload.js",
  "cursor-buddy-state.js",
  "spatial-preload.js",
  "spatial-selection.js",
  "lib/desktop-security.js",
  "lib/voice-runtime.js",
  "lib/workspace-paths.js",
  "renderer/index.html",
  "renderer/cursor-buddy.html",
  "renderer/cursor-buddy.css",
  "renderer/cursor-buddy.js",
  "renderer/point-directives.js",
  "renderer/desktop-points.js",
  "renderer/desktop-voice.js",
  "renderer/desktop-spatial.js",
  "renderer/spatial-overlay.html",
  "renderer/spatial-overlay.css",
  "renderer/spatial-overlay.js",
]);

function isFile(file) {
  try {
    return fs.statSync(file).isFile();
  } catch (_) {
    return false;
  }
}

function isDirectory(directory) {
  try {
    return fs.statSync(directory).isDirectory();
  } catch (_) {
    return false;
  }
}

function assertNonEmptyFile(file, label = file) {
  if (!isFile(file) || fs.statSync(file).size === 0) {
    throw new Error(`missing or empty packaged ${label}: ${file}`);
  }
  return file;
}

function normalizeArchivePath(file) {
  return String(file).replace(/\\/g, "/").replace(/^\/+/, "").replace(/\/+$/, "");
}

function listAsarFiles(archive) {
  let asar;
  try {
    // electron-builder already installs this dependency. Keeping inspection in
    // process avoids shelling out to a possibly different global `asar` tool.
    asar = require("@electron/asar");
  } catch (error) {
    throw new Error(`cannot inspect packaged app.asar: ${error.message}`);
  }
  return asar.listPackage(archive);
}

function validateVoiceManifest(
  resourcesDir,
  {
    platformName = process.platform,
    assertVoiceModel,
    requireBundled = process.env.OLLAMAX_REQUIRE_BUNDLED_VOICE === "1",
  } = {}
) {
  const manifestPath = assertNonEmptyFile(
    path.join(resourcesDir, "voice", "manifest.json"),
    "voice manifest"
  );

  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  } catch (error) {
    throw new Error(`invalid packaged voice manifest: ${error.message}`);
  }

  const recognition = manifest?.recognition;
  if (
    manifest?.privacy !== "on-device-only" ||
    recognition?.engine !== "whisper.cpp" ||
    typeof recognition.bundled !== "boolean"
  ) {
    throw new Error("packaged voice manifest does not declare the local Whisper runtime contract");
  }
  if (requireBundled && recognition.bundled !== true) {
    throw new Error("packaged release requires voice manifest bundled: true");
  }

  if (recognition.bundled) {
    // Verify the copied package payload, not only the CI source download. The
    // test-only callback lets unit tests avoid creating a 148 MB fixture; real
    // afterPack calls leave it undefined and always perform the SHA-256 check.
    assertBundledWhisperRuntime(path.join(resourcesDir, "voice"), recognition, platformName, {
      assertModel: assertVoiceModel,
    });
  }

  // `bundled: false` remains valid for an ordinary source checkout. Any
  // package that claims the runtime is bundled must satisfy the full contract
  // above, including the native executable, model, license, and declared DLLs.
  return manifest;
}

function validateApplicationFiles(resourcesDir, { listArchive = listAsarFiles } = {}) {
  const archivePath = path.join(resourcesDir, "app.asar");
  const appDirectory = path.join(resourcesDir, "app");
  const unpackedDirectory = `${archivePath}.unpacked`;
  let archiveFiles = new Set();
  let inspectedArchive = false;

  if (fs.existsSync(archivePath)) {
    assertNonEmptyFile(archivePath, "app.asar");
    const listed = listArchive(archivePath);
    if (!Array.isArray(listed)) {
      throw new Error("could not enumerate packaged app.asar files");
    }
    archiveFiles = new Set(listed.map(normalizeArchivePath));
    inspectedArchive = true;
  }

  const hasUnpackedApp = isDirectory(appDirectory) || isDirectory(unpackedDirectory);
  if (!inspectedArchive && !hasUnpackedApp) {
    throw new Error(
      `packaged application is missing both ${archivePath} and an unpacked app directory`
    );
  }

  const missing = REQUIRED_APP_FILES.filter((relativePath) => {
    if (archiveFiles.has(relativePath)) return false;
    return ![appDirectory, unpackedDirectory].some((root) => isFile(path.join(root, relativePath)));
  });
  if (missing.length) {
    throw new Error(`packaged app is missing required voice/spatial files: ${missing.join(", ")}`);
  }

  return {
    storage: inspectedArchive ? "app.asar" : "unpacked app directory",
    requiredFiles: REQUIRED_APP_FILES.slice(),
  };
}

function validatePackagedResources({
  resourcesDir,
  platformName,
  listArchive,
  assertVoiceModel,
  requireBundledVoice = process.env.OLLAMAX_REQUIRE_BUNDLED_VOICE === "1",
} = {}) {
  if (!resourcesDir) throw new Error("package validation needs a Resources directory");

  const targetPlatform = platformName || process.platform;
  const engineName = targetPlatform === "win32" ? "forge.exe" : "forge";
  assertNonEmptyFile(path.join(resourcesDir, "bin", engineName), `forge engine (${engineName})`);
  const manifest = validateVoiceManifest(resourcesDir, {
    platformName: targetPlatform,
    assertVoiceModel,
    requireBundled: requireBundledVoice,
  });
  const app = validateApplicationFiles(resourcesDir, { listArchive });

  return {
    resourcesDir,
    engineName,
    appStorage: app.storage,
    voiceRuntimeBundled: manifest.recognition.bundled,
  };
}

function resourcesDirForAfterPack(context) {
  if (!context?.appOutDir || !context?.electronPlatformName) {
    throw new Error("Electron Builder afterPack context is incomplete");
  }
  if (context.electronPlatformName !== "darwin") {
    return path.join(context.appOutDir, "resources");
  }

  const productFilename = context.packager?.appInfo?.productFilename;
  if (!productFilename) throw new Error("could not resolve the packaged macOS app name");
  return path.join(context.appOutDir, `${productFilename}.app`, "Contents", "Resources");
}

function validateAfterPack(context) {
  const resourcesDir = resourcesDirForAfterPack(context);
  const result = validatePackagedResources({
    resourcesDir,
    platformName: context.electronPlatformName,
  });
  console.log(
    `[afterPack] validated ${result.appStorage}; ${result.engineName}; ` +
      `voice runtime ${result.voiceRuntimeBundled ? "bundled" : "optional"}`
  );
  return result;
}

module.exports = {
  REQUIRED_APP_FILES,
  assertNonEmptyFile,
  normalizeArchivePath,
  resourcesDirForAfterPack,
  validateAfterPack,
  validateApplicationFiles,
  validatePackagedResources,
  validateVoiceManifest,
};

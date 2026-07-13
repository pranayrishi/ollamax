"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const {
  REQUIRED_APP_FILES,
  validatePackagedResources,
} = require("../scripts/validate-package");
const {
  WHISPER_CPP_VERSION,
  WHISPER_LICENSE_FILE,
  WHISPER_MODEL_BYTES,
  WHISPER_MODEL_FILE,
  WHISPER_MODEL_SHA256,
} = require("../scripts/voice-contract.cjs");

function makeTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-package-test-"));
}

function writeFile(file, contents = "x") {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, contents);
}

function writeVoiceManifest(resourcesDir, recognition = { engine: "whisper.cpp", bundled: false }) {
  writeFile(
    path.join(resourcesDir, "voice", "manifest.json"),
    JSON.stringify({
      privacy: "on-device-only",
      recognition,
    })
  );
}

function bundledRecognition(overrides = {}) {
  return {
    engine: "whisper.cpp",
    binary: "whisper-cli",
    defaultModel: WHISPER_MODEL_FILE,
    bundled: true,
    version: WHISPER_CPP_VERSION,
    modelSha256: WHISPER_MODEL_SHA256,
    modelBytes: WHISPER_MODEL_BYTES,
    licenseFile: WHISPER_LICENSE_FILE,
    windowsRuntimeFiles: [],
    ...overrides,
  };
}

function writeBundledVoice(
  resourcesDir,
  { platformName = "linux", windowsRuntimeFiles = [], includeDependencies = true } = {}
) {
  writeVoiceManifest(resourcesDir, bundledRecognition({ windowsRuntimeFiles }));
  const voiceDir = path.join(resourcesDir, "voice");
  const binary = path.join(voiceDir, platformName === "win32" ? "whisper-cli.exe" : "whisper-cli");
  writeFile(binary, "binary");
  if (platformName !== "win32") fs.chmodSync(binary, 0o755);
  writeFile(path.join(voiceDir, WHISPER_MODEL_FILE), "model fixture");
  writeFile(path.join(voiceDir, WHISPER_LICENSE_FILE), "MIT License\n");
  if (includeDependencies) {
    for (const dependency of windowsRuntimeFiles) writeFile(path.join(voiceDir, dependency), "dll");
  }
}

function writeEngine(resourcesDir, platformName = "linux") {
  writeFile(path.join(resourcesDir, "bin", platformName === "win32" ? "forge.exe" : "forge"));
}

function writeUnpackedApp(resourcesDir, omitted = new Set()) {
  for (const relativePath of REQUIRED_APP_FILES) {
    if (!omitted.has(relativePath)) writeFile(path.join(resourcesDir, "app", relativePath));
  }
}

test("accepts an unpacked package with an optional local Whisper runtime", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeVoiceManifest(resourcesDir);
    writeUnpackedApp(resourcesDir);

    const result = validatePackagedResources({
      resourcesDir,
      platformName: "linux",
      // This fixture models a source/development package. Keep the test
      // deterministic when the release packager exports its strict runtime
      // requirement to the process environment.
      requireBundledVoice: false,
    });
    assert.equal(result.engineName, "forge");
    assert.equal(result.appStorage, "unpacked app directory");
    assert.equal(result.voiceRuntimeBundled, false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("release package mode rejects an optional voice manifest after copying", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeVoiceManifest(resourcesDir);
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "linux",
        requireBundledVoice: true,
      }),
      /requires voice manifest bundled: true/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("accepts required files enumerated from app.asar", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir, "win32");
    writeVoiceManifest(resourcesDir);
    writeFile(path.join(resourcesDir, "app.asar"), "archive placeholder");

    const result = validatePackagedResources({
      resourcesDir,
      platformName: "win32",
      listArchive: () => REQUIRED_APP_FILES.map((file) => `/${file}`),
      requireBundledVoice: false,
    });
    assert.equal(result.engineName, "forge.exe");
    assert.equal(result.appStorage, "app.asar");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a package missing a required spatial overlay file", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeVoiceManifest(resourcesDir);
    writeUnpackedApp(resourcesDir, new Set(["renderer/spatial-overlay.js"]));

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "linux",
        requireBundledVoice: false,
      }),
      /renderer\/spatial-overlay\.js/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a package missing the click-through cursor companion preload", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeVoiceManifest(resourcesDir);
    writeUnpackedApp(resourcesDir, new Set(["cursor-buddy-preload.js"]));

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "linux",
        requireBundledVoice: false,
      }),
      /cursor-buddy-preload\.js/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("package contract includes the local POINT parser and desktop bridge", () => {
  assert.ok(REQUIRED_APP_FILES.includes("renderer/point-directives.js"));
  assert.ok(REQUIRED_APP_FILES.includes("renderer/desktop-points.js"));
  assert.ok(REQUIRED_APP_FILES.includes("renderer/bridge.js"));
  assert.ok(REQUIRED_APP_FILES.includes("renderer/hub-bridge.js"));
  assert.ok(REQUIRED_APP_FILES.includes("renderer/main.js"));
  assert.ok(REQUIRED_APP_FILES.includes("lib/attachment-preview.js"));
  assert.ok(REQUIRED_APP_FILES.includes("lib/desktop-auth.js"));
  assert.ok(REQUIRED_APP_FILES.includes("lib/desktop-security.js"));
  assert.ok(REQUIRED_APP_FILES.includes("lib/workspace-paths.js"));
});

test("accepts a declared bundled Windows runtime only with its executable, model, license, and DLLs", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir, "win32");
    writeBundledVoice(resourcesDir, {
      platformName: "win32",
      windowsRuntimeFiles: ["ggml-runtime.dll"],
    });
    writeUnpackedApp(resourcesDir);

    let checkedModel = null;
    const result = validatePackagedResources({
      resourcesDir,
      platformName: "win32",
      assertVoiceModel: (file) => { checkedModel = file; },
    });
    assert.equal(result.voiceRuntimeBundled, true);
    assert.equal(checkedModel, path.join(resourcesDir, "voice", WHISPER_MODEL_FILE));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a bundled package that omits the platform-native whisper executable", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir, "win32");
    writeVoiceManifest(resourcesDir, bundledRecognition());
    writeFile(path.join(resourcesDir, "voice", WHISPER_MODEL_FILE), "model fixture");
    writeFile(path.join(resourcesDir, "voice", WHISPER_LICENSE_FILE), "MIT License\n");
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "win32",
        assertVoiceModel: () => {},
      }),
      /whisper-cli executable/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a bundled package that omits the Whisper model even in a lightweight test", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeVoiceManifest(resourcesDir, bundledRecognition());
    const binary = path.join(resourcesDir, "voice", "whisper-cli");
    writeFile(binary, "binary");
    fs.chmodSync(binary, 0o755);
    writeFile(path.join(resourcesDir, "voice", WHISPER_LICENSE_FILE), "MIT License\n");
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "linux",
        assertVoiceModel: () => {},
      }),
      /bundled Whisper model/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a non-executable bundled whisper-cli after a Unix package copy", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeBundledVoice(resourcesDir);
    fs.chmodSync(path.join(resourcesDir, "voice", "whisper-cli"), 0o644);
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "linux",
        assertVoiceModel: () => {},
      }),
      /not marked executable/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a bundled Windows package that loses a declared DLL dependency", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir, "win32");
    writeBundledVoice(resourcesDir, {
      platformName: "win32",
      windowsRuntimeFiles: ["ggml-runtime.dll"],
      includeDependencies: false,
    });
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({
        resourcesDir,
        platformName: "win32",
        assertVoiceModel: () => {},
      }),
      /ggml-runtime\.dll/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a bundled package whose model is not the reviewed payload", () => {
  const root = makeTempDir();
  try {
    const resourcesDir = path.join(root, "resources");
    writeEngine(resourcesDir);
    writeBundledVoice(resourcesDir);
    writeUnpackedApp(resourcesDir);

    assert.throws(
      () => validatePackagedResources({ resourcesDir, platformName: "linux" }),
      new RegExp(`${WHISPER_MODEL_BYTES} bytes`)
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

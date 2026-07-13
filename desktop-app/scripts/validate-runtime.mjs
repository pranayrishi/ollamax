// Validate the source-side local-runtime contract before electron-builder
// copies it. A normal checkout intentionally keeps the runtime optional; the
// release workflow stages a verified native runtime and sets
// OLLAMAX_REQUIRE_BUNDLED_VOICE=1 before packaging.
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { assertBundledWhisperRuntime } = require("./voice-contract.cjs");

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const voiceDir = resolve(appDir, "voice");
const manifestPath = resolve(voiceDir, "manifest.json");

if (!existsSync(manifestPath)) throw new Error(`missing voice runtime manifest: ${manifestPath}`);
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const recognition = manifest?.recognition;
if (manifest?.privacy !== "on-device-only" || recognition?.engine !== "whisper.cpp") {
  throw new Error("voice runtime manifest must declare on-device whisper.cpp recognition");
}
if (typeof recognition.bundled !== "boolean") {
  throw new Error("voice runtime manifest must explicitly declare whether Whisper is bundled");
}

const requireBundled = process.env.OLLAMAX_REQUIRE_BUNDLED_VOICE === "1";
if (requireBundled && recognition.bundled !== true) {
  throw new Error(
    "OLLAMAX_REQUIRE_BUNDLED_VOICE=1 requires release staging to set voice manifest bundled: true"
  );
}

if (recognition.bundled) {
  // This includes size + SHA-256 after CI copied the model, instead of merely
  // checking that an arbitrary nonempty file happened to be present.
  assertBundledWhisperRuntime(voiceDir, recognition, process.platform);
  console.log("validated bundled, checksummed local Whisper runtime");
} else {
  console.log("validated on-device voice manifest (runtime intentionally optional for this build)");
}

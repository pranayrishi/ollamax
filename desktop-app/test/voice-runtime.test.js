"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const voice = require("../lib/voice-runtime");

function wav(dataBytes = 320) {
  const buffer = Buffer.alloc(44 + dataBytes);
  buffer.write("RIFF", 0, "ascii");
  buffer.writeUInt32LE(buffer.length - 8, 4);
  buffer.write("WAVE", 8, "ascii");
  buffer.write("fmt ", 12, "ascii");
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(1, 22);
  buffer.writeUInt32LE(16_000, 24);
  buffer.writeUInt32LE(32_000, 28);
  buffer.writeUInt16LE(2, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write("data", 36, "ascii");
  buffer.writeUInt32LE(dataBytes, 40);
  return buffer;
}

test("cleans whisper timestamps and progress lines", () => {
  assert.equal(
    voice.cleanTranscript("system_info: x\n[00:00:01.000 --> 00:00:02.000]  replicate the search bar\n"),
    "replicate the search bar"
  );
});

test("accepts canonical recorder WAV and rejects arbitrary, malformed, or oversized audio", () => {
  assert.doesNotThrow(() => voice.assertWav(wav()));
  assert.throws(() => voice.assertWav(Buffer.from("not a wav")), /WAV/);
  assert.throws(() => voice.assertWav(Buffer.alloc(44)), /WAV/);
  assert.throws(() => voice.assertWav(Buffer.alloc(voice.MAX_WAV_BYTES + 1)), /WAV/);
  const wrongRate = wav();
  wrongRate.writeUInt32LE(44_100, 24);
  assert.throws(() => voice.assertWav(wrongRate), /canonical 16 kHz mono PCM/);
  const wrongDataLength = wav();
  wrongDataLength.writeUInt32LE(0, 40);
  assert.throws(() => voice.assertWav(wrongDataLength), /canonical 16 kHz mono PCM/);
});

test("resolves explicitly configured local whisper files without network access", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-voice-test-"));
  const bin = path.join(root, "whisper-cli");
  const model = path.join(root, "ggml-base.en.bin");
  fs.writeFileSync(bin, "binary");
  fs.writeFileSync(model, "model");
  try {
    const found = voice.resolveWhisperRuntime({
      env: { OLLAMAX_WHISPER_PATH: bin, OLLAMAX_WHISPER_MODEL: model, PATH: "" },
      platform: "linux",
    });
    assert.equal(found.available, true);
    assert.equal(found.binary, bin);
    assert.equal(found.model, model);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("missing whisper model produces an actionable local-only state", () => {
  const state = voice.resolveWhisperRuntime({ env: { PATH: "" }, platform: "linux", moduleDir: os.tmpdir() });
  assert.equal(state.available, false);
  assert.match(state.reason, /Local whisper\.cpp|Whisper ggml/);
});

test("Windows speech script is encoded separately from spoken text", () => {
  assert.match(voice.windowsSpeakScript(), /SAPI\.SpVoice/);
  assert.doesNotMatch(voice.windowsSpeakScript(), /user supplied/i);
});

test("macOS speech passes assistant text through stdin instead of process arguments", async () => {
  let invocation;
  await voice.speakText("private assistant response", {
    runtime: { available: true, kind: "macos-say", command: "/usr/bin/say" },
    runLocal: async (command, args, options) => {
      invocation = { command, args, input: options.input };
      return { stdout: "", stderr: "" };
    },
  });
  assert.deepEqual(invocation, {
    command: "/usr/bin/say",
    args: ["-r", "190"],
    input: "private assistant response",
  });
});

test("Windows PATH lookup does not double-append .exe", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-win-path-test-"));
  const binary = path.join(root, "whisper-cli.exe");
  fs.writeFileSync(binary, "binary");
  try {
    const env = { PATH: root, PATHEXT: ".EXE;.CMD" };
    assert.equal(voice.findOnPath("whisper-cli.exe", env, "win32"), binary);
    assert.equal(voice.findOnPath("whisper-cli", env, "win32"), binary);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a short-lived local command with stdin does not surface an EPIPE", async () => {
  await assert.doesNotReject(
    voice.runLocal(process.execPath, ["-e", "process.exit(0)"], {
      input: "local voice text",
      timeoutMs: 2_000,
    })
  );
});

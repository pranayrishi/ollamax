"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { resolveWorkspacePath } = require("../lib/workspace-paths");

function temporaryWorkspace() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-workspace-paths-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-workspace-outside-"));
  fs.mkdirSync(path.join(root, "src"));
  fs.writeFileSync(path.join(root, "src", "index.js"), "export {};\n");
  fs.writeFileSync(path.join(outside, "secret.txt"), "outside\n");
  return { root, outside };
}

test("direct IDE paths stay in the workspace and support only a missing final file", () => {
  const { root, outside } = temporaryWorkspace();
  try {
    const existing = resolveWorkspacePath(root, path.join(root, "src", "index.js"));
    assert.equal(existing.target, path.join(root, "src", "index.js"));

    const newFile = resolveWorkspacePath(root, path.join(root, "src", "new.js"), {
      allowMissingFinal: true,
    });
    assert.equal(newFile.target, path.join(root, "src", "new.js"));

    assert.match(resolveWorkspacePath(root, path.join(outside, "secret.txt")).error, /workspace/);
    assert.match(resolveWorkspacePath(root, path.join(root, "missing", "child.js"), { allowMissingFinal: true }).error, /does not exist/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  }
});

test("direct IDE paths reject symlink components", (t) => {
  const { root, outside } = temporaryWorkspace();
  try {
    const link = path.join(root, "outside-link");
    try {
      fs.symlinkSync(outside, link, "dir");
    } catch (error) {
      t.skip(`symlink fixture unavailable: ${error.code || error.message}`);
      return;
    }
    assert.match(resolveWorkspacePath(root, path.join(link, "secret.txt")).error, /symlink/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  }
});

test("agent preview paths require a relative non-traversing path", () => {
  const { root, outside } = temporaryWorkspace();
  try {
    assert.equal(
      resolveWorkspacePath(root, "src/index.js", { requireRelative: true }).relative,
      "src/index.js"
    );
    assert.match(
      resolveWorkspacePath(root, path.join(outside, "secret.txt"), { requireRelative: true }).error,
      /workspace/
    );
    assert.match(resolveWorkspacePath(root, "../secret.txt", { requireRelative: true }).error, /workspace/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  }
});

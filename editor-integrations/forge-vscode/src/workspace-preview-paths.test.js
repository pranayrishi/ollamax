"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { resolveWorkspacePreviewPath } = require("./workspace-preview-paths");

function temporaryWorkspace() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-preview-workspace-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-preview-outside-"));
  fs.mkdirSync(path.join(root, "src"));
  fs.writeFileSync(path.join(root, "src", "index.js"), "export {};\n");
  fs.writeFileSync(path.join(outside, "secret.txt"), "outside\n");
  return { root, outside };
}

function cleanup(...paths) {
  for (const value of paths) fs.rmSync(value, { recursive: true, force: true });
}

test("preview resolver accepts regular workspace files and safe new write targets", () => {
  const { root, outside } = temporaryWorkspace();
  try {
    const canonicalRoot = fs.realpathSync(root);
    const existing = resolveWorkspacePreviewPath(root, "src/index.js");
    assert.equal(existing.target, path.join(canonicalRoot, "src", "index.js"));
    assert.equal(existing.relative, "src/index.js");
    assert.equal(existing.exists, true);

    const proposed = resolveWorkspacePreviewPath(root, "new/nested/file.js", { allowMissing: true });
    assert.equal(proposed.target, path.join(canonicalRoot, "new", "nested", "file.js"));
    assert.equal(proposed.exists, false);

    for (const unsafe of ["../secret.txt", path.join(outside, "secret.txt"), "", ".", "C:\\outside.txt"]) {
      assert.match(resolveWorkspacePreviewPath(root, unsafe, { allowMissing: true }).error, /workspace/);
    }
  } finally {
    cleanup(root, outside);
  }
});

test("preview resolver rejects symlink components and non-regular targets", (t) => {
  const { root, outside } = temporaryWorkspace();
  try {
    try {
      fs.symlinkSync(outside, path.join(root, "outside-link"), "dir");
      fs.symlinkSync(path.join(outside, "secret.txt"), path.join(root, "secret-link"), "file");
    } catch (error) {
      t.skip(`symlink fixture unavailable: ${error.code || error.message}`);
      return;
    }

    assert.match(resolveWorkspacePreviewPath(root, "outside-link/secret.txt").error, /symlink/);
    assert.match(resolveWorkspacePreviewPath(root, "secret-link").error, /symlink/);
    assert.match(resolveWorkspacePreviewPath(root, "src").error, /regular/);
    assert.match(resolveWorkspacePreviewPath(root, "missing.txt").error, /does not exist/);
  } finally {
    cleanup(root, outside);
  }
});

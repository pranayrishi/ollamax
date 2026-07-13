"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { MAX_ATTACHMENT_PREVIEW_BYTES, readAttachmentPreview } = require("../lib/attachment-preview");

test("attachment preview reads only its bounded prefix", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-attachment-preview-"));
  const file = path.join(root, "large.txt");
  fs.writeFileSync(file, "a".repeat(MAX_ATTACHMENT_PREVIEW_BYTES + 32_000), "utf8");
  try {
    const preview = readAttachmentPreview(file);
    assert.equal(preview.length, MAX_ATTACHMENT_PREVIEW_BYTES);
    assert.equal(preview, "a".repeat(MAX_ATTACHMENT_PREVIEW_BYTES));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("attachment preview refuses non-regular files", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ollamax-attachment-preview-"));
  const target = path.join(root, "target.txt");
  const link = path.join(root, "link.txt");
  fs.writeFileSync(target, "private text", "utf8");
  try {
    assert.equal(readAttachmentPreview(root), "");
    try {
      fs.symlinkSync(target, link);
      assert.equal(readAttachmentPreview(link), "");
    } catch (error) {
      if (!error || error.code !== "EPERM") throw error;
    }
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

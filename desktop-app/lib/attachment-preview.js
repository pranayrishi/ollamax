"use strict";

// Bounded attachment reads for the privileged file picker. `readFileSync(...)
// .slice(...)` still allocates the entire source file first; this helper opens
// only a regular file and reads at most the preview budget from its descriptor.

const fs = require("fs");

const MAX_ATTACHMENT_PREVIEW_BYTES = 200_000;

function readAttachmentPreview(filePath, options = {}) {
  const fsImpl = options.fsImpl || fs;
  const maximum = Math.max(1, Math.min(Number(options.maxBytes) || MAX_ATTACHMENT_PREVIEW_BYTES, MAX_ATTACHMENT_PREVIEW_BYTES));
  if (typeof filePath !== "string" || !filePath) return "";
  let stat;
  try {
    stat = fsImpl.lstatSync(filePath);
  } catch (_) {
    return "";
  }
  if (!stat.isFile() || stat.isSymbolicLink() || !Number.isFinite(stat.size) || stat.size < 0) return "";
  const wanted = Math.min(Math.floor(stat.size), maximum);
  if (wanted === 0) return "";
  const noFollow = fsImpl.constants && fsImpl.constants.O_NOFOLLOW ? fsImpl.constants.O_NOFOLLOW : 0;
  let fd = null;
  try {
    fd = fsImpl.openSync(filePath, fsImpl.constants.O_RDONLY | noFollow);
    // Recheck through the opened descriptor to prevent a path swap between the
    // lstat and open. This is a preview only, so a conservative empty result is
    // safer than following an unexpected object.
    if (!fsImpl.fstatSync(fd).isFile()) return "";
    const buffer = Buffer.allocUnsafe(wanted);
    let offset = 0;
    while (offset < wanted) {
      const read = fsImpl.readSync(fd, buffer, offset, wanted - offset, offset);
      if (!read) break;
      offset += read;
    }
    return buffer.subarray(0, offset).toString("utf8");
  } catch (_) {
    return "";
  } finally {
    if (fd !== null) {
      try {
        fsImpl.closeSync(fd);
      } catch (_) {}
    }
  }
}

module.exports = { MAX_ATTACHMENT_PREVIEW_BYTES, readAttachmentPreview };

"use strict";

// Resolve a desktop IDE path without allowing an in-workspace symlink to turn
// the renderer bridge into a read/write capability for files outside the
// opened project. The Rust agent tools have descriptor-relative protection;
// this is the matching guard for the direct Monaco-style IDE bridge.

const fs = require("fs");
const path = require("path");

function isWithinWorkspace(root, target) {
  const relative = path.relative(root, target);
  return (
    relative === "" ||
    (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative))
  );
}

function resolveWorkspacePath(root, value, options = {}) {
  const { allowRoot = false, allowMissingFinal = false, requireRelative = false } = options;
  if (!root || typeof root !== "string") return { error: "no workspace is open" };
  if (typeof value !== "string" || !value.trim() || value.includes("\0")) {
    return { error: "a non-empty path is required" };
  }
  if (
    requireRelative &&
    (path.isAbsolute(value) ||
      path.win32.isAbsolute(value) ||
      path.posix.isAbsolute(value) ||
      /^[a-zA-Z]:/.test(value) ||
      value.split(/[\\/]+/).some((part) => part === ".."))
  ) {
    return { error: "path must stay inside the opened workspace" };
  }

  const target = requireRelative ? path.resolve(root, value) : path.resolve(value);
  if (!isWithinWorkspace(root, target) || (!allowRoot && target === root)) {
    return { error: "path must stay inside the opened workspace" };
  }

  const relative = path.relative(root, target);
  const components = relative ? relative.split(path.sep).filter(Boolean) : [];
  let current = root;
  for (let index = 0; index < components.length; index += 1) {
    current = path.join(current, components[index]);
    try {
      if (fs.lstatSync(current).isSymbolicLink()) {
        return { error: "paths containing symlinks cannot be opened or changed" };
      }
    } catch (error) {
      if (error && error.code === "ENOENT" && allowMissingFinal && index === components.length - 1) {
        return { target, relative: relative.replace(/\\/g, "/") };
      }
      if (error && error.code === "ENOENT") return { error: "path does not exist" };
      return { error: `could not inspect path: ${error.message}` };
    }
  }
  return { target, relative: relative.replace(/\\/g, "/") };
}

module.exports = { isWithinWorkspace, resolveWorkspacePath };

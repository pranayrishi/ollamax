"use strict";

// Proposed-edit previews are a read capability too. Keep them inside the
// canonical workspace and reject every existing symlink component before the
// extension reads a file for VS Code's diff view. The Rust engine remains the
// authoritative, descriptor-relative write boundary; this helper makes the
// UI preview obey the same user-visible workspace scope.

const fs = require("fs");
const path = require("path");

function isWithinWorkspace(root, target) {
  const relative = path.relative(root, target);
  return (
    relative === "" ||
    (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative))
  );
}

function invalidRelativePath(value) {
  return (
    typeof value !== "string" ||
    !value.trim() ||
    value.includes("\0") ||
    path.isAbsolute(value) ||
    path.win32.isAbsolute(value) ||
    path.posix.isAbsolute(value) ||
    /^[a-zA-Z]:/.test(value) ||
    value.split(/[\\/]+/).some((component) => component === "..")
  );
}

/**
 * Resolve a model-proposed workspace-relative file path for an editor preview.
 * Existing symlinks, directories, device files, and traversal are rejected.
 * A write preview may describe a new file below a verified existing ancestor;
 * the actual engine still validates and creates it through its own capability.
 */
function resolveWorkspacePreviewPath(root, value, options = {}) {
  const { allowMissing = false } = options;
  if (!root || typeof root !== "string") return { error: "no workspace is open" };
  if (invalidRelativePath(value)) return { error: "path must stay inside the opened workspace" };

  let canonicalRoot;
  try {
    canonicalRoot = fs.realpathSync(root);
    if (!fs.statSync(canonicalRoot).isDirectory()) {
      return { error: "opened workspace is not a directory" };
    }
  } catch (error) {
    return { error: `could not inspect opened workspace: ${error.message}` };
  }

  const target = path.resolve(canonicalRoot, value);
  if (!isWithinWorkspace(canonicalRoot, target) || target === canonicalRoot) {
    return { error: "path must stay inside the opened workspace" };
  }

  const relative = path.relative(canonicalRoot, target);
  const components = relative.split(path.sep).filter(Boolean);
  let current = canonicalRoot;
  for (let index = 0; index < components.length; index += 1) {
    current = path.join(current, components[index]);
    const isFinal = index === components.length - 1;
    try {
      const metadata = fs.lstatSync(current);
      if (metadata.isSymbolicLink()) {
        return { error: "paths containing symlinks cannot be previewed" };
      }
      if (isFinal && !metadata.isFile()) {
        return { error: "only regular workspace files can be previewed" };
      }
      if (!isFinal && !metadata.isDirectory()) {
        return { error: "path contains a non-directory component" };
      }
    } catch (error) {
      if (error && error.code === "ENOENT" && allowMissing) {
        // All components before this one were verified non-symlinks. A missing
        // suffix is valid for fs_write because the engine creates directories
        // beneath that safe existing ancestor through its own sandbox.
        return { target, relative: relative.replace(/\\/g, "/"), exists: false };
      }
      if (error && error.code === "ENOENT") return { error: "path does not exist" };
      return { error: `could not inspect path: ${error.message}` };
    }
  }

  return { target, relative: relative.replace(/\\/g, "/"), exists: true };
}

module.exports = { isWithinWorkspace, resolveWorkspacePreviewPath };

"use strict";

// Pure desktop-window security policy helpers. Keeping these independent from
// Electron makes the allow-list testable without launching a GUI process. The
// main process supplies the real BrowserWindow/WebContents objects.

const path = require("path");
const { fileURLToPath } = require("url");

function isExactFileUrl(value, expectedPath) {
  if (typeof value !== "string" || !expectedPath) return false;
  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "file:") return false;
    // A fragment or query still names the same on-disk document, but refusing
    // both removes an unnecessary alternate navigation shape from the main
    // renderer's trust boundary.
    if (parsed.search || parsed.hash) return false;
    return path.resolve(fileURLToPath(parsed)) === path.resolve(expectedPath);
  } catch (_) {
    return false;
  }
}

function isTrustedMainWebContents(webContents, mainWindow, expectedRendererPath) {
  if (!webContents || !mainWindow || typeof mainWindow.isDestroyed !== "function") return false;
  if (mainWindow.isDestroyed() || !mainWindow.webContents) return false;
  if (webContents.id !== mainWindow.webContents.id) return false;
  if (typeof webContents.getURL !== "function") return false;
  return isExactFileUrl(webContents.getURL(), expectedRendererPath);
}

function isAudioOnlyPermissionRequest(permission, details) {
  if (permission !== "media" || !details || details.isMainFrame !== true) return false;
  const mediaTypes = details.mediaTypes;
  return Array.isArray(mediaTypes) && mediaTypes.length === 1 && mediaTypes[0] === "audio";
}

function isAudioOnlyPermissionCheck(permission, details) {
  return !!(
    permission === "media" &&
    details &&
    details.isMainFrame === true &&
    details.mediaType === "audio"
  );
}

function isLoopbackHostname(hostname) {
  const host = String(hostname || "").toLowerCase();
  return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]";
}

// URLs opened through the operating system are a capability boundary just as
// much as a BrowserWindow navigation. Permit ordinary HTTPS links and explicit
// loopback HTTP for local development/OAuth only; reject file:, custom schemes,
// embedded credentials, and remote cleartext URLs.
function safeExternalUrl(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > 4_096) return null;
  try {
    const parsed = new URL(value);
    if (parsed.username || parsed.password) return null;
    if (parsed.protocol === "https:") return parsed.toString();
    if (parsed.protocol === "http:" && isLoopbackHostname(parsed.hostname)) return parsed.toString();
  } catch (_) {}
  return null;
}

module.exports = {
  isAudioOnlyPermissionCheck,
  isAudioOnlyPermissionRequest,
  isExactFileUrl,
  isLoopbackHostname,
  safeExternalUrl,
  isTrustedMainWebContents,
};

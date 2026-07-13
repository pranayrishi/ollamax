"use strict";

const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const { pathToFileURL } = require("node:url");

const {
  isAudioOnlyPermissionCheck,
  isAudioOnlyPermissionRequest,
  isExactFileUrl,
  safeExternalUrl,
  isTrustedMainWebContents,
} = require("../lib/desktop-security");

const renderer = path.resolve("/tmp/ollamax-security-test", "renderer", "index.html");

test("only the exact bundled renderer file is a trusted local URL", () => {
  const exact = pathToFileURL(renderer).toString();
  assert.equal(isExactFileUrl(exact, renderer), true);
  assert.equal(isExactFileUrl(`${exact}#fragment`, renderer), false);
  assert.equal(isExactFileUrl("https://example.test/", renderer), false);
  assert.equal(isExactFileUrl(pathToFileURL(path.dirname(renderer)).toString(), renderer), false);
});

test("trust requires the live primary window and exact local document", () => {
  const exact = pathToFileURL(renderer).toString();
  const mainContents = { id: 42, getURL: () => exact };
  const mainWindow = { isDestroyed: () => false, webContents: mainContents };
  assert.equal(isTrustedMainWebContents(mainContents, mainWindow, renderer), true);
  assert.equal(
    isTrustedMainWebContents({ id: 43, getURL: () => exact }, mainWindow, renderer),
    false
  );
  assert.equal(
    isTrustedMainWebContents({ id: 42, getURL: () => "https://example.test/" }, mainWindow, renderer),
    false
  );
  assert.equal(
    isTrustedMainWebContents(mainContents, { isDestroyed: () => true, webContents: mainContents }, renderer),
    false
  );
});

test("permission policy allows only primary-frame microphone media", () => {
  assert.equal(
    isAudioOnlyPermissionRequest("media", { isMainFrame: true, mediaTypes: ["audio"] }),
    true
  );
  assert.equal(
    isAudioOnlyPermissionCheck("media", { isMainFrame: true, mediaType: "audio" }),
    true
  );
  for (const details of [
    { isMainFrame: true, mediaTypes: ["video"] },
    { isMainFrame: true, mediaTypes: ["audio", "video"] },
    { isMainFrame: false, mediaTypes: ["audio"] },
  ]) {
    assert.equal(isAudioOnlyPermissionRequest("media", details), false);
  }
  assert.equal(isAudioOnlyPermissionCheck("media", { isMainFrame: true, mediaType: "video" }), false);
  assert.equal(isAudioOnlyPermissionCheck("display-capture", { isMainFrame: true, mediaType: "audio" }), false);
});

test("external links are limited to HTTPS or explicit loopback HTTP", () => {
  assert.equal(safeExternalUrl("https://github.com/pranayrishi/ollamax"), "https://github.com/pranayrishi/ollamax");
  assert.equal(safeExternalUrl("http://127.0.0.1:7878/callback"), "http://127.0.0.1:7878/callback");
  for (const value of [
    "file:///etc/passwd",
    "mailto:test@example.com",
    "http://example.com/",
    "https://user:secret@example.com/",
    "javascript:alert(1)",
  ]) {
    assert.equal(safeExternalUrl(value), null, value);
  }
});

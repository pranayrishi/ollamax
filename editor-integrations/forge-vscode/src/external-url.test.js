"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { isLiteralLoopbackHostname, safeExternalUrl } = require("./external-url");

test("recognizes only literal loopback hostnames", () => {
  for (const hostname of ["localhost", "LOCALHOST", "127.0.0.1", "::1", "[::1]"]) {
    assert.equal(isLiteralLoopbackHostname(hostname), true, hostname);
  }

  for (const hostname of ["localhost.evil.test", "127.0.0.2", "0.0.0.0", "example.test", ""]) {
    assert.equal(isLiteralLoopbackHostname(hostname), false, hostname);
  }
});

test("allows HTTPS and literal-loopback HTTP without credentials", () => {
  assert.equal(safeExternalUrl("https://ollamax.example/path?next=1"), "https://ollamax.example/path?next=1");
  assert.equal(safeExternalUrl("http://localhost:3000/callback"), "http://localhost:3000/callback");
  assert.equal(safeExternalUrl("http://127.0.0.1:43123/callback"), "http://127.0.0.1:43123/callback");
  assert.equal(safeExternalUrl("http://[::1]:43123/callback"), "http://[::1]:43123/callback");
});

test("rejects unsafe browser targets", () => {
  for (const candidate of [
    "javascript:alert(1)",
    "data:text/html,hello",
    "file:///tmp/secret",
    "command:workbench.action.openSettings",
    "http://example.test/callback",
    "http://localhost.evil.test/callback",
    "https://user:password@ollamax.example/",
    "http://user:password@127.0.0.1/",
    "",
    "x".repeat(4_097),
  ]) {
    assert.equal(safeExternalUrl(candidate), null, candidate.slice(0, 80));
  }
});

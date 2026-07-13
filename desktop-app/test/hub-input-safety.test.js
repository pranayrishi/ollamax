"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

function hubHelpers(file) {
  const source = fs.readFileSync(file, "utf8");
  const count = source.match(/function boundedCount\(value, max\) \{\n([\s\S]*?)\n  \}/);
  const items = source.match(/function items\(value\) \{\n([\s\S]*?)\n  \}/);
  assert.ok(count, `missing boundedCount in ${file}`);
  assert.ok(items, `missing items in ${file}`);
  return new Function(
    `${count[0]}\n${items[0]}\nreturn { boundedCount, items };` // eslint-disable-line no-new-func
  )();
}

test("Hub renderer normalizes untrusted counts and array-like values before templates", () => {
  const root = path.resolve(__dirname, "..", "..");
  const files = [
    path.join(root, "desktop-app", "renderer", "hub.js"),
    path.join(root, "editor-integrations", "forge-vscode", "media", "hub.js"),
  ];
  for (const file of files) {
    const { boundedCount, items } = hubHelpers(file);
    assert.equal(boundedCount('</li><img src="https://example.test/x">', 10_000), 0);
    assert.equal(boundedCount(Number.NaN, 10_000), 0);
    assert.equal(boundedCount(12_345, 10_000), 10_000);
    assert.deepEqual(items({ length: '</div><img src="https://example.test/x">' }), []);
    assert.equal(items(["one", "two"]).length, 2);
  }
});

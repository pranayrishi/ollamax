"use strict";

// The chat renderer deliberately uses a very small Markdown renderer. Its
// output reaches innerHTML, so quoted fenced-code labels must be escaped just
// as carefully as angle brackets. Exercise the actual helper text from both
// shipped renderer copies; this catches a future one-sided security regression
// without needing to launch Electron or VS Code.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

function escapeHelperFrom(file) {
  const source = fs.readFileSync(file, "utf8");
  const match = source.match(/function escapeHtml\(s\) \{\n([\s\S]*?)\n  \}/);
  assert.ok(match, `missing escapeHtml helper in ${file}`);
  // The extracted helper is repository-owned source and receives only the
  // fixture below. This keeps the test coupled to the implementation that is
  // actually sent to the privileged renderer.
  return new Function("s", match[1]); // eslint-disable-line no-new-func
}

test("model output escapes attribute delimiters in desktop and VS Code chat renderers", () => {
  const root = path.resolve(__dirname, "..", "..");
  const rendererFiles = [
    path.join(root, "desktop-app", "renderer", "main.js"),
    path.join(root, "editor-integrations", "forge-vscode", "media", "main.js"),
  ];
  for (const file of rendererFiles) {
    const escapeHtml = escapeHelperFrom(file);
    assert.equal(
      escapeHtml('lang" onmouseover="bad\' <tag>&'),
      "lang&quot; onmouseover=&quot;bad&#39; &lt;tag&gt;&amp;"
    );
  }
});

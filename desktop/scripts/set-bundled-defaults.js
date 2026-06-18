#!/usr/bin/env node
// @ts-check
"use strict";
//
// desktop/scripts/set-bundled-defaults.js
//
// Bake in-box default settings into the bundled forge-vscode extension via
// `contributes.configurationDefaults` — the SUPPORTED VS Code mechanism for a
// fork to ship default settings. (product.json has NO `defaultSettingsOverrides`
// key; it is silently ignored, which is why the gate must be defaulted here.)
//
// Idempotent: re-running just rewrites the same keys.
//
//   node set-bundled-defaults.js <path/to/package.json> [accountServerUrl]
//
// - forge.serverPath = ""  → backend.js then prefers the bundled <ext>/bin/forge
// - forge.accountServer = <url> (if given) → enables the login gate by default
//   (NOTE: a user can override this in Settings, so the gate is a UX gate, not a
//   hard wall — enforce server-side if it must be non-bypassable).

const fs = require("fs");

const [, , pkgPath, accountServer] = process.argv;
if (!pkgPath) {
  console.error("usage: set-bundled-defaults.js <package.json> [accountServerUrl]");
  process.exit(1);
}

const pkg = JSON.parse(fs.readFileSync(pkgPath, "utf8"));
pkg.contributes = pkg.contributes || {};
const defaults = pkg.contributes.configurationDefaults || {};

// Prefer the engine bundled inside the extension (backend.js resolves it when
// serverPath is unset/default).
defaults["forge.serverPath"] = "";

if (accountServer && String(accountServer).trim()) {
  defaults["forge.accountServer"] = String(accountServer).trim();
} else {
  // No server provided → leave the gate OFF (don't ship a broken sign-in wall).
  delete defaults["forge.accountServer"];
}

pkg.contributes.configurationDefaults = defaults;
fs.writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");
console.log(
  `set configurationDefaults: serverPath="" (bundled engine)` +
    (defaults["forge.accountServer"]
      ? `, accountServer="${defaults["forge.accountServer"]}" (gate ON)`
      : `, accountServer unset (gate OFF)`)
);

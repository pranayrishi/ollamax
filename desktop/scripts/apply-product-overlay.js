#!/usr/bin/env node
// @ts-check
"use strict";
//
// desktop/scripts/apply-product-overlay.js
//
// Merge the rebrand overlay (desktop/product.json.example) into a Code-OSS
// checkout's product.json. MERGES keys (does not overwrite the whole file) and
// skips __-prefixed comment keys. Idempotent.
//
//   node apply-product-overlay.js <fork>/product.json desktop/product.json.example

const fs = require("fs");

const [, , basePath, overlayPath] = process.argv;
if (!basePath || !overlayPath) {
  console.error("usage: apply-product-overlay.js <product.json> <overlay.json>");
  process.exit(1);
}

const base = JSON.parse(fs.readFileSync(basePath, "utf8"));
const overlay = JSON.parse(fs.readFileSync(overlayPath, "utf8"));
let merged = 0;
for (const k of Object.keys(overlay)) {
  if (k.startsWith("__")) continue; // comment key
  base[k] = overlay[k];
  merged++;
}
fs.writeFileSync(basePath, JSON.stringify(base, null, 2) + "\n");
console.log(`merged ${merged} rebrand key(s) into ${basePath}`);

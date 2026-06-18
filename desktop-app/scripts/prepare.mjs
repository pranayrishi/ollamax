// Copies the EXISTING panel UI (single source of truth in the extension's
// media/) into renderer/, and stages the forge engine binary into bin/ for
// packaging. Run by `npm start` / `npm run dist`. We reuse the UI rather than
// forking it, so the app and the (now-retired) panel never drift.
import { existsSync, mkdirSync, copyFileSync, chmodSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");
const media = resolve(root, "editor-integrations", "forge-vscode", "media");
const renderer = resolve(here, "..", "renderer");
const bin = resolve(here, "..", "bin");

mkdirSync(renderer, { recursive: true });
mkdirSync(bin, { recursive: true });

// Reused UI files (committed source lives in the extension).
for (const f of ["main.js", "main.css", "hub.js", "hub.css"]) {
  const src = resolve(media, f);
  if (existsSync(src)) {
    copyFileSync(src, resolve(renderer, f));
    console.log(`copied UI: ${f}`);
  } else {
    console.warn(`WARN: missing ${src}`);
  }
}

// Stage the engine binary (dev: repo release build). CI overrides this by
// dropping the platform-native binary into bin/ before packaging.
const exe = process.platform === "win32" ? "forge.exe" : "forge";
const engine = resolve(root, "target", "release", exe);
if (existsSync(engine)) {
  copyFileSync(engine, resolve(bin, exe));
  try {
    chmodSync(resolve(bin, exe), 0o755);
  } catch (_) {}
  console.log(`staged engine: ${exe}`);
} else {
  console.warn(
    `WARN: engine not found at ${engine} — run \`cargo build --release\` first (CI does this).`
  );
}

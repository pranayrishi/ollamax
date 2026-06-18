// Copies the EXISTING panel UI (single source of truth in the extension's
// media/) into renderer/, and stages the forge engine binary into bin/ for
// packaging. Run by `npm start` / `npm run dist`. We reuse the UI rather than
// forking it, so the app and the (now-retired) panel never drift.
import { existsSync, mkdirSync, copyFileSync, chmodSync, readdirSync } from "node:fs";
import { dirname, resolve, basename } from "node:path";
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

// Stage the IDE editor/terminal assets (#3) from node_modules when installed
// (the dev/CI flow runs `npm install` before this). Best-effort — ide.js falls
// back gracefully (textarea editor / "terminal needs install" note) when absent.
const nm = resolve(here, "..", "node_modules");
function copyDir(src, dst) {
  if (!existsSync(src)) return false;
  mkdirSync(dst, { recursive: true });
  for (const e of readdirSync(src, { withFileTypes: true })) {
    const s = resolve(src, e.name);
    const d = resolve(dst, e.name);
    if (e.isDirectory()) copyDir(s, d);
    else copyFileSync(s, d);
  }
  return true;
}
if (copyDir(resolve(nm, "monaco-editor", "min", "vs"), resolve(renderer, "vs"))) {
  console.log("staged Monaco editor (vs/)");
} else {
  console.warn("Monaco not installed — editor uses the textarea fallback (run npm install)");
}
for (const from of [
  resolve(nm, "@xterm/xterm/css/xterm.css"),
  resolve(nm, "@xterm/xterm/lib/xterm.js"),
  resolve(nm, "@xterm/addon-fit/lib/addon-fit.js"),
]) {
  if (existsSync(from)) {
    copyFileSync(from, resolve(renderer, basename(from)));
    console.log(`staged ${basename(from)}`);
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

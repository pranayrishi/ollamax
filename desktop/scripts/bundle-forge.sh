#!/usr/bin/env bash
#
# desktop/scripts/bundle-forge.sh
#
# Stage two things into a Code-OSS fork so the desktop app ships ready to use:
#   1. the `forge` Rust binary (which provides `forge serve`), placed INSIDE the
#      extension at extensions/forge-vscode/bin/ so it travels with the built-in
#      regardless of app layout, and
#   2. the chat extension (editor-integrations/forge-vscode) dropped into the
#      fork's extensions/ dir — Code-OSS auto-discovers source-tree built-ins via
#      glob('extensions/*/package.json'), so NO product.json entry is needed (and
#      listing it in builtInExtensions would wrongly trigger a marketplace download).
#
# Zero-config: backend.js resolves <ext>/bin/forge when forge.serverPath is the
# default, and set-bundled-defaults.js bakes contributes.configurationDefaults.
#
# Gated like bootstrap.sh — pass RUN_REAL=1 (bootstrap.sh does this for you).
#
set -euo pipefail

FORK_DIR="${1:-./code-oss}"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
EXT_SRC="${REPO_ROOT}/editor-integrations/forge-vscode"
DEST="${FORK_DIR}/extensions/forge-vscode"

if [[ "${RUN_REAL:-0}" != "1" ]]; then
  echo "STATUS: gated. Re-run with RUN_REAL=1 to stage forge + the extension into ${FORK_DIR}."
  echo "  - builds the release binary, copies it to ${DEST}/bin/"
  echo "  - copies the extension into extensions/forge-vscode (auto-discovered built-in)"
  echo "  - injects contributes.configurationDefaults (bundled engine + gate)"
  exit 0
fi

# Platform-correct binary name.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) BIN="forge.exe" ;;
  *)                    BIN="forge" ;;
esac

# 1. Build the release binary.
( cd "${REPO_ROOT}" && cargo build --release )

# 2. Copy the extension into the fork (rsync, excluding dev noise), then drop the
#    binary inside it so a built-in carries its own engine.
mkdir -p "${DEST}"
if command -v rsync >/dev/null; then
  rsync -a --delete \
    --exclude '.git' --exclude '*.map' --exclude '.DS_Store' --exclude 'node_modules' \
    "${EXT_SRC}/" "${DEST}/"
else
  cp -R "${EXT_SRC}/." "${DEST}/"
fi
mkdir -p "${DEST}/bin"
cp "${REPO_ROOT}/target/release/${BIN}" "${DEST}/bin/${BIN}"
chmod +x "${DEST}/bin/${BIN}"

# 2b. Bundle whisper.cpp + a ggml model so Voice navigation works zero-config.
#     Built/staged ahead of time under desktop/vendor/whisper (gitignored, large).
#     voice.js (_resolveWhisper) prefers <ext>/bin/whisper-cli + ggml-base.en.bin
#     when forge.whisperPath/whisperModel are unset. All on-device; audio never
#     leaves the machine. Skipped gracefully if the vendor dir isn't present.
WHISPER_VENDOR="${REPO_ROOT}/desktop/vendor/whisper"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) WBIN="whisper-cli.exe" ;;
  *)                    WBIN="whisper-cli" ;;
esac
if [[ -f "${WHISPER_VENDOR}/${WBIN}" && -f "${WHISPER_VENDOR}/ggml-base.en.bin" ]]; then
  cp "${WHISPER_VENDOR}/${WBIN}" "${DEST}/bin/${WBIN}"
  chmod +x "${DEST}/bin/${WBIN}"
  cp "${WHISPER_VENDOR}/ggml-base.en.bin" "${DEST}/bin/ggml-base.en.bin"
  echo "  voice  : bundled ${WBIN} + ggml-base.en.bin (zero-config STT)"
else
  echo "  voice  : whisper vendor assets missing at ${WHISPER_VENDOR} — Voice will prompt for setup"
fi

# 3. Bake in-box defaults via contributes.configurationDefaults (the SUPPORTED
#    mechanism — product.json has no defaultSettingsOverrides key). forge.serverPath
#    stays "" so backend.js prefers the bundled bin; set FORGE_ACCOUNT_SERVER to a
#    deployed URL to enable the login gate by default.
node "${REPO_ROOT}/desktop/scripts/set-bundled-defaults.js" \
  "${DEST}/package.json" "${FORGE_ACCOUNT_SERVER:-}"

echo "Staged: forge engine + chat panel as a built-in under ${DEST}"
echo "  binary : ${DEST}/bin/${BIN}"
echo "  gate   : forge.accountServer default = '${FORGE_ACCOUNT_SERVER:-<unset; gate off>}'"

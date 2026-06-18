#!/usr/bin/env bash
#
# desktop/scripts/bootstrap.sh — SCAFFOLD (not yet wired to run end-to-end).
#
# Documents the concrete steps to stand up a Code-OSS fork that ships the
# Ollama-Forge chat panel as a built-in. This script intentionally STOPS and
# prints guidance rather than performing a multi-GB clone + build, because
# Phase 3 (fork/rebrand/package) is scoped as "scaffold + document" this round.
#
# Run order once we execute Phase 3 for real:
#   1. ./bootstrap.sh            # clone Code-OSS, pin a release tag
#   2. apply product.json overlay (desktop/product.json.example)
#   3. ./bundle-forge.sh         # stage the forge binary + extension as built-ins
#   4. yarn && yarn gulp vscode-<platform>   # build the branded app
#
set -euo pipefail

FORK_DIR="${FORK_DIR:-$(cd "$(dirname "$0")/.." && pwd)/code-oss}"
# Pin a known-good Code-OSS release tag rather than tracking main.
VSCODE_TAG="${VSCODE_TAG:-1.95.0}"

echo "ForgeCode desktop bootstrap (SCAFFOLD)"
echo "  fork dir : ${FORK_DIR}"
echo "  vscode   : tag ${VSCODE_TAG}"
echo

cat <<'NOTE'
STATUS: scaffold. This script does not yet perform the clone/build. To execute
Phase 3 for real, remove the `exit 0` below and ensure you have:
  - git, Node.js (matching .nvmrc in the vscode checkout), yarn
  - ~10 GB free disk, a working native toolchain (Xcode CLT / build-essential / MSVC)
NOTE
exit 0

# --- everything below is the documented-but-gated real procedure ---

# 1. Clone Code-OSS at a pinned tag.
if [[ ! -d "${FORK_DIR}" ]]; then
  git clone --depth 1 --branch "${VSCODE_TAG}" https://github.com/microsoft/vscode.git "${FORK_DIR}"
fi

# 2. Apply the rebrand overlay. (A real impl merges keys rather than copying;
#    shown here as the intent.)
#    node desktop/scripts/apply-product-overlay.js "${FORK_DIR}/product.json" desktop/product.json.example

# 3. Stage the forge binary + chat extension as built-ins.
#    ./desktop/scripts/bundle-forge.sh "${FORK_DIR}"

# 4. Install deps and build the branded desktop app for this platform.
( cd "${FORK_DIR}" && yarn )
# macOS:   ( cd "${FORK_DIR}" && yarn gulp vscode-darwin-arm64 )
# Linux:   ( cd "${FORK_DIR}" && yarn gulp vscode-linux-x64 )
# Windows: ( cd "${FORK_DIR}" && yarn gulp vscode-win32-x64 )

echo "Done. The built app is a sibling of ${FORK_DIR} (e.g. VSCode-darwin-arm64)."

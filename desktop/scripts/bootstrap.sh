#!/usr/bin/env bash
#
# desktop/scripts/bootstrap.sh
#
# Stand up a Code-OSS fork that ships the Ollamax chat panel (forge-vscode)
# as a built-in plus the bundled `forge` engine. Every command below was checked
# against microsoft/vscode source + VSCodium (see VSCODE_REPLATFORM_REPORT).
#
# This performs a multi-GB clone + build, so it is GATED: it prints the plan and
# stops unless you opt in with RUN_REAL=1. Run on a build machine with the
# prerequisites below — NOT in CI without ~8 GB RAM / ~15 GB disk.
#
#   RUN_REAL=1 ./bootstrap.sh
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FORK_DIR="${FORK_DIR:-$(cd "$(dirname "$0")/.." && pwd)/code-oss}"
# Pin a real, verified Code-OSS release tag (>= 1.94 so the toolchain is npm, and
# >= 1.85 so it satisfies the extension's engines.vscode "^1.85.0").
VSCODE_TAG="${VSCODE_TAG:-1.95.3}"

echo "Ollamax desktop bootstrap"
echo "  repo root : ${REPO_ROOT}"
echo "  fork dir  : ${FORK_DIR}"
echo "  vscode    : tag ${VSCODE_TAG}"
echo

if [[ "${RUN_REAL:-0}" != "1" ]]; then
  cat <<NOTE
STATUS: gated. This does a real clone + build. Re-run with RUN_REAL=1 once you have:
  - git, a C/C++ toolchain (Xcode CLT / build-essential / MSVC 2022 Build Tools)
  - Node matching the checkout's .nvmrc (tag ${VSCODE_TAG} -> 20.18.0), via fnm/nvm
  - python3 + setuptools (node-gyp), and ~8 GB RAM, ~15 GB free disk
What it will do: verify the tag, full-clone Code-OSS on a forge/ branch, npm ci,
apply the product.json rebrand, stage forge engine + extension as a built-in,
then build the per-OS app via 'npm run gulp vscode-<platform>-<arch>-min'.
NOTE
  exit 0
fi

# 0. Verify the pinned tag actually exists (never build on a typo'd ref).
echo "Verifying tag ${VSCODE_TAG} exists upstream…"
git ls-remote --tags --exit-code https://github.com/microsoft/vscode.git \
  "refs/tags/${VSCODE_TAG}" >/dev/null \
  || { echo "ERROR: vscode tag ${VSCODE_TAG} not found"; exit 1; }

# 1. FULL clone (not --depth 1) so the fork can be rebased onto future tags, then
#    check out the tag on a working branch.
if [[ ! -d "${FORK_DIR}/.git" ]]; then
  git clone https://github.com/microsoft/vscode.git "${FORK_DIR}"
fi
git -C "${FORK_DIR}" fetch --tags origin
git -C "${FORK_DIR}" checkout "tags/${VSCODE_TAG}" -b "forge/${VSCODE_TAG}" 2>/dev/null \
  || git -C "${FORK_DIR}" checkout "forge/${VSCODE_TAG}"

# 2. Select the Node version the checkout pins, and prep node-gyp's Python.
if [[ -f "${FORK_DIR}/.nvmrc" ]]; then
  NODE_WANT="$(cat "${FORK_DIR}/.nvmrc")"
  echo "Checkout wants Node ${NODE_WANT} (.nvmrc). Current: $(node -v 2>/dev/null || echo none)"
  command -v fnm >/dev/null && fnm use "${NODE_WANT}" 2>/dev/null || true
fi
python3 -m pip install --quiet setuptools 2>/dev/null || true

# 3. Install deps with npm (VS Code migrated yarn -> npm in 1.94). Skip the heavy
#    binary downloads not needed at install time; gulp fetches Electron later.
export ELECTRON_SKIP_BINARY_DOWNLOAD=1
export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
( cd "${FORK_DIR}" && npm ci )

# 4. Apply the rebrand overlay (MERGE keys, skip __comments) into product.json.
node "${REPO_ROOT}/desktop/scripts/apply-product-overlay.js" \
  "${FORK_DIR}/product.json" "${REPO_ROOT}/desktop/product.json.example"

# 5. Stage the forge engine + chat extension as a built-in (own script).
RUN_REAL=1 bash "${REPO_ROOT}/desktop/scripts/bundle-forge.sh" "${FORK_DIR}"

# 5b. Rasterize media/forge.svg -> resources/{darwin,win32,linux} icons.
RUN_REAL=1 bash "${REPO_ROOT}/desktop/scripts/make-icons.sh" "${FORK_DIR}" || \
  echo "WARN: icon generation skipped/failed (needs ImageMagick + iconutil); continuing"

# 6. Build the per-OS app. Run gulp via npm so it inherits the 8 GB heap.
case "$(uname -s)" in
  Darwin) TARGET="vscode-darwin-$([[ "$(uname -m)" == arm64 ]] && echo arm64 || echo x64)-min" ;;
  Linux)  TARGET="vscode-linux-x64-min" ;;
  *)      TARGET="vscode-win32-x64-min" ;;
esac
echo "Building ${TARGET} …"
( cd "${FORK_DIR}" && npm run gulp "${TARGET}" )

# CRITICAL: ensure the forge engine is INSIDE the BUILT app's bundled extension.
# gulp's built-in packaging strips dirs ignored by .vscodeignore (and bin/ isn't a
# standard extension dir), so the staged engine can vanish from the final app —
# which leaves a VS Code app that can't start `forge serve`. Copy it into the
# final tree directly. This is the load-bearing zero-config bit (audit finding).
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) BIN="forge.exe" ;;
  *) BIN="forge" ;;
esac
OUT_PARENT="$(cd "${FORK_DIR}/.." && pwd)"
APP_EXT="$(find "${OUT_PARENT}" -maxdepth 8 -type d -path '*/extensions/forge-vscode' 2>/dev/null | head -1)"
if [[ -n "${APP_EXT}" && -f "${REPO_ROOT}/target/release/${BIN}" ]]; then
  mkdir -p "${APP_EXT}/bin"
  cp "${REPO_ROOT}/target/release/${BIN}" "${APP_EXT}/bin/${BIN}"
  chmod +x "${APP_EXT}/bin/${BIN}" || true
  echo "post-build: staged forge engine into ${APP_EXT}/bin/${BIN}"
else
  echo "WARN: could not stage engine post-build (APP_EXT='${APP_EXT}')"
fi

echo "Done. The built app tree is a SIBLING of ${FORK_DIR} (e.g. ../VSCode-darwin-arm64)."
echo "Wrap it into a .dmg/.exe/.deb per OS (see desktop/scripts/sign-*.sh and README)."

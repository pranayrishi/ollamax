#!/usr/bin/env bash
#
# desktop/scripts/bundle-forge.sh — SCAFFOLD.
#
# Stages two things into a Code-OSS fork so the desktop app ships ready to use:
#   1. the `forge` Rust binary (which provides `forge serve`), and
#   2. the Phase 2 chat extension (editor-integrations/forge-vscode) as a
#      *built-in* extension, so the chat panel is present out of the box.
#
# The extension's `forge.serverPath` default ("forge") is overridden at bundle
# time to point at the binary shipped inside the app's resources, so the user
# never has to configure a path.
#
set -euo pipefail

FORK_DIR="${1:-./code-oss}"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

echo "Staging forge backend + chat extension into ${FORK_DIR} (SCAFFOLD)"
cat <<'NOTE'
STATUS: scaffold. The steps below are the intended procedure; the script exits
before mutating anything. Wire these up when executing Phase 3 for real.
NOTE
exit 0

# 1. Build the release binary.
( cd "${REPO_ROOT}" && cargo build --release )

# 2. Copy it into the app resources. The exact path differs per platform; for a
#    built app it lands under Contents/Resources/app/bin (darwin) or resources/
#    app/bin (linux/win). For a dev build, stage under the fork's resources.
mkdir -p "${FORK_DIR}/resources/app/bin"
cp "${REPO_ROOT}/target/release/forge" "${FORK_DIR}/resources/app/bin/forge"

# 3. Copy the extension in as a built-in.
DEST="${FORK_DIR}/extensions/forge-vscode"
mkdir -p "${DEST}"
cp -r "${REPO_ROOT}/editor-integrations/forge-vscode/." "${DEST}/"

# 4. Point the bundled extension at the bundled binary so no user config is
#    needed. (A real impl edits package.json's default or injects a setting.)
#    node desktop/scripts/set-bundled-server-path.js "${DEST}/package.json"

echo "Staged. The fork now contains forge + the chat panel as a built-in."

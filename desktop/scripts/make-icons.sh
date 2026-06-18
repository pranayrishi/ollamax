#!/usr/bin/env bash
#
# desktop/scripts/make-icons.sh — rasterize the anvil glyph (forge.svg) into the
# platform icons a Code-OSS build expects under resources/{darwin,win32,linux},
# replacing Microsoft's proprietary marks. GATED behind RUN_REAL=1.
#
# Requires: ImageMagick (`magick`/`convert`) + librsvg, and on macOS `iconutil`.
# Windows .ico generation also uses ImageMagick. Run on macOS for the .icns.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FORK_DIR="${1:-$(cd "$(dirname "$0")/.." && pwd)/code-oss}"
SRC="${REPO_ROOT}/editor-integrations/forge-vscode/media/forge.svg"

if [[ "${RUN_REAL:-0}" != "1" ]]; then
  echo "STATUS: gated. Re-run with RUN_REAL=1 (needs ImageMagick + iconutil)."
  echo "  source : ${SRC}"
  echo "  targets: ${FORK_DIR}/resources/{darwin/code.icns, win32/code.ico, linux/code.png}"
  exit 0
fi

command -v magick >/dev/null || command -v convert >/dev/null || {
  echo "ERROR: ImageMagick not found (brew install imagemagick librsvg)"; exit 1; }
IM="$(command -v magick || command -v convert)"
[[ -f "$SRC" ]] || { echo "ERROR: source glyph not found: $SRC"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# 1. Linux PNG (512px) + the iconset PNGs.
mkdir -p "${FORK_DIR}/resources/linux" "${FORK_DIR}/resources/darwin" "${FORK_DIR}/resources/win32"
"$IM" -background none "$SRC" -resize 512x512 "${FORK_DIR}/resources/linux/code.png"

# 2. macOS .icns via iconutil (needs the standard iconset sizes).
if command -v iconutil >/dev/null; then
  ICONSET="${WORK}/code.iconset"; mkdir -p "$ICONSET"
  for s in 16 32 64 128 256 512; do
    "$IM" -background none "$SRC" -resize ${s}x${s}   "${ICONSET}/icon_${s}x${s}.png"
    "$IM" -background none "$SRC" -resize $((s*2))x$((s*2)) "${ICONSET}/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns "$ICONSET" -o "${FORK_DIR}/resources/darwin/code.icns"
else
  echo "WARN: iconutil not present (run on macOS) — skipped code.icns"
fi

# 3. Windows .ico (multi-size).
"$IM" -background none "$SRC" -define icon:auto-resize=16,24,32,48,64,128,256 \
  "${FORK_DIR}/resources/win32/code.ico"

echo "icons written under ${FORK_DIR}/resources/{darwin,win32,linux}"

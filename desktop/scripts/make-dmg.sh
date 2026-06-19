#!/usr/bin/env bash
#
# make-dmg.sh <App.app> <out.dmg>
#
# Build a macOS .dmg whose Finder window shows a branded background with the
# drag-to-Applications layout AND the one-time first-launch instructions for the
# unsigned app (right-click → Open). The background art is desktop/scripts/dmg-bg.png
# (rendered from dmg-bg.svg). Falls back to a plain .dmg if Finder window-scripting
# is unavailable (e.g. no GUI/Automation permission), so a build never fails here.
#
set -euo pipefail

APP="${1:?usage: make-dmg.sh <App.app> <out.dmg>}"
OUT="${2:?usage: make-dmg.sh <App.app> <out.dmg>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BG="${HERE}/dmg-bg.png"
VOL="Ollamax"
APPNAME="$(basename "$APP")"

STAGING="$(mktemp -d)"
RW="$(mktemp -u).dmg"
trap 'rm -rf "$STAGING" "$RW" 2>/dev/null || true' EXIT

cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
mkdir "$STAGING/.background"
[ -f "$BG" ] && cp "$BG" "$STAGING/.background/dmg-bg.png"

SIZE_MB=$(( $(du -sm "$STAGING" | cut -f1) + 100 ))
hdiutil create -srcfolder "$STAGING" -volname "$VOL" -fs HFS+ -format UDRW -size "${SIZE_MB}m" "$RW" -ov >/dev/null

hdiutil detach "/Volumes/$VOL" >/dev/null 2>&1 || true
DEV="$(hdiutil attach -readwrite -noverify -noautoopen "$RW" | grep -E '^/dev/' | head -1 | awk '{print $1}')"
sleep 2

LAYOUT_OK=0
if [ -f "$BG" ]; then
  if osascript >/dev/null 2>&1 <<EOF
tell application "Finder"
  tell disk "$VOL"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, 840, 540}
    set vo to the icon view options of container window
    set arrangement of vo to not arranged
    set icon size of vo to 104
    set background picture of vo to file ".background:dmg-bg.png"
    set position of item "$APPNAME" of container window to {170, 205}
    set position of item "Applications" of container window to {470, 205}
    update without registering applications
    delay 1
    close
  end tell
end tell
EOF
  then
    [ -f "/Volumes/$VOL/.DS_Store" ] && LAYOUT_OK=1
  fi
fi

sync
hdiutil detach "$DEV" >/dev/null 2>&1 || hdiutil detach "$DEV" -force >/dev/null 2>&1 || true

rm -f "$OUT"
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$OUT" >/dev/null

if [ "$LAYOUT_OK" = "1" ]; then
  echo "OK: $OUT (custom window background + first-launch layout applied)"
else
  echo "OK: $OUT (plain layout — Finder window-scripting unavailable; background not applied)"
fi

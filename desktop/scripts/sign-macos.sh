#!/usr/bin/env bash
#
# desktop/scripts/sign-macos.sh — Developer ID sign + notarize + staple a Code-OSS
# app, then build a .dmg. GATED behind RUN_REAL=1 (needs an .app from the fork
# build + Apple Developer ID certs). From CI secrets:
#   APPLE_CERT_P12_BASE64, APPLE_CERT_PASSWORD, APPLE_TEAM_ID,
#   APPLE_NOTARY_APPLE_ID, APPLE_NOTARY_PASSWORD
#
# NOTE: signing/notarization is UNVALIDATED here (no certs, no built .app). The
# commands below are corrected per review (proper identity capture, inside-out
# signing, hardened-runtime entitlements, no --deep) but must be validated on a
# real build machine.
#
set -euo pipefail

# Derive the .app from the actual gulp output (sibling of the checkout), arch-aware.
ARCH="$([[ "$(uname -m)" == arm64 ]] && echo arm64 || echo x64)"
APP="${APP_PATH:-$(cd "$(dirname "$0")/.." && pwd)/../VSCode-darwin-${ARCH}/Ollamax.app}"
DMG="${DMG_PATH:-dist/Ollamax-macos-${ARCH}.dmg}"
ENTITLEMENTS="$(cd "$(dirname "$0")/.." && pwd)/assets/entitlements.mac.plist"

if [[ "${RUN_REAL:-0}" != "1" ]]; then
  echo "STATUS: gated. Build the fork app first, then re-run with RUN_REAL=1 + Apple certs."
  echo "  app: ${APP}"
  exit 0
fi
[[ -d "$APP" ]] || { echo "ERROR: app not found at $APP (build the fork first)"; exit 1; }
mkdir -p dist

# 1. Import the Developer ID cert into a temporary keychain.
echo "$APPLE_CERT_P12_BASE64" | base64 -d > /tmp/dev.p12
security create-keychain -p "" build.keychain
security import /tmp/dev.p12 -k build.keychain -P "$APPLE_CERT_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "" build.keychain
rm -f /tmp/dev.p12

# 2. Resolve the FULL signing identity (e.g. "Developer ID Application: Name (TEAMID)")
#    from the keychain — NOT a hand-built "Developer ID Application: <teamid>" string.
IDENTITY="$(security find-identity -v -p codesigning build.keychain \
  | awk -F'"' '/Developer ID Application/{print $2; exit}')"
[[ -n "$IDENTITY" ]] || { echo "ERROR: no Developer ID Application identity in keychain"; exit 1; }
echo "Signing as: $IDENTITY"

sign() { codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$1"; }

# 3. INSIDE-OUT sign (Apple discourages --deep for notarized Electron bundles):
#    sign the bundled engine + every nested Mach-O / helper / framework deepest
#    path first, then the outer .app LAST.
if [[ -x "$APP/Contents/Resources/app/extensions/forge-vscode/bin/forge" ]]; then
  sign "$APP/Contents/Resources/app/extensions/forge-vscode/bin/forge"
fi
# Nested binaries (dylibs, .node, helper executables), deepest first:
while IFS= read -r -d '' f; do sign "$f"; done < <(
  find "$APP/Contents" \( -name '*.dylib' -o -name '*.node' \) -type f -print0 | sort -rz)
# Helper .app bundles and frameworks, then the main app:
while IFS= read -r -d '' b; do sign "$b"; done < <(
  find "$APP/Contents" \( -name '*.app' -o -name '*.framework' \) -print0 | sort -rz)
sign "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# 4. Notarize (Apple staples a ticket so Gatekeeper trusts it offline).
ditto -c -k --keepParent "$APP" /tmp/forge.zip
xcrun notarytool submit /tmp/forge.zip \
  --apple-id "$APPLE_NOTARY_APPLE_ID" --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_NOTARY_PASSWORD" --wait
xcrun stapler staple "$APP"

# 5. Build the .dmg.
hdiutil create -volname "Ollamax" -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "signed + notarized: $DMG"

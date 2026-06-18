#!/usr/bin/env bash
#
# desktop/scripts/sign-macos.sh — Developer ID sign + notarize + staple the
# macOS app, then build a .dmg. SCAFFOLD: the real commands are below; it exits
# early until the fork build produces an .app. Needs (from CI secrets):
#   APPLE_CERT_P12_BASE64, APPLE_CERT_PASSWORD, APPLE_TEAM_ID,
#   APPLE_NOTARY_APPLE_ID, APPLE_NOTARY_PASSWORD
set -euo pipefail

APP="${APP_PATH:-desktop/code-oss/../VSCode-darwin-arm64/ForgeCode.app}"
DMG="dist/ForgeCode-universal.dmg"

cat <<'NOTE'
STATUS: scaffold. Wire desktop/scripts/bootstrap.sh + bundle-forge.sh to produce
the .app first, then remove the `exit 0` below.
NOTE
exit 0

mkdir -p dist

# 1. Import the Developer ID cert into a temporary keychain.
echo "$APPLE_CERT_P12_BASE64" | base64 -d > /tmp/dev.p12
security create-keychain -p "" build.keychain
security import /tmp/dev.p12 -k build.keychain -P "$APPLE_CERT_PASSWORD" -T /usr/bin/codesign
security list-keychains -s build.keychain
security set-key-partition-list -S apple-tool:,apple: -s -k "" build.keychain
rm -f /tmp/dev.p12

# 2. Deep-sign the app with hardened runtime (required for notarization).
codesign --deep --force --options runtime --timestamp \
  --sign "Developer ID Application: $APPLE_TEAM_ID" "$APP"

# 3. Notarize (Apple staples a ticket so Gatekeeper trusts it offline).
ditto -c -k --keepParent "$APP" /tmp/forge.zip
xcrun notarytool submit /tmp/forge.zip \
  --apple-id "$APPLE_NOTARY_APPLE_ID" --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_NOTARY_PASSWORD" --wait
xcrun stapler staple "$APP"

# 4. Build the .dmg.
hdiutil create -volname "ForgeCode" -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "✅ signed + notarized: $DMG"

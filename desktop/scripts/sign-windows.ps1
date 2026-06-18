# desktop/scripts/sign-windows.ps1 — build + Authenticode-sign the Windows installer.
# GATED behind $env:RUN_REAL -eq "1" (needs the fork build's app tree + a code-signing
# cert). Needs (CI secrets): WINDOWS_CERT_PFX_BASE64, WINDOWS_CERT_PASSWORD.
# An EV cert is strongly preferred so SmartScreen trusts the download immediately.
#
# NOTE: unvalidated here (no cert, no built app). The installer must be PRODUCED
# before signing — gulp emits an unpackaged app tree, not an .exe. Build it with
# Inno Setup (build/win32/code.iss in the checkout) or reuse desktop-app's NSIS.
$ErrorActionPreference = "Stop"

$appDir    = $env:APP_DIR;    if (-not $appDir)    { $appDir    = "..\VSCode-win32-x64" }
$installer = $env:INSTALLER;  if (-not $installer) { $installer = "dist\ForgeCodeSetup.exe" }

if ($env:RUN_REAL -ne "1") {
  Write-Host "STATUS: gated. Build the fork app tree, then re-run with RUN_REAL=1 + a cert."
  Write-Host "  app tree : $appDir"
  Write-Host "  installer: $installer (built below)"
  exit 0
}

New-Item -ItemType Directory -Force -Path "dist" | Out-Null

# 1. Build the installer from the gulp app tree (Inno Setup). iscc ships via
#    `winget install JRSoftware.InnoSetup`. The .iss is in the vscode checkout.
$iss = Join-Path (Split-Path $PSScriptRoot -Parent) "code-oss\build\win32\code.iss"
if (Test-Path $iss) {
  & iscc /O"dist" /F"ForgeCodeSetup" $iss
} elseif (-not (Test-Path $installer)) {
  throw "No installer at $installer and no Inno Setup script at $iss — produce the installer first."
}

# 2. Authenticode-sign it (signtool ships with the Windows SDK on windows-latest).
[IO.File]::WriteAllBytes("$env:TEMP\cert.pfx", [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))
& signtool sign `
  /f "$env:TEMP\cert.pfx" /p "$env:WINDOWS_CERT_PASSWORD" `
  /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  $installer
& signtool verify /pa $installer
Remove-Item "$env:TEMP\cert.pfx" -Force
Write-Host "Authenticode-signed: $installer"

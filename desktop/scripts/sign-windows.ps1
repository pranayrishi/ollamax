# desktop/scripts/sign-windows.ps1 — Authenticode-sign the Windows installer.
# SCAFFOLD: real commands below; exits early until the NSIS installer exists.
# Needs (CI secrets): WINDOWS_CERT_PFX_BASE64, WINDOWS_CERT_PASSWORD.
# An EV cert is strongly preferred so SmartScreen trusts the download immediately.
$ErrorActionPreference = "Stop"

Write-Host "STATUS: scaffold. Wire the fork build to produce ForgeCodeSetup.exe, then remove the early exit."
exit 0

$installer = "dist\ForgeCodeSetup.exe"
[IO.File]::WriteAllBytes("$env:TEMP\cert.pfx", [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))

# signtool ships with the Windows SDK on windows-latest runners.
& signtool sign `
  /f "$env:TEMP\cert.pfx" /p "$env:WINDOWS_CERT_PASSWORD" `
  /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  $installer

& signtool verify /pa $installer
Remove-Item "$env:TEMP\cert.pfx" -Force
Write-Host "Authenticode-signed: $installer"

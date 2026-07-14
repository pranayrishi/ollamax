# Ollama-Forge — quick setup (UNSIGNED build). Windows.
# Run:  powershell -ExecutionPolicy Bypass -File install.ps1
$ErrorActionPreference = "Stop"
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
Write-Host "-- Ollama-Forge setup (unsigned build) --"

# 1) forge CLI
$bin = Join-Path $dir "forge.exe"
if (Test-Path $bin) { Unblock-File $bin -ErrorAction SilentlyContinue }  # clear "downloaded from internet"
$dest = Join-Path $env:LOCALAPPDATA "Programs\OllamaForge"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item $bin (Join-Path $dest "forge.exe") -Force
Write-Host "forge installed to $dest"
# Add to the user PATH if missing.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$dest*") {
  [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
  Write-Host "Added $dest to your PATH (restart your terminal)."
}

# 2) VS Code extension (optional)
$vsix = Get-ChildItem -Path $dir -Filter "forge-vscode*.vsix" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($vsix -and (Get-Command code -ErrorAction SilentlyContinue)) {
  code --install-extension $vsix.FullName | Out-Null
  Write-Host "VS Code panel installed."
} elseif ($vsix) {
  Write-Host "VS Code 'code' command not found. Install manually:"
  Write-Host "  VS Code -> Extensions -> ... -> Install from VSIX -> $($vsix.FullName)"
}

# 3) Ollama prerequisite
if (Get-Command ollama -ErrorAction SilentlyContinue) {
  Write-Host "Ollama detected - pull a model, e.g.:  ollama pull qwen3.5:9b"
} else {
  Write-Host "Ollama NOT found - install from https://ollama.com/download (required)."
}
Write-Host "Done. Try:  forge --help   (or open the chat panel in VS Code)"

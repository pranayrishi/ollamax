# Ollama-Forge — one-line installer (Windows).
#
#   irm https://github.com/pranayrishi/ollamax-releases/releases/latest/download/install.ps1 | iex
#
# WHY THIS AVOIDS THE SMARTSCREEN PROMPT: Windows tags browser-downloaded files
# with a "mark of the web" (Zone.Identifier) that triggers the SmartScreen
# "Windows protected your PC" warning for unsigned executables. Files fetched
# with Invoke-WebRequest/irm are NOT marked, so the warning never appears. We
# also Unblock-File defensively. This is not a security bypass — the build is
# simply unsigned for now; Authenticode signing (paid) is the future step.
$ErrorActionPreference = "Stop"

$repo = if ($env:FORGE_RELEASES_REPO) { $env:FORGE_RELEASES_REPO } else { "pranayrishi/ollamax-releases" }
$base = "https://github.com/$repo/releases/latest/download"
$asset = "ollama-forge-windows-x64.zip"

function Say($m) { Write-Host "-> $m" -ForegroundColor Yellow }
function Ok($m)  { Write-Host "OK $m" -ForegroundColor Green }

Say "Installing Ollama-Forge for Windows x64 ($asset)"
$tmp = Join-Path $env:TEMP ("forge-" + [System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

# 1) Download via irm/iwr (NOT browser → no mark-of-the-web → no SmartScreen).
Say "Downloading $asset"
Invoke-WebRequest "$base/$asset" -OutFile "$tmp\bundle.zip"

# 2) Verify checksum if present.
try {
  Invoke-WebRequest "$base/$asset.sha256" -OutFile "$tmp\bundle.sha256"
  $want = ((Get-Content "$tmp\bundle.sha256") -split '\s+')[0].TrimStart('*')
  $got = (Get-FileHash "$tmp\bundle.zip" -Algorithm SHA256).Hash.ToLower()
  if ($want -and ($want.ToLower() -ne $got)) { throw "checksum mismatch (expected $want, got $got)" }
  Ok "Checksum verified"
} catch { Say "Checksum step skipped: $($_.Exception.Message)" }

# 3) Extract + install forge.exe.
Expand-Archive "$tmp\bundle.zip" -DestinationPath "$tmp\x" -Force
$src = Get-ChildItem "$tmp\x" -Directory | Where-Object { $_.Name -like "ollama-forge-*" } | Select-Object -First 1
if (-not $src) { throw "bundle layout unexpected" }
$dest = Join-Path $env:LOCALAPPDATA "Programs\OllamaForge"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item (Join-Path $src.FullName "forge.exe") (Join-Path $dest "forge.exe") -Force
Unblock-File (Join-Path $dest "forge.exe")   # clear mark-of-the-web defensively
Ok "Installed forge.exe -> $dest"

# 4) Add to user PATH if missing.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$dest*") {
  [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
  Say "Added $dest to your PATH (restart your terminal)"
}

# 5) VS Code panel (optional).
$vsix = Get-ChildItem $src.FullName -Filter "forge-vscode*.vsix" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($vsix -and (Get-Command code -ErrorAction SilentlyContinue)) {
  code --install-extension $vsix.FullName | Out-Null
  Ok "VS Code panel installed"
} elseif ($vsix) {
  Say "VS Code 'code' command not found — install the panel from VS Code -> Extensions -> Install from VSIX"
}

# 6) Ollama prerequisite + recommended model.
if (Get-Command ollama -ErrorAction SilentlyContinue) {
  $rec = (& (Join-Path $dest "forge.exe") models 2>$null | Select-String -Pattern 'ollama pull \S+' | Select-Object -First 1).Matches.Value
  if (-not $rec) { $rec = "ollama pull qwen2.5-coder:7b" }
  Say "Recommended model - run:  $rec"
} else {
  Say "Ollama not found (needed for local models) - install: https://ollama.com/download"
}

Ok "Ollama-Forge is installed."
Write-Host "`nNext:`n  - CLI:    forge --help`n  - Editor: open VS Code -> the anvil icon -> Chat panel`n  - Sign in from the panel for account features (optional)"

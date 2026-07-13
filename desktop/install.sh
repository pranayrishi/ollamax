#!/bin/sh
# Ollama-Forge — one-line installer (macOS / Linux).
#
#   curl -fsSL https://github.com/pranayrishi/ollamax-releases/releases/latest/download/install.sh | sh
#
# WHY THIS AVOIDS THE GATEKEEPER WARNING: macOS only quarantines files written
# by quarantine-aware apps (browsers, Mail, etc.) via the `com.apple.quarantine`
# extended attribute — that's what makes Gatekeeper say "forge cannot be opened
# because it is from an unidentified developer". `curl` does NOT set that flag,
# so a curl-downloaded binary just runs. (This is how Homebrew/rustup/Ollama
# ship unsigned CLI tools.) We ALSO strip quarantine defensively, belt-and-suspenders.
#
# This is NOT a security bypass: the build is simply unsigned for now. Signing +
# notarization (the paid step) is what removes the warning for *browser*/.dmg
# downloads — that's a future milestone, not this round.
#
# Transparent + idempotent: it echoes each step and is safe to re-run.
set -eu

REPO="${FORGE_RELEASES_REPO:-pranayrishi/ollamax-releases}"
BASE="https://github.com/${REPO}/releases/latest/download"
BIN_DIR="${FORGE_BIN_DIR:-$HOME/.local/bin}"

say() { printf '\033[1;33m→\033[0m %s\n' "$1"; }
ok()  { printf '\033[1;32m✓\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$1" >&2; exit 1; }

# --- 1. Detect OS + arch, choose the right bundle -------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64)  asset="ollama-forge-macos-arm64.tar.gz" ;;
      x86_64)
        die "Intel macOS has no published prebuilt Ollamax bundle yet. Build from source instead: https://github.com/pranayrishi/ollamax (requires Rust), or use an Apple Silicon Mac."
        ;;
      *) die "unsupported macOS arch: $arch" ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64|amd64) asset="ollama-forge-linux-x64.tar.gz" ;;
      *) die "unsupported Linux arch: $arch (only x86_64 is published right now)" ;;
    esac ;;
  *)
    die "unsupported OS: $os — on Windows run the PowerShell command instead" ;;
esac
say "Installing Ollama-Forge for $os/$arch ($asset)"

command -v curl >/dev/null 2>&1 || die "curl is required"

# --- 2. Download the bundle VIA CURL (not quarantined) --------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $asset"
curl -fsSL "$BASE/$asset" -o "$tmp/bundle.tgz" || die "download failed: $BASE/$asset"

# --- 3. Verify the checksum if the sidecar is present ---------------------
if curl -fsSL "$BASE/$asset.sha256" -o "$tmp/bundle.sha256" 2>/dev/null; then
  want="$(awk '{print $1}' "$tmp/bundle.sha256" | tr -d '*')"
  if command -v shasum >/dev/null 2>&1; then
    got="$(shasum -a 256 "$tmp/bundle.tgz" | awk '{print $1}')"
  else
    got="$(sha256sum "$tmp/bundle.tgz" | awk '{print $1}')"
  fi
  [ "$want" = "$got" ] || die "checksum mismatch (expected $want, got $got)"
  ok "Checksum verified"
fi

# --- 4. Extract + install forge onto PATH --------------------------------
mkdir -p "$tmp/x"
tar -xzf "$tmp/bundle.tgz" -C "$tmp/x"
src="$(find "$tmp/x" -maxdepth 1 -type d -name 'ollama-forge-*' | head -1)"
[ -n "$src" ] && [ -f "$src/forge" ] || die "bundle layout unexpected (no forge binary)"

mkdir -p "$BIN_DIR"
cp "$src/forge" "$BIN_DIR/forge"
chmod +x "$BIN_DIR/forge"
# Defensive: strip quarantine on anything we wrote (no-op if absent / Linux).
if [ "$os" = "Darwin" ]; then
  xattr -dr com.apple.quarantine "$BIN_DIR/forge" 2>/dev/null || true
fi
ok "Installed forge → $BIN_DIR/forge"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Add it to your PATH:  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc && . ~/.zshrc" ;;
esac

# --- 5. VS Code panel (optional) -----------------------------------------
vsix="$(find "$src" -name 'forge-vscode*.vsix' | head -1 || true)"
if [ -n "$vsix" ] && command -v code >/dev/null 2>&1; then
  code --install-extension "$vsix" >/dev/null 2>&1 && ok "VS Code panel installed" || say "VS Code panel install skipped"
elif [ -n "$vsix" ]; then
  say "VS Code 'code' command not found — install the panel from VS Code → Extensions → Install from VSIX"
fi

# --- 6. Ollama prerequisite + recommended model --------------------------
if command -v ollama >/dev/null 2>&1; then
  rec="$("$BIN_DIR/forge" models 2>/dev/null | grep -oE 'ollama pull [^ ]+' | head -1 || true)"
  [ -n "$rec" ] || rec="ollama pull qwen3.5:4b"
  # Prompt only with a real terminal attached (true even for `curl … | sh`,
  # since only stdin is the pipe). Otherwise just print the command.
  if [ -t 1 ] && [ -r /dev/tty ]; then
    printf '\033[1;33m?\033[0m Pull the recommended model now (%s)? [y/N] ' "$rec"
    ans=""; read -r ans </dev/tty 2>/dev/null || ans=""
    case "$ans" in
      y|Y) sh -c "$rec" && ok "Model pulled" ;;
      *) say "Skipped — when ready run:  $rec" ;;
    esac
  else
    say "Recommended model — run:  $rec"
  fi
else
  say "Ollama not found (needed for local models) — install: https://ollama.com/download"
fi

# --- 7. Done -------------------------------------------------------------
ok "Ollama-Forge is installed."
printf '\nNext:\n  • CLI:    forge --help\n  • Editor: open VS Code → the anvil icon → Chat panel\n  • Sign in from the panel for account features (optional)\n'

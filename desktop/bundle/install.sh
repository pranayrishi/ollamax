#!/usr/bin/env sh
# Ollama-Forge — quick setup (UNSIGNED build). macOS / Linux.
# Installs the `forge` CLI, (optionally) the VS Code panel, and checks Ollama.
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
echo "── Ollama-Forge setup (unsigned build) ──"

# 1) forge CLI ---------------------------------------------------------------
BIN="forge"
chmod +x "$DIR/$BIN" 2>/dev/null || true
# macOS: clear the Gatekeeper quarantine so the unsigned binary runs without
# "cannot be opened because the developer cannot be verified".
if [ "$(uname)" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$DIR/$BIN" 2>/dev/null || true
fi
mkdir -p "$HOME/.local/bin"
cp "$DIR/$BIN" "$HOME/.local/bin/forge"
chmod +x "$HOME/.local/bin/forge"
echo "✓ forge installed to ~/.local/bin/forge"
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "  → add ~/.local/bin to your PATH (e.g. echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc)" ;;
esac

# 2) VS Code extension (optional) -------------------------------------------
VSIX="$(ls "$DIR"/forge-vscode*.vsix 2>/dev/null | head -1 || true)"
if [ -n "$VSIX" ] && command -v code >/dev/null 2>&1; then
  code --install-extension "$VSIX" >/dev/null && echo "✓ VS Code panel installed"
elif [ -n "$VSIX" ]; then
  echo "• VS Code 'code' command not found. Install the panel manually:"
  echo "    VS Code → Extensions → ··· → Install from VSIX → $VSIX"
fi

# 3) Ollama prerequisite -----------------------------------------------------
if command -v ollama >/dev/null 2>&1; then
  echo "✓ Ollama detected — pull a model, e.g.:  ollama pull qwen3.5:9b"
else
  echo "! Ollama NOT found — install it from https://ollama.com/download (required for local inference)"
fi

echo "Done. Try:  forge --help   (or open the chat panel in VS Code)"

#!/usr/bin/env bash
#
# Ollama-Forge installer.
#
# Builds from source with cargo and installs the `forge` binary into
# ~/.local/forge/bin. Idempotent — safe to re-run. Does NOT touch your shell
# rc files unless --update-shell is passed; instead it prints what to add.

set -euo pipefail

FORGE_VERSION="0.1.0"
INSTALL_DIR="${FORGE_INSTALL_DIR:-${HOME}/.local/forge}"
BIN_DIR="${INSTALL_DIR}/bin"

DRY_RUN=0
UPDATE_SHELL=0

usage() {
    cat <<EOF
Usage: ./install.sh [options]

Options:
  --dry-run        Show what would happen without changing anything.
  --update-shell   Append a PATH line to ~/.bashrc and ~/.zshrc (off by default).
  --prefix DIR     Install into DIR/bin instead of ~/.local/forge/bin.
  -h, --help       Show this help.

Environment:
  FORGE_INSTALL_DIR  Equivalent to --prefix.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)      DRY_RUN=1; shift ;;
        --update-shell) UPDATE_SHELL=1; shift ;;
        --prefix)       INSTALL_DIR="$2"; BIN_DIR="${INSTALL_DIR}/bin"; shift 2 ;;
        -h|--help)      usage; exit 0 ;;
        *)
            echo "install.sh: unknown option: $1" >&2
            echo "Try './install.sh --help' for usage." >&2
            exit 2
            ;;
    esac
done

run() {
    if [[ $DRY_RUN -eq 1 ]]; then
        echo "[dry-run] $*"
    else
        eval "$@"
    fi
}

err() {
    echo "install.sh: $*" >&2
}

# ----- preflight -----

if ! command -v cargo >/dev/null 2>&1; then
    err "cargo not found in PATH."
    err "Ollama-Forge currently builds from source. Install Rust:"
    err "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

if ! command -v ollama >/dev/null 2>&1; then
    echo "warning: \`ollama\` not found in PATH."
    echo "         forge will install fine, but you'll need Ollama running"
    echo "         before \`forge status\`, \`forge chat\`, etc. work."
    echo "         Install: https://ollama.com/download"
    echo
fi

OS="$(uname -s)"
ARCH="$(uname -m)"
case "${OS}" in
    Linux|Darwin) ;;
    *) err "unsupported OS: ${OS}"; exit 1 ;;
esac
case "${ARCH}" in
    x86_64|aarch64|arm64) ;;
    *) err "unsupported architecture: ${ARCH}"; exit 1 ;;
esac

echo "Installing Ollama-Forge v${FORGE_VERSION}"
echo "  OS:        ${OS} (${ARCH})"
echo "  Prefix:    ${INSTALL_DIR}"
echo "  From:      $(pwd)"
echo

# ----- build -----

run "mkdir -p \"${BIN_DIR}\""

echo "Building (cargo build --release)…"
run "cargo build --release"

if [[ ! -f target/release/forge && $DRY_RUN -eq 0 ]]; then
    err "build did not produce target/release/forge"
    exit 1
fi

run "install -m 0755 target/release/forge \"${BIN_DIR}/forge\""

# ----- shell integration (opt-in) -----

PATH_LINE="export PATH=\"${BIN_DIR}:\$PATH\""

if [[ $UPDATE_SHELL -eq 1 ]]; then
    for rc in "${HOME}/.bashrc" "${HOME}/.zshrc"; do
        if [[ -f "${rc}" ]] && ! grep -Fq "${BIN_DIR}" "${rc}" 2>/dev/null; then
            echo "Appending PATH to ${rc}"
            run "printf '\\n# Added by ollama-forge installer\\n%s\\n' \"${PATH_LINE}\" >> \"${rc}\""
        fi
    done
fi

# ----- done -----

echo
echo "✅ Installed: ${BIN_DIR}/forge"
echo

case ":${PATH}:" in
    *":${BIN_DIR}:"*)
        echo "PATH already includes ${BIN_DIR}. Try:"
        ;;
    *)
        echo "${BIN_DIR} is NOT in your PATH. Add it with:"
        echo "    ${PATH_LINE}"
        echo
        echo "Or re-run this installer with --update-shell. Then:"
        ;;
esac

echo "    forge status"
echo "    forge --help"

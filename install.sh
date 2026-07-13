#!/usr/bin/env bash
#
# Ollama-Forge installer.
#
# Builds from source with cargo and installs the `forge` binary into
# ~/.local/forge/bin. Idempotent — safe to re-run. Does NOT touch your shell
# rc files unless --update-shell is passed; instead it prints what to add.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_VERSION="$(awk -F'"' '/^version = "/ { print $2; exit }' "$SCRIPT_DIR/Cargo.toml" 2>/dev/null || true)"
FORGE_VERSION="${FORGE_VERSION:-${SOURCE_VERSION:-0.2.1}}"
INSTALL_DIR="${FORGE_INSTALL_DIR:-${HOME}/.local/forge}"
BIN_DIR="${INSTALL_DIR}/bin"
# Release bundles are published from the private source repository to this
# public companion repository. Accept the older plural spelling too because it
# is used by the one-line desktop installers.
RELEASE_REPO="${FORGE_RELEASE_REPO:-${FORGE_RELEASES_REPO:-pranayrishi/ollamax-releases}}"
RELEASE_TAG="${FORGE_RELEASE_TAG:-v${FORGE_VERSION}}"

DRY_RUN=0
UPDATE_SHELL=0
PREFER_PREBUILT=0

usage() {
    cat <<EOF
Usage: ./install.sh [options]

Options:
  --dry-run        Show what would happen without changing anything.
  --update-shell   Append a PATH line to ~/.bashrc and ~/.zshrc (off by default).
  --prebuilt       Try to download a prebuilt release binary instead of
                   building from source. Falls back to source if no matching
                   release is found.
  --prefix DIR     Install into DIR/bin instead of ~/.local/forge/bin.
  -h, --help       Show this help.

Environment:
  FORGE_INSTALL_DIR  Equivalent to --prefix.
  FORGE_RELEASE_REPO GitHub repo to fetch prebuilt binaries from
                     (default: pranayrishi/ollamax-releases).
  FORGE_RELEASE_TAG  Release tag to fetch for --prebuilt
                     (default: v<the version in Cargo.toml>).
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)      DRY_RUN=1; shift ;;
        --update-shell) UPDATE_SHELL=1; shift ;;
        --prebuilt)     PREFER_PREBUILT=1; shift ;;
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

# Detect the published bundle name early so the prebuilt path can use the same
# release contract as .github/workflows/release.yml. These bundles are named by
# platform label (not Rust target triple) and live in the public releases repo.
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"
PREBUILT_PLATFORM=""
case "${HOST_OS}-${HOST_ARCH}" in
    Linux-x86_64)   PREBUILT_PLATFORM="linux-x64" ;;
    Darwin-arm64)   PREBUILT_PLATFORM="macos-arm64" ;;
    # The release workflow deliberately does not publish an Intel macOS bundle.
    # Leave this empty so --prebuilt takes the documented source-build fallback.
    Darwin-x86_64)  ;;
    *) ;;
esac

# If --prebuilt was requested, try the release path first. Falls through to
# source build if any step fails.
PREBUILT_OK=0
if [[ $PREFER_PREBUILT -eq 1 && -n "$PREBUILT_PLATFORM" ]]; then
    if command -v curl >/dev/null 2>&1; then
        archive="ollama-forge-${PREBUILT_PLATFORM}.tar.gz"
        if [[ "$RELEASE_TAG" == "latest" ]]; then
            url="https://github.com/${RELEASE_REPO}/releases/latest/download/${archive}"
        else
            url="https://github.com/${RELEASE_REPO}/releases/download/${RELEASE_TAG}/${archive}"
        fi
        echo "Trying prebuilt: $url"
        tmpdir="$(mktemp -d)"
        if curl -fsSL "$url" -o "$tmpdir/$archive" 2>/dev/null; then
            run "mkdir -p \"${BIN_DIR}\""
            run "tar -xzf \"$tmpdir/$archive\" -C \"$tmpdir\""
            extracted="$tmpdir/ollama-forge-${PREBUILT_PLATFORM}/forge"
            if [[ -f "$extracted" ]]; then
                run "install -m 0755 \"$extracted\" \"${BIN_DIR}/forge\""
                PREBUILT_OK=1
                echo "✅ Installed prebuilt binary"
            fi
        else
            echo "(no prebuilt release for ${PREBUILT_PLATFORM} at ${RELEASE_TAG}; falling back to source build)"
        fi
        rm -rf "$tmpdir"
    fi
elif [[ $PREFER_PREBUILT -eq 1 ]]; then
    echo "(no prebuilt release for ${HOST_OS}/${HOST_ARCH}; falling back to source build)"
fi

if [[ $PREBUILT_OK -eq 0 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        err "cargo not found in PATH."
        err "Ollama-Forge needs Rust to build from source. Install:"
        err "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        err "Or re-run with --prebuilt to fetch a release binary."
        exit 1
    fi
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

if [[ $PREBUILT_OK -eq 0 ]]; then
    run "mkdir -p \"${BIN_DIR}\""

    echo "Building (cargo build --release)…"
    run "cargo build --release"

    if [[ ! -f target/release/forge && $DRY_RUN -eq 0 ]]; then
        err "build did not produce target/release/forge"
        exit 1
    fi

    run "install -m 0755 target/release/forge \"${BIN_DIR}/forge\""
fi

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

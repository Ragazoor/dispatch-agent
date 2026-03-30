#!/usr/bin/env bash
set -euo pipefail

REPO="Ragazoor/dispatch"
BINARY="dispatch"
INSTALL_DIR="${HOME}/.local/bin"

# ── Platform check ────────────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

if [[ "${OS}" != "Linux" ]]; then
    echo "error: dispatch only supports Linux (got ${OS})" >&2
    exit 1
fi

if [[ "${ARCH}" != "x86_64" ]]; then
    echo "error: dispatch only provides pre-built binaries for x86_64 (got ${ARCH})" >&2
    echo "       Build from source with: cargo build --release" >&2
    exit 1
fi

TARGET="x86_64-unknown-linux-gnu"

# ── Resolve version ───────────────────────────────────────────────────────────

if [[ -n "${VERSION:-}" ]]; then
    TAG="${VERSION}"
else
    echo "Fetching latest release..."
    TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    if [[ -z "${TAG}" ]]; then
        echo "error: could not determine latest release. Set VERSION= to install a specific version." >&2
        exit 1
    fi
fi

echo "Installing ${BINARY} ${TAG}..."

# ── Download ──────────────────────────────────────────────────────────────────

ARTIFACT="${BINARY}-${TARGET}"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}"
TMP="$(mktemp)"
trap 'rm -f "${TMP}"' EXIT

echo "Downloading ${URL}..."
curl -fsSL --progress-bar -o "${TMP}" "${URL}"

# ── Install ───────────────────────────────────────────────────────────────────

mkdir -p "${INSTALL_DIR}"
install -m 755 "${TMP}" "${INSTALL_DIR}/${BINARY}"
echo "Installed to ${INSTALL_DIR}/${BINARY}"

# Warn if ~/.local/bin is not in PATH
if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
    echo ""
    echo "  Note: ${INSTALL_DIR} is not in your PATH."
    echo "  Add this to your shell profile:"
    echo "    export PATH=\"\${HOME}/.local/bin:\${PATH}\""
fi

# ── Configure Claude Code ─────────────────────────────────────────────────────

echo ""
echo "Configuring Claude Code..."
"${INSTALL_DIR}/${BINARY}" setup

# ── Prerequisites checklist ───────────────────────────────────────────────────

echo ""
echo "Prerequisites checklist:"

check_dep() {
    local cmd="$1"
    local note="$2"
    if command -v "${cmd}" &>/dev/null; then
        echo "  [x] ${cmd}"
    else
        echo "  [ ] ${cmd}  ← ${note}"
    fi
}

check_dep tmux   "required — dispatch must run inside a tmux session"
check_dep git    "required — dispatch creates git worktrees for agents"
check_dep claude "required — Claude Code CLI (https://claude.ai/code)"
check_dep gh     "optional — needed for the Review Board (gh auth login)"

echo ""
echo "Done. Run 'dispatch tui' inside a tmux session to start."

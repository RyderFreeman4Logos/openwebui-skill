#!/usr/bin/env bash
# Downloadable installer for openwebui-chat. Run with:
# curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/openwebui-skill/main/install.sh | bash

set -euo pipefail

REPOSITORY_URL="https://github.com/RyderFreeman4Logos/openwebui-skill"
MISE_BIN="${HOME}/.local/bin/mise"

echo "openwebui-chat installer"
echo "========================"

echo
echo "[1/5] Checking for mise..."
if ! command -v mise >/dev/null 2>&1 && [[ -x "${MISE_BIN}" ]]; then
    export PATH="${HOME}/.local/bin:${PATH}"
fi

if ! command -v mise >/dev/null 2>&1; then
    echo "mise is not installed; installing it without root..."
    curl https://mise.run | sh
    export PATH="${HOME}/.local/bin:${PATH}"
fi

if ! command -v mise >/dev/null 2>&1; then
    echo "Error: mise was not found after installation." >&2
    exit 1
fi

echo
echo "[2/5] Installing openwebui-chat via mise cargo backend..."
mise use -g rust@stable
mise use -g "cargo:openwebui-chat@git+${REPOSITORY_URL}"

if ! command -v openwebui-chat >/dev/null 2>&1; then
    echo "Error: openwebui-chat was not found after mise install." >&2
    echo "Try running 'mise activate bash' or check mise shims." >&2
    exit 1
fi

echo
echo "[3/5] Initializing interactive XDG configuration..."
if ! exec 3</dev/tty 2>/dev/null; then
    echo "Error: configuration requires an interactive terminal (/dev/tty)." >&2
    exit 1
fi
openwebui-chat config init <&3
exec 3<&-

echo
echo "[4/5] Running diagnostics..."
openwebui-chat doctor

echo
echo "[5/5] Optional: install the agent skill separately"
echo "  npx skills add RyderFreeman4Logos/openwebui-skill"
echo
echo "Installation complete. Run 'openwebui-chat --help' for usage."
echo "To update later: mise upgrade cargo:openwebui-chat"

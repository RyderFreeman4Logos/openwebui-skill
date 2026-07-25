#!/bin/bash
# openwebui-chat installer
# Installs the binary and Hermes skill without root.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_NAME="openwebui-chat"
INSTALL_DIR="${HOME}/.local/bin"
SKILL_DIR="${HOME}/.hermes/skills/openwebui-chat"

echo "openwebui-chat installer"
echo "========================"

# 1. Build the binary
echo ""
echo "[1/4] Building release binary..."
cd "$SCRIPT_DIR"
if command -v cargo &>/dev/null; then
    cargo build --release
elif command -v rustc &>/dev/null; then
    echo "Warning: cargo not found, using rustc directly"
    rustc --edition 2021 -O src/main.rs -o "target/release/${BINARY_NAME}"
else
    echo "Error: Neither cargo nor rustc found. Install Rust from https://rustup.rs"
    exit 1
fi

BINARY_PATH="target/release/${BINARY_NAME}"
if [[ ! -f "$BINARY_PATH" ]]; then
    echo "Error: Binary not found at ${BINARY_PATH}"
    exit 1
fi

# 2. Install binary
echo ""
echo "[2/4] Installing binary to ${INSTALL_DIR}/"
mkdir -p "$INSTALL_DIR"
cp "$BINARY_PATH" "${INSTALL_DIR}/${BINARY_NAME}"
chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

# Check PATH
if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
    echo ""
    echo "  Warning: ${INSTALL_DIR} is not in your PATH."
    echo "  Add this to your shell profile:"
    echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
fi

# 3. Install Hermes skill
echo ""
echo "[3/4] Installing Hermes skill to ${SKILL_DIR}/"
if [[ -d "${HOME}/.hermes" ]]; then
    mkdir -p "$SKILL_DIR"
    if [[ -d "${SCRIPT_DIR}/skills/openwebui-chat" ]]; then
        cp -r "${SCRIPT_DIR}/skills/openwebui-chat/"* "$SKILL_DIR/"
    else
        echo "  (skill files not found, skipping skill installation)"
    fi
else
    echo "  (~/.hermes not found, skipping skill installation)"
fi

# 4. Print required environment variables
echo ""
echo "[4/4] Environment setup"
echo "  Set these environment variables (or use ~/.config/openwebui-chat/config.toml):"
echo ""
echo "    export OPENWEBUI_BASE_URL=http://your-openwebui:8080"
echo "    export OPENWEBUI_API_KEY=sk-your-api-key"
echo "    export OPENWEBUI_DEFAULT_MODEL=your-model"
echo ""
echo "  Copy .env.example for reference."

# 5. Non-destructive diagnostic
echo ""
echo "Running diagnostic..."
if [[ -f "${INSTALL_DIR}/${BINARY_NAME}" ]]; then
    "${INSTALL_DIR}/${BINARY_NAME}" doctor || true
fi

echo ""
echo "Installation complete."
echo "Run '${BINARY_NAME} --help' for usage."

#!/bin/bash
# openwebui-chat uninstaller
# Removes the binary and Hermes skill.

set -euo pipefail

BINARY_NAME="openwebui-chat"
INSTALL_DIR="${HOME}/.local/bin"
SKILL_DIR="${HOME}/.hermes/skills/openwebui-chat"

echo "openwebui-chat uninstaller"
echo "=========================="

# Remove binary
if [[ -f "${INSTALL_DIR}/${BINARY_NAME}" ]]; then
    echo "Removing ${INSTALL_DIR}/${BINARY_NAME}"
    rm -f "${INSTALL_DIR}/${BINARY_NAME}"
else
    echo "Binary not found at ${INSTALL_DIR}/${BINARY_NAME} (already removed?)"
fi

# Remove skill (only if it's ours)
if [[ -d "$SKILL_DIR" ]]; then
    echo "Removing skill directory ${SKILL_DIR}"
    rm -rf "$SKILL_DIR"
else
    echo "Skill directory not found at ${SKILL_DIR}"
fi

echo ""
echo "Uninstallation complete."
echo "Note: ~/.config/openwebui-chat/config.toml was not removed (if it exists)."

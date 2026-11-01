#!/usr/bin/env bash
# ==============================================================================
# Gravhive Linux Uninstaller
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ -f "${SCRIPT_DIR}/install.sh" ]; then
    exec bash "${SCRIPT_DIR}/install.sh" --uninstall "$@"
else
    echo "Stopping gravhive daemon..."
    ghive kill 2>/dev/null || true

    echo "Removing systemd service..."
    if command -v systemctl >/dev/null 2>&1; then
        sudo systemctl stop gravhive.service 2>/dev/null || true
        sudo systemctl disable gravhive.service 2>/dev/null || true
        sudo rm -f /etc/systemd/system/gravhive.service
        sudo systemctl daemon-reload || true
    fi

    echo "Removing binaries..."
    sudo rm -f /usr/local/bin/ghive /usr/local/bin/ghived ~/.local/bin/ghive ~/.local/bin/ghived

    echo "Gravhive has been uninstalled."
fi


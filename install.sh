#!/usr/bin/env bash
# ==============================================================================
# Gravhive Linux Installer
# Website: https://github.com/your-org/GravHive
# Description: Automated installer for Gravhive process manager & daemon
# ==============================================================================

set -euo pipefail

# --- Color formatting ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# --- Default Paths ---
INSTALL_DIR="/usr/local/bin"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_NAME="gravhive.service"
GRAVHIVE_HOME_DIR="${HOME}/.gravhive"
LOG_DIR="/var/log/gravhive"

print_banner() {
    echo -e "${CYAN}${BOLD}"
    cat << "EOF"
   ____                 _     _             
  / ___|_ __ __ ___   _| |__ (_)_   _____   
 | |  _| '__/ _` \ \ / / '_ \| \ \ / / _ \  
 | |_| | | | (_| |\ V /| | | | |\ V /  __/  
  \____|_|  \__,_| \_/ |_| |_|_| \_/ \___|  
                                            
 Next-Generation Production Process Manager in Rust
EOF
    echo -e "${NC}"
}

log_info() {
    echo -e "${CYAN}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

check_root_or_sudo() {
    # If user explicitly requested user-mode installation or passed --user
    if [ "${USER_MODE:-false}" = "true" ] || [ "${1:-}" = "--user" ]; then
        log_info "Installing in user mode to ~/.local/bin (no root/sudo required)"
        INSTALL_DIR="${HOME}/.local/bin"
        SYSTEMD_DIR="${HOME}/.config/systemd/user"
        SUDO=""
        return 0
    fi

    if [ "$EUID" -ne 0 ]; then
        if command -v sudo >/dev/null 2>&1; then
            SUDO="sudo"
        else
            log_warn "Sudo is not available. Falling back to user directory installation (~/.local/bin)."
            INSTALL_DIR="${HOME}/.local/bin"
            SYSTEMD_DIR="${HOME}/.config/systemd/user"
            SUDO=""
        fi
    else
        SUDO=""
    fi
}

check_environment() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    if [ "$OS" != "Linux" ]; then
        log_warn "This installer is designed for Linux (detected: $OS)."
        if [ "$OS" = "Darwin" ]; then
            log_info "For macOS, please build with 'cargo build --release' and use 'ghive startup' for launchd."
        fi
    fi

    log_info "Detected OS: ${BOLD}${OS}${NC} (${ARCH})"

    case "$ARCH" in
        x86_64|amd64)
            TARGET_ARCH="x86_64-unknown-linux-musl"
            ALT_TARGET_ARCH="x86_64-unknown-linux-gnu"
            ;;
        aarch64|arm64)
            TARGET_ARCH="aarch64-unknown-linux-musl"
            ALT_TARGET_ARCH="aarch64-unknown-linux-gnu"
            ;;
        *)
            TARGET_ARCH="generic"
            ALT_TARGET_ARCH="generic"
            log_warn "Architecture ${ARCH} will require building from source."
            ;;
    esac
}

check_cgroups_v2() {
    if [ -f "/sys/fs/cgroup/cgroup.controllers" ]; then
        log_success "Linux cgroups v2 detected. Kernel isolation & hard limits are supported."
    else
        log_warn "Linux cgroups v2 not detected or running in legacy mode. Process limits will use user-space watchdog."
    fi
}

build_or_copy_binaries() {
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    
    # 1. Check if packaged release binaries exist directly in this directory (from GitHub Release tarball)
    if [ -f "${SCRIPT_DIR}/ghive" ] && [ -f "${SCRIPT_DIR}/ghived" ]; then
        log_info "Found pre-built release binaries in current directory"
        SRC_GHIVE="${SCRIPT_DIR}/ghive"
        SRC_GHIVED="${SCRIPT_DIR}/ghived"
    # 2. Check if local release binaries exist in workspace target/release/
    elif [ -f "${SCRIPT_DIR}/target/release/ghive" ] && [ -f "${SCRIPT_DIR}/target/release/ghived" ]; then
        log_info "Found existing release binaries in target/release/"
        SRC_GHIVE="${SCRIPT_DIR}/target/release/ghive"
        SRC_GHIVED="${SCRIPT_DIR}/target/release/ghived"
    # 3. Check if target/<TARGET_ARCH>/release exists (static musl binary)
    elif [ -f "${SCRIPT_DIR}/target/${TARGET_ARCH}/release/ghive" ] && [ -f "${SCRIPT_DIR}/target/${TARGET_ARCH}/release/ghived" ]; then
        log_info "Found static release binaries in target/${TARGET_ARCH}/release/"
        SRC_GHIVE="${SCRIPT_DIR}/target/${TARGET_ARCH}/release/ghive"
        SRC_GHIVED="${SCRIPT_DIR}/target/${TARGET_ARCH}/release/ghived"
    # 4. Check if target/<ALT_TARGET_ARCH>/release exists (gnu binary)
    elif [ -f "${SCRIPT_DIR}/target/${ALT_TARGET_ARCH}/release/ghive" ] && [ -f "${SCRIPT_DIR}/target/${ALT_TARGET_ARCH}/release/ghived" ]; then
        log_info "Found release binaries in target/${ALT_TARGET_ARCH}/release/"
        SRC_GHIVE="${SCRIPT_DIR}/target/${ALT_TARGET_ARCH}/release/ghive"
        SRC_GHIVED="${SCRIPT_DIR}/target/${ALT_TARGET_ARCH}/release/ghived"
    # 5. Build from source using Cargo
    elif [ -f "${SCRIPT_DIR}/Cargo.toml" ]; then
        log_info "Building Gravhive from local source using Cargo (release mode)..."
        if ! command -v cargo >/dev/null 2>&1; then
            log_error "Cargo/Rust is required to build from source. Please install Rust via https://rustup.rs"
            exit 1
        fi
        (cd "${SCRIPT_DIR}" && cargo build --release)
        SRC_GHIVE="${SCRIPT_DIR}/target/release/ghive"
        SRC_GHIVED="${SCRIPT_DIR}/target/release/ghived"
    else
        log_error "Binaries not found and Cargo.toml missing. Run this script from the GravHive project directory or release archive."
        exit 1
    fi

    # Create target installation directory
    if [ -n "$SUDO" ]; then
        $SUDO mkdir -p "$INSTALL_DIR"
        $SUDO cp "$SRC_GHIVE" "${INSTALL_DIR}/ghive"
        $SUDO cp "$SRC_GHIVED" "${INSTALL_DIR}/ghived"
        $SUDO chmod 755 "${INSTALL_DIR}/ghive" "${INSTALL_DIR}/ghived"
    else
        mkdir -p "$INSTALL_DIR"
        cp "$SRC_GHIVE" "${INSTALL_DIR}/ghive"
        cp "$SRC_GHIVED" "${INSTALL_DIR}/ghived"
        chmod 755 "${INSTALL_DIR}/ghive" "${INSTALL_DIR}/ghived"
    fi

    log_success "Installed 'ghive'  -> ${INSTALL_DIR}/ghive"
    log_success "Installed 'ghived' -> ${INSTALL_DIR}/ghived"
}

setup_directories() {
    log_info "Configuring Gravhive runtime directories..."
    mkdir -p "${GRAVHIVE_HOME_DIR}/logs"
    mkdir -p "${GRAVHIVE_HOME_DIR}/plugins"
    chmod 700 "${GRAVHIVE_HOME_DIR}"
    log_success "Runtime directory initialized at ${GRAVHIVE_HOME_DIR}"
}

setup_systemd_service() {
    if ! command -v systemctl >/dev/null 2>&1; then
        log_warn "systemd not detected on this system. Skipping systemd service setup."
        return 0
    fi

    log_info "Setting up systemd service for gravhive daemon..."

    SERVICE_PATH="${SYSTEMD_DIR}/${SERVICE_NAME}"
    CURRENT_USER="${SUDO_USER:-$USER}"

    SERVICE_CONTENT="[Unit]
Description=Gravhive Process Manager Daemon
Documentation=https://github.com/your-org/GravHive
After=network.target

[Service]
Type=simple
User=${CURRENT_USER}
ExecStart=${INSTALL_DIR}/ghived
Restart=always
RestartSec=3
KillMode=mixed
TimeoutStopSec=10
LimitNOFILE=65535
LimitNPROC=65535
Environment=PATH=/usr/local/bin:/usr/bin:/bin:${HOME}/.local/bin
Environment=GHIVE_HOME=${HOME}/.gravhive

[Install]
WantedBy=multi-user.target"

    if [ -n "$SUDO" ]; then
        echo "$SERVICE_CONTENT" | $SUDO tee "$SERVICE_PATH" > /dev/null
        $SUDO chmod 644 "$SERVICE_PATH"
        $SUDO systemctl daemon-reload
        $SUDO systemctl enable "$SERVICE_NAME"
        $SUDO systemctl restart "$SERVICE_NAME" || true
    else
        mkdir -p "$SYSTEMD_DIR"
        echo "$SERVICE_CONTENT" > "$SERVICE_PATH"
        chmod 644 "$SERVICE_PATH"
        systemctl --user daemon-reload || true
        systemctl --user enable "$SERVICE_NAME" || true
        systemctl --user restart "$SERVICE_NAME" || true
    fi

    log_success "Systemd service created and enabled at ${SERVICE_PATH}"
}

verify_installation() {
    log_info "Verifying installation..."

    if ! command -v "${INSTALL_DIR}/ghive" >/dev/null 2>&1; then
        log_error "Installation verification failed: 'ghive' not found in ${INSTALL_DIR}"
        exit 1
    fi

    VERSION="$("${INSTALL_DIR}/ghive" --version 2>/dev/null || echo "v0.1.0")"
    log_success "Gravhive successfully installed: ${BOLD}${VERSION}${NC}"

    # Path warning if not in PATH
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        log_warn "'$INSTALL_DIR' is not in your current PATH."
        log_info "Add this line to your ~/.bashrc or ~/.zshrc:"
        echo -e "    ${BOLD}export PATH=\"\$PATH:${INSTALL_DIR}\"${NC}"
    fi
}

uninstall() {
    print_banner
    log_info "Starting Gravhive uninstallation..."
    check_root_or_sudo

    # 1. Stop systemd service
    if command -v systemctl >/dev/null 2>&1; then
        if [ -n "$SUDO" ]; then
            $SUDO systemctl stop "$SERVICE_NAME" 2>/dev/null || true
            $SUDO systemctl disable "$SERVICE_NAME" 2>/dev/null || true
            $SUDO rm -f "${SYSTEMD_DIR}/${SERVICE_NAME}"
            $SUDO systemctl daemon-reload 2>/dev/null || true
        else
            systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
            systemctl --user disable "$SERVICE_NAME" 2>/dev/null || true
            rm -f "${HOME}/.config/systemd/user/${SERVICE_NAME}"
            systemctl --user daemon-reload 2>/dev/null || true
        fi
        log_success "Removed systemd service."
    fi

    # 2. Stop running daemon
    "${INSTALL_DIR}/ghive" kill 2>/dev/null || true

    # 3. Remove binaries
    if [ -n "$SUDO" ]; then
        $SUDO rm -f "${INSTALL_DIR}/ghive" "${INSTALL_DIR}/ghived"
    else
        rm -f "${INSTALL_DIR}/ghive" "${INSTALL_DIR}/ghived"
    fi
    log_success "Removed binaries from ${INSTALL_DIR}."

    # 4. State directory prompt
    echo ""
    read -p "Do you want to delete all configuration, logs, and state in ~/.gravhive? [y/N]: " -n 1 -r
    echo ""
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        rm -rf "${GRAVHIVE_HOME_DIR}"
        log_success "Removed ${GRAVHIVE_HOME_DIR}."
    else
        log_info "Kept state directory at ${GRAVHIVE_HOME_DIR}."
    fi

    log_success "Gravhive uninstalled successfully."
    exit 0
}

# --- Main Flow ---
main() {
    if [ "${1:-}" = "--uninstall" ] || [ "${1:-}" = "-u" ] || [ "${1:-}" = "uninstall" ]; then
        uninstall
    fi

    print_banner
    check_root_or_sudo "${1:-}"
    check_environment
    check_cgroups_v2
    setup_directories
    build_or_copy_binaries
    setup_systemd_service
    verify_installation

    echo ""
    echo -e "${GREEN}${BOLD}================================================================${NC}"
    echo -e "${GREEN}${BOLD} Gravhive is ready to use!                                      ${NC}"
    echo -e "${GREEN}${BOLD}================================================================${NC}"
    echo -e "Quick Start Examples:"
    echo -e "  ${CYAN}ghive start app.js --name \"my-app\"${NC}          # Start Node.js app"
    echo -e "  ${CYAN}ghive start server.ts -i max${NC}                  # Start Bun/Deno cluster"
    echo -e "  ${CYAN}ghive list${NC}                                    # View running processes"
    echo -e "  ${CYAN}ghive monit${NC}                                   # Open full-screen TUI"
    echo -e "  ${CYAN}ghive logs <app> -f${NC}                            # Live stream logs"
    echo -e "  ${CYAN}ghive deploy production${NC}                        # Deploy to production"
    echo ""
}

main "$@"

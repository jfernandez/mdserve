#!/bin/bash
set -euo pipefail

# mdserve installer script
# Usage: curl -sSfL https://raw.githubusercontent.com/jfernandez/mdserve/main/install.sh | bash

# Repository information
REPO_OWNER="jfernandez"
REPO_NAME="mdserve"
BINARY_NAME="mdserve"

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# Cleanup function
cleanup() {
    if [ -n "${TEMP_FILE:-}" ] && [ -f "$TEMP_FILE" ]; then
        rm -f "$TEMP_FILE"
    fi
}

# Set trap for cleanup
trap cleanup EXIT

# Logging functions
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

fatal() {
    error "$1"
    exit 1
}

# Check if command exists
has_command() {
    command -v "$1" >/dev/null 2>&1
}

# Download function that tries curl first, then wget
download() {
    local url="$1"
    local output="$2"

    if has_command curl; then
        curl -sSfL "$url" -o "$output"
    elif has_command wget; then
        wget -q "$url" -O "$output"
    else
        fatal "Neither curl nor wget is available. Please install one of them."
    fi
}

# Get latest release tag from GitHub API
get_latest_release() {
    local api_url="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest"
    local response

    if has_command curl; then
        response=$(curl -sSf "$api_url")
    elif has_command wget; then
        response=$(wget -qO- "$api_url")
    else
        fatal "Neither curl nor wget is available. Please install one of them."
    fi

    # Extract tag_name from JSON response (simple grep/sed approach to avoid jq dependency)
    echo "$response" | grep '"tag_name":' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
}

# Detect platform and architecture
detect_platform() {
    local os arch

    os=$(uname -s)
    arch=$(uname -m)

    # Normalize OS
    case "$os" in
        Linux*) os="linux" ;;
        Darwin*) os="darwin" ;;
        CYGWIN*|MINGW*|MSYS*) fatal "Windows is not currently supported" ;;
        *) fatal "Unsupported operating system: $os" ;;
    esac

    # Normalize architecture
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) fatal "Unsupported architecture: $arch" ;;
    esac

    # Map to binary names used in releases
    case "$os-$arch" in
        linux-x86_64) echo "x86_64-unknown-linux-musl" ;;
        darwin-x86_64) echo "x86_64-apple-darwin" ;;
        darwin-aarch64) echo "aarch64-apple-darwin" ;;
        *) fatal "No binary available for $os-$arch" ;;
    esac
}

# Find the best installation directory
find_install_dir() {
    # Check for user override
    if [ -n "${MDSERVE_INSTALL_DIR:-}" ]; then
        echo "$MDSERVE_INSTALL_DIR"
        return
    fi

    # Try system-wide directory first (if we can write to it)
    if [ -w "/usr/local/bin" ] || [ "$EUID" = 0 ]; then
        echo "/usr/local/bin"
        return
    fi

    # Try user directories
    for dir in "$HOME/.local/bin" "$HOME/bin"; do
        if [ -d "$dir" ] && [ -w "$dir" ]; then
            echo "$dir"
            return
        fi
    done

    # Create ~/.local/bin if it doesn't exist (XDG standard)
    local local_bin="$HOME/.local/bin"
    if mkdir -p "$local_bin" 2>/dev/null; then
        echo "$local_bin"
        return
    fi

    # Final fallback
    local fallback_dir="$HOME/.mdserve/bin"
    mkdir -p "$fallback_dir"
    echo "$fallback_dir"
}

# Check if directory is in PATH
is_in_path() {
    local dir="$1"
    case ":$PATH:" in
        *":$dir:"*) return 0 ;;
        *) return 1 ;;
    esac
}

# Main installation function
install_mdserve() {
    info "Installing $BINARY_NAME..."

    # Detect platform
    info "Detecting platform..."
    local target
    target=$(detect_platform)
    info "Detected platform: $target"

    # Get latest release
    info "Fetching latest release information..."
    local version
    version=$(get_latest_release)
    if [ -z "$version" ]; then
        fatal "Failed to get latest release information"
    fi
    info "Latest release: $version"

    # Construct download URL
    local binary_name="${BINARY_NAME}-${target}"
    local download_url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${version}/${binary_name}"

    # Create temporary file
    TEMP_FILE=$(mktemp)

    # Download binary
    info "Downloading $binary_name..."
    if ! download "$download_url" "$TEMP_FILE"; then
        fatal "Failed to download binary from $download_url"
    fi

    # Find installation directory
    local install_dir
    install_dir=$(find_install_dir)
    info "Installing to: $install_dir"

    # Check if we need sudo for system directory
    local use_sudo=""
    if [ "$install_dir" = "/usr/local/bin" ] && [ "$EUID" != 0 ] && [ ! -w "$install_dir" ]; then
        if has_command sudo; then
            info "Administrator privileges required for system installation"
            use_sudo="sudo"
        else
            fatal "Cannot write to $install_dir and sudo is not available"
        fi
    fi

    # Install binary
    local install_path="$install_dir/$BINARY_NAME"
    if [ -n "$use_sudo" ]; then
        $use_sudo cp "$TEMP_FILE" "$install_path"
        $use_sudo chmod +x "$install_path"
    else
        cp "$TEMP_FILE" "$install_path"
        chmod +x "$install_path"
    fi

    # Verify installation
    if [ ! -x "$install_path" ]; then
        fatal "Installation failed: $install_path is not executable"
    fi

    # Test the binary
    if ! "$install_path" --version >/dev/null 2>&1; then
        warn "Binary installed but --version check failed. This might be normal if the binary doesn't support --version."
    fi

    success "$BINARY_NAME $version installed successfully to $install_path"

    # Check PATH
    if ! is_in_path "$install_dir"; then
        warn "⚠️  $install_dir is not in your PATH"
        info "Add it to your PATH by adding this line to your shell profile:"
        echo "    export PATH=\"$install_dir:\$PATH\""
        echo ""
        info "Or run the binary directly: $install_path"
    else
        info "✅ You can now run: $BINARY_NAME"
    fi
}

# Script entry point
main() {
    # Check for help flag
    for arg in "$@"; do
        case "$arg" in
            -h|--help)
                echo "mdserve installer"
                echo ""
                echo "Usage: $0 [options]"
                echo ""
                echo "Environment variables:"
                echo "  MDSERVE_INSTALL_DIR   Override installation directory"
                echo ""
                echo "Examples:"
                echo "  # Install to default location"
                echo "  curl -sSfL https://raw.githubusercontent.com/$REPO_OWNER/$REPO_NAME/main/install.sh | bash"
                echo ""
                echo "  # Install to custom directory"
                echo "  MDSERVE_INSTALL_DIR=~/my-tools curl -sSfL ... | bash"
                exit 0
                ;;
        esac
    done

    install_mdserve
}

# Run main function with all arguments
main "$@"
#!/bin/bash
# Tianyan Build Script for Linux/macOS
# Builds GUI and Tauri desktop application

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Output functions
info() { echo -e "${CYAN}[INFO]${NC} $1"; }
success() { echo -e "${GREEN}[OK]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Default parameters
DEV_MODE=false
SKIP_GUI=false
VERBOSE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --dev|-d)
            DEV_MODE=true
            shift
            ;;
        --skip-gui|-s)
            SKIP_GUI=true
            shift
            ;;
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  -d, --dev        Development mode (cargo tauri dev)"
            echo "  -s, --skip-gui   Skip GUI build"
            echo "  -v, --verbose    Verbose output"
            echo "  -h, --help       Show help"
            exit 0
            ;;
        *)
            error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Check dependencies
check_dependencies() {
    info "Checking dependencies..."

    # Check Rust/Cargo
    if ! command_exists cargo; then
        error "Cargo not found. Please install Rust: https://rustup.rs/"
        exit 1
    fi
    success "Cargo is installed"

    # Check trunk
    if ! command_exists trunk; then
        warn "Trunk not found. Installing..."
        cargo install trunk
        if [ $? -ne 0 ]; then
            error "Failed to install Trunk"
            exit 1
        fi
    fi
    success "Trunk is installed"

    # Check tauri-cli
    if ! cargo install --list | grep -q "tauri-cli"; then
        warn "tauri-cli not found. Installing..."
        cargo install tauri-cli
        if [ $? -ne 0 ]; then
            error "Failed to install tauri-cli"
            exit 1
        fi
    fi
    success "tauri-cli is installed"

    # Check Node.js
    if ! command_exists node; then
        warn "Node.js not found. Some features may not work"
    else
        success "Node.js is installed"
    fi
}

# Build GUI (Yew/WASM)
build_gui() {
    if [ "$SKIP_GUI" = true ]; then
        info "Skipping GUI build"
        return
    fi

    info "Building GUI (Yew/WASM)..."

    cd "$PROJECT_ROOT/gui"

    if [ "$VERBOSE" = true ]; then
        trunk build --release
    else
        trunk build --release 2>&1 | sed 's/^/  /'
    fi

    if [ $? -ne 0 ]; then
        error "GUI build failed"
        exit 1
    fi

    success "GUI build completed"
    info "Output directory: gui/dist/"
}

# Start development mode
start_dev() {
    info "Starting development mode..."
    info "This will start both GUI and Tauri dev servers"

    cd "$PROJECT_ROOT/tauri"
    cargo tauri dev
}

# Build Tauri application
build_tauri() {
    info "Building Tauri desktop app..."

    cd "$PROJECT_ROOT/tauri"

    if [ "$VERBOSE" = true ]; then
        cargo tauri build
    else
        cargo tauri build 2>&1 | sed 's/^/  /'
    fi

    if [ $? -ne 0 ]; then
        error "Tauri build failed"
        exit 1
    fi

    success "Tauri build completed"

    # Show output paths
    BUNDLE_DIR="$PROJECT_ROOT/tauri/target/release/bundle"
    if [ -d "$BUNDLE_DIR" ]; then
        info "Installer output directory:"
        find "$BUNDLE_DIR" -type f \( -name "*.msi" -o -name "*.deb" -o -name "*.rpm" -o -name "*.dmg" -o -name "*.AppImage" \) 2>/dev/null | while read -r file; do
            echo "  - $file"
        done
    fi
}

# Main function
main() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}  Tianyan Build Script${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""

    # Change to project root
    cd "$PROJECT_ROOT"

    # Check dependencies
    check_dependencies
    echo ""

    if [ "$DEV_MODE" = true ]; then
        # Development mode
        start_dev
    else
        # Build mode
        build_gui
        echo ""
        build_tauri
        echo ""
        success "Build completed!"
    fi
}

# Execute main function
main

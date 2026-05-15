# Tianyan Build Script for Windows PowerShell
# Builds GUI and Tauri desktop application

param(
    [switch]$Dev = $false,
    [switch]$SkipGui = $false,
    [switch]$Verbose = $false
)

$ErrorActionPreference = "Stop"

# Color output functions
function Write-Info($msg) { Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Success($msg) { Write-Host "[OK] $msg" -ForegroundColor Green }
function Write-Warn($msg) { Write-Host "[WARN] $msg" -ForegroundColor Yellow }
function Write-Err($msg) { Write-Host "[ERROR] $msg" -ForegroundColor Red }

# Check if command exists
function Test-Command($cmd) {
    $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue)
}

# Check dependencies
function Check-Dependencies {
    Write-Info "Checking dependencies..."

    # Check Rust/Cargo
    if (-not (Test-Command "cargo")) {
        Write-Err "Cargo not found. Please install Rust: https://rustup.rs/"
        exit 1
    }
    Write-Success "Cargo is installed"

    # Check trunk
    if (-not (Test-Command "trunk")) {
        Write-Warn "Trunk not found. Installing..."
        cargo install trunk
        if ($LASTEXITCODE -ne 0) {
            Write-Err "Failed to install Trunk"
            exit 1
        }
    }
    Write-Success "Trunk is installed"

    # Check tauri-cli
    $tauriCli = cargo install --list | Select-String "tauri-cli"
    if (-not $tauriCli) {
        Write-Warn "tauri-cli not found. Installing..."
        cargo install tauri-cli
        if ($LASTEXITCODE -ne 0) {
            Write-Err "Failed to install tauri-cli"
            exit 1
        }
    }
    Write-Success "tauri-cli is installed"

    # Check Node.js
    if (-not (Test-Command "node")) {
        Write-Warn "Node.js not found. Some features may not work"
    } else {
        Write-Success "Node.js is installed"
    }
}

# Build GUI (Yew/WASM)
function Build-Gui {
    if ($SkipGui) {
        Write-Info "Skipping GUI build"
        return
    }

    Write-Info "Building GUI (Yew/WASM)..."

    Push-Location "$PSScriptRoot\..\gui"
    try {
        $buildOutput = trunk build --release 2>&1
        $exitCode = $LASTEXITCODE

        if ($Verbose) {
            $buildOutput | ForEach-Object { "  $_" }
        } else {
            $buildOutput | ForEach-Object { "  $_" }
        }

        if ($exitCode -ne 0) {
            Write-Err "GUI build failed"
            exit 1
        }

        Write-Success "GUI build completed"
        Write-Info "Output directory: gui/dist/"
    } finally {
        Pop-Location
    }
}

# Start development mode
function Start-Dev {
    Write-Info "Starting development mode..."
    Write-Info "This will start both GUI and Tauri dev servers"

    Push-Location "$PSScriptRoot\..\tauri"
    try {
        cargo tauri dev
    } finally {
        Pop-Location
    }
}

# Build Tauri application
function Build-Tauri {
    Write-Info "Building Tauri desktop app..."

    Push-Location "$PSScriptRoot\..\tauri"
    try {
        $buildOutput = cargo tauri build 2>&1
        $exitCode = $LASTEXITCODE

        $buildOutput | ForEach-Object { "  $_" }

        if ($exitCode -ne 0) {
            Write-Err "Tauri build failed"
            exit 1
        }

        Write-Success "Tauri build completed"

        # Show output paths
        $bundleDir = "$PSScriptRoot\..\tauri\target\release\bundle"
        if (Test-Path $bundleDir) {
            Write-Info "Installer output directory:"
            Get-ChildItem -Path $bundleDir -Recurse -Include "*.msi","*.exe","*.msi.zip" | ForEach-Object {
                Write-Host "  - $($_.FullName)" -ForegroundColor Gray
            }
        }
    } finally {
        Pop-Location
    }
}

# Main function
function Main {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Blue
    Write-Host "  Tianyan Build Script" -ForegroundColor Blue
    Write-Host "========================================" -ForegroundColor Blue
    Write-Host ""

    # Change to project root
    Set-Location "$PSScriptRoot\.."

    # Check dependencies
    Check-Dependencies
    Write-Host ""

    if ($Dev) {
        # Development mode
        Start-Dev
    } else {
        # Build mode
        Build-Gui
        Write-Host ""
        Build-Tauri
        Write-Host ""
        Write-Success "Build completed!"
    }
}

# Execute main function
Main

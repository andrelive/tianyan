# Tianyan Build Script for Windows PowerShell
# Builds GUI and Tauri desktop application

param(
    [switch]$Dev = $false,
    [switch]$SkipGui = $false,
    [switch]$Verbose = $false
)

# native 命令的 stderr 不是错误（PowerShell 5.1 会把 cargo 的 warning 当
# NativeCommandError 抛出 → 打包脚本误判失败）；成败以 `$LASTEXITCODE` 为准。
$ErrorActionPreference = "Continue"

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

    # Check Node.js (required for React frontend build)
    if (-not (Test-Command "node")) {
        Write-Err "Node.js not found. Please install Node.js 18+: https://nodejs.org/"
        exit 1
    }
    Write-Success "Node.js is installed"

    # Check npm
    if (-not (Test-Command "npm")) {
        Write-Err "npm not found. Please install Node.js (includes npm)"
        exit 1
    }
    Write-Success "npm is installed"

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

    # Check protoc（lance-encoding 的构建依赖；CI 用 arduino/setup-protoc 安装，
    # 本地脚本此前没有该检查 → 冷编译 lancedb 时可能莫名失败）
    if (Test-Command "protoc") {
        Write-Success "protoc is installed"
    } else {
        Write-Warn "protoc not found（lancedb/lance-encoding 冷编译需要）。已有构建缓存时可继续；从零编译请先安装 protoc"
    }
}

# Build GUI (React/TypeScript)
function Build-Gui {
    if ($SkipGui) {
        Write-Info "Skipping GUI build"
        return
    }

    Write-Info "Building GUI (React/TypeScript)..."

    Push-Location "$PSScriptRoot\..\gui-vite"
    try {
        Write-Info "Installing npm dependencies..."
        $installOutput = npm install 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Err "npm install failed"
            if ($Verbose) { $installOutput | ForEach-Object { "  $_" } }
            exit 1
        }

        Write-Info "Building React frontend..."
        $buildOutput = npm run build 2>&1
        $exitCode = $LASTEXITCODE

        if ($Verbose) {
            $buildOutput | ForEach-Object { "  $_" }
        }

        if ($exitCode -ne 0) {
            Write-Err "GUI build failed"
            exit 1
        }

        Write-Success "GUI build completed"
        Write-Info "Output directory: gui-vite/dist/"
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
        # createUpdaterArtifacts=true 时 Tauri 要求签名密钥；本地未设密钥则
        # 覆盖为 false（本地构建不需要 .sig——发布签名由 CI 承担，
        # TAURI_SIGNING_PRIVATE_KEY 在 release.yml 的 secrets 注入）。
        $tauriArgs = @('tauri', 'build')
        if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and -not $env:TAURI_SIGNING_PRIVATE_KEY_PATH) {
            # PowerShell 5.1 向 native 程序传参时内联 JSON 的双引号会被剥离
            # （tauri-cli 收到无引号 JSON → "key must be a string" 解析失败）。
            # 改经临时文件传 --config（tauri-cli 支持 JSON 文件路径），
            # 绕过 PowerShell 的引号序列化差异。
            $overrideFile = Join-Path $env:TEMP "tianyan-tauri-local-override.json"
            '{"bundle":{"createUpdaterArtifacts":false}}' | Set-Content -Path $overrideFile -Encoding ASCII
            $tauriArgs += @('--config', $overrideFile)
            Write-Info "未检测到签名密钥：本次构建不产出 updater 签名（.sig）"
        }
        $buildOutput = cargo @tauriArgs 2>&1
        $exitCode = $LASTEXITCODE

        $buildOutput | ForEach-Object { "  $_" }

        if ($exitCode -ne 0) {
            Write-Err "Tauri build failed"
            exit 1
        }

        Write-Success "Tauri build completed"

        # Show output paths
        # 产物在 workspace 共享 target 下（<仓库根>\target\release\bundle），
        # 而非 <tauri>\target —— 此前路径写错，脚本总是列不出产物。
        $bundleDir = "$PSScriptRoot\..\target\release\bundle"
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
        Write-Info "Note: tauri build 的 beforeBuildCommand 会再执行一次前端构建（重复构建；只想构建一次可用 -SkipGui）"
        Build-Tauri
        Write-Host ""
        Write-Success "Build completed!"
    }
}

# Execute main function
Main

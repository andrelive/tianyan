# 天演 (Tianyan) 项目测试自动化脚本
# 用法: .\scripts\test.ps1 [level]
#   level: lint | unit | integration | e2e | gui-build | gui-e2e | bench | all (默认)
#
# G1：前端门禁并入统一入口——lint 含前端 eslint+prettier+typecheck，
# unit 含前端 vitest，all 含前端生产构建（此前 test.ps1 只跑 Rust，
# 前端回归不会被统一门禁捕获）。

param(
    [string]$Level = "all"
)

$ErrorActionPreference = "Stop"

Write-Host "========== 天演自动化测试 ==========" -ForegroundColor Cyan
Write-Host "测试层级: $Level" -ForegroundColor Yellow

function Test-Lint {
    Write-Host ">>>>> 0. 静态检查 (Lint)" -ForegroundColor Green
    Write-Host "  - Rust 格式化检查..."
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw "fmt 检查失败" }

    Write-Host "  - Rust Clippy 检查..."
    cargo clippy --workspace -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "clippy 检查失败" }

    Write-Host "  - 工具目录 freshness 校验 (A3)..."
    & "$PSScriptRoot\gen-tool-catalog.ps1" -Check
    if ($LASTEXITCODE -ne 0) { throw "工具目录已漂移，请运行 scripts/gen-tool-catalog.ps1" }

    Write-Host "  - 前端 ESLint + Prettier..."
    Push-Location gui-vite
    try {
        npm run lint
        if ($LASTEXITCODE -ne 0) { throw "前端 lint 检查失败" }
    } finally {
        Pop-Location
    }

    Write-Host "  - 前端 TypeScript 类型检查..."
    Push-Location gui-vite
    try {
        npm run typecheck
        if ($LASTEXITCODE -ne 0) { throw "前端 typecheck 失败" }
    } finally {
        Pop-Location
    }
    Write-Host "静态检查通过" -ForegroundColor Green
}

function Test-Unit {
    Write-Host ">>>>> 1. 单元测试 (Unit Tests)" -ForegroundColor Green
    Write-Host "  - Rust 单元测试..."
    cargo test --workspace --lib -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Rust 单元测试失败" }

    Write-Host "  - 前端 Vitest..."
    Push-Location gui-vite
    try {
        npx vitest run
        if ($LASTEXITCODE -ne 0) { throw "前端单元测试失败" }
    } finally {
        Pop-Location
    }
    Write-Host "单元测试通过" -ForegroundColor Green
}

function Test-Integration {
    Write-Host ">>>>> 2. 集成测试 (Integration Tests)" -ForegroundColor Green
    cargo test --workspace --test '*' -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "集成测试失败" }
    Write-Host "集成测试通过" -ForegroundColor Green
}

function Test-E2E {
    Write-Host ">>>>> 3. Rust E2E 测试 (HTTP 级)" -ForegroundColor Green
    cargo test --workspace --test e2e_tests -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "E2E 测试失败" }
    Write-Host "Rust E2E 测试通过" -ForegroundColor Green
}

function Test-GuiBuild {
    Write-Host ">>>>> 3b. 前端生产构建 (Vite Build)" -ForegroundColor Green
    Push-Location gui-vite
    try {
        npm run build
        if ($LASTEXITCODE -ne 0) { throw "前端构建失败" }
    } finally {
        Pop-Location
    }
    Write-Host "前端构建通过" -ForegroundColor Green
}

function Test-GuiE2E {
    Write-Host ">>>>> 4. GUI E2E 测试 (Playwright, 真实后端)" -ForegroundColor Green
    # 预编译后端，避免 playwright webServer 首次编译超时
    cargo build -p tianyan-server
    if ($LASTEXITCODE -ne 0) { throw "后端预编译失败" }

    # 端口占用预检：8765 (mock-llm) / 3000 (后端) / 5100 (vite)
    foreach ($port in 8765, 3000, 5100) {
        if (Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue) {
            throw "端口 $port 已被占用。请先停止占用该端口的进程（如开发中的 tianyan server / vite dev / mock-llm）再运行 GUI E2E。"
        }
    }

    Push-Location gui-vite
    try {
        # CI=1 强制 reuseExistingServer=false + retries=2 + workers=1，保证确定性
        $env:CI = "1"
        npx playwright test
        if ($LASTEXITCODE -ne 0) { throw "GUI E2E 测试失败" }
    } finally {
        Remove-Item Env:CI -ErrorAction SilentlyContinue
        Pop-Location
    }
    Write-Host "GUI E2E 测试通过" -ForegroundColor Green
}

function Test-Benchmark {
    Write-Host ">>>>> 5. 性能基准 (Benchmarks)" -ForegroundColor Green
    # 注意：不要传 -- --verbose —— bench harness 会拒绝该参数
    cargo bench -p tianyan-server
    if ($LASTEXITCODE -ne 0) { throw "基准测试失败" }
    Write-Host "基准测试完成" -ForegroundColor Green
}

function Test-All {
    Test-Lint
    Test-Unit
    Test-Integration
    Test-E2E
    Test-GuiBuild
    Test-GuiE2E
    Test-Benchmark
}

switch ($Level) {
    "lint"        { Test-Lint }
    "unit"        { Test-Unit }
    "integration"  { Test-Integration }
    "e2e"         { Test-E2E }
    "gui-build"   { Test-GuiBuild }
    "gui-e2e"     { Test-GuiE2E }
    "bench"       { Test-Benchmark }
    "all"         { Test-All }
    default       {
        Write-Host "未知层级 '$Level'。可选: lint, unit, integration, e2e, gui-build, gui-e2e, bench, all" -ForegroundColor Red
        exit 1
    }
}

Write-Host "========== 测试全部通过 ==========" -ForegroundColor Cyan

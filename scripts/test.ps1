# 天演 (Tianyan) 项目测试自动化脚本
# 用法: .\scripts\test.ps1 [level]
#   level: unit | integration | e2e | bench | all (默认)

param(
    [string]$Level = "all"
)

$ErrorActionPreference = "Stop"

Write-Host "========== 天演自动化测试 ==========" -ForegroundColor Cyan
Write-Host "测试层级: $Level" -ForegroundColor Yellow

function Test-Unit {
    Write-Host ">>>>> 1. 单元测试 (Unit Tests)" -ForegroundColor Green
    cargo test --workspace --lib -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "单元测试失败" }
    Write-Host "单元测试通过" -ForegroundColor Green
}

function Test-Integration {
    Write-Host ">>>>> 2. 集成测试 (Integration Tests)" -ForegroundColor Green
    cargo test --workspace --test '*' -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "集成测试失败" }
    Write-Host "集成测试通过" -ForegroundColor Green
}

function Test-E2E {
    Write-Host ">>>>> 3. E2E 测试" -ForegroundColor Green
    cargo test --workspace --test e2e_tests -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "E2E 测试失败" }
    Write-Host "E2E 测试通过" -ForegroundColor Green
}

function Test-Benchmark {
    Write-Host ">>>>> 4. 性能基准 (Benchmarks)" -ForegroundColor Green
    cargo bench -p tianyan-server -- --verbose
    if ($LASTEXITCODE -ne 0) { throw "基准测试失败" }
    Write-Host "基准测试完成" -ForegroundColor Green
}

function Test-Lint {
    Write-Host ">>>>> 0. 静态检查 (Lint)" -ForegroundColor Green
    Write-Host "  - 格式化检查..."
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw "fmt 检查失败" }

    Write-Host "  - Clippy 检查..."
    cargo clippy --workspace -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "clippy 检查失败" }
    Write-Host "静态检查通过" -ForegroundColor Green
}

function Test-All {
    Test-Lint
    Test-Unit
    Test-Integration
    Test-E2E
    Test-Benchmark
}

switch ($Level) {
    "lint"        { Test-Lint }
    "unit"        { Test-Unit }
    "integration"  { Test-Integration }
    "e2e"         { Test-E2E }
    "bench"       { Test-Benchmark }
    "all"         { Test-All }
    default       {
        Write-Host "未知层级 '$Level'。可选: lint, unit, integration, e2e, bench, all" -ForegroundColor Red
        exit 1
    }
}

Write-Host "========== 测试全部通过 ==========" -ForegroundColor Cyan

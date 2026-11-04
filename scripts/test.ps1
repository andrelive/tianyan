# 天演 (Tianyan) 项目测试自动化脚本
# 用法: .\scripts\test.ps1 [level]
#   level: lint | unit | integration | e2e | bench | all (默认)
#
# 前端（gui-vite）已于 2026-11 退役：本脚本不再跑 eslint / typecheck / vitest /
# playwright（Web 端不再是产品前端，CI 也已摘除，见 quality.yml 末尾注释）。
# 需要时进 `gui-vite/` 手工运行（`npm run lint` / `npm run dev` / `npx playwright test`）。

param(
    [string]$Level = "all"
)

# native 命令的 stderr 不是错误：PowerShell 5.1 在 Stop 模式下会把 cargo 的
# warning/进度输出（写 stderr）当成 NativeCommandError 抛出，脚本被**误判失败**
# （实测：cargo check 被标记 failed，实际已 Finished）。关键步骤一律用显式
# `$LASTEXITCODE` 检查（见下文每步 throw），故这里用 Continue。
$ErrorActionPreference = "Continue"

Write-Host "========== 天演自动化测试 ==========" -ForegroundColor Cyan
Write-Host "测试层级: $Level" -ForegroundColor Yellow

function Test-Lint {
    Write-Host ">>>>> 0. 静态检查 (Lint)" -ForegroundColor Green
    Write-Host "  - Rust 格式化检查..."
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw "fmt 检查失败" }

    # gui-native 是**独立 workspace**（ADR-049 后自带 `[workspace]`，不进主 workspace），
    # 上面的 `cargo fmt --check` / `cargo clippy` 都覆盖不到它。实测漏过一次：
    # 54b04c6/5d08ab1 两次改动留下 3 文件 13 行格式漂移，本地门禁全绿而 `cargo fmt`
    # 一跑就变（只在下次谁顺手格式化时才发现）。同理 clippy 也漏了 4 处既存告警
    # （`explicit_auto_deref` 3 处 + 文档列表续行 1 处），攒到出包前才撞见。
    # 代价：gui-native 冷编译一次很慢（GPUI 全链），但增量只有几秒；CI 不跑本脚本。
    Write-Host "  - gui-native 格式化检查（独立 workspace，上面覆盖不到）..."
    Push-Location gui-native
    try {
        cargo fmt -- --check
        if ($LASTEXITCODE -ne 0) { throw "gui-native fmt 检查失败（进 gui-native 目录跑 cargo fmt 后重试）" }
        Write-Host "  - gui-native Clippy 检查..."
        cargo clippy -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw "gui-native clippy 检查失败" }
    } finally {
        Pop-Location
    }

    Write-Host "  - Rust Clippy 检查（--all-targets：与 CI 同范围，避免「本地绿 / CI 红」）..."
    cargo clippy --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "clippy 检查失败" }

    Write-Host "  - 工具目录 freshness 校验 (A3)..."
    & "$PSScriptRoot\gen-tool-catalog.ps1" -Check
    if ($LASTEXITCODE -ne 0) { throw "工具目录已漂移，请运行 scripts/gen-tool-catalog.ps1" }
    # 产品版本多处副本的钉死点：主 workspace / gui-native / MSI 清单。
    # 实测漂过一次（主 workspace 1.0.1 与 GUI 1.1.0 并存 →「关于」页与 /health 各报一个号）。
    Write-Host "  - 产品版本一致性校验..."
    & "$PSScriptRoot\check-version.ps1"
    if ($LASTEXITCODE -ne 0) { throw "版本不一致，请按根 Cargo.toml 的注释同步各处版本" }


    Write-Host "静态检查通过" -ForegroundColor Green
}

function Test-Unit {
    Write-Host ">>>>> 1. 单元测试 (Unit Tests)" -ForegroundColor Green
    Write-Host "  - Rust 单元测试..."
    cargo test --workspace --lib -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Rust 单元测试失败" }

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

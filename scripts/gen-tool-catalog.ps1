# 天演工具目录生成器（A3：DSH gen-tool-catalog 纪律吸收）
#
# 用法:
#   .\scripts\gen-tool-catalog.ps1            # 生成 docs/architecture/tool-catalog.md
#   .\scripts\gen-tool-catalog.ps1 -Check     # freshness 校验（CI 门禁，退出码 1 表示漂移）
#
# 命令本体来自仓库 `.cargo/config.toml` 的 alias（`catalog-check` /
# `catalog-gen`）——**命令单一来源**：CI（`.github/workflows/quality.yml`）、
# 本脚本、`scripts/wsl-ci-parity.sh` 一律调用别名，换生成器位置只改
# `.cargo/config.toml` 一处。（0.5.9 教训：生成器由 core 迁到 server 侧时，
# CI 里内联的旧命令未同步 → 门禁必红 → tag 推送后不出包。）

param(
    [switch]$Check
)

# native 命令的 stderr 不是错误（PowerShell 5.1 会把 cargo 的 warning 当
# NativeCommandError 抛出 → `-Check` 直接崩溃而非报告漂移）；成败以
# `$LASTEXITCODE` 为准。
$ErrorActionPreference = "Continue"

# 从仓库根目录解析（脚本可能在任意 cwd 下被调用）
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

# 分段 Join-Path（不写 "docs\..."）：CI 在 Linux 跑 pwsh，反斜杠不是分隔符，
# 单段拼接会得到 `docs\architecture\...` 这样的坏路径（-Check 分支不用它，
# 但生成分支要跨平台可用）。
$catalogPath = Join-Path (Join-Path (Join-Path $repoRoot "docs") "architecture") "tool-catalog.md"

if ($Check) {
    Write-Host ">>>>> 工具目录 freshness 校验..."
    # 别名定义在 `.cargo/config.toml`：权威生成器在 server 侧（只有它能同时
    # 装配 core 内置工具与组件工具 todo/goal/schedule_task），覆盖全部对模型
    # 可见的工具。
    cargo catalog-check
    if ($LASTEXITCODE -ne 0) {
        Write-Host "工具目录已漂移！请运行 .\scripts\gen-tool-catalog.ps1 重新生成" -ForegroundColor Red
        exit 1
    }
    Write-Host "工具目录与代码一致" -ForegroundColor Green
} else {
    Write-Host ">>>>> 生成工具目录..."
    # 写入必须 UTF-8 **无 BOM**：PowerShell 5.1 的 `Set-Content -Encoding UTF8`
    # 会写 BOM，与 example 输出（无 BOM）不一致 → `-Check` 恒报漂移
    # （0.3.15 排查）；行尾保持 LF（example 比较时会归一化 CRLF）。
    $generated = cargo catalog-gen
    if ($LASTEXITCODE -ne 0) { exit 1 }
    $text = ($generated -join "`n") + "`n"
    [System.IO.File]::WriteAllText(
        $catalogPath,
        $text,
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "已生成 $catalogPath" -ForegroundColor Green
}
# 天演工具目录生成器（A3：DSH gen-tool-catalog 纪律吸收）
#
# 用法:
#   .\scripts\gen-tool-catalog.ps1            # 生成 docs/architecture/tool-catalog.md
#   .\scripts\gen-tool-catalog.ps1 -Check     # freshness 校验（CI 门禁，退出码 1 表示漂移）

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

$catalogPath = Join-Path $repoRoot "docs\architecture\tool-catalog.md"

if ($Check) {
    Write-Host ">>>>> 工具目录 freshness 校验..."
    cargo run -q -p tianyan-core --example tool_catalog -- --check
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
    $generated = cargo run -q -p tianyan-core --example tool_catalog
    if ($LASTEXITCODE -ne 0) { exit 1 }
    $text = ($generated -join "`n") + "`n"
    [System.IO.File]::WriteAllText(
        $catalogPath,
        $text,
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "已生成 $catalogPath" -ForegroundColor Green
}
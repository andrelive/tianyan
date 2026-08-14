# 天演工具目录生成器（A3：DSH gen-tool-catalog 纪律吸收）
#
# 用法:
#   .\scripts\gen-tool-catalog.ps1            # 生成 docs/architecture/tool-catalog.md
#   .\scripts\gen-tool-catalog.ps1 -Check     # freshness 校验（CI 门禁，退出码 1 表示漂移）

param(
    [switch]$Check
)

$ErrorActionPreference = "Stop"

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
    cargo run -q -p tianyan-core --example tool_catalog | Set-Content -Path $catalogPath -Encoding UTF8
    if ($LASTEXITCODE -ne 0) { exit 1 }
    Write-Host "已生成 $catalogPath" -ForegroundColor Green
}
# 重编带度量（ADR-049 期 0 交付物）
#
# 用途：测量「改一行代码 → 重编多少源码行 / 花多少秒」，是插件化收益的唯一客观判据。
# 原理：只 touch 目标文件（改 mtime、不改内容）→ 触发 rustc 重编 → 解析 cargo 的
#       "Compiling <crate>" 列表 → 按 crate 源码行数求和 = 重编行数。
#
# 用法：
#   .\scripts\measure-rebuild.ps1                                   # 三场景基线（build）
#   .\scripts\measure-rebuild.ps1 -Incremental                      # 开增量编译对比
#   .\scripts\measure-rebuild.ps1 -Command check -Targets core/src/executor/web.rs
#
# 判据（ADR-049）：
#   现状基线 = core+server+mcp+tauri 全量（源码口径 56,541 行）
#   闸门 A  = 改一行工具插件 ≤ 3,000 行源码（前提：装配器已二分）

param(
    [ValidateSet('check', 'build')][string]$Command = 'build',
    [string[]]$Targets = @(
        'core/src/executor/web.rs',        # 场景 1：改一个工具
        'core/src/agent/loop.rs',          # 场景 2：改内核机制
        'core/src/common/types/uri.rs'     # 场景 3：改 S 级契约
    ),
    [switch]$Incremental,
    [switch]$SkipWarm
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# ---- crate 源码行数表（排除测试：cargo build 不编译 #[cfg(test)] 与 *_tests.rs）----
function Get-SourceLines {
    param([string]$Dir)
    if (-not (Test-Path $Dir)) { return 0 }
    $all = Get-ChildItem $Dir -Recurse -Filter *.rs -File
    $testFiles = $all | Where-Object {
        $_.Name -like '*_tests.rs' -or $_.Name -eq 'tests.rs' -or $_.FullName -match '\\tests\\' -or $_.FullName -match 'test_utils'
    }
    $sum = 0
    foreach ($f in ($all | Where-Object { $testFiles.FullName -notcontains $_.FullName })) {
        $txt = Get-Content -Raw $f.FullName
        # 只把“#[cfg(test)] 后跟 mod”当作测试模块起点（Rust 惯例：测试模块在文件末尾）；
        # 仅修饰单个 helper 的 #[cfg(test)] 不应把其后代码全算成测试。
        $m = [regex]::Match($txt, '(?m)^#\[cfg\(test\)\]\s*\r?\n\s*(pub\s+)?mod\s')
        $total = ($txt -split "`n").Count
        if ($m.Success) {
            # 内联测试段：只计 #[cfg(test)] 之前的行
            $before = ($txt.Substring(0, $m.Index) -split "`n").Count
            $sum += $before
        } else {
            $sum += $total
        }
    }
    return $sum
}

$crateLines = [ordered]@{
    'tianyan-core'   = Get-SourceLines 'core'
    'tianyan-server' = Get-SourceLines 'server'
    'tianyan-mcp'    = Get-SourceLines 'mcp'
    'tianyan-tauri'  = Get-SourceLines 'tauri'
}
$totalSrc = ($crateLines.Values | Measure-Object -Sum).Sum

Write-Host "== 重编带度量（ADR-049）==" -ForegroundColor Cyan
Write-Host ("命令     : cargo {0} --workspace{1}" -f $Command, $(if ($Incremental) { '  [CARGO_INCREMENTAL=1]' } else { '' }))
Write-Host ("源码行数 : " + (($crateLines.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '  ') + "  |  合计 $totalSrc")
Write-Host ("场景     : " + ($Targets -join ' , '))

if ($Incremental) { $env:CARGO_INCREMENTAL = '1' } else { $env:CARGO_INCREMENTAL = '0' }

# ---- 预热到 no-op ----
if (-not $SkipWarm) {
    Write-Host "`n[预热] 循环至 no-op（最多 3 轮）..." -ForegroundColor Yellow
    for ($i = 1; $i -le 3; $i++) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $out = & cargo $Command --workspace 2>&1
        $sw.Stop()
        $c = @($out | Select-String -Pattern '^\s+Compiling\s')
        Write-Host ("  第 {0} 轮 {1:N1}s，Compiling 条目 {2}" -f $i, $sw.Elapsed.TotalSeconds, $c.Count)
        if ($c.Count -eq 0) { break }
    }
}

# ---- 逐场景测量 ----
$rows = @()
foreach ($t in $Targets) {
    if (-not (Test-Path $t)) { Write-Warning "跳过（文件不存在）: $t"; continue }
    Write-Host "`n[测量] $t" -ForegroundColor Yellow
    (Get-Item $t).LastWriteTime = Get-Date

    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = & cargo $Command --workspace 2>&1
    $sw.Stop()

    $compiling = @($out | Select-String -Pattern '^\s+Compiling\s+(\S+)\s' | ForEach-Object { $_.Matches[0].Groups[1].Value })
    $rebuilt = @()
    $lines = 0
    foreach ($c in $crateLines.Keys) {
        if ($compiling -contains $c) { $rebuilt += $c; $lines += $crateLines[$c] }
    }

    $rows += [pscustomobject]@{
        Target        = $t
        Seconds       = [math]::Round($sw.Elapsed.TotalSeconds, 2)
        Crates        = ($rebuilt -join ',')
        RebuiltLines  = $lines
        OfTotal       = if ($totalSrc) { "{0:N1}%" -f (100 * $lines / $totalSrc) } else { 'n/a' }
    }
}

Write-Host "`n== 汇总 ==" -ForegroundColor Green
$rows | Format-Table -AutoSize

$incDir = 'target/debug/incremental'
if (Test-Path $incDir) {
    $sizeMb = [math]::Round(((Get-ChildItem $incDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB), 1)
    $sessions = (Get-ChildItem $incDir -Directory -ErrorAction SilentlyContinue).Count
    Write-Host ("增量目录 : {0} 会话 / {1} MB" -f $sessions, $sizeMb)
}

Write-Host "`nJSON:"
$rows | ConvertTo-Json -Compress

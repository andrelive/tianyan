# 产品版本一致性校验（`scripts/test.ps1 lint` 门禁的一步）
#
# 为什么需要它：天演有两个 workspace —— 主 workspace（core/server/plugins/tauri，
# 版本继承 `[workspace.package]`）与 `gui-native/`（独立 workspace，无法继承）。
# 于是「产品版本」天然存在**多处副本**，实测已经漂过一次：
#   主 workspace = 1.0.1（`/health` 报的是它）  而 gui-native = 1.1.0（exe 属性）
#   → 「关于」页显示 1.1.0、`/health` 报 1.0.1，同一个产品两个号。
#
# 本脚本只做一件事：把这些副本钉死。任何一处不一致，lint 直接红。
#
# 版本单源 = 根 `Cargo.toml` 的 `[workspace.package] version`（3 段，如 1.1.0）。
# - MSI 的 4 段 ProductVersion 由 `gui-native/installer/build-msi.ps1` 补 `.0` 注入，
#   `tianyan.wxs` 里**不得**出现硬编码版本（本脚本会检查）。
# - `gui-vite/package.json` 的 version 是 npm 包元数据、不参与产品发布，**不纳入**校验。

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path $PSScriptRoot -Parent
$fail = 0

function Get-TomlSectionVersion {
    param([string]$Path, [string]$Section)
    if (-not (Test-Path $Path)) { return $null }
    $inSection = $false
    foreach ($line in Get-Content $Path -Encoding utf8) {
        $t = $line.Trim()
        if ($t -match '^\[') { $inSection = ($t -eq "[$Section]"); continue }
        if ($inSection -and $t -match '^version\s*=\s*"([^"]+)"') { return $matches[1] }
    }
    return $null
}

function Report {
    param([string]$Name, [string]$Value, [string]$Expect, [string]$Note)
    $ok = ($Value -eq $Expect)
    if (-not $ok) { $script:fail++ }
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    $color = if ($ok) { 'DarkGreen' } else { 'Red' }
    $line = "  [$mark] $Name = '$Value'"
    if (-not $ok) { $line += "  期望 '$Expect'" }
    if ($Note) { $line += "  $Note" }
    Write-Host $line -ForegroundColor $color
}

Write-Host "版本一致性校验（单源 = 根 Cargo.toml [workspace.package] version）" -ForegroundColor Cyan

$rootVer = Get-TomlSectionVersion -Path (Join-Path $repoRoot 'Cargo.toml') -Section 'workspace.package'
if (-not $rootVer) { throw "读不到根 Cargo.toml 的 [workspace.package] version" }
Write-Host "  单源版本：$rootVer`n" -ForegroundColor DarkGray

# 1. 主 workspace（单源本身，只校验格式）
if ($rootVer -notmatch '^\d+\.\d+\.\d+$') {
    Write-Host "  [FAIL] 根 Cargo.toml 版本格式应为 x.y.z，实际 '$rootVer'" -ForegroundColor Red
    $fail++
}

# 2. gui-native（独立 workspace，唯一必须手改的副本）
$guiVer = Get-TomlSectionVersion -Path (Join-Path $repoRoot 'gui-native/Cargo.toml') -Section 'package'
Report -Name 'gui-native/Cargo.toml' -Value $guiVer -Expect $rootVer -Note '（决定 exe 的 FileVersion/ProductVersion）'

# 3. tauri 壳（退役后目录消失 → 自动跳过）
$tauriConf = Join-Path $repoRoot 'tauri/tauri.conf.json'
if (Test-Path $tauriConf) {
    $tauriVer = $null
    try {
        $tauriVer = (Get-Content $tauriConf -Raw -Encoding utf8 | ConvertFrom-Json).version
    }
    catch {
        Write-Host "  [FAIL] 解析 tauri/tauri.conf.json 失败：$($_.Exception.Message)" -ForegroundColor Red
        $fail++
    }
    if ($tauriVer) {
        # tauri 的 version 允许 3 段；若写成 4 段（x.y.z.w）只比前三段
        $tauriVer3 = ($tauriVer -split '\.' | Select-Object -First 3) -join '.'
        Report -Name 'tauri/tauri.conf.json' -Value $tauriVer3 -Expect $rootVer -Note '（tauri 壳退役后本行自动消失）'
    }
}
else {
    Write-Host "  [SKIP] tauri/ 不存在（已退役）" -ForegroundColor DarkGray
}

# 4. MSI 清单不得硬编码版本（必须由 build-msi.ps1 注入）
$wxs = Join-Path $repoRoot 'gui-native/installer/tianyan.wxs'
if (-not (Test-Path $wxs)) {
    Write-Host "  [FAIL] 找不到 $wxs" -ForegroundColor Red
    $fail++
}
else {
    $wxsText = Get-Content $wxs -Raw -Encoding utf8
    $hasPlaceholder = $wxsText -match 'Version="\$\(var\.ProductVersion\)"'
    if ($hasPlaceholder) {
        Write-Host "  [PASS] tianyan.wxs 的 Package/@Version = `$(var.ProductVersion)（注入，无硬编码）" -ForegroundColor DarkGreen
    }
    else {
        Write-Host "  [FAIL] tianyan.wxs 的 Package/@Version 必须是 `$(var.ProductVersion)，不得硬编码版本号" -ForegroundColor Red
        $fail++
    }
    # ARP 显示的是 DisplayVersion（产品版本，3 段），也必须注入 —— 否则「设置→应用」
    # 会退回 ProductVersion 的 4 段形式（1.1.0.0），与「关于」页/`/health` 不一致。
    if ($wxsText -match 'Id="DisplayVersion"\s+Value="\$\(var\.DisplayVersion\)"') {
        Write-Host "  [PASS] tianyan.wxs 的 DisplayVersion = `$(var.DisplayVersion)（ARP 显示 3 段产品版本）" -ForegroundColor DarkGreen
    }
    else {
        Write-Host "  [FAIL] tianyan.wxs 必须显式设置 <Property Id=`"DisplayVersion`" Value=`"`$(var.DisplayVersion)`" />，否则 ARP 显示 4 段版本" -ForegroundColor Red
        $fail++
    }
    # 兜底：wxs 里任何形如 Version="1.2.3" 的硬编码都算漂移风险
    if ($wxsText -match 'Version="\d+\.\d+\.\d+(\.\d+)?"') {
        Write-Host '  [FAIL] tianyan.wxs 里仍有硬编码版本号（形如 Version="x.y.z"）' -ForegroundColor Red
        $fail++
    }
}

# 5. 打包脚本存在（版本注入与产物命名的唯一实现处）
$buildScript = Join-Path $repoRoot 'gui-native/installer/build-msi.ps1'
if (Test-Path $buildScript) {
    Write-Host "  [PASS] gui-native/installer/build-msi.ps1 存在（版本注入 + 产物命名唯一实现）" -ForegroundColor DarkGreen
}
else {
    Write-Host "  [FAIL] 缺 gui-native/installer/build-msi.ps1（否则 MSI 版本只能手工写）" -ForegroundColor Red
    $fail++
}

Write-Host ""
if ($fail -gt 0) {
    Write-Host "版本一致性校验失败：$fail 项。改版本时这几处必须一起改（见根 Cargo.toml 的注释）。" -ForegroundColor Red
    exit 1
}
Write-Host "版本一致性校验通过（$rootVer）" -ForegroundColor Green
exit 0

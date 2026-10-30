# 天演原生客户端 MSI 打包（版本单源）
#
# 为什么要有这个脚本，而不是直接 `wix build`：
#   1. **版本单源**：MSI 的 `ProductVersion` 必须来自 `gui-native/Cargo.toml`
#      （否则 wxs 里一处、Cargo.toml 一处、主 workspace 一处、tauri.conf.json
#      一处，四处各写各的 → 实测已经漂成 1.0.1 / 1.1.0 并存，「关于」页和
#      `/health` 各报一个号）；
#   2. **产物名同源**：`tianyan-native-<ver>-x64.msi` 的文件名也由同一版本生成，
#      避免「包名说 1.1.0、里面装的是 1.0.1」；
#   3. MSI 的 ProductVersion 是**4 段**（`1.1.0.0`），Cargo 是 3 段（`1.1.0`），
#      补 `.0` 这件事只应在一处做。
#
# 用法（先 `cargo build --release`，本脚本会检查 exe 是否存在）：
#   pwsh -File gui-native\installer\build-msi.ps1
#   pwsh -File gui-native\installer\build-msi.ps1 -SkipBuild     # exe 已就绪时

[CmdletBinding()]
param(
    # 跳过 `cargo build --release`（默认会跑，防止拿旧 exe 打包）
    [switch]$SkipBuild,
    # 只校验不打包（CI 的 lint 门禁可复用）
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$guiRoot = Split-Path $here -Parent              # gui-native/
$repoRoot = Split-Path $guiRoot -Parent          # 仓库根
$cargoToml = Join-Path $guiRoot 'Cargo.toml'
$exe = Join-Path $guiRoot 'target\release\tianyan-gui-native.exe'
$wxs = Join-Path $here 'tianyan.wxs'

# ── 1. 从 Cargo.toml 读版本（只认 [package] 段，不抓依赖的 version） ──────────
function Get-PackageVersion {
    param([string]$Path)
    $inSection = $false
    foreach ($line in Get-Content $Path -Encoding utf8) {
        $t = $line.Trim()
        if ($t -match '^\[') { $inSection = ($t -eq '[package]'); continue }
        if ($inSection -and $t -match '^version\s*=\s*"([^"]+)"') { return $matches[1] }
    }
    return $null
}

$ver3 = Get-PackageVersion -Path $cargoToml
if (-not $ver3) { throw "无法从 $cargoToml 的 [package] 段读到 version" }
if ($ver3 -notmatch '^\d+\.\d+\.\d+$') { throw "版本格式应为 x.y.z，实际：$ver3" }
# MSI 要求 4 段；第 4 段留给「同一版本的多次重建」以外的语义（这里恒 0）
$ver4 = "$ver3.0"

Write-Host "版本（Cargo.toml）: $ver3" -ForegroundColor Cyan
Write-Host "版本（MSI，4 段） : $ver4" -ForegroundColor Cyan

# ── 2. wxs 必须用注入变量，不能自己硬编码版本 ────────────────────────────────
$wxsText = Get-Content $wxs -Raw -Encoding utf8
if ($wxsText -notmatch 'Version="\$\(var\.ProductVersion\)"') {
    throw "tianyan.wxs 的 Package/@Version 必须是 `$(var.ProductVersion)（当前不是：版本会与 Cargo.toml 漂移）"
}

$msiName = "tianyan-native-$ver3-x64.msi"
$msiPath = Join-Path $here $msiName

if ($CheckOnly) {
    Write-Host "检查通过（未打包）。预期产物：$msiName" -ForegroundColor Green
    exit 0
}

# ── 3. 构建 release exe（默认做，避免拿旧 exe 打包） ─────────────────────────
if (-not $SkipBuild) {
    Write-Host "构建 release exe…" -ForegroundColor Yellow
    Push-Location $guiRoot
    try {
        # CARGO_INCREMENTAL=0：本机增量缓存偶发 rust-lld undefined symbol
        $env:CARGO_INCREMENTAL = '0'
        cmd /c "cargo build --release 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败" }
    }
    finally { Pop-Location }
}

if (-not (Test-Path $exe)) {
    throw "找不到 release exe：$exe（先 cd gui-native && cargo build --release）"
}
$exeInfo = Get-Item $exe
$exeVer = $exeInfo.VersionInfo.ProductVersion
Write-Host ("exe: {0:N1} MB  FileVersion={1}  {2}" -f ($exeInfo.Length / 1MB), $exeVer, $exeInfo.LastWriteTime) -ForegroundColor DarkGray
if ($exeVer -ne $ver3) {
    # 这不是致命错误，但几乎总是「忘了重新 build」——显式告警而不是静默打包旧产物
    Write-Host "⚠️ exe 的 ProductVersion=$exeVer 与 Cargo.toml=$ver3 不一致（多半是旧 exe，建议去掉 -SkipBuild 重跑）" -ForegroundColor Yellow
}

# ── 4. 打包 ─────────────────────────────────────────────────────────────────
Write-Host "wix build → $msiName" -ForegroundColor Yellow
$t0 = Get-Date
Push-Location $here
try {
    # `-d ProductVersion=` 注入 wxs 里的 `$(var.ProductVersion)`
    wix build -d "ProductVersion=$ver4" -o $msiName tianyan.wxs 2>&1 | Out-String | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "wix build 失败（exit $LASTEXITCODE）" }
}
finally { Pop-Location }

$msi = Get-Item $msiPath
$sha = (Get-FileHash $msiPath -Algorithm SHA256).Hash
$elapsed = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)

Write-Host ""
Write-Host "✅ 打包完成" -ForegroundColor Green
Write-Host ("   MSI      : " + $msi.FullName)
Write-Host ("   ProductV : " + $ver4 + "   （来自 " + $cargoToml + "）")
Write-Host ("   体积     : " + [math]::Round($msi.Length / 1MB, 2) + " MB   耗时 " + $elapsed + "s")
Write-Host ("   SHA256   : " + $sha)
Write-Host ("   安装     : msiexec /i `"" + $msi.FullName + "`" /qn /norestart")

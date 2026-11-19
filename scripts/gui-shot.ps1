# 界面截图（内建 `render_to_image`，不抓屏）。
#
# 为什么必须用这条通道：`CopyFromScreen` 抓的是**屏幕像素**，PowerShell 默认
# DPI-unaware，`GetWindowRect` 返回虚拟化坐标（本机 150% 缩放下物理 1920×1230
# 的窗口只报 1295×857），于是只抓到窗口左上角一块——症状极像「右下角的元素没
# 渲染」，据此误判布局失效、白查数轮的事故已经发生过两次（2026-10、2026-11）。
# 内建截图取的是**渲染输出**，不受 DPI 与遮挡影响。
#
# 用法（先 `cargo build --features screenshot`）：
#   .\scripts\gui-shot.ps1                                   # 设置页当前栏目
#   .\scripts\gui-shot.ps1 -Tab retrieval -Out .\a.png
#   .\scripts\gui-shot.ps1 -Tab storage -EditPath storage.cleanup_days
#   .\scripts\gui-shot.ps1 -Tab retrieval -WidthPx 1350      # 窄窗口（物理像素）
#
# 输出尺寸是**客户区物理像素**（本机 1920×1230 = 逻辑 1280×820 @150%）。
# 退出码：0 = 截到了；1 = 失败（会打印原因）。

#requires -Version 7

param(
    [int]$Port = 3010,
    [string]$Tab = "",
    # 进入某张卡片的编辑态：给字段路径（如 `storage.cleanup_days`）。
    [string]$EditPath = "",
    [string]$Out = "$env:TEMP\tianyan-shot.png",
    # 抓图前改窗口外框宽度（物理像素）；0 = 不改。用于核对窄窗口观感。
    [int]$WidthPx = 0,
    # 跑完不退出实例（下次复用，省启动时间）
    [switch]$KeepAlive
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repo "gui-native\target\debug\tianyan-gui-native.exe"
$base = "http://127.0.0.1:$Port"

if (-not (Test-Path $exe)) {
    Write-Host "找不到 $exe —— 先 cargo build --features screenshot" -ForegroundColor Red
    exit 1
}
$Out = [System.IO.Path]::GetFullPath($Out)
$outDir = Split-Path -Parent $Out
if ($outDir) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
Remove-Item -LiteralPath $Out -ErrorAction SilentlyContinue

function Act {
    param([hashtable]$Payload, [int]$SettleMs = 500)
    $body = @{} + $Payload
    $body["settle_ms"] = $SettleMs
    $json = $body | ConvertTo-Json -Depth 8 -Compress
    # 落 UTF-8 文件再发：PowerShell 直接塞中文 `-Body` 会按 ANSI 编码，服务端收到乱码
    $tmp = Join-Path $env:TEMP ("gui-shot-act-" + [guid]::NewGuid().ToString("N") + ".json")
    [System.IO.File]::WriteAllText($tmp, $json, (New-Object System.Text.UTF8Encoding($false)))
    try {
        $resp = Invoke-RestMethod -Method Post -Uri "$base/act" -ContentType "application/json; charset=utf-8" -InFile $tmp -TimeoutSec 60
    } finally {
        Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
    # 动作失败是 HTTP 200 + {ok:false}：不检查就会「全绿但什么都没发生」
    if (-not $resp.ok) { throw "act 失败（$($Payload.action)）：$($resp.error)" }
    return $resp
}

# ── 实例：已在跑就复用，否则起一个**可见**窗口（隐藏窗口没有可用帧，只能截出 1×1）
$running = $false
try { $running = [bool](Invoke-RestMethod -Uri "$base/health" -TimeoutSec 2).ok } catch { $running = $false }
$proc = $null
if (-not $running) {
    $env:TIANYAN_GUI_AUTOMATION_PORT = "$Port"
    Remove-Item Env:\TIANYAN_GUI_START_HIDDEN -ErrorAction SilentlyContinue
    $proc = Start-Process -FilePath $exe -WorkingDirectory (Join-Path $repo "gui-native") -PassThru
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        try { if ((Invoke-RestMethod -Uri "$base/health" -TimeoutSec 2).ok) { $running = $true; break } } catch { }
        Start-Sleep -Milliseconds 400
    }
    if (-not $running) { throw "自动化端口未就绪（$base）" }
    Start-Sleep -Milliseconds 800
    Write-Host "已启动 PID $($proc.Id)" -ForegroundColor DarkGray
} else {
    Write-Host "复用已在运行的实例（$base）" -ForegroundColor DarkGray
}

try {
    if ($Tab -ne "") {
        Act @{ action = "switch_tab"; view = "settings" } 500 | Out-Null
        Act @{ action = "settings_tab"; tab = $Tab } 700 | Out-Null
    }
    if ($EditPath -ne "") {
        Act @{ action = "settings_edit"; path = $EditPath } 700 | Out-Null
    }
    if ($WidthPx -gt 0 -and $proc) {
        Add-Type @"
using System;
using System.Runtime.InteropServices;
public class GuiShotW32 {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
}
"@
        $proc.Refresh()
        [GuiShotW32]::SetWindowPos($proc.MainWindowHandle, [IntPtr]::Zero, 40, 40, $WidthPx, 900, 0x0004) | Out-Null
        Start-Sleep -Milliseconds 900
    }
    Act @{ action = "screenshot"; path = $Out } 700 | Out-Null
    if (-not (Test-Path $Out)) { throw "内建截图没写出文件：$Out" }
    $item = Get-Item $Out
    Write-Host "已截图：$Out（$($item.Length) 字节）" -ForegroundColor Green
    if ([string]::IsNullOrEmpty($Tab) -and $WidthPx -gt 0) {
        Write-Host "提示：改了窗口宽度但没有切栏目，截图可能与上一帧相同" -ForegroundColor DarkYellow
    }
} catch {
    Write-Host "失败：$_" -ForegroundColor Red
    if ($_.Exception.Message -like "*未启用截图*") {
        Write-Host "这台二进制没开 screenshot feature：cargo build --features screenshot" -ForegroundColor Yellow
    }
    exit 1
} finally {
    if ($proc -and -not $KeepAlive) {
        try { Act @{ action = "quit" } 200 | Out-Null } catch { }
        Start-Sleep -Milliseconds 1200
        $alive = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
        if ($alive) { Stop-Process -Id $proc.Id -Force }
    }
}

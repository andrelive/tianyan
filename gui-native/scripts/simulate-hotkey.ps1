# 全局快捷键端到端验证：**真的按下组合键**（不是调动作）
#
# 为什么单独一个脚本：`automation-smoke.ps1` 验证的是「动作做对了什么」，
# 而这里验证的是「系统按键 → RegisterHotKey → 动作」这条完整链路。两者的
# 失败模式不同（一个坏在动作实现，一个坏在注册/消息循环），混在一起会让
# 失败原因看不出来。
#
# 用法：
#   pwsh -NoProfile -File .\scripts\simulate-hotkey.ps1
#   pwsh -NoProfile -File .\scripts\simulate-hotkey.ps1 -Combo "ctrl+alt+y"
#
# 前提：GUI 用同一个 `TIANYAN_GUI_HOTKEY` 启动（默认 Ctrl+Alt+T）。
# 注意：tauri 版也注册 Ctrl+Alt+T，两个客户端同跑时后注册的那个会失败——
# 冲突环境下改用 -Combo 换一个组合，并在启动 GUI 时设同一个环境变量。
param(
    [string]$BaseUrl = "http://127.0.0.1:3010",
    [string]$Combo = "ctrl+alt+t"
)

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$script:fail = 0
function Check($cond, $msg) {
    if ($cond) { Write-Host "  [ok] $msg" -ForegroundColor Green }
    else { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:fail++ }
}

$keySim = @"
using System;
using System.Runtime.InteropServices;
public class TyKeySim {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
}
"@
if (-not ("TyKeySim" -as [type])) { Add-Type -TypeDefinition $keySim }

function Get-State {
    return (Invoke-RestMethod -Uri "$BaseUrl/state" -TimeoutSec 10).state
}

function Send-Combo([string]$combo) {
    $VK = @{ ctrl = 0x11; control = 0x11; alt = 0x12; shift = 0x10; win = 0x5B }
    $KEYUP = 0x0002
    $mods = @()
    $main = $null
    foreach ($raw in $combo.Split('+')) {
        $p = $raw.Trim().ToLower()
        if ($VK.ContainsKey($p)) { $mods += $VK[$p] }
        elseif ($p.Length -eq 1) { $main = [byte][char]$p.ToUpper() }
        elseif ($p -match '^f(\d+)$') { $main = [byte](0x70 + [int]$Matches[1] - 1) }
    }
    if ($null -eq $main) { throw "无法解析组合键：$combo" }

    foreach ($m in $mods) { [TyKeySim]::keybd_event([byte]$m, 0, 0, [UIntPtr]::Zero) }
    Start-Sleep -Milliseconds 30
    [TyKeySim]::keybd_event($main, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 30
    [TyKeySim]::keybd_event($main, 0, $KEYUP, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 30
    [int[]]$rev = $mods
    [array]::Reverse($rev)
    foreach ($m in $rev) { [TyKeySim]::keybd_event([byte]$m, 0, $KEYUP, [UIntPtr]::Zero) }
}

Write-Host "== 全局快捷键端到端（模拟真实按键）==" -ForegroundColor Cyan
$s = Get-State
Write-Host ("  GUI 侧注册状态：registered={0} combo={1}" -f $s.usage.hotkey.registered, $s.usage.hotkey.combo)

if (-not $s.usage.hotkey.registered) {
    Write-Host "  快捷键未注册（组合键被占用？见 GUI 日志「全局快捷键不可用」）" -ForegroundColor Yellow
    Write-Host "  提示：tauri 版也注册 Ctrl+Alt+T。用 -Combo 换组合，并以同一个" -ForegroundColor Yellow
    Write-Host "        TIANYAN_GUI_HOTKEY 启动 GUI 后重试。" -ForegroundColor Yellow
    exit 2
}

$before = $s.usage.tray.window_visible
Write-Host ("  初始窗口可见：{0}" -f $before)

Send-Combo $Combo
Start-Sleep -Milliseconds 700
$mid = (Get-State).usage.tray.window_visible
Check ($mid -ne $before) ("按 $Combo 后窗口可见性翻转（{0} → {1}）" -f $before, $mid)

Send-Combo $Combo
Start-Sleep -Milliseconds 700
$after = (Get-State).usage.tray.window_visible
Check ($after -eq $before) ("再按一次回到原状态（{0} → {1}）" -f $mid, $after)

Write-Host ""
if ($script:fail -eq 0) {
    Write-Host "全局快捷键端到端通过" -ForegroundColor Green
    exit 0
} else {
    Write-Host ("{0} 项失败" -f $script:fail) -ForegroundColor Red
    exit 1
}

# 键盘类回归：Enter 发送时消息里不该凭空多出一个换行
#
# 为什么要有它：`InputBaseState` 处理 Enter 的方式是「先在光标处插入换行 → 再 emit
# `InputEvent::PressEnter`」。光标在末尾时这个换行跟在最后（Markdown 渲染不显形），
# 光标在**中间**时它就卡在句子中间 —— 用户实测报的就是这个。它的正确性只能靠
# 「真的按键 + 真的落库」来验，看代码或截图都不可靠。
#
# 用法：
#   pwsh -NoProfile -File .\scripts\verify-enter-send.ps1 -Exe gui-native\target\debug\tianyan-gui-native.exe
#   pwsh -NoProfile -File .\scripts\verify-enter-send.ps1 -Exe <旧版 exe> -Tag before   # 复现
#
# 断言（打印到控制台，供人工对照新旧两版）：
#   input_after_paste  —— `/state` 读到的输入框内容（**仅供参考**：实测它在这里
#                          常为空串，因为 `/state` 走的是另一条通道，粘贴事件还没
#                          被 UI 线程落进 state 快照；真正的判据看下面两行）
#   input_after_enter  —— 发送后输入框应被清空
#   db_user_texts      —— 库里的 user 消息正文（repr，换行会显示成 \n）
#   state_last_newlines—— `/state` 的 recent 末尾 newlines（新版才有该字段）
#
# 权威判据是 `db_user_texts`：会话消息的真相在 SQLite（ADR-018）。实测对照：
#   未修复 → `'ABC\nDEF'  newlines=1`（光标在中间按 Enter，换行卡在句子里）
#   已修复 → `'ABCDEF'    newlines=0`
#
# 隔离纪律：`TIANYAN_CONFIG` 指独立配置、`storage.data_dir` 指临时目录、
# `TIANYAN_GUI_AUTOMATION_PORT` 指独立端口，全程不碰 `D:\tianyan`。
# ⚠️ 收尾只按 PID 杀（`Start-Process -PassThru` 拿到的那一个）—— 按进程名会连
# 用户正在用的实例一起杀掉（踩过）。

#requires -Version 7
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [int]$Port = 3097,
    [string]$Tag = "enter"
)

$ErrorActionPreference = "Continue"
$repo = Split-Path $PSScriptRoot -Parent
$exePath = if ([System.IO.Path]::IsPathRooted($Exe)) { $Exe } else { Join-Path $repo $Exe }
if (-not (Test-Path $exePath)) { Write-Host "exe 不存在：$exePath" -ForegroundColor Red; exit 1 }

$root = Join-Path $env:TEMP "tianyan-scratch\enter-$Tag"
if (Test-Path $root) { Remove-Item $root -Recurse -Force }
$data = Join-Path $root "data"
New-Item -ItemType Directory -Force -Path $data | Out-Null

# 配置：复制真实配置后只改 data_dir（保证字段齐全，避免默认值缺失导致启动失败）
$src = Join-Path $env:USERPROFILE ".tianyan\tianyan.toml"
if (-not (Test-Path $src)) { Write-Host "找不到配置模板：$src" -ForegroundColor Red; exit 1 }
$cfg = Join-Path $root "tianyan.toml"
$dataToml = ($data -replace '\\', '/')
(Get-Content $src) | ForEach-Object {
    if ($_ -match '^\s*data_dir\s*=') { "data_dir = '$dataToml'" } else { $_ }
} | Set-Content -Path $cfg -Encoding utf8

$env:TIANYAN_CONFIG = $cfg
$env:TIANYAN_GUI_AUTOMATION_PORT = "$Port"
$base = "http://127.0.0.1:$Port"

Write-Host "[$Tag] 启动隔离实例（可见窗口，键盘事件需要前台焦点）…" -ForegroundColor Yellow
$proc = Start-Process -FilePath $exePath -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 200; $i++) {
        try {
            $null = Invoke-RestMethod "$base/health" -TimeoutSec 2
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 300 }
    }
    if (-not $ready) { Write-Host "[$Tag] 自动化端口未就绪" -ForegroundColor Red; exit 1 }

    Start-Sleep -Milliseconds 800
    # 焦点：优先用动作（新版有；旧版没有该动作 → 回退点击窗口中央偏下的输入框）
    $focused = $false
    try {
        $r = Invoke-RestMethod "$base/act" -Method Post -TimeoutSec 5 `
            -ContentType "application/json" -Body '{"action":"composer_focus"}'
        $focused = [bool]$r.ok
    } catch { }

    [void][Microsoft.VisualBasic.Interaction]::AppActivate($proc.Id)
    Start-Sleep -Milliseconds 600
    if (-not $focused) {
        # 回退：窗口居中点击一次（输入卡片在底部；默认窗口 1280×800 时约 (640, 700)）
        Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, int e);
  public struct RECT { public int L, T, R, B; }
}
"@
        $h = (Get-Process -Id $proc.Id).MainWindowHandle
        if ($h -ne [IntPtr]::Zero) {
            [void][W]::SetForegroundWindow($h)
            $rect = New-Object W+RECT
            [void][W]::GetWindowRect($h, [ref]$rect)
            $x = [int](($rect.L + $rect.R) / 2)
            $y = [int]($rect.B - 90)
            [void][W]::SetCursorPos($x, $y)
            [W]::mouse_event(0x0002, 0, 0, 0, 0)   # LEFTDOWN
            [W]::mouse_event(0x0004, 0, 0, 0, 0)   # LEFTUP
            Write-Host "[$Tag] 回退：已点击输入框 ($x,$y)" -ForegroundColor DarkYellow
        }
        Start-Sleep -Milliseconds 400
    }

    Set-Clipboard -Value "ABCDEF"
    $ws = New-Object -ComObject WScript.Shell
    $ws.SendKeys("^v")
    Start-Sleep -Milliseconds 700
    # ⚠️ `/state` 的返回是 `{ ok, state }` 包装（`/act` 同此形），字段在 `.state` 下 ——
    # 直接读 `$st.input` 永远是空串（实测踩过，差点误判成「粘贴没进去」）。
    $st = Invoke-RestMethod "$base/state" -TimeoutSec 5
    Write-Host "[$Tag] input_after_paste = '$($st.state.input)'" -ForegroundColor Cyan

    # 光标移到正文中间（末尾 3 个字符之后），再按 Enter —— 这是 bug 的触发条件
    1..3 | ForEach-Object { $ws.SendKeys("{LEFT}"); Start-Sleep -Milliseconds 90 }
    $ws.SendKeys("{ENTER}")
    Start-Sleep -Milliseconds 2000

    try {
        $st = Invoke-RestMethod "$base/state" -TimeoutSec 5
        Write-Host "[$Tag] input_after_enter = '$($st.state.input)'" -ForegroundColor Cyan
        $last = @($st.state.messages.recent)[-1]
        if ($null -ne $last -and $null -ne $last.PSObject.Properties["newlines"]) {
            Write-Host "[$Tag] state_last_newlines = $($last.newlines)" -ForegroundColor Cyan
        }
    } catch { }
} finally {
    # 只杀本脚本启动的这一份（按 PID）
    try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue } catch { }
}

# 落库核对：正文里的换行在库中最客观（渲染会掩盖末尾换行）
Start-Sleep -Milliseconds 500
$db = Join-Path $data "tianyan.db"
if (Test-Path $db) {
    $py = Join-Path $root "q.py"
    @'
import sqlite3, sys
con = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
cur = con.cursor()
cur.execute("SELECT role, text FROM session_messages WHERE role='user' ORDER BY id DESC LIMIT 3")
rows = cur.fetchall()
if not rows:
    print("db_user_texts: <空>")
else:
    print("db_user_texts:")
    for role, text in rows:
        print(f"  {role}: {text!r}  newlines={text.count(chr(10))}")
con.close()
'@ | Set-Content -Path $py -Encoding utf8
    python $py $db
} else {
    Write-Host "库不存在（$db）—— 发送可能没成功" -ForegroundColor DarkYellow
}
Write-Host "[$Tag] 隔离目录：$root" -ForegroundColor DarkGray

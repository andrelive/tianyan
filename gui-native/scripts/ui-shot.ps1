# ui-shot.ps1 — 原生客户端（GPUI）截图：本机无 Playwright 时的自动验证手段
#
# 用法：
#   pwsh -NoProfile -File gui-native/scripts/ui-shot.ps1 -Out "$env:TEMP\shot.png"
#   pwsh -NoProfile -File gui-native/scripts/ui-shot.ps1 -Out shot.png -MinW 800 -MinH 600
#
# 做什么：找到 `tianyan-gui-native` 进程的主窗口 → 置顶 → 按屏幕区域截图（PNG）。
#
# ⛔ 关键坑（实测踩过，别再犯）：
# PowerShell 默认是 **DPI-unaware** 进程。在 150% 缩放的屏幕上（本机 2560x1440 @150%），
# `GetWindowRect` 返回的是**虚拟化坐标**——物理 1920x1230 的窗口只报成 1295x857，
# 于是 `CopyFromScreen` 只抓到窗口左上角那一块。症状极像「右下角的元素没渲染」，
# 曾据此误判 GPUI 的 flex 布局失效、白查了数轮。**必须**先 SetProcessDpiAwareness(2)。
#
# 另一个坑：`CopyFromScreen` 抓的是屏幕像素，**窗口被别的窗口挡住就会抓到遮挡物**。
# 所以下面先 ShowWindow + SetWindowPos(HWND_TOPMOST) + SetForegroundWindow 再抓。
param(
    [string]$Out = "",
    [string]$ProcessName = "tianyan-gui-native",
    [int]$MinW = 800,
    [int]$MinH = 600,
    [int]$Tries = 30
)

$dpi = @"
using System;
using System.Runtime.InteropServices;
public class TyDpiAware {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
    public static void Enable() {
        try { SetProcessDpiAwareness(2); } catch { }
        try { SetProcessDPIAware(); } catch { }
    }
}
"@
if (-not ("TyDpiAware" -as [type])) { Add-Type -TypeDefinition $dpi }
[TyDpiAware]::Enable()

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$src = @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class TyShot {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    public static List<IntPtr> ByPid(uint want) {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == want && IsWindowVisible(h)) list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
"@
if (-not ("TyShot" -as [type])) { Add-Type -TypeDefinition $src }

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "进程 $ProcessName 未运行"; exit 1 }

$screenArea = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Output ("屏幕（物理）= {0}x{1}" -f $screenArea.Width, $screenArea.Height)

# GPUI 首帧之前窗口可能只有几十像素，等到「够大」再抓
$best = $null; $bh = [IntPtr]::Zero; $bw = 0; $bhh = 0
for ($i = 0; $i -lt $Tries; $i++) {
    $bw = 0; $bhh = 0; $best = $null; $bh = [IntPtr]::Zero
    foreach ($h in [TyShot]::ByPid([uint32]$proc.Id)) {
        $rr = New-Object TyShot+RECT
        [void][TyShot]::GetWindowRect($h, [ref]$rr)
        $w0 = $rr.Right - $rr.Left; $h0 = $rr.Bottom - $rr.Top
        if (($w0 * $h0) -gt ($bw * $bhh)) { $bw = $w0; $bhh = $h0; $best = $rr; $bh = $h }
    }
    if ($best -and $bw -ge $MinW -and $bhh -ge $MinH) { break }
    Start-Sleep -Milliseconds 500
}
if (-not $bh) { Write-Error "没有可见顶层窗口"; exit 1 }
if ($bw -lt $MinW -or $bhh -lt $MinH) {
    Write-Error "窗口尺寸不足：${bw}x${bhh}（进程可能启动失败或窗口未创建）"; exit 1
}

[void][TyShot]::ShowWindow($bh, 9)
[void][TyShot]::SetWindowPos($bh, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002)
[void][TyShot]::SetForegroundWindow($bh)
Start-Sleep -Milliseconds 1200

# 窗口若越出屏幕，裁到屏幕内（越界部分是黑的）
$left = [Math]::Max($best.Left, 0)
$top = [Math]::Max($best.Top, 0)
$right = [Math]::Min($best.Right, $screenArea.Width)
$bottom = [Math]::Min($best.Bottom, $screenArea.Height)
$w = $right - $left; $hh = $bottom - $top
if ($w -le 0 -or $hh -le 0) { Write-Error "窗口完全在屏幕外"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($left, $top, 0, 0, (New-Object System.Drawing.Size $w, $hh))
if (-not $Out) {
    $dir = Join-Path $env:TEMP "tianyan-scratch"
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $Out = Join-Path $dir ("ui-shot-{0}.png" -f (Get-Date -Format "yyyyMMdd-HHmmss"))
}
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output ("$Out  ({0}x{1})  窗口=({2},{3})-({4},{5})  句柄={6}" -f $w, $hh, $best.Left, $best.Top, $best.Right, $best.Bottom, $bh)

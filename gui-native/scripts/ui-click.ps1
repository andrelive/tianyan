# ui-click.ps1 — 原生客户端（GPUI）鼠标点击：配合 ui-shot.ps1 做端到端自证
#
# 用法（坐标是**窗口内物理像素**，原点 = 窗口左上角含标题栏）：
#   pwsh -NoProfile -File gui-native/scripts/ui-click.ps1 -X 260 -Y 120
#   pwsh -NoProfile -File gui-native/scripts/ui-click.ps1 -X 260 -Y 120 -DoubleClick
#
# 只移动不点击（验证 hover 效果：行内操作按钮只在悬停时出现）：
#   pwsh -NoProfile -File gui-native/scripts/ui-click.ps1 -X 300 -Y 320 -MoveOnly
#
# 坐标怎么定：用 ui-shot.ps1 截一张图，图内像素坐标就是这里要传的值
# （两者都走 DPI-aware 物理口径，不需要换算缩放比）。
#
# 典型闭环：启动 exe → ui-click 选中会话 → ui-shot 截图 → 人/模型读图核对。
param(
    [int]$X = 0,
    [int]$Y = 0,
    [string]$ProcessName = "tianyan-gui-native",
    [switch]$DoubleClick,
    # 只把鼠标移到目标位置、不点击：hover 类效果（如行内按钮）只能这样验
    [switch]$MoveOnly
)

$dpi = @"
using System;
using System.Runtime.InteropServices;
public class TyDpiAwareClick {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
    public static void Enable() {
        try { SetProcessDpiAwareness(2); } catch { }
        try { SetProcessDPIAware(); } catch { }
    }
}
"@
if (-not ("TyDpiAwareClick" -as [type])) { Add-Type -TypeDefinition $dpi }
[TyDpiAwareClick]::Enable()

$src = @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class TyClick {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
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
if (-not ("TyClick" -as [type])) { Add-Type -TypeDefinition $src }

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "进程 $ProcessName 未运行"; exit 1 }

$best = $null; $bh = [IntPtr]::Zero; $area = 0
foreach ($h in [TyClick]::ByPid([uint32]$proc.Id)) {
    $rr = New-Object TyClick+RECT
    [void][TyClick]::GetWindowRect($h, [ref]$rr)
    $a = ($rr.Right - $rr.Left) * ($rr.Bottom - $rr.Top)
    if ($a -gt $area) { $area = $a; $best = $rr; $bh = $h }
}
if (-not $bh) { Write-Error "没有可见顶层窗口"; exit 1 }

[void][TyClick]::ShowWindow($bh, 9)
[void][TyClick]::SetWindowPos($bh, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002)
[void][TyClick]::SetForegroundWindow($bh)
Start-Sleep -Milliseconds 600

# 最小化时 rect 是假值（-32000 附近 / 约 237x39），恢复后必须重新取一次，
# 否则点击坐标会算到屏幕外（实测：点在 (-32000+300, …) 上，等于没点）。
$rr = New-Object TyClick+RECT
[void][TyClick]::GetWindowRect($bh, [ref]$rr)
$best = $rr

$ax = $best.Left + $X
$ay = $best.Top + $Y
[void][TyClick]::SetCursorPos($ax, $ay)
Start-Sleep -Milliseconds 150
if ($MoveOnly) {
    Write-Output ("移动 窗口内({0},{1}) -> 屏幕({2},{3})" -f $X, $Y, $ax, $ay)
    exit 0
}
# LEFTDOWN = 0x0002, LEFTUP = 0x0004
[TyClick]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[TyClick]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
if ($DoubleClick) {
    Start-Sleep -Milliseconds 80
    [TyClick]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [TyClick]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}
Write-Output ("点击 窗口内({0},{1}) -> 屏幕({2},{3})" -f $X, $Y, $ax, $ay)

# 诊断：启动瞬间该进程创建了几个顶层窗口（任务栏按钮的来源）
#
# 目的：用户观察到「任务栏图标反复出现又消失 3~4 次」。日志里没有任何重复启动
# 的痕迹，所以要直接看窗口层：枚举该 PID 的所有顶层窗口 + 可见性 + 类名，
# 每 80ms 一次，持续 N 秒，输出「窗口集合发生变化」的时刻。
param(
    [string]$Exe = ".\target\debug\tianyan-gui-native.exe",
    [int]$Seconds = 12,
    [string]$ShotDir = ""
)

$code = @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class WinEnum {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    public static List<string> List(uint targetPid) {
        var res = new List<string>();
        EnumWindows((h, l) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == targetPid) {
                var t = new StringBuilder(256); GetWindowTextW(h, t, 256);
                var c = new StringBuilder(256); GetClassNameW(h, c, 256);
                res.Add(h.ToInt64() + "|" + (IsWindowVisible(h) ? "vis" : "hid") + "|" + c.ToString() + "|" + t.ToString());
            }
            return true;
        }, IntPtr.Zero);
        return res;
    }
}
"@
if (-not ("WinEnum" -as [type])) { Add-Type -TypeDefinition $code }

$p = Start-Process -FilePath $Exe -PassThru
Write-Host "启动 PID=$($p.Id)"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$prev = $null
$events = @()
while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
    $cur = [WinEnum]::List([uint32]$p.Id)
    $sig = ($cur | ForEach-Object { ($_ -split '\|')[1..2] -join '/' } | Sort-Object) -join ' ; '
    if ($sig -ne $prev) {
        $el = [math]::Round($sw.Elapsed.TotalSeconds, 2)
        $events += "$el s -> $sig"
        Write-Host ("  {0,6}s  {1}" -f $el, $sig)
        $prev = $sig
    }
    Start-Sleep -Milliseconds 80
}
Write-Host ""
Write-Host "窗口集合变化次数（含首帧）= $($events.Count)"
Write-Host "最终窗口："
[WinEnum]::List([uint32]$p.Id) | ForEach-Object { "  $_" }
Write-Host ""
Write-Host "进程仍在运行：$(-not $p.HasExited)   PID=$($p.Id)"

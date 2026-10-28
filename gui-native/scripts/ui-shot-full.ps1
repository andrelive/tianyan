# 全屏截图（含桌面通知区域）。与 ui-shot.ps1 的区别：那个只抓天演窗口，
# 系统通知浮在屏幕右下角、在窗口之外，必须抓全屏才看得到。
param([string]$Out = "$env:TEMP\tianyan-scratch\fullscreen.png")

$dpi = @"
using System;
using System.Runtime.InteropServices;
public class TyDpiAwareFull {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
    public static void Enable() {
        try { SetProcessDpiAwareness(2); } catch { }
        try { SetProcessDPIAware(); } catch { }
    }
}
"@
if (-not ("TyDpiAwareFull" -as [type])) { Add-Type -TypeDefinition $dpi }
[TyDpiAwareFull]::Enable()

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Output "$Out  ($($b.Width)x$($b.Height))"

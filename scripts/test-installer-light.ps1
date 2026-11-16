# Opt-in visual check: opens only the installer preview and never invokes installation.
[CmdletBinding()]
param(
    [string]$PreviewTestExecutable,
    [string]$Snapshot = 'after'
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $PreviewTestExecutable) {
    $PreviewTestExecutable = (Get-ChildItem -LiteralPath (Join-Path $root 'target\debug\deps') -Filter 'mini_launcher_setup-*.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}
if (-not $PreviewTestExecutable -or -not (Test-Path -LiteralPath $PreviewTestExecutable -PathType Leaf)) {
    throw '先运行 cargo test -p mini-launcher-installer --bin mini-launcher-setup --no-run，或指定 -PreviewTestExecutable。'
}
if ($Snapshot -notmatch '^[a-zA-Z0-9_-]+$') { throw 'Snapshot must be a simple file name.' }
$output = Join-Path $root 'artifacts\installer-light'
[void][IO.Directory]::CreateDirectory($output)
Add-Type -AssemblyName System.Drawing
if (-not ('InstallerLightCheck' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class InstallerLightCheck {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X,Y; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref Point p);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int command);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int hgt,uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
}
'@
}
[void][InstallerLightCheck]::SetThreadDpiAwarenessContext([IntPtr](-4))
$process = $null
$window = [IntPtr]::Zero
try {
    $existing = [InstallerLightCheck]::FindWindow('MiniLauncherInstallerWnd', 'MiniLauncher 安装向导')
    if ($existing -ne [IntPtr]::Zero) {
        [void][InstallerLightCheck]::PostMessage($existing, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 300
        if ([InstallerLightCheck]::FindWindow('MiniLauncherInstallerWnd', 'MiniLauncher 安装向导') -ne [IntPtr]::Zero) {
            throw '现有安装器未退出，停止检查以免截错窗口。'
        }
    }
    $process = Start-Process -FilePath $PreviewTestExecutable -ArgumentList @('ui::tests::preview_light_theme', '--exact', '--ignored', '--nocapture') -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        if ($process.HasExited) { throw 'Installer preview exited before opening its window.' }
        $window = [InstallerLightCheck]::FindWindow('MiniLauncherInstallerWnd', 'MiniLauncher 安装向导')
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'Installer preview window was not created.' }
    # FindWindow can succeed while WM_CREATE is still initializing DirectWrite.
    Start-Sleep -Milliseconds 1000
    [void][InstallerLightCheck]::ShowWindow($window, 9)
    [void][InstallerLightCheck]::SetWindowPos($window, [IntPtr](-1), 0,0,0,0,0x53)
    Start-Sleep -Milliseconds 700
    if (-not [InstallerLightCheck]::IsWindowVisible($window)) { throw 'Installer preview is not visible.' }
    [void][InstallerLightCheck]::DwmFlush()
    $rect = [InstallerLightCheck+Rect]::new()
    $client = [InstallerLightCheck+Rect]::new()
    $origin = [InstallerLightCheck+Point]::new()
    [void][InstallerLightCheck]::GetWindowRect($window, [ref]$rect)
    [void][InstallerLightCheck]::GetClientRect($window, [ref]$client)
    [void][InstallerLightCheck]::ClientToScreen($window, [ref]$origin)
    $bitmap = [Drawing.Bitmap]::new(($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top))
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left,$rect.Top,0,0,$bitmap.Size)
        $bitmap.Save((Join-Path $output "$Snapshot.png"), [Drawing.Imaging.ImageFormat]::Png)
        $scale = $client.Right / 600.0
        $offsetX = $origin.X - $rect.Left
        $offsetY = $origin.Y - $rect.Top
        $darkPixels = 0
        for ($y = [int](20*$scale); $y -lt [int](80*$scale); $y++) {
            for ($x = [int](90*$scale); $x -lt [int](560*$scale); $x++) {
                $pixel = $bitmap.GetPixel(($offsetX+$x),($offsetY+$y))
                if ($pixel.R -lt 140 -and $pixel.G -lt 140 -and $pixel.B -lt 140) { $darkPixels++ }
            }
        }
        $button = $bitmap.GetPixel(($offsetX+[int](450*$scale)),($offsetY+[int](400*$scale)))
        $passed = $darkPixels -gt 100 -and $button.R -lt 80 -and $button.B -gt ($button.R+80)
        $result = [ordered]@{
            theme='light (test process only)'; screenshot="$Snapshot.png"; dark_header_pixels=$darkPixels;
            install_button_rgb=@($button.R,$button.G,$button.B); passed=$passed;
            scope='Only opened and captured the preview; never clicked install or changed system theme.'
        }
        $result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $output "$Snapshot.json") -Encoding utf8
        Write-Output ($result | ConvertTo-Json -Compress)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
} finally {
    if ($window -ne [IntPtr]::Zero) { [void][InstallerLightCheck]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
    if ($process -and -not $process.WaitForExit(2000)) { Stop-Process -Id $process.Id -Force }
}
if (-not $passed) { throw 'Light installer header/button contrast check failed; inspect the screenshot.' }

Add-Type -AssemblyName System.Windows.Forms,System.Drawing
if (-not ('LauncherUiCheck' -as [type])) {
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class LauncherUiCheck {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendKey(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
}
'@
}
[void][LauncherUiCheck]::SetThreadDpiAwarenessContext([IntPtr](-4))

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\debug\mini-launcher.exe'
$artifactDir = Join-Path $root 'artifacts'
[void][IO.Directory]::CreateDirectory($artifactDir)

# Kill any existing mini-launcher
Get-Process mini-launcher -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Output "Starting mini-launcher..."
$psi = [Diagnostics.ProcessStartInfo]::new($exe, "--background")
$psi.UseShellExecute = $false
$proc = [Diagnostics.Process]::Start($psi)

try {
    $window = [IntPtr]::Zero
    for ($i = 0; $i -lt 50; $i++) {
        $window = [LauncherUiCheck]::FindWindow("MiniLauncher.Native.Window", "MiniLauncher")
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 100
    }
    if ($window -eq [IntPtr]::Zero) { throw "Window not found!" }
    Write-Output "Found window $window"

    # Send SHOW (0x8003)
    [void][LauncherUiCheck]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 600
    [void][LauncherUiCheck]::SetForegroundWindow($window)
    [void][LauncherUiCheck]::DwmFlush()

    # Capture idle state
    $rect = [LauncherUiCheck+Rect]::new()
    [void][LauncherUiCheck]::GetWindowRect($window, [ref]$rect)
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    Write-Output "Window rect: Left=$($rect.Left), Top=$($rect.Top), Width=$w, Height=$h"

    if ($w -gt 0 -and $h -gt 0) {
        $img = [Drawing.Bitmap]::new($w, $h)
        $g = [Drawing.Graphics]::FromImage($img)
        $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $img.Size)
        $idlePath = Join-Path $artifactDir "verify-ui-idle.png"
        $img.Save($idlePath, [Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose()
        $img.Dispose()
        Write-Output "Saved idle screenshot to $idlePath"
    }

    # Now type 'calc' via WM_CHAR
    Write-Output "Typing 'calc'..."
    foreach ($ch in 'calc'.ToCharArray()) {
        [void][LauncherUiCheck]::PostMessage($window, 0x0102, [IntPtr][int]$ch, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 600
    [void][LauncherUiCheck]::DwmFlush()

    [void][LauncherUiCheck]::GetWindowRect($window, [ref]$rect)
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    Write-Output "After typing 'calc' rect: Left=$($rect.Left), Top=$($rect.Top), Width=$w, Height=$h"

    if ($w -gt 0 -and $h -gt 0) {
        $img = [Drawing.Bitmap]::new($w, $h)
        $g = [Drawing.Graphics]::FromImage($img)
        $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $img.Size)
        $typedPath = Join-Path $artifactDir "verify-ui-typed.png"
        $img.Save($typedPath, [Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose()
        $img.Dispose()
        Write-Output "Saved typed screenshot to $typedPath"
    }

    # Clear input with Ctrl+A and Backspace
    [void][LauncherUiCheck]::PostMessage($window, 0x0100, [IntPtr]0x41, [IntPtr]0) # VK_A (Ctrl+A simulated)
    # Or send Esc then Show again
    [void][LauncherUiCheck]::PostMessage($window, 0x0100, [IntPtr]0x1B, [IntPtr]0) # Esc
    Start-Sleep -Milliseconds 400
    [void][LauncherUiCheck]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero) # Show
    Start-Sleep -Milliseconds 400

    # Type Chinese text: '设置'
    Write-Output "Typing Chinese '设置'..."
    foreach ($ch in '设置'.ToCharArray()) {
        [void][LauncherUiCheck]::PostMessage($window, 0x0102, [IntPtr][int]$ch, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 600
    [void][LauncherUiCheck]::DwmFlush()

    [void][LauncherUiCheck]::GetWindowRect($window, [ref]$rect)
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    Write-Output "After typing '设置' rect: Left=$($rect.Left), Top=$($rect.Top), Width=$w, Height=$h"

    if ($w -gt 0 -and $h -gt 0) {
        $img = [Drawing.Bitmap]::new($w, $h)
        $g = [Drawing.Graphics]::FromImage($img)
        $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $img.Size)
        $cjkPath = Join-Path $artifactDir "verify-ui-cjk.png"
        $img.Save($cjkPath, [Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose()
        $img.Dispose()
        Write-Output "Saved CJK screenshot to $cjkPath"
    }
}
finally {
    if ($proc -and -not $proc.HasExited) {
        $proc.Kill()
        $proc.WaitForExit(1000)
    }
}

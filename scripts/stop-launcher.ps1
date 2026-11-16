$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
$running = @(Get-Process -Name mini-launcher -ErrorAction SilentlyContinue | Where-Object {
    -not $_.Path -or $_.Path.StartsWith($projectRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
})
if ($running.Count -eq 0) { return }
if (-not ('MiniLauncherStopNative' -as [type])) {
    Add-Type 'using System; using System.Runtime.InteropServices; public static class MiniLauncherStopNative { [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t); [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l); }'
}
$window = [MiniLauncherStopNative]::FindWindow('MiniLauncher.Native.Window', 'MiniLauncher')
if ($window -ne [IntPtr]::Zero) { [void][MiniLauncherStopNative]::PostMessage($window, 0x0111, [IntPtr]4, [IntPtr]::Zero) }
foreach ($process in $running) {
    if (-not $process.WaitForExit(3000)) { Stop-Process -Id $process.Id -Force }
    Write-Output "已关闭 MiniLauncher PID $($process.Id)"
}

param(
    [ValidateSet('run', 'build', 'test', 'check', 'fmt', 'clippy')]
    [string]$Task = 'run',
    [switch]$Release,
    [string]$Example,
    [string]$Bin,
    [switch]$Background
)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    if (Test-Path -LiteralPath "$PSScriptRoot\.tools\cargo\bin\cargo.exe") {
        $env:CARGO_HOME = "$PSScriptRoot\.tools\cargo"
        $env:RUSTUP_HOME = "$PSScriptRoot\.tools\rustup"
        $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    }
    if ($Task -eq 'run' -and (-not $Bin -or $Bin -eq 'mini-launcher')) {
        # A resident release binary otherwise wins the single-instance check,
        # making cargo run silently activate an older build.
        $projectExecutables = @("$PSScriptRoot\target\debug\mini-launcher.exe", "$PSScriptRoot\target\release\mini-launcher.exe")
        $running = @(Get-Process mini-launcher -ErrorAction SilentlyContinue | Where-Object { -not $_.Path -or $_.Path -in $projectExecutables })
        if ($running.Count -gt 0) {
            if (-not ('MiniLauncherDevNative' -as [type])) {
                Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MiniLauncherDevNative {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
            }
            $window = [MiniLauncherDevNative]::FindWindow('MiniLauncher.Native.Window','MiniLauncher')
            $owner = 0
            [void][MiniLauncherDevNative]::GetWindowThreadProcessId($window,[ref]$owner)
            if ($owner -in $running.Id) { [void][MiniLauncherDevNative]::PostMessage($window,0x0111,[IntPtr]4,[IntPtr]::Zero) }
            foreach ($process in $running) {
                if (-not $process.WaitForExit(1000)) {
                    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                }
            }
        }
    }
    $cargoArgs = @($Task)
    if ($Task -ne 'fmt') { $cargoArgs += @('--target-dir', (Join-Path $PSScriptRoot 'target')) }
    if ($Example) { $cargoArgs += @('--example', $Example) }
    if ($Bin) { $cargoArgs += @('--bin', $Bin) }
    if ($Release -and $Task -in @('run','build','test','check')) { $cargoArgs += '--release' }
    if ($Task -eq 'clippy') { $cargoArgs += @('--all-targets', '--', '-D', 'warnings') }
    if ($Task -eq 'run' -and $Background) { $cargoArgs += @('--', '--background') }
    if ($Task -eq 'run' -and (-not $Bin -or $Bin -eq 'mini-launcher')) {
        $settingsArgs = @('build', '-p', 'mini-launcher-config', '--target-dir', (Join-Path $PSScriptRoot 'target'))
        if ($Release) { $settingsArgs += '--release' }
        & cargo @settingsArgs
        if ($LASTEXITCODE -ne 0) { throw 'GUI 设置管理器编译失败' }
    }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo $Task failed (exit $LASTEXITCODE)" }
} finally { Pop-Location }

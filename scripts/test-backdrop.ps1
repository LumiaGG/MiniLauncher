param(
    [string]$Executable = "$PSScriptRoot\..\target\release\mini-launcher.exe",
    [ValidateSet('dark','light')][string]$Theme = 'dark',
    [ValidateSet('mica','acrylic','none')][string[]]$Modes = @('mica','acrylic','none'),
    [string]$ExpectedAccent = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'host-input.ps1')
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
if (-not ('BackdropCheck' -as [type])) {
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class BackdropCheck {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendKey(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr a,int x,int y,int w,int ht,uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h,uint a,out int v,uint size);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
}
'@
}
[void][BackdropCheck]::SetThreadDpiAwarenessContext([IntPtr](-4))
$root = Split-Path $PSScriptRoot -Parent
$resident = Get-Process mini-launcher -ErrorAction SilentlyContinue | Where-Object { $_.Path -in @((Join-Path $root 'target/debug/mini-launcher.exe').Replace('/','\'),(Join-Path $root 'target/release/mini-launcher.exe').Replace('/','\')) } | Select-Object -First 1
$restorePath = if ($resident) { $resident.Path } else { $null }
$artifactDir = Join-Path $root 'artifacts'
$testData = Join-Path $artifactDir 'backdrop-appdata'
[void][IO.Directory]::CreateDirectory((Join-Path $testData 'MiniLauncher'))
$settings = Join-Path $testData 'MiniLauncher/settings.toml'
$form = [Windows.Forms.Form]::new()
$form.Text = 'MiniLauncher backdrop verification'
$form.FormBorderStyle = 'None'
$form.StartPosition = 'Manual'
$form.Bounds = [Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$form.BackColor = [Drawing.Color]::RoyalBlue
$form.Show()
[Windows.Forms.Application]::DoEvents()

$start = [Diagnostics.ProcessStartInfo]::new([IO.Path]::GetFullPath($Executable))
$start.UseShellExecute = $false
$start.WindowStyle = 'Hidden'
$start.Arguments = '--background'
$start.Environment['APPDATA'] = $testData
$start.Environment['MINILAUNCHER_DATA_DIR'] = Join-Path $testData 'MiniLauncher'
$process = $null
try {
    $existing = [BackdropCheck]::FindWindow('MiniLauncher.Native.Window','MiniLauncher')
    if ($existing -ne [IntPtr]::Zero) {
        if (-not $resident) { throw 'An unrelated launcher is running' }
        [void][BackdropCheck]::PostMessage($existing,0x0111,[IntPtr]4,[IntPtr]::Zero)
        if (-not $resident.WaitForExit(3000)) { throw 'Existing launcher did not exit' }
    }
    @'
theme = "dark"
backdrop = "mica"
hotkey = "Ctrl+Alt+F11"
[everything]
enabled = false
[plugins]
enabled = false
'@ -replace 'theme = "dark"',('theme = "'+$Theme+'"') | Set-Content -LiteralPath $settings -Encoding utf8
    $process = [Diagnostics.Process]::Start($start)
    $window = [IntPtr]::Zero
    for ($i=0; $i -lt 100; $i++) {
        $window = [BackdropCheck]::FindWindow('MiniLauncher.Native.Window','MiniLauncher')
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    }
    if ($window -eq [IntPtr]::Zero) { throw 'Launcher did not start' }
    [void][BackdropCheck]::PostMessage($window,0x8003,[IntPtr]::Zero,[IntPtr]::Zero)
    Start-Sleep -Milliseconds 400
    foreach ($mode in $Modes) {
        (Get-Content -Raw -LiteralPath $settings) -replace 'backdrop = "[^"]+"',('backdrop = "'+$mode+'"') | Set-Content -LiteralPath $settings -Encoding utf8
        Start-Sleep -Milliseconds 500
        [void][BackdropCheck]::SetForegroundWindow($window)
        [void][BackdropCheck]::SetWindowPos($window,[IntPtr](-1),($form.Left+120),($form.Top+180),0,0,0x11)
        [void][BackdropCheck]::DwmFlush()
        $material = 0
        $hr = [BackdropCheck]::DwmGetWindowAttribute($window,38,[ref]$material,4)
        $expectedMaterial = @{mica=2;acrylic=3;none=1}[$mode]
        if ($hr -lt 0 -or $material -ne $expectedMaterial) { throw "Expected material $expectedMaterial; actual=$material hr=$hr" }
        Write-Output "$mode material=$material hr=$hr"
        foreach ($stage in @('placeholder','typed','selected','cleared')) {
            switch ($stage) {
                'placeholder' { Set-HostInput $window '' }
                'typed' { Set-HostInput $window '中文 DPI Acrylic' }
                'selected' { Select-HostInput $window }
                'cleared' { Set-HostInput $window '' }
            }
            [Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 250
            [void][BackdropCheck]::DwmFlush()
            $rect = [BackdropCheck+Rect]::new()
            [void][BackdropCheck]::GetWindowRect($window,[ref]$rect)
            $image = [Drawing.Bitmap]::new(($rect.Right-$rect.Left),($rect.Bottom-$rect.Top))
            $graphics = [Drawing.Graphics]::FromImage($image)
            try {
                $graphics.CopyFromScreen($rect.Left,$rect.Top,0,0,$image.Size)
                $path = Join-Path $artifactDir "backdrop-$mode-$stage.png"
                $image.Save($path)
                # Width is configurable; derive logical coordinates from DPI.
                $scale = [BackdropCheck]::GetDpiForWindow($window)/96.0
                $inside = $image.GetPixel([int](550*$scale),[int](20*$scale))
                $outside = $image.GetPixel([int](550*$scale),[int](48*$scale))
                $difference = [Math]::Abs($inside.R-$outside.R)+[Math]::Abs($inside.G-$outside.G)+[Math]::Abs($inside.B-$outside.B)
                Write-Output "$mode stage=$stage backgroundDifference=$difference inside=$($inside.R),$($inside.G),$($inside.B) outside=$($outside.R),$($outside.G),$($outside.B) image=$path"
                if ($difference -gt 20) { throw "$mode $stage input has a separate opaque background" }
                if ($stage -eq 'typed') {
                    if ($ExpectedAccent) {
                        $expectedRgb = [int[]]($ExpectedAccent -split ',')
                        if ($expectedRgb.Length -ne 3) { throw 'ExpectedAccent must be R,G,B' }
                        $marker = $image.GetPixel([int](22*$scale),[int](93*$scale))
                        $accentDifference = [Math]::Abs($marker.R-$expectedRgb[0])+[Math]::Abs($marker.G-$expectedRgb[1])+[Math]::Abs($marker.B-$expectedRgb[2])
                        if ($accentDifference -gt 3) { throw "Selection marker does not match accent $ExpectedAccent; actual=$($marker.R),$($marker.G),$($marker.B)" }
                        Write-Output "Selection marker matches system accent $ExpectedAccent"
                    }
                    $readablePixels = 0
                    for ($y=[int](14*$scale); $y -lt [int](42*$scale); $y++) {
                        for ($x=[int](128*$scale); $x -lt [int](300*$scale); $x++) {
                            $textPixel = $image.GetPixel($x,$y)
                            if ($Theme -eq 'dark' -and [Math]::Min($textPixel.R,[Math]::Min($textPixel.G,$textPixel.B)) -gt 190) { $readablePixels++ }
                            if ($Theme -eq 'light' -and [Math]::Max($textPixel.R,[Math]::Max($textPixel.G,$textPixel.B)) -lt 70) { $readablePixels++ }
                        }
                    }
                    if ($readablePixels -lt 30) { throw "$mode input text is missing or lacks contrast" }
                }
            } finally { $graphics.Dispose(); $image.Dispose() }
        }
    }
} finally {
    if ($null -ne $process -and -not $process.HasExited) {
        [void][BackdropCheck]::PostMessage($window,0x0111,[IntPtr]4,[IntPtr]::Zero)
        if (-not $process.WaitForExit(3000)) { $process.Kill() }
    }
    $form.Close()
    $form.Dispose()
    if ($restorePath) {
        $restore = [Diagnostics.ProcessStartInfo]::new($restorePath)
        $restore.UseShellExecute = $false
        $restore.WindowStyle = 'Hidden'
        $restore.Arguments = '--background'
        [void][Diagnostics.Process]::Start($restore)
    }
}

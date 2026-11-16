param(
    [string]$Executable = "$PSScriptRoot\..\target\release\mini-launcher.exe",
    [ValidateSet('light','dark')][string]$InitialTheme = 'light',
    [switch]$Integrations
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'host-input.ps1')
Add-Type -AssemblyName System.Drawing
if (-not ('MiniLauncherSmoke.Native' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
namespace MiniLauncherSmoke {
    public static class Native {
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
        [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
        [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendKey(IntPtr h,uint m,IntPtr w,IntPtr l);
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
        [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
        [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
        [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr h,int id,uint modifiers,uint key);
        [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h,int id);
        [DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint action,uint parameter,out uint value,uint flags);
        [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr process,uint flags);
        [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
        [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h,uint attribute,out int value,uint size);
        [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
    }
}
'@
}
function Assert-True($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Wait-Until([scriptblock]$Condition, [int]$Milliseconds = 10000) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while (-not (& $Condition)) {
        if ($timer.ElapsedMilliseconds -gt $Milliseconds) { throw 'Timed out waiting for native window' }
        Start-Sleep -Milliseconds 25
    }
}
function Capture-Window([IntPtr]$Window, [string]$Path, [switch]$NoRepaint) {
    if ($NoRepaint) { Capture-Display $Window $Path; return }
    [void][MiniLauncherSmoke.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))
    $rect = New-Object MiniLauncherSmoke.Native+Rect
    [void][MiniLauncherSmoke.Native]::GetWindowRect($Window, [ref]$rect)
    $bitmap = New-Object Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    try {
        Assert-True ([MiniLauncherSmoke.Native]::PrintWindow($Window, $dc, 2)) 'PrintWindow failed'
    }
    finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
    try { $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png) } finally { $bitmap.Dispose() }
}
function Capture-Display([IntPtr]$Window, [string]$Path) {
    [void][MiniLauncherSmoke.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))
    $rect = New-Object MiniLauncherSmoke.Native+Rect
    [void][MiniLauncherSmoke.Native]::GetWindowRect($Window,[ref]$rect)
    $bitmap = [Drawing.Bitmap]::new(($rect.Right-$rect.Left),($rect.Bottom-$rect.Top))
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left,$rect.Top,0,0,$bitmap.Size)
        $bitmap.Save($Path,[Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose();$bitmap.Dispose() }
}
function Assert-LaunchedTarget([string]$Target, [scriptblock]$Action) {
    $key = $Target.ToLowerInvariant()
    $previousCount = 0
    if (Test-Path -LiteralPath $historyPath) {
        $existing = Get-Content -Raw -LiteralPath $historyPath | ConvertFrom-Json -AsHashtable
        if ($existing.ContainsKey($key)) { $previousCount = $existing[$key].count }
    }
    if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker }
    & $Action
    Wait-Until { Test-Path -LiteralPath $marker }
    Wait-Until {
        if (-not (Test-Path -LiteralPath $historyPath)) { return $false }
        $current = Get-Content -Raw -LiteralPath $historyPath | ConvertFrom-Json -AsHashtable
        $current.ContainsKey($key) -and $current[$key].count -gt $previousCount
    }
    [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
    Wait-Until { [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
}
$artifactDir = [IO.Path]::GetFullPath("$PSScriptRoot\..\artifacts")
$testData = Join-Path $artifactDir 'smoke-appdata'
[void][IO.Directory]::CreateDirectory((Join-Path $testData 'MiniLauncher'))
$historyPath = Join-Path $testData 'MiniLauncher\history.json'
if (Test-Path -LiteralPath $historyPath) { Remove-Item -LiteralPath $historyPath }
$fixtureDirectory = [IO.Path]::GetFullPath("$PSScriptRoot\..\target\test-fixtures")
[void][IO.Directory]::CreateDirectory($fixtureDirectory)
$fixture = Join-Path $fixtureDirectory 'smoke app 启动.exe'
$marker = Join-Path $artifactDir 'launch-marker.txt'
if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker }
$localCargo = [IO.Path]::GetFullPath("$PSScriptRoot\..\.tools\cargo")
if (Test-Path -LiteralPath "$localCargo\bin\rustc.exe") {
    $env:CARGO_HOME = $localCargo
    $env:RUSTUP_HOME = [IO.Path]::GetFullPath("$PSScriptRoot\..\.tools\rustup")
    $rustc = "$localCargo\bin\rustc.exe"
} else { $rustc = (Get-Command rustc -ErrorAction Stop).Source }
& $rustc --edition=2021 "$PSScriptRoot\launch-fixture.rs" -C opt-level=s -o $fixture
if ($LASTEXITCODE -ne 0) { throw 'Failed to build the launch fixture' }
((@'
theme = "light"
backdrop = "none"
window_width = 720
hotkey = "Ctrl+Alt+Space"
max_results = 6
search_paths = []
[aliases]
'@) + "`n'MiniLauncher Launch Check' = '$fixture'`n").Replace('theme = "light"', "theme = `"$InitialTheme`"") | Set-Content -LiteralPath (Join-Path $testData 'MiniLauncher\settings.toml') -Encoding utf8
foreach ($group in @('Keyboard','Wheel','Drag')) {
    foreach ($number in 1..20) {
        $target = Join-Path $artifactDir ('{0}-item-{1:D2}.exe' -f $group.ToLowerInvariant(),$number)
        Copy-Item -LiteralPath $fixture -Destination $target -Force
        "'Scroll {0} {1:D2}' = '{2}'" -f $group,$number,$target | Add-Content -LiteralPath (Join-Path $testData 'MiniLauncher\settings.toml') -Encoding utf8
    }
}
$pinyinTarget = Join-Path $artifactDir 'pinyin-wechat.exe'
Copy-Item -LiteralPath $fixture -Destination $pinyinTarget -Force
"'微信' = '$pinyinTarget'" | Add-Content -LiteralPath (Join-Path $testData 'MiniLauncher\settings.toml') -Encoding utf8
# A real WeChat installation may also match wx. Pin the isolated fixture above it
# through test-only history so the click never launches the user's application.
$fixtureHistory = @{}
$fixtureHistory[$pinyinTarget.ToLowerInvariant()] = @{ count = 30; last_used = 1 }
$fixtureHistory | ConvertTo-Json | Set-Content -LiteralPath $historyPath -Encoding utf8
if ($Integrations) {
    $pluginBinary = [IO.Path]::GetFullPath("$PSScriptRoot\..\target\release\examples\echo-plugin.exe")
    Assert-True (Test-Path -LiteralPath $pluginBinary) 'Build the echo-plugin example before running integration tests'
    foreach ($entry in @(
        @{id='smoke-echo'; keyword='echo'; args=@('--fixture-target',$fixture)},
        @{id='smoke-hang'; keyword='hang'; args=@('--hang-query')},
        @{id='smoke-malformed'; keyword='badjson'; args=@('--malformed-query')}
    )) {
        $pluginDir = Join-Path $testData "MiniLauncher\plugins\$($entry.id)"
        [void][IO.Directory]::CreateDirectory($pluginDir)
        Copy-Item -LiteralPath $pluginBinary -Destination (Join-Path $pluginDir 'echo-plugin.exe') -Force
        @{id=$entry.id;name=$entry.id;protocol_version=1;executable='echo-plugin.exe';args=$entry.args;keywords=@($entry.keyword);timeout_ms=300;idle_timeout_secs=1} |
            ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $pluginDir 'plugin.json') -Encoding utf8
    }
    $fileTarget = Join-Path $artifactDir 'minilauncher-everything-smoke.exe'
    Copy-Item -LiteralPath $fixture -Destination $fileTarget -Force
}
& (Join-Path $PSScriptRoot 'stop-launcher.ps1')
$start = New-Object Diagnostics.ProcessStartInfo
$start.FileName = [IO.Path]::GetFullPath($Executable)
$start.Arguments = '--background'
$start.UseShellExecute = $false
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$start.Environment['APPDATA'] = $testData
$start.Environment['MINILAUNCHER_DATA_DIR'] = Join-Path $testData 'MiniLauncher'
$start.Environment['MINILAUNCHER_SMOKE_MARKER'] = $marker
$process = [Diagnostics.Process]::Start($start)
try {
    $script:window = [IntPtr]::Zero
    Wait-Until { $script:window = [MiniLauncherSmoke.Native]::FindWindow('MiniLauncher.Native.Window', 'MiniLauncher'); $script:window -ne [IntPtr]::Zero }
    Start-Sleep -Milliseconds 500
    Assert-True (-not [MiniLauncherSmoke.Native]::IsWindowVisible($window)) '--background unexpectedly showed a window'
    $registered = [MiniLauncherSmoke.Native]::RegisterHotKey([IntPtr]::Zero, 91, 3, 32)
    if ($registered) { [void][MiniLauncherSmoke.Native]::UnregisterHotKey([IntPtr]::Zero, 91) }
    Assert-True (-not $registered) 'The configured global hotkey was not registered'
    [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
    Wait-Until { [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
    Start-Sleep -Milliseconds 500
    Capture-Window $window (Join-Path $artifactDir "launcher-$InitialTheme.png")
    if ($InitialTheme -eq 'dark') {
        foreach ($active in @(0,1,0,1)) {
            [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0086, [IntPtr]$active, [IntPtr]::Zero)
            [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0085, [IntPtr]1, [IntPtr]::Zero)
        }
        $frameScreenshot = Join-Path $artifactDir 'launcher-activation-frame.png'
        Capture-Window $window $frameScreenshot -NoRepaint
        $frame = [Drawing.Bitmap]::new($frameScreenshot)
        try {
            $edge = $frame.GetPixel(3, [int]($frame.Height/2))
            Assert-True ($edge.R -lt 100 -and $edge.G -lt 100 -and $edge.B -lt 100) 'Activation repainted a light legacy window frame'
        } finally { $frame.Dispose() }
    }
    # Exercise the host-owned editor through its keyboard path.
    Set-HostInput $window ('')
    [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0102, [IntPtr]97, [IntPtr]::Zero)
    [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0102, [IntPtr]97, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 150
    $typedScreenshot = Join-Path $artifactDir "launcher-typed-$InitialTheme.png"
    Capture-Window $window $typedScreenshot
    $typed = [Drawing.Bitmap]::new($typedScreenshot)
    try {
        $scale = $typed.Width / 720.0
        $contrastingPixels = 0
        # The caret is light even when the glyphs are incorrectly black.
        # Restrict this region to the bodies of the two 'a' glyphs.
        for ($x = [int](130*$scale); $x -lt [int](146*$scale); $x++) {
            for ($y = [int](20*$scale); $y -lt [int](38*$scale); $y++) {
                $pixel = $typed.GetPixel($x,$y)
                if ($InitialTheme -eq 'dark' -and $pixel.R -gt 150 -and $pixel.G -gt 150 -and $pixel.B -gt 150) { $contrastingPixels++ }
                if ($InitialTheme -eq 'light' -and $pixel.R -lt 120 -and $pixel.G -lt 120 -and $pixel.B -lt 120) { $contrastingPixels++ }
            }
        }
        Assert-True ($contrastingPixels -gt 10) 'Typed text has insufficient contrast after initial DPI/font setup'
        $background = $typed.GetPixel([int](180*$scale),[int](28*$scale))
        if ($InitialTheme -eq 'dark') { Assert-True ($background.R -lt 70 -and $background.G -lt 70 -and $background.B -lt 70) 'Dark input has an unexpected light background' }
    } finally { $typed.Dispose() }
    Set-HostInput $window ('jsb')
    Start-Sleep -Milliseconds 150
    Capture-Window $window (Join-Path $artifactDir 'launcher-pinyin.png')
    Set-HostInput $window ('(2 + 3) * 4')
    Start-Sleep -Milliseconds 150
    Capture-Window $window (Join-Path $artifactDir 'launcher-calculator.png')
    Set-HostInput $window ('MiniLauncher Launch Check')
    Start-Sleep -Milliseconds 150
    [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0100, [IntPtr]13, [IntPtr]::Zero)
    Wait-Until { Test-Path -LiteralPath $marker }
    Assert-True ((Get-Content -Raw -LiteralPath $marker) -eq 'launched') 'ShellExecute did not launch the fixture'
    $historyPath = Join-Path $testData 'MiniLauncher\history.json'
    Wait-Until { Test-Path -LiteralPath $historyPath }
    $history = Get-Content -Raw -LiteralPath $historyPath | ConvertFrom-Json -AsHashtable
    Assert-True ($history.ContainsKey($fixture.ToLowerInvariant())) 'Successful launch was not persisted to history'
    [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
    Wait-Until { [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
    Set-HostInput $window ('Scroll Keyboard')
    Start-Sleep -Milliseconds 200
    foreach ($step in 1..7) { [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]40,[IntPtr]::Zero) }
    Start-Sleep -Milliseconds 100
    Assert-LaunchedTarget (Join-Path $artifactDir 'keyboard-item-08.exe') {
        [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]13,[IntPtr]::Zero)
    }
    Set-HostInput $window ('Scroll Wheel')
    Start-Sleep -Milliseconds 200
    $wheelLines = 3
    [void][MiniLauncherSmoke.Native]::SystemParametersInfo(0x0068,0,[ref]$wheelLines,0)
    $scrollFirst = if ($wheelLines -eq [uint32]::MaxValue) { 6 } else { [Math]::Min(14,$wheelLines) }
    # The input control has focus; its wheel message must reach the result list.
    [void][MiniLauncherSmoke.Native]::SendKey($window,0x020A,[IntPtr](-7864320),[IntPtr]::Zero)
    Start-Sleep -Milliseconds 150
    $clickPoint = [IntPtr](([int](306*$scale) -shl 16) -bor [int](120*$scale))
    Assert-LaunchedTarget (Join-Path $artifactDir ('wheel-item-{0:D2}.exe' -f ($scrollFirst+3))) {
        [void][MiniLauncherSmoke.Native]::SendKey($window,0x0202,[IntPtr]::Zero,$clickPoint)
    }
    Set-HostInput $window ('Scroll Drag')
    Start-Sleep -Milliseconds 200
    $thumbStart = [IntPtr](([int](162*$scale) -shl 16) -bor [int](707*$scale))
    $thumbEnd = [IntPtr](([int](519*$scale) -shl 16) -bor [int](707*$scale))
    [void][MiniLauncherSmoke.Native]::SendKey($window,0x0201,[IntPtr]1,$thumbStart)
    [void][MiniLauncherSmoke.Native]::SendKey($window,0x0200,[IntPtr]1,$thumbEnd)
    [void][MiniLauncherSmoke.Native]::SendKey($window,0x0202,[IntPtr]::Zero,$thumbEnd)
    Start-Sleep -Milliseconds 150
    Capture-Window $window (Join-Path $artifactDir 'launcher-scroll-bottom.png')
    $firstRow = [IntPtr](([int](182*$scale) -shl 16) -bor [int](120*$scale))
    Assert-LaunchedTarget (Join-Path $artifactDir 'drag-item-15.exe') {
        [void][MiniLauncherSmoke.Native]::SendKey($window,0x0202,[IntPtr]::Zero,$firstRow)
    }
    Set-HostInput $window ('Scroll Drag 01')
    Start-Sleep -Milliseconds 200
    Assert-LaunchedTarget (Join-Path $artifactDir 'drag-item-01.exe') {
        [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]13,[IntPtr]::Zero)
    }
    # Real IME composition/candidate behavior requires a user-run desktop check.
    if ($Integrations) {
        # Execute the callback item, avoiding changes to the user's clipboard.
        Set-HostInput $window ('echo first')
        Start-Sleep -Milliseconds 300
        $echoPath = Join-Path $testData 'MiniLauncher\plugins\smoke-echo\echo-plugin.exe'
        $firstPlugin = @(Get-Process echo-plugin -ErrorAction SilentlyContinue | Where-Object Path -EQ $echoPath)
        Assert-True ($firstPlugin.Count -eq 1) 'Echo plugin did not start exactly one process'
        Set-HostInput $window ('echo integration')
        Start-Sleep -Milliseconds 150
        $secondPlugin = @(Get-Process echo-plugin -ErrorAction SilentlyContinue | Where-Object Path -EQ $echoPath)
        Assert-True ($secondPlugin.Count -eq 1 -and $firstPlugin[0].Id -eq $secondPlugin[0].Id) 'Plugin was restarted for each query'
        Capture-Window $window (Join-Path $artifactDir 'launcher-plugin.png')
        [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]40,[IntPtr]::Zero)
        Assert-LaunchedTarget $fixture { [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]13,[IntPtr]::Zero) }
        Set-HostInput $window ('MiniLauncher Launch Check')
        Wait-Until { @(Get-Process echo-plugin -ErrorAction SilentlyContinue | Where-Object Path -EQ $echoPath).Count -eq 0 } 4000
        foreach ($badQuery in @('hang timeout','badjson invalid')) {
            Set-HostInput $window ($badQuery)
            Start-Sleep -Milliseconds 600
            Capture-Window $window (Join-Path $artifactDir ($badQuery.Split(' ')[0] + '-plugin.png'))
            Set-HostInput $window ('MiniLauncher Launch Check')
            Start-Sleep -Milliseconds 150
            Assert-LaunchedTarget $fixture { [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]13,[IntPtr]::Zero) }
        }
        Set-HostInput $window ('hang cancel-me')
        Start-Sleep -Milliseconds 120
        Set-HostInput $window ('MiniLauncher Launch Check')
        $hangPath = Join-Path $testData 'MiniLauncher\plugins\smoke-hang\echo-plugin.exe'
        Wait-Until { @(Get-Process echo-plugin -ErrorAction SilentlyContinue | Where-Object Path -EQ $hangPath).Count -eq 0 } 2000
        Set-HostInput $window (('e "' + $fileTarget + '"'))
        Start-Sleep -Milliseconds 600
        Capture-Window $window (Join-Path $artifactDir 'launcher-everything.png')
        Assert-LaunchedTarget $fileTarget { [void][MiniLauncherSmoke.Native]::SendKey($window,0x0100,[IntPtr]13,[IntPtr]::Zero) }
        $settingsFile = Join-Path $testData 'MiniLauncher\settings.toml'
        foreach ($material in @(@{name='mica';value=2},@{name='acrylic';value=3})) {
            $configText = Get-Content -Raw -LiteralPath $settingsFile
            $configText -replace 'backdrop = "[^"]+"',('backdrop = "'+$material.name+'"') | Set-Content -LiteralPath $settingsFile -Encoding utf8
            [void][MiniLauncherSmoke.Native]::PostMessage($window,0x0111,[IntPtr]3,[IntPtr]::Zero)
            Start-Sleep -Milliseconds 300
            $actualMaterial = 0
            $attributeResult = [MiniLauncherSmoke.Native]::DwmGetWindowAttribute($window,38,[ref]$actualMaterial,4)
            if ($attributeResult -ge 0) { Assert-True ($actualMaterial -eq $material.value) 'DWM backdrop attribute was not applied' }
            Capture-Window $window (Join-Path $artifactDir ('launcher-'+$material.name+'.png'))
            Capture-Display $window (Join-Path $artifactDir ('launcher-'+$material.name+'-display.png'))
        }
        (Get-Content -Raw -LiteralPath $settingsFile) -replace 'backdrop = "[^"]+"','backdrop = "none"' | Set-Content -LiteralPath $settingsFile -Encoding utf8
        [void][MiniLauncherSmoke.Native]::PostMessage($window,0x0111,[IntPtr]3,[IntPtr]::Zero)
        Start-Sleep -Milliseconds 200
    }
    $initialGdi = [MiniLauncherSmoke.Native]::GetGuiResources($process.Handle, 0)
    foreach ($i in 1..200) {
        Set-HostInput $window ("g stress $i")
    }
    Set-HostInput $window ('g 中文 & Rust')
    Start-Sleep -Milliseconds 200
    Capture-Window $window (Join-Path $artifactDir 'launcher-web.png')
    $finalGdi = [MiniLauncherSmoke.Native]::GetGuiResources($process.Handle, 0)
    Assert-True ($finalGdi -le $initialGdi + 10) 'GDI object count grew unexpectedly during query stress'
    [void][MiniLauncherSmoke.Native]::SendKey($window, 0x0100, [IntPtr]27, [IntPtr]::Zero)
    Wait-Until { -not [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
    $duplicate = [Diagnostics.Process]::Start($start)
    Assert-True ($duplicate.WaitForExit(3000)) 'Second instance did not exit'
    Wait-Until { [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
    # Theme reload uses the same path as the tray command, without touching user settings.
    $configPath = Join-Path $testData 'MiniLauncher\settings.toml'
    (Get-Content -Raw -LiteralPath $configPath).Replace('theme = "light"', 'theme = "dark"') | Set-Content -LiteralPath $configPath -Encoding utf8
    [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x0111, [IntPtr]3, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 300
    Capture-Window $window (Join-Path $artifactDir 'launcher-dark.png')
    [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    Wait-Until { -not [MiniLauncherSmoke.Native]::IsWindowVisible($window) }
    $process.Refresh()
    $cpu = $process.TotalProcessorTime.TotalMilliseconds
    Start-Sleep -Seconds 3
    $process.Refresh()
    $report = [ordered]@{
        passed = $true
        initial_theme = $InitialTheme
        activation_frame_checked = ($InitialTheme -eq 'dark')
        integrations_checked = [bool]$Integrations
        checks = @('background startup','host-owned editor','global hotkey registration','typed text contrast after initial DPI setup','Unicode input','Enter launches Unicode path with spaces','history persistence','keyboard selects beyond six rows','wheel and offset-aware click','scrollbar drag to last page','query resets scroll position','200 rapid queries','stable GDI objects','Escape hides','single instance','theme reload','close hides')
        private_memory_mb = [Math]::Round($process.PrivateMemorySize64 / 1MB, 2)
        working_set_mb = [Math]::Round($process.WorkingSet64 / 1MB, 2)
        idle_cpu_ms_over_3s = [Math]::Round($process.TotalProcessorTime.TotalMilliseconds - $cpu, 3)
        manual_checks = @('real IME composition and candidate window', 'cross-monitor DPI')
        gdi_before = $initialGdi
        gdi_after = $finalGdi
        binary_kb = [Math]::Round((Get-Item -LiteralPath $start.FileName).Length / 1KB, 1)
        limitations = 'IME candidate handling and multi-monitor DPI require manual testing. This does not measure end-to-end query latency.'
    }
    $report | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactDir 'smoke-report.json') -Encoding utf8
    $report | ConvertTo-Json
} finally {
    if (-not $process.HasExited) {
        [void][MiniLauncherSmoke.Native]::PostMessage($window, 0x0111, [IntPtr]4, [IntPtr]::Zero)
        if (-not $process.WaitForExit(3000)) { $process.Kill(); $process.WaitForExit() }
    }
    $process.Dispose()
}

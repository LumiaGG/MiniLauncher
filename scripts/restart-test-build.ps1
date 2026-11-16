[CmdletBinding()]
param(
    [string]$OutputDirectory = "$PSScriptRoot\..\target\packages",
    [string]$InstallDirectory,
    [switch]$SkipStart,
    [switch]$RebuildSettings
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
if (-not $InstallDirectory) { $InstallDirectory = Join-Path $output 'unpacked/MiniLauncher' }
$install = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDirectory)
$pluginIds = @('path-opener','two-factor','clipboard-history','caffeine','process-killer','piin')
. "$PSScriptRoot\data-directory.ps1"
# Resolve the active data location before touching any running component.
$dataDirectory = Get-LauncherDataDirectory $install
Write-Output '关闭现有宿主及相关组件……'
& "$PSScriptRoot\stop-launcher.ps1"
foreach ($process in @(Get-Process -Name (@('mini-launcher', 'mini-launcher-config') + $pluginIds) -ErrorAction SilentlyContinue)) {
    # Only terminate components from this project or this installation's plugin directories.
    if ($process.Path -and (
        $process.Path.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        $process.Path.StartsWith($install + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        $process.Path.StartsWith((Join-Path $dataDirectory 'plugins') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase))) {
        Stop-Process -Id $process.Id -Force -ErrorAction Stop
        [void]$process.WaitForExit(3000)
    }
}
Write-Output '增量编译宿主及内置插件，按需编译设置管理器，并更新发行包……'
& "$PSScriptRoot\build-release.ps1" -OutputDirectory $output -ReuseSettings:(-not $RebuildSettings)
if ([IO.Path]::GetFullPath($install) -ne [IO.Path]::GetFullPath((Join-Path $output 'unpacked/MiniLauncher'))) {
    [void][IO.Directory]::CreateDirectory($install)
    foreach ($name in @('mini-launcher.exe', 'mini-launcher-config.exe')) {
        Copy-Item -LiteralPath (Join-Path $output "unpacked/MiniLauncher/$name") -Destination (Join-Path $install $name) -Force
    }
}
foreach ($id in $pluginIds) {
    $directory = Join-Path $dataDirectory "plugins/$id"
    if (-not (Test-Path -LiteralPath $directory -PathType Container)) { continue }
    Copy-Item -LiteralPath (Join-Path $output "unpacked/$id/$id.exe") -Destination (Join-Path $directory "$id.exe") -Force
    foreach ($asset in @('settings.default.toml', 'settings.schema.json', 'Images')) {
        $assetSource = Join-Path $output "unpacked/$id/$asset"
        if (Test-Path -LiteralPath $assetSource) {
            Copy-Item -LiteralPath $assetSource -Destination (Join-Path $directory $asset) -Recurse -Force
        }
    }
    $manifestPath = Join-Path $directory 'plugin.json'
    # Refresh protocol and hotkey declarations, preserving installed keywords/arguments and all plugin data.
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $source = Get-Content -LiteralPath (Join-Path $root "plugins/$id/plugin.json") -Raw | ConvertFrom-Json
    $manifest.protocol_version = $source.protocol_version
    $manifest.timeout_ms = $source.timeout_ms
    $manifest.PSObject.Properties.Remove('accepts_images')
    if ($source.accepts_images) {
        $manifest | Add-Member -MemberType NoteProperty -Name accepts_images -Value $true
    }
    # Hotkey declarations are plugin-owned; refresh them and drop removed fields (unknown fields are rejected).
    foreach ($obsolete in @('selection_hotkey', 'global_hotkey')) {
        $manifest.PSObject.Properties.Remove($obsolete)
    }
    $manifest.PSObject.Properties.Remove('hotkeys')
    if ($null -ne $source.hotkeys) {
        $manifest | Add-Member -MemberType NoteProperty -Name hotkeys -Value @($source.hotkeys)
    }
    [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 20) + "`n", [Text.UTF8Encoding]::new($false))
}
Write-Output "发行产物及已安装插件已更新：$output"
if (-not $SkipStart) {
    $process = Start-Process -FilePath (Join-Path $install 'mini-launcher.exe') -WorkingDirectory $install -WindowStyle Hidden -PassThru
    Write-Output "已启动新版 MiniLauncher PID $($process.Id)"
    if ($process.WaitForExit(1500)) { throw "新版宿主提前退出，退出码 $($process.ExitCode)" }
}

[CmdletBinding()]
param(
    [string]$InstallDirectory = "$PSScriptRoot\..\target\packages\unpacked\MiniLauncher",
    [ValidateSet('path-opener', 'two-factor', 'clipboard-history', 'caffeine', 'process-killer', 'piin')]
    [string[]]$Plugins = @('path-opener', 'two-factor', 'clipboard-history', 'caffeine', 'process-killer', 'piin'),
    [switch]$IncludeExample,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
# Binaries are compiled here; only the plugins are installed into -InstallDirectory.
$buildPath = Join-Path $projectRoot 'target\release'
$installPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDirectory)
$hostPath = Join-Path $installPath 'mini-launcher.exe'
$hostConfigPath = Join-Path $installPath 'mini-launcher-config.exe'

function Copy-PluginFile([string]$Source, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "缺少插件文件：$Source"
    }
    # A plugin process may still be releasing its executable after the host exits.
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
            return
        } catch {
            if ($attempt -eq 10) { throw }
            Start-Sleep -Milliseconds 300
        }
    }
}

function Stop-InstallHost([string]$Executable) {
    $running = @(Get-Process mini-launcher -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and [string]::Equals(
            [IO.Path]::GetFullPath($_.Path), $Executable, [StringComparison]::OrdinalIgnoreCase)
    })
    if ($running.Count -eq 0) { return }
    if (-not ('MiniLauncherPluginInstallNative' -as [type])) {
        Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MiniLauncherPluginInstallNative {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
    }
    $window = [MiniLauncherPluginInstallNative]::FindWindow('MiniLauncher.Native.Window', 'MiniLauncher')
    $owner = 0
    [void][MiniLauncherPluginInstallNative]::GetWindowThreadProcessId($window, [ref]$owner)
    if ($owner -in $running.Id) {
        [void][MiniLauncherPluginInstallNative]::PostMessage($window, 0x0111, [IntPtr]4, [IntPtr]::Zero)
    }
    foreach ($process in $running) {
        if (-not $process.WaitForExit(2000)) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            [void]$process.WaitForExit(2000)
        }
    }
    Write-Output "已退出旧宿主：$Executable"
}

if (-not $SkipBuild) {
    & "$projectRoot\dev.ps1" build -Release
    if (-not (Test-Path -LiteralPath (Join-Path $buildPath 'mini-launcher.exe') -PathType Leaf)) {
        throw "宿主未构建：$buildPath"
    }
    if ($IncludeExample) {
        & "$projectRoot\dev.ps1" build -Release -Example echo-plugin
    }
}

if (-not (Test-Path -LiteralPath $hostPath -PathType Leaf)) {
    throw "未找到宿主：$hostPath。请先构建，或用 -InstallDirectory 指定安装目录。"
}
# The install directory's --initialize-data/--register-plugin must match the
# freshly built binaries, otherwise plugin protocol changes would not apply.
if (-not $SkipBuild) {
    $installedHost = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash
    $builtHost = (Get-FileHash -LiteralPath (Join-Path $buildPath 'mini-launcher.exe') -Algorithm SHA256).Hash
    $installedConfig = if (Test-Path -LiteralPath $hostConfigPath -PathType Leaf) {
        (Get-FileHash -LiteralPath $hostConfigPath -Algorithm SHA256).Hash
    }
    $builtConfig = (Get-FileHash -LiteralPath (Join-Path $buildPath 'mini-launcher-config.exe') -Algorithm SHA256).Hash
    if ($installedHost -ne $builtHost -or $installedConfig -ne $builtConfig) {
        throw "安装目录的宿主不是本次构建（$installPath）。请先运行 scripts\build-release.ps1 更新该目录，或改用 -SkipBuild 只重装插件。"
    }
}
Stop-InstallHost $hostPath
& $hostPath --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory $installPath
$pluginsDirectory = Join-Path $dataDirectory 'plugins'
[void][IO.Directory]::CreateDirectory($pluginsDirectory)

foreach ($id in $Plugins) {
    $source = Join-Path $projectRoot "plugins\$id"
    $manifestPath = Join-Path $source 'plugin.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "插件清单不存在：$manifestPath"
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.id -ne $id) { throw "插件清单 id 与目录不一致：$id" }
    $executable = [string]$manifest.executable
    if ($executable -ne "$id.exe") { throw "插件程序名与目录不一致：$id" }
    $built = Join-Path $buildPath $executable
    if (-not (Test-Path -LiteralPath $built -PathType Leaf)) {
        throw "插件未构建：$built（重新运行本脚本，或加 -SkipBuild 前先构建）"
    }

    $destination = Join-Path $pluginsDirectory $id
    [void][IO.Directory]::CreateDirectory($destination)
    Copy-PluginFile $built (Join-Path $destination $executable)

    foreach ($name in @('plugin.json', 'settings.default.toml', 'README.md', 'settings.schema.json')) {
        $file = Join-Path $source $name
        if (Test-Path -LiteralPath $file -PathType Leaf) {
            Copy-PluginFile $file (Join-Path $destination $name)
        }
    }
    $imagesSource = Join-Path $source 'Images'
    if (Test-Path -LiteralPath $imagesSource -PathType Container) {
        Copy-Item -LiteralPath $imagesSource -Destination $destination -Recurse -Force
    }

    & $hostPath --register-plugin ([IO.Path]::GetFullPath($destination))
    if ($LASTEXITCODE -ne 0) { throw "插件配置登记失败：$id" }
    Write-Output "已重新安装：$id -> $destination"
}

if ($IncludeExample) {
    $built = Join-Path $buildPath 'examples\echo-plugin.exe'
    if (-not (Test-Path -LiteralPath $built -PathType Leaf)) {
        throw "示例插件未构建：$built"
    }
    $destination = Join-Path $pluginsDirectory 'mini-echo'
    [void][IO.Directory]::CreateDirectory($destination)
    Copy-PluginFile $built (Join-Path $destination 'echo-plugin.exe')
    Copy-PluginFile (Join-Path $projectRoot 'examples\echo-plugin.json') (Join-Path $destination 'plugin.json')
    Write-Output "已重新安装：echo（开发示例）-> $destination"
}

Write-Output "全部插件已覆盖安装到：$pluginsDirectory"
Write-Output "数据目录：$dataDirectory（已有历史、账号和插件设置均保留）"
Write-Output '重新启动宿主，或从托盘选择“重新加载配置 / 扫描应用”后生效。'
$installedHostHash = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash
$builtHostHash = (Get-FileHash -LiteralPath (Join-Path $buildPath 'mini-launcher.exe') -Algorithm SHA256).Hash
if ($installedHostHash -ne $builtHostHash) {
    Write-Warning "安装目录的宿主 ($hostPath) 不是本次构建；插件已按新协议安装，请运行 scripts\build-release.ps1 或从源目录启动新版宿主，否则 text_panel 等新协议不生效。"
}

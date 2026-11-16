[CmdletBinding()]
param(
    [string]$InstallDirectory = "$PSScriptRoot\..\target\packages\unpacked\MiniLauncher",
    [switch]$PackageOnly,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$installPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDirectory)
$hostPath = Join-Path $installPath 'mini-launcher.exe'
$packageRoot = Join-Path $projectRoot 'target\packages\text-panel-demo'
$archivePath = Join-Path $projectRoot 'target\packages\text-panel-demo-windows-x64.zip'

if (-not $PackageOnly -and -not (Test-Path -LiteralPath $hostPath -PathType Leaf)) {
    throw "未找到宿主：$hostPath。请指定 -InstallDirectory，或用 -PackageOnly 仅生成插件包。"
}
if (-not $SkipBuild) {
    & "$projectRoot\dev.ps1" build -Release -Example text-panel-demo
}
$executable = Join-Path $projectRoot 'target\release\examples\text-panel-demo.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw '请先构建 text-panel-demo 示例。' }

# The package is optional; it does not join the host's bundled plugin list.
[void][IO.Directory]::CreateDirectory($packageRoot)
Copy-Item -LiteralPath $executable -Destination (Join-Path $packageRoot 'text-panel-demo.exe') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\text-panel-demo.json') -Destination (Join-Path $packageRoot 'plugin.json') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'docs\text-panel-demo.md') -Destination (Join-Path $packageRoot 'README.md') -Force
foreach ($name in @('LICENSE', 'COPYRIGHT', 'THIRD_PARTY_NOTICES.html')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $packageRoot -Force
}
Compress-Archive -LiteralPath $packageRoot -DestinationPath $archivePath -Force
Write-Output "插件包：$archivePath"
if ($PackageOnly) { return }

. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory $installPath
$destination = Join-Path $dataDirectory 'plugins\text-panel-demo'
$manifestPath = Join-Path $destination 'plugin.json'
if (Test-Path -LiteralPath $destination) {
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or
        (Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json).id -ne 'text-panel-demo') {
        throw "目标目录已有其他内容，停止安装：$destination"
    }
}
# Stop the matching installation, including any resident demo process in its Job.
foreach ($process in @(Get-Process -Name mini-launcher, text-panel-demo -ErrorAction SilentlyContinue)) {
    if ($process.Path -and (
        [string]::Equals($process.Path, $hostPath, [StringComparison]::OrdinalIgnoreCase) -or
        [string]::Equals($process.Path, (Join-Path $destination 'text-panel-demo.exe'), [StringComparison]::OrdinalIgnoreCase))) {
        Stop-Process -Id $process.Id -Force
        if (-not $process.WaitForExit(3000)) { throw "程序仍在运行：$($process.Path)" }
    }
}
& $hostPath --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
[void][IO.Directory]::CreateDirectory($destination)
foreach ($file in Get-ChildItem -LiteralPath $packageRoot -File) {
    Copy-Item -LiteralPath $file.FullName -Destination $destination -Force
}
& $hostPath --register-plugin $destination
if ($LASTEXITCODE -ne 0) { throw '插件登记失败' }
Write-Output "已安装：$destination"
Write-Output '重新启动该目录的 MiniLauncher，输入 textdemo 查看场景。'

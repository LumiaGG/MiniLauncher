param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
& "$projectRoot\dev.ps1" build -Release -Bin path-opener
& "$projectRoot\dev.ps1" build -Release -Bin mini-launcher
$launcher = Join-Path $projectRoot 'target\release\mini-launcher.exe'
& $launcher --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory (Split-Path $launcher -Parent)
if (-not $Destination) { $Destination = Join-Path $dataDirectory 'plugins\path-opener' }
$executable = Join-Path $projectRoot 'target\release\path-opener.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Path Opener build failed' }
[void][IO.Directory]::CreateDirectory($Destination)
Copy-Item -LiteralPath $executable -Destination (Join-Path $Destination 'path-opener.exe') -Force
$source = Join-Path $projectRoot 'plugins\path-opener'
foreach ($name in @('plugin.json', 'settings.default.toml')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $Destination $name) -Force
}
& (Join-Path $projectRoot 'target\release\mini-launcher.exe') --register-plugin ([IO.Path]::GetFullPath($Destination))
if ($LASTEXITCODE -ne 0) { throw '路径打开插件配置登记失败' }
Write-Output "已安装路径打开插件：$Destination"
Write-Output '从托盘重新加载后输入 path 路径 或 p 路径。'

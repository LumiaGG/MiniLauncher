param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
& "$projectRoot\dev.ps1" build -Release -Example echo-plugin
& "$projectRoot\dev.ps1" build -Release -Bin mini-launcher
$launcher = Join-Path $projectRoot 'target\release\mini-launcher.exe'
& $launcher --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory (Split-Path $launcher -Parent)
if (-not $Destination) { $Destination = Join-Path $dataDirectory 'plugins\mini-echo' }
$executable = Join-Path $projectRoot 'target\release\examples\echo-plugin.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Example plugin build failed' }
[void][IO.Directory]::CreateDirectory($Destination)
Copy-Item -LiteralPath $executable -Destination (Join-Path $Destination 'echo-plugin.exe') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\echo-plugin.json') -Destination (Join-Path $Destination 'plugin.json') -Force
Write-Output "已安装示例插件：$Destination"
Write-Output '从托盘重新加载后，输入 echo 内容。'

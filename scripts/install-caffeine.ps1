param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
& "$projectRoot\dev.ps1" build -Release -Bin caffeine
& "$projectRoot\dev.ps1" build -Release -Bin mini-launcher
$launcher = Join-Path $projectRoot 'target\release\mini-launcher.exe'
& $launcher --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory (Split-Path $launcher -Parent)
if (-not $Destination) { $Destination = Join-Path $dataDirectory 'plugins\caffeine' }
[void][IO.Directory]::CreateDirectory($Destination)
Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\caffeine.exe') -Destination (Join-Path $Destination 'caffeine.exe') -Force
foreach ($name in @('plugin.json', 'settings.default.toml', 'settings.schema.json', 'README.md')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot "plugins\caffeine\$name") -Destination (Join-Path $Destination $name) -Force
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'plugins\caffeine\Images') -Destination $Destination -Recurse -Force
& $launcher --register-plugin ([IO.Path]::GetFullPath($Destination))
if ($LASTEXITCODE -ne 0) { throw '防休眠插件配置登记失败' }
Write-Output "已安装防休眠插件：$Destination"
Write-Output '从托盘选择“重新加载配置 / 扫描应用”或重启宿主后生效；输入 caf 开启或设置定时防休眠。'

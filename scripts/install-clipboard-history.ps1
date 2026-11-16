param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
& "$projectRoot\dev.ps1" build -Release -Bin clipboard-history
& "$projectRoot\dev.ps1" build -Release -Bin mini-launcher
$launcher = Join-Path $projectRoot 'target\release\mini-launcher.exe'
& $launcher --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory (Split-Path $launcher -Parent)
if (-not $Destination) { $Destination = Join-Path $dataDirectory 'plugins\clipboard-history' }
[void][IO.Directory]::CreateDirectory($Destination)
Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\clipboard-history.exe') -Destination (Join-Path $Destination 'clipboard-history.exe') -Force
foreach ($name in @('plugin.json', 'settings.default.toml', 'settings.schema.json', 'README.md')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot "plugins\clipboard-history\$name") -Destination (Join-Path $Destination $name) -Force
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'plugins\clipboard-history\Images') -Destination $Destination -Recurse -Force
& $launcher --register-plugin ([IO.Path]::GetFullPath($Destination))
if ($LASTEXITCODE -ne 0) { throw '剪贴板历史插件配置登记失败' }
Write-Output "已安装剪贴板历史插件：$Destination"
Write-Output '首次使用需退出并启动新版宿主；输入 clip 搜索历史，clip /pause 暂停记录。'

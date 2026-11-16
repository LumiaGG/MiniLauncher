param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath("$PSScriptRoot\..")
& "$projectRoot\dev.ps1" build -Release -Bin two-factor
& "$projectRoot\dev.ps1" build -Release -Bin mini-launcher
$launcher = Join-Path $projectRoot 'target\release\mini-launcher.exe'
& $launcher --initialize-data
if ($LASTEXITCODE -ne 0) { throw '数据目录初始化失败' }
. "$PSScriptRoot\data-directory.ps1"
$dataDirectory = Get-LauncherDataDirectory (Split-Path $launcher -Parent)
if (-not $Destination) { $Destination = Join-Path $dataDirectory 'plugins\two-factor' }
$executable = Join-Path $projectRoot 'target\release\two-factor.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Two Factor build failed' }
[void][IO.Directory]::CreateDirectory($Destination)
Copy-Item -LiteralPath $executable -Destination (Join-Path $Destination 'two-factor.exe') -Force
foreach ($name in @('plugin.json', 'settings.default.toml')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot "plugins\two-factor\$name") -Destination (Join-Path $Destination $name) -Force
}
& (Join-Path $projectRoot 'target\release\mini-launcher.exe') --register-plugin ([IO.Path]::GetFullPath($Destination))
if ($LASTEXITCODE -ne 0) { throw '两步验证码插件配置登记失败' }
Write-Output "已安装两步验证码插件：$Destination"
Write-Output '从托盘重新加载，输入 2fa。账号库独立保存，升级安装不会覆盖账号。'

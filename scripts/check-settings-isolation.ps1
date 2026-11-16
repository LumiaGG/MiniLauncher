[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $root
try {
    if (Test-Path -LiteralPath "$root\.tools\cargo\bin\cargo.exe") {
        $env:CARGO_HOME = "$root\.tools\cargo"
        $env:RUSTUP_HOME = "$root\.tools\rustup"
        $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    }
    $hostDependencies = @(& cargo tree --locked --offline -p mini-launcher --edges normal --prefix none)
    if ($LASTEXITCODE -ne 0) { throw '无法检查宿主依赖' }
    if ($hostDependencies -match '^(gpui|rfd|ratatui|crossterm)([- ]|$)') {
        throw '宿主引入了设置界面依赖，进程隔离检查失败'
    }
    $settingsDependencies = @(& cargo tree --locked --offline -p mini-launcher-config --edges normal --prefix none)
    if ($LASTEXITCODE -ne 0) { throw '无法检查设置依赖' }
    if (-not ($settingsDependencies -match '^gpui-kit ')) { throw '设置程序未使用 GPUI Kit' }
    Write-Output '通过：GPUI 只在独立设置程序的依赖树中，宿主不携带设置 UI 依赖。'
} finally { Pop-Location }

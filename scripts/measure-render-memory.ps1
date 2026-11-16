[CmdletBinding()]
param([switch]$Release)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$savedCargoHome = $env:CARGO_HOME
$savedRustupHome = $env:RUSTUP_HOME
$savedPath = $env:PATH
Push-Location $projectRoot
try {
    & (Join-Path $PSScriptRoot 'stop-launcher.ps1')
    if (Test-Path -LiteralPath '.tools\cargo\bin\cargo.exe') {
        $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
        $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
        $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    }
    $cargoArgs = @('test', '--offline', '--target-dir', (Join-Path $projectRoot 'target'), '--bin', 'mini-launcher')
    if ($Release) { $cargoArgs += '--release' }
    $cargoArgs += @('windows::gfx::benchmark::hidden_render_memory', '--', '--ignored', '--exact', '--nocapture', '--test-threads=1')
    cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Renderer memory measurement failed' }
    Write-Output "Report: $projectRoot\artifacts\render-memory.json"
} finally {
    $env:CARGO_HOME = $savedCargoHome
    $env:RUSTUP_HOME = $savedRustupHome
    $env:PATH = $savedPath
    Pop-Location
}

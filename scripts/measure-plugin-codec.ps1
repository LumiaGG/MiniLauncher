param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (-not $SkipBuild) {
    & (Join-Path $root 'dev.ps1') build -Release -Example plugin-codec-bench
}
& (Join-Path $root 'target/release/examples/plugin-codec-bench.exe')
if ($LASTEXITCODE -ne 0) { throw 'Plugin codec benchmark failed' }

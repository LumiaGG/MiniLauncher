[CmdletBinding()]
param(
    [string]$Tag,
    [string]$Repository,
    [string]$NotesFile,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$arguments = @('-B', (Join-Path $PSScriptRoot 'publish-release.py'))
if ($Tag) { $arguments += @('--tag', $Tag) }
if ($Repository) { $arguments += @('--repository', $Repository) }
if ($NotesFile) { $arguments += @('--notes-file', [IO.Path]::GetFullPath($NotesFile)) }
if ($SkipBuild) { $arguments += '--skip-build' }
& python @arguments
if ($LASTEXITCODE -ne 0) { throw "发行发布失败，退出码 $LASTEXITCODE" }

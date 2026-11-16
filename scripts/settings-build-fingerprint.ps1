[CmdletBinding()]
param(
    [string]$ProjectRoot = "$PSScriptRoot\.."
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($ProjectRoot)
# Track paths as well as contents so deletions/renames invalidate the cache.
# Shared src/ is intentionally excluded for fast test restarts; use a full
# settings build when testing changes to the library consumed by settings.
$inputs = @('Cargo.toml', 'Cargo.lock', 'build.rs')
foreach ($directory in @('settings-gui', '.cargo')) {
    $path = Join-Path $root $directory
    if (Test-Path -LiteralPath $path -PathType Container) {
        $inputs += Get-ChildItem -LiteralPath $path -File -Recurse | ForEach-Object {
            $_.FullName.Substring($root.Length + 1).Replace('\', '/')
        }
    }
}
$records = foreach ($relative in ($inputs | Sort-Object -Unique)) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $root $relative) -Algorithm SHA256).Hash
    "$relative=$hash"
}
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($records -join "`n"))
    ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
}
finally {
    $sha.Dispose()
}

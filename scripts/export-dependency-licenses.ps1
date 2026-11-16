[CmdletBinding()]
param(
    [string]$OutputPath = "$PSScriptRoot\..\docs\dependency-licenses.csv"
)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$savedCargoHome = $env:CARGO_HOME
$savedRustupHome = $env:RUSTUP_HOME
$savedPath = $env:PATH
Push-Location $projectRoot
try {
    if (Test-Path -LiteralPath "$projectRoot\.tools\cargo\bin\cargo.exe") {
        $env:CARGO_HOME = "$projectRoot\.tools\cargo"
        $env:RUSTUP_HOME = "$projectRoot\.tools\rustup"
        $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    }
    # Conservative inventory of the Windows resolved graph, including build and
    # development dependencies. This is not a list of code linked into binaries.
    $metadataText = & cargo metadata --offline --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed; fetch locked dependencies before exporting' }
    $metadata = $metadataText | ConvertFrom-Json
    $nodes = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $seen = @{}
    $queue = [Collections.Generic.Queue[string]]::new()
    foreach ($id in $metadata.workspace_members) { $queue.Enqueue($id) }
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if ($seen.ContainsKey($id)) { continue }
        $seen[$id] = $true
        foreach ($dependency in $nodes[$id].deps) { $queue.Enqueue($dependency.pkg) }
    }
    $metadata.packages |
        Where-Object { $_.source -and $seen.ContainsKey($_.id) } |
        Sort-Object name, version |
        Select-Object name, version, license, repository,
            @{Name='source'; Expression={ "https://crates.io/crates/$($_.name)/$($_.version)" }} |
        Export-Csv -LiteralPath $OutputPath -NoTypeInformation -Encoding utf8
    Write-Output "Dependency license inventory: $OutputPath"
}
finally {
    Pop-Location
    $env:CARGO_HOME = $savedCargoHome
    $env:RUSTUP_HOME = $savedRustupHome
    $env:PATH = $savedPath
}

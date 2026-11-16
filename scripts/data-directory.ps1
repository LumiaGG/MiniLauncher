function Get-LauncherDataDirectory([string]$InstallDirectory) {
    if ($env:MINILAUNCHER_DATA_DIR) { return $env:MINILAUNCHER_DATA_DIR }
    $selector = Join-Path $InstallDirectory 'portable.toml'
    if ((Test-Path -LiteralPath $selector) -and
        (Select-String -LiteralPath $selector -Pattern '^\s*migrate_from\s*=' -Quiet)) {
        throw '请退出并重新启动宿主完成数据迁移后，再安装插件。'
    }
    if ((Test-Path -LiteralPath $selector) -and
        (Select-String -LiteralPath $selector -Pattern '^\s*enabled\s*=\s*false\s*(#.*)?$' -Quiet)) {
        return (Join-Path $env:APPDATA 'MiniLauncher')
    }
    return (Join-Path $InstallDirectory 'data')
}

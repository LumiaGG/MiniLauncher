[CmdletBinding()]
param([string]$Executable = "$PSScriptRoot\..\target\release\mini-launcher.exe")
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
$executablePath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Executable)
$data = Join-Path $root ('artifacts/application-management-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($data)
$settingsPath = Join-Path $data 'settings.toml'
$historyPath = Join-Path $data 'history.json'
@'
hotkey = "Ctrl+Alt+Shift+F12"
start_at_login = false
[plugins]
enabled = false
[everything]
enabled = false
[web_search]
enabled = false
'@ | Set-Content -LiteralPath $settingsPath -Encoding utf8
'{"fixture.exe":{"count":42,"last_used":123,"pinned_queries":["one","two"],"removed":true}}' |
    Set-Content -LiteralPath $historyPath -Encoding utf8
if (-not ('MiniLauncherApplicationManagement' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MiniLauncherApplicationManagement {
    [StructLayout(LayoutKind.Sequential)] struct CopyData { public UIntPtr tag; public uint size; public IntPtr data; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string name, string title);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, UIntPtr w, ref CopyData data, uint flags, uint timeout, out UIntPtr result);
    public static bool Send(IntPtr window, string json) {
        byte[] bytes = System.Text.Encoding.UTF8.GetBytes(json);
        var memory = Marshal.AllocHGlobal(bytes.Length);
        try {
            Marshal.Copy(bytes, 0, memory, bytes.Length);
            var data = new CopyData { size = (uint)bytes.Length, data = memory };
            UIntPtr accepted;
            return SendMessageTimeout(window, 0x004A, UIntPtr.Zero, ref data, 3, 2000, out accepted) != IntPtr.Zero && accepted.ToUInt64() == 1;
        } finally { Marshal.FreeHGlobal(memory); }
    }
}
'@
}
& "$PSScriptRoot\stop-launcher.ps1"
$savedDataDirectory = $env:MINILAUNCHER_DATA_DIR
$hostProcess = $null
try {
    $env:MINILAUNCHER_DATA_DIR = $data
    $hostProcess = Start-Process -FilePath $executablePath -ArgumentList '--background' -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        if ($hostProcess.HasExited) { throw 'Test host exited before initialization' }
        $window = [MiniLauncherApplicationManagement]::FindWindow('MiniLauncher.Native.Window', 'MiniLauncher')
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'Test host window was not created' }
    # State is attached after window creation. Retry the idempotent request while startup finishes.
    $request = @{ jsonrpc = '2.0'; id = 1; method = 'applications.update'; params = @{
        settings_path = $settingsPath; edits = @(
            @{ action = 'unpin'; key = 'fixture.exe'; query = 'one' },
            @{ action = 'restore'; key = 'fixture.exe' }
        )
    }}
    $accepted = $false
    do {
        $accepted = [MiniLauncherApplicationManagement]::Send($window, ($request | ConvertTo-Json -Depth 6 -Compress))
        if ($accepted) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $accepted) { throw 'Host did not accept application edits' }
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $usage = (Get-Content -LiteralPath $historyPath -Raw | ConvertFrom-Json).'fixture.exe'
        if (-not $usage.removed -and @($usage.pinned_queries).Count -eq 1 -and $usage.pinned_queries[0] -eq 'two') { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($usage.removed -or @($usage.pinned_queries).Count -ne 1 -or $usage.pinned_queries[0] -ne 'two') { throw 'Application edits were not persisted' }
    if ($usage.count -ne 42 -or $usage.last_used -ne 123) { throw 'Usage history was overwritten' }
    $request.params.settings_path = Join-Path $root 'Cargo.toml'
    if ([MiniLauncherApplicationManagement]::Send($window, ($request | ConvertTo-Json -Depth 6 -Compress))) { throw 'Host accepted edits for another profile' }
    if ([MiniLauncherApplicationManagement]::Send($window, '{')) { throw 'Host accepted malformed JSON' }
    Write-Output 'PASS: unpin, restore, history preservation and invalid request rejection (no UI interaction)'
} finally {
    $env:MINILAUNCHER_DATA_DIR = $savedDataDirectory
    if ($hostProcess -and -not $hostProcess.HasExited) {
        & "$PSScriptRoot\stop-launcher.ps1"
    }
}

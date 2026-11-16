# Shared input helpers for explicitly requested desktop tests. No child edit HWND exists.
Add-Type -AssemblyName System.Windows.Forms
if (-not ('MiniLauncherHostInput' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MiniLauncherHostInput {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendKey(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
}
function Select-HostInput([IntPtr]$Window) {
    [void][MiniLauncherHostInput]::SetForegroundWindow($Window)
    if ([MiniLauncherHostInput]::GetForegroundWindow() -ne $Window) {
        throw 'Cannot focus the launcher for the requested desktop test'
    }
    [Windows.Forms.SendKeys]::SendWait('^a')
}
function Set-HostInput([IntPtr]$Window, [string]$Text) {
    Select-HostInput $Window
    [Windows.Forms.SendKeys]::SendWait('{BACKSPACE}')
    foreach ($unit in $Text.ToCharArray()) {
        [void][MiniLauncherHostInput]::SendKey($Window, 0x0102, [IntPtr][int]$unit, [IntPtr]::Zero)
    }
}

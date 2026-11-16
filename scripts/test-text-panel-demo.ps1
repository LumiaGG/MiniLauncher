# Explicitly opt-in desktop test: opens the launcher, uses the clipboard, and captures its window.
[CmdletBinding()]
param(
    [string]$OutputDirectory = "$PSScriptRoot\..\artifacts\text-panel-demo-ui",
    [string]$Executable = "$PSScriptRoot\..\target\release\mini-launcher.exe",
    [string[]]$CaseIds
)
$ErrorActionPreference = 'Stop'
if ($CaseIds) { $CaseIds = @($CaseIds | ForEach-Object { $_ -split ',' }) }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$exe = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Executable)
$plugin = Join-Path $root 'target\release\examples\text-panel-demo.exe'
foreach ($path in @($exe, $plugin)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing executable: $path" }
}
[void][IO.Directory]::CreateDirectory($output)
$data = Join-Path $output ('profile-' + [Guid]::NewGuid().ToString('N'))
$pluginDirectory = Join-Path $data 'plugins\text-panel-demo'
[void][IO.Directory]::CreateDirectory($pluginDirectory)
Copy-Item -LiteralPath $plugin -Destination $pluginDirectory
Copy-Item -LiteralPath (Join-Path $root 'examples\text-panel-demo.json') -Destination (Join-Path $pluginDirectory 'plugin.json')
@'
hotkey = "Ctrl+Alt+Shift+F12"
theme = "dark"
backdrop = "none"
window_width = 856
max_results = 8
start_at_login = false
search_paths = []
[everything]
enabled = false
[web_search]
enabled = false
[plugins]
enabled = true
max_processes = 1
'@ | Set-Content -LiteralPath (Join-Path $data 'settings.toml') -Encoding utf8
. "$PSScriptRoot\host-input.ps1"
Add-Type -AssemblyName System.Drawing, UIAutomationClient, UIAutomationTypes
if (-not ('TextPanelUiNative' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class TextPanelUiNative {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
}
'@
}
[void][TextPanelUiNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
$results = [Collections.Generic.List[object]]::new()
$previousReport = Join-Path $output 'report.json'
if ($CaseIds -and (Test-Path -LiteralPath $previousReport)) {
    $previous = Get-Content -LiteralPath $previousReport -Raw | ConvertFrom-Json
    foreach ($case in $previous.cases) {
        if ($case.id -notin $CaseIds) {
            if (-not $case.PSObject.Properties['run_profile']) { $case | Add-Member run_profile $previous.profile }
            $results.Add($case)
        }
    }
}
$savedData = $env:MINILAUNCHER_DATA_DIR
$clipboardBackup = [Windows.Forms.Clipboard]::GetDataObject()
$hostProcess = $null
$window = [IntPtr]::Zero
$script:lastNode = $null

function Assert-Ui([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Get-ResultNode {
    $element = [Windows.Automation.AutomationElement]::FromHandle($window)
    $condition = [Windows.Automation.PropertyCondition]::new(
        [Windows.Automation.AutomationElement]::ControlTypeProperty,
        [Windows.Automation.ControlType]::ListItem)
    $nodes = $element.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
    if ($nodes.Count -gt 0) {
        $node = $nodes[0].Current
        return @{ name = $node.Name; help = $node.HelpText; id = $node.AutomationId }
    }
    return $null
}
function Wait-Result([string]$Expected) {
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    do {
        $node = Get-ResultNode
        if ($node -and ($node.name + "`n" + $node.help).Contains($Expected)) {
            $script:lastNode = $node
            Start-Sleep -Milliseconds 300
            return
        }
        Start-Sleep -Milliseconds 80
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "UI Automation did not expose expected text: $Expected"
}
function Show-Scene([string]$Scene, [string]$Expected) {
    [void][TextPanelUiNative]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 180
    [void][TextPanelUiNative]::SetForegroundWindow($window)
    # Ctrl+F focuses the query even when the previous case left a text section focused.
    [Windows.Forms.SendKeys]::SendWait('^f')
    Set-HostInput $window $(if ($Scene) { "textdemo $Scene" } else { 'textdemo' })
    Wait-Result $Expected
}
function Capture-Ui([string]$Name) {
    Assert-Ui ([TextPanelUiNative]::GetForegroundWindow() -eq $window) 'Launcher lost foreground focus; screenshot would be unreliable'
    [void][TextPanelUiNative]::DwmFlush()
    $rect = [TextPanelUiNative+Rect]::new()
    [void][TextPanelUiNative]::GetWindowRect($window, [ref]$rect)
    $menu = [TextPanelUiNative]::FindWindow('MiniLauncher.ItemMenu', '项目操作')
    if ($menu -ne [IntPtr]::Zero) {
        $menuRect = [TextPanelUiNative+Rect]::new()
        [void][TextPanelUiNative]::GetWindowRect($menu, [ref]$menuRect)
        $rect.Left = [Math]::Min($rect.Left, $menuRect.Left)
        $rect.Top = [Math]::Min($rect.Top, $menuRect.Top)
        $rect.Right = [Math]::Max($rect.Right, $menuRect.Right)
        $rect.Bottom = [Math]::Max($rect.Bottom, $menuRect.Bottom)
    }
    Assert-Ui ($rect.Right -gt $rect.Left -and $rect.Bottom -gt $rect.Top) 'Invalid window bounds'
    $bitmap = [Drawing.Bitmap]::new(($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top))
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
        $bitmap.Save((Join-Path $output "$Name.png"), [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    return "$Name.png"
}
function Copy-Body {
    [Windows.Forms.SendKeys]::SendWait('^a')
    [Windows.Forms.SendKeys]::SendWait('^c')
    Start-Sleep -Milliseconds 100
    return [Windows.Forms.Clipboard]::GetText()
}
function Select-FirstLine {
    # Drag within the first body line of a status-bearing panel, without pressing Ctrl.
    $scale = [TextPanelUiNative]::GetDpiForWindow($window) / 96.0
    $start = [IntPtr](([int](190*$scale) -shl 16) -bor [int](49*$scale))
    $end = [IntPtr](([int](190*$scale) -shl 16) -bor [int](220*$scale))
    [void][MiniLauncherHostInput]::SendKey($window, 0x0201, [IntPtr]1, $start)
    [void][MiniLauncherHostInput]::SendKey($window, 0x0200, [IntPtr]1, $end)
    [void][MiniLauncherHostInput]::SendKey($window, 0x0202, [IntPtr]::Zero, $end)
    Start-Sleep -Milliseconds 150
}
function Get-CopySelectionMenu {
    [Windows.Forms.SendKeys]::SendWait('+{F10}')
    Start-Sleep -Milliseconds 150
    $menu = [TextPanelUiNative]::FindWindow('MiniLauncher.ItemMenu', '项目操作')
    Assert-Ui ($menu -ne [IntPtr]::Zero) 'Selection context menu did not open'
    $element = [Windows.Automation.AutomationElement]::FromHandle($menu)
    $condition = [Windows.Automation.PropertyCondition]::new(
        [Windows.Automation.AutomationElement]::NameProperty, '复制选中文字')
    return $element.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
}
function Click-Retry {
    $scale = [TextPanelUiNative]::GetDpiForWindow($window) / 96.0
    $point = [IntPtr](([int](117*$scale) -shl 16) -bor [int](776*$scale))
    [void][MiniLauncherHostInput]::SendKey($window, 0x0201, [IntPtr]1, $point)
    [void][MiniLauncherHostInput]::SendKey($window, 0x0202, [IntPtr]::Zero, $point)
}
function Add-Case([string]$Id, [string]$Scene, [string]$Expected, [string]$Description, [scriptblock]$Action) {
    if ($CaseIds -and $Id -notin $CaseIds) { return }
    $record = [ordered]@{ id=$Id; command="textdemo $Scene"; description=$Description; status='PASS'; details=''; screenshots=@(); run_profile=$data }
    try {
        Show-Scene $Scene $Expected
        if ($Action) { & $Action $record }
        $record.screenshots += Capture-Ui $Id
        $record.details = if ($record.details) { $record.details } else { '界面已显示预期内容，附真实窗口截图；排版与可读性另行人工审阅。' }
    } catch {
        $record.status = 'FAIL'
        $record.details = $_.Exception.Message
        try { $record.screenshots += Capture-Ui "$Id-failed" } catch { }
    }
    $results.Add([pscustomobject]$record)
    Write-Output "$($record.status): $Id - $Description"
}

try {
    # The user explicitly authorized closing existing launcher instances for this desktop test.
    foreach ($process in @(Get-Process mini-launcher -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $process.Id -Force
        [void]$process.WaitForExit(3000)
    }
    $env:MINILAUNCHER_DATA_DIR = $data
    $hostProcess = Start-Process -FilePath $exe -ArgumentList '--background' -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        Assert-Ui (-not $hostProcess.HasExited) 'Host exited during startup'
        $window = [TextPanelUiNative]::FindWindow('MiniLauncher.Native.Window', 'MiniLauncher')
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 80
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert-Ui ($window -ne [IntPtr]::Zero) 'Host window was not created'
    Start-Sleep -Milliseconds 500

    Add-Case '01-navigation' '' '场景导航' '默认两个导航区块' $null
    Add-Case '02-four' 'four' '④ 结论' '四区块 auto 布局' {
        param($record)
        foreach ($text in @('① 需求', '② 方案', '③ 验证', '④ 结论')) {
            Assert-Ui ($script:lastNode.help.Contains($text)) "Missing section: $text"
        }
        $record.details = 'UIA 同时包含四个区块；截图检查四块是否实际可见。'
    }
    Add-Case '03-vertical' 'vertical' '④ 结论' '纵向四区块' $null
    Add-Case '04-compare-copy' 'compare' '对照阅读' 'Tab 进入正文、全选复制、Tab 切换区块' {
        param($record)
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $first = Copy-Body
        Assert-Ui ($first.StartsWith('The quick brown fox')) 'First section copy did not match source'
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $second = Copy-Body
        Assert-Ui ($second.StartsWith('敏捷的棕色狐狸')) 'Tab did not focus the second section'
        [Windows.Forms.SendKeys]::SendWait('{ENTER}')
        Assert-Ui ([TextPanelUiNative]::GetForegroundWindow() -eq $window) 'Body Enter unexpectedly executed the result'
        Assert-Ui ([Windows.Forms.Clipboard]::GetText() -eq $second) 'Body Enter changed the clipboard'
        [Windows.Forms.SendKeys]::SendWait('+{TAB}')
        Assert-Ui ((Copy-Body).StartsWith('The quick brown fox')) 'Shift+Tab did not return to the first section'
        $record.details = 'Tab 进入原文及译文，Ctrl+A/C 得到对应正文；正文 Enter 不执行条目、不修改剪贴板；Shift+Tab 返回原文。截图保留原文选区。'
    }
    Add-Case '05-unequal' 'unequal' '自适应宽度' '长短正文的宽度分配' $null
    Add-Case '06-long' 'long' '120 段' '双区块长文与内部滚动' {
        param($record)
        $record.screenshots += Capture-Ui '06-long-before'
        $rect = [TextPanelUiNative+Rect]::new()
        [void][TextPanelUiNative]::GetWindowRect($window, [ref]$rect)
        $scale = [TextPanelUiNative]::GetDpiForWindow($window) / 96.0
        [void][TextPanelUiNative]::SetCursorPos(($rect.Left + [int](200*$scale)), ($rect.Top + [int](220*$scale)))
        # Wheel input is sent at the left section, using the window's screen coordinates.
        $x = $rect.Left + [int](200*$scale)
        $y = $rect.Top + [int](220*$scale)
        $point = [IntPtr](($y -shl 16) -bor ($x -band 0xffff))
        [void][MiniLauncherHostInput]::SendKey($window, 0x020A, [IntPtr](-720 -shl 16), $point)
        Start-Sleep -Milliseconds 200
        $record.details = '已向左侧正文发送向下六格滚轮，提供前后截图；实际滚动与另一块是否独立需对照截图审阅。'
    }
    Add-Case '07-unicode-copy' 'unicode' '多语言与换行' 'Unicode 与 Emoji 完整复制' {
        param($record)
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $text = Copy-Body
        foreach ($marker in @('你好，世界', '👩‍💻', '👨‍👩‍👧‍👦', "e$([char]0x301)")) {
            Assert-Ui ($text.Contains($marker)) "Unicode copy missing marker: $marker"
        }
        $record.details = '复制结果包含中文、ZWJ Emoji、家庭 Emoji 与组合重音字符。字形和选区见截图。'
    }
    Add-Case '08-static-copy' 'static' '可选正文与静态说明' 'Tab 跳过静态区块' {
        param($record)
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $text = Copy-Body
        Assert-Ui ($text.StartsWith('本块 selectable=true')) 'Tab did not skip the static section'
        $record.details = '首个 Tab 进入中间的可选正文，复制内容匹配；静态区块没有获得正文焦点。'
    }
    Add-Case '09-blank' 'blank' '空字符串' '空正文与占位' {
        param($record)
        [Windows.Forms.Clipboard]::SetText('text-panel-demo-empty-sentinel')
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $text = Copy-Body
        Assert-Ui ($text -eq 'text-panel-demo-empty-sentinel') 'Empty body copied placeholder text or the search query'
        $record.details = '空正文 Ctrl+A/C 不复制占位或搜索框文字，剪贴板哨兵保持不变；“暂无内容”占位是否显示由截图人工确认。'
    }
    Add-Case '10-empty' 'empty' '当前没有内容' '无区块的 empty 状态' $null
    Add-Case '11-loading-retry' 'loading' '模拟加载中' 'loading → ready' {
        param($record)
        $record.screenshots += Capture-Ui '11-loading-before'
        [Windows.Forms.SendKeys]::SendWait('^r')
        Wait-Result '已恢复正常'
        $record.details = 'Ctrl+R 后 UIA 与界面均切换到已恢复结果；附重试前后截图。'
    }
    Add-Case '12-error-retry' 'error' '模拟请求失败' 'error → ready' {
        param($record)
        $record.screenshots += Capture-Ui '12-error-before'
        [Windows.Forms.SendKeys]::SendWait('^r')
        Wait-Result '已恢复正常'
        $record.details = 'Ctrl+R 后错误状态恢复正常；附重试前后截图。'
    }
    Add-Case '13-retained' 'retained' '上次成功的内容' '错误提示同时保留正文' $null
    Add-Case '14-append-selection' 'append' '追加日志 01' '追加更新保留有效选区' {
        param($record)
        Select-FirstLine
        $record.screenshots += Capture-Ui '14-append-before'
        $selection = Get-CopySelectionMenu
        Assert-Ui ($selection -and $selection.Current.IsEnabled) 'Mouse selection was not established before append'
        $record.screenshots += Capture-Ui '14-append-menu-before'
        $selection.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
        Start-Sleep -Milliseconds 150
        Assert-Ui ([Windows.Forms.Clipboard]::GetText().StartsWith('固定前缀')) 'Mouse-selected baseline did not copy the expected prefix'
        Click-Retry
        Wait-Result '追加日志 02'
        $selection = Get-CopySelectionMenu
        $enabled = $selection -and $selection.Current.IsEnabled
        $record.screenshots += Capture-Ui '14-append-menu-after'
        [Windows.Forms.SendKeys]::SendWait('{ESC}')
        Assert-Ui $enabled '追加前鼠标选区已建立且菜单允许复制；追加后“复制选中文字”被禁用，原有效选区丢失。'
        $record.details = '鼠标建立首行选区，追加后菜单仍允许复制原选区；未使用存在焦点问题的 Ctrl+A/C 作为前置操作。'
    }
    Add-Case '15-replace-selection' 'replace' '版本 A' '替换更新清除失效选区' {
        param($record)
        Select-FirstLine
        $record.screenshots += Capture-Ui '15-replace-before'
        $selection = Get-CopySelectionMenu
        Assert-Ui ($selection -and $selection.Current.IsEnabled) 'Mouse selection was not established before replacement'
        $selection.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
        Start-Sleep -Milliseconds 150
        Assert-Ui ([Windows.Forms.Clipboard]::GetText().StartsWith('版本 A')) 'Mouse-selected baseline did not copy version A'
        Click-Retry
        Wait-Result '版本 B'
        $selection = Get-CopySelectionMenu
        $enabled = $selection -and $selection.Current.IsEnabled
        [Windows.Forms.SendKeys]::SendWait('{ESC}')
        Assert-Ui (-not $enabled) 'Replaced selection remained available to copy'
        $record.details = '鼠标建立版本 A 首行选区，替换为版本 B 后“复制选中文字”被禁用，旧选区清除。'
    }
    Add-Case '16-actions-menu' 'actions' '第一段正文' '打开通用右键扩展菜单' {
        param($record)
        [Windows.Forms.SendKeys]::SendWait('+{F10}')
        Start-Sleep -Milliseconds 200
        # The menu is a separate window and overlaps the launcher screenshot.
        $menu = [TextPanelUiNative]::FindWindow('MiniLauncher.ItemMenu', '项目操作')
        Assert-Ui ($menu -ne [IntPtr]::Zero) 'Result context menu did not open'
        $record.screenshots += Capture-Ui '16-actions-menu-open'
        [Windows.Forms.SendKeys]::SendWait('{ESC}')
        $record.details = 'Shift+F10 成功打开扩展菜单并用 Esc 取消。整项复制另测；未执行跨应用 OLE 拖放。'
    }
    Add-Case '17-limit-copy' 'limit' '65,520' '接近 64 KiB 的正文显示与完整复制' {
        param($record)
        [Windows.Forms.SendKeys]::SendWait('{TAB}')
        $text = Copy-Body
        Assert-Ui ([Text.Encoding]::UTF8.GetByteCount($text) -eq 65520) 'Copied large body byte count was not 65520'
        $record.details = 'Ctrl+A / Ctrl+C 后按 UTF-8 计数恰为 65,520 字节，未截断。'
    }
    Add-Case '18-single' 'single' '操作指南' '单区块操作说明' $null
    Add-Case '19-actions-copy' 'actions' '第一段正文' '搜索框 Enter 复制整项正文' {
        param($record)
        $record.screenshots += Capture-Ui '19-actions-before'
        [Windows.Forms.SendKeys]::SendWait('{ENTER}')
        Start-Sleep -Milliseconds 250
        $text = [Windows.Forms.Clipboard]::GetText()
        Assert-Ui ($text.StartsWith("第一段正文。`n保留原始换行。`n`n第二段正文。")) 'Default action did not copy both sections with original newlines'
        [void][TextPanelUiNative]::PostMessage($window, 0x8003, [IntPtr]::Zero, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 250
        [void][TextPanelUiNative]::SetForegroundWindow($window)
        $record.details = '在搜索框按 Enter 后剪贴板包含两块原始正文，以空行分隔；执行后窗口按预期隐藏。'
    }
} finally {
    $env:MINILAUNCHER_DATA_DIR = $savedData
    if ($hostProcess -and -not $hostProcess.HasExited) {
        [void][TextPanelUiNative]::PostMessage($window, 0x0111, [IntPtr]4, [IntPtr]::Zero)
        if (-not $hostProcess.WaitForExit(2000)) { Stop-Process -Id $hostProcess.Id -Force }
    }
    if ($clipboardBackup) { [Windows.Forms.Clipboard]::SetDataObject($clipboardBackup, $true) }
}

$revision = & git -C $root rev-parse --short HEAD
$results = @($results | Sort-Object id)
$passed = @($results | Where-Object status -eq 'PASS').Count
$report = [ordered]@{
    timestamp = (Get-Date).ToString('o'); revision = $revision; executable = $exe;
    profile = $data; theme = 'dark'; logical_width = 856;
    passed = $passed; failed = $results.Count - $passed; cases = @($results);
    limitations = @('截图须结合人工审阅，UIA 内容存在不等于全部像素可见。', '未执行跨应用拖放、压力测试、完整主题或多屏 DPI 回归。')
}
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $output 'report.json') -Encoding utf8
$encode = { param($value) [Net.WebUtility]::HtmlEncode([string]$value) }
$sections = foreach ($case in $results) {
    $pictures = foreach ($picture in $case.screenshots) { '<a href="{0}"><img loading="lazy" src="{0}" alt="{1}"></a>' -f $picture, (& $encode $case.id) }
    '<section><h2><span class="{0}">{0}</span> {1}</h2><code>{2}</code><p>{3}</p>{4}</section>' -f
        $case.status, (& $encode $case.description), (& $encode $case.command), (& $encode $case.details), ($pictures -join "`n")
}
@"
<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>文本面板 Demo 界面测试报告</title>
<style>body{font:16px/1.7 'Segoe UI','Microsoft YaHei',sans-serif;background:#f4f5f7;color:#202734;max-width:1120px;margin:32px auto;padding:0 24px}section{background:white;padding:24px;margin:24px 0;border-radius:12px}h1,h2{line-height:1.4}h2{font-size:21px}.PASS{color:#087f5b}.FAIL{color:#c92a2a}img{display:block;max-width:100%;height:auto;margin:16px 0;border:1px solid #ddd;border-radius:8px}code{background:#eef1f5;padding:4px 8px}a{color:#2360bc}</style>
<h1>文本面板 Demo · 界面测试报告</h1><p>时间：$($report.timestamp) · 提交：$revision · 深色主题 · 856 逻辑像素</p>
<p><b>自动检查：$passed 通过 / $($report.failed) 失败。</b>截图来自实际窗口的屏幕捕获。UIA 检查验证宿主暴露的结果内容，剪贴板检查验证真实复制行为；截图另行审阅。</p>
<p>使用隔离配置；未执行跨应用拖放、压力测试或完整主题与多屏 DPI 回归。测试结束已退出测试实例并恢复剪贴板。机器可读记录：<a href="report.json">report.json</a>。</p>
$($sections -join "`n")
</html>
"@ | Set-Content -LiteralPath (Join-Path $output 'report.html') -Encoding utf8
Write-Output "Report: $(Join-Path $output 'report.html') ($passed/$($results.Count) automatic checks passed)"
if ($report.failed) { exit 1 }

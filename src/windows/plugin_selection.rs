//! Copy the text selected in the window that was active before a plugin shortcut.
use super::*;
use std::time::Duration;

fn key(key: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
unsafe fn copy_selection(target: usize, process: u32) -> Result<String, String> {
    let current = || {
        let mut owner = 0;
        GetWindowThreadProcessId(target as HWND, &mut owner);
        GetForegroundWindow() as usize == target
            && owner == process
            && IsWindow(target as HWND) != 0
    };
    let started = Instant::now();
    while [VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|k| GetAsyncKeyState(*k as i32) < 0)
    {
        if !current() || started.elapsed() > Duration::from_millis(1500) {
            return Err("选中文字复制已取消：窗口改变或修饰键未释放".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if !current() {
        return Err("前台窗口已改变，已取消选中文字翻译".into());
    }
    let sequence = GetClipboardSequenceNumber();
    let held = GetAsyncKeyState(VK_CONTROL as i32) < 0;
    let inputs = if held {
        vec![key(0x43, false), key(0x43, true)]
    } else {
        vec![
            key(VK_CONTROL, false),
            key(0x43, false),
            key(0x43, true),
            key(VK_CONTROL, true),
        ]
    };
    let sent = SendInput(
        inputs.len() as u32,
        inputs.as_ptr(),
        size_of::<INPUT>() as i32,
    );
    if sent != inputs.len() as u32 {
        if !held {
            let release = [key(0x43, true), key(VK_CONTROL, true)];
            SendInput(2, release.as_ptr(), size_of::<INPUT>() as i32);
        }
        return Err("系统无法复制选中文字，请检查目标应用权限".into());
    }
    std::thread::sleep(Duration::from_millis(200));
    if !current() {
        return Err("前台窗口已改变，已取消选中文字翻译".into());
    }
    if sequence == GetClipboardSequenceNumber() {
        return Err("未获取到选中文字，请先在其他应用中选择文本".into());
    }
    for _ in 0..10 {
        if OpenClipboard(null_mut()) != 0 {
            let result = read_text();
            CloseClipboard();
            return result;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("剪贴板正被其他应用占用，请重试".into())
}
unsafe fn read_text() -> Result<String, String> {
    let handle = GetClipboardData(13);
    if handle.is_null() {
        return Err("所选内容不是文本".into());
    }
    let length = GlobalSize(handle) / 2;
    if length > input::MAX_LENGTH + 1 {
        return Err("选中文字过长，请分段翻译".into());
    }
    let pointer = GlobalLock(handle) as *const u16;
    if pointer.is_null() {
        return Err("无法读取所选文本".into());
    }
    let slice = std::slice::from_raw_parts(pointer, length);
    let end = slice.iter().position(|c| *c == 0).unwrap_or(length);
    let text = String::from_utf16_lossy(&slice[..end]);
    GlobalUnlock(handle);
    if text.trim().is_empty() {
        Err("选中文字为空".into())
    } else {
        Ok(text)
    }
}
impl State {
    pub(super) unsafe fn show_plugin_scope(&mut self, plugin: String, text: &str) {
        self.scope = scope::Scope::Plugin(plugin);
        self.query.clear_results();
        self.viewport.reset(0, self.visible_rows);
        self.editor.set_text(text);
        self.query.query_text = text.to_string();
        self.sync_height();
        self.invalidate();
        // show() starts the single search after both scope and input are ready.
        self.show();
    }

    /// Plugin global shortcut without selection copy: open the plugin scope, or
    /// hide the launcher when it is already showing that scope.
    pub(super) unsafe fn toggle_plugin_scope(&mut self, plugin: String) {
        if !self.plugin_scopes.iter().any(|(id, _)| *id == plugin) {
            return;
        }
        // Invalidate any in-flight selection copy so it cannot replace this scope.
        self.selection_request = self.selection_request.wrapping_add(1);
        if IsWindowVisible(self.hwnd) != 0
            && self.closing.is_none()
            && self.scope == scope::Scope::Plugin(plugin.clone())
        {
            self.hide();
            return;
        }
        self.show_plugin_scope(plugin, "");
    }
    pub(super) unsafe fn start_plugin_selection(&mut self, plugin: String) {
        let target = GetForegroundWindow();
        if target.is_null() || target == self.hwnd {
            return;
        }
        let mut process = 0;
        GetWindowThreadProcessId(target, &mut process);
        self.selection_request = self.selection_request.wrapping_add(1);
        let request = self.selection_request;
        let sender = self.event_sender.clone();
        let hwnd = self.hwnd as usize;
        let target = target as usize;
        std::thread::spawn(move || {
            let result = copy_selection(target, process);
            send_event(
                &sender,
                hwnd,
                UiEvent::PluginSelection {
                    request,
                    plugin,
                    result,
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an interactive foreground desktop; run explicitly with UI-test approval"]
    fn copies_selected_unicode_text_without_reusing_old_clipboard() {
        unsafe {
            let previous = GetForegroundWindow();
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                wide("STATIC").as_ptr(),
                wide("Selection copy check").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                500,
                180,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!window.is_null());
            let edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide("prefix 你好 world suffix").as_ptr(),
                WS_CHILD | WS_VISIBLE | ES_MULTILINE as u32,
                0,
                0,
                450,
                100,
                window,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!edit.is_null());
            SetForegroundWindow(window);
            if GetForegroundWindow() != window && !previous.is_null() {
                let foreground_thread = GetWindowThreadProcessId(previous, null_mut());
                let current_thread = GetCurrentThreadId();
                if foreground_thread != current_thread
                    && AttachThreadInput(current_thread, foreground_thread, 1) != 0
                {
                    SetForegroundWindow(window);
                    AttachThreadInput(current_thread, foreground_thread, 0);
                }
            }
            SetFocus(edit);
            SendMessageW(edit, EM_SETSEL, 7, 15);
            let mut process = 0;
            GetWindowThreadProcessId(window, &mut process);
            let (sender, receiver) = mpsc::channel();
            let target = window as usize;
            std::thread::spawn(move || {
                sender.send(copy_selection(target, process)).unwrap();
            });
            let started = Instant::now();
            let result = loop {
                let mut message: MSG = zeroed();
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                if let Ok(result) = receiver.try_recv() {
                    break result;
                }
                assert!(started.elapsed() < Duration::from_secs(4));
                std::thread::sleep(Duration::from_millis(5));
            };
            DestroyWindow(window);
            if !previous.is_null() {
                SetForegroundWindow(previous);
            }
            assert_eq!(result.unwrap(), "你好 world");
        }
    }
}

//! Restore the input target captured before activation, then send one Ctrl+V.
use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

#[derive(Clone, Copy)]
pub(super) struct Target {
    window: usize,
    focus: usize,
    window_process: u32,
    focus_process: u32,
    focus_thread: u32,
    terminal: bool,
}

unsafe fn window_class(window: HWND) -> String {
    let mut class = [0u16; 256];
    let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
    String::from_utf16_lossy(&class[..length.max(0) as usize])
}

unsafe fn terminal_focus(window: HWND, focus: HWND, process: u32) -> bool {
    if focus.is_null()
        || IsWindow(focus) == 0
        || IsWindowVisible(focus) == 0
        || IsWindowEnabled(focus) == 0
        || (focus != window && IsChild(window, focus) == 0)
    {
        return false;
    }
    let mut owner = 0;
    GetWindowThreadProcessId(focus, &mut owner);
    if owner != process {
        return false;
    }
    focus == window
        || matches!(
            window_class(focus).as_str(),
            "Windows.UI.Input.InputSite.WindowClass"
                | "Microsoft.UI.Input.InputSite.WindowClass"
                | "Windows.UI.Composition.DesktopWindowContentBridge"
                | "Microsoft.UI.Content.DesktopChildSiteBridge"
                | "Windows.UI.Core.CoreWindow"
        )
}

unsafe fn gui_focus(thread: u32) -> HWND {
    let mut info: GUITHREADINFO = zeroed();
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    if GetGUIThreadInfo(thread, &mut info) != 0 {
        info.hwndFocus
    } else {
        null_mut()
    }
}

impl Target {
    pub(super) unsafe fn capture(launcher: HWND) -> Option<Self> {
        let window = GetForegroundWindow();
        if window.is_null() || window == launcher {
            return None;
        }
        let mut window_process = 0;
        let thread = GetWindowThreadProcessId(window, &mut window_process);
        let focus = gui_focus(thread);
        if focus.is_null() || (focus != window && IsChild(window, focus) == 0) {
            return None;
        }
        let mut focus_process = 0;
        let focus_thread = GetWindowThreadProcessId(focus, &mut focus_process);
        let terminal = window_class(window) == "CASCADIA_HOSTING_WINDOW_CLASS"
            && terminal_focus(window, focus, window_process);
        let target = Self {
            window: window as usize,
            focus: focus as usize,
            window_process,
            focus_process,
            focus_thread: if terminal { thread } else { focus_thread },
            terminal,
        };
        target.valid().then_some(target)
    }

    unsafe fn valid(&self) -> bool {
        let window = self.window as HWND;
        let focus = self.focus as HWND;
        if self.terminal {
            let mut process = 0;
            let thread = GetWindowThreadProcessId(window, &mut process);
            return IsWindow(window) != 0
                && IsWindowVisible(window) != 0
                && IsWindowEnabled(window) != 0
                && IsIconic(window) == 0
                && process == self.window_process
                && thread == self.focus_thread
                && window_class(window) == "CASCADIA_HOSTING_WINDOW_CLASS";
        }
        if IsWindow(window) == 0
            || IsWindow(focus) == 0
            || IsWindowVisible(window) == 0
            || IsWindowVisible(focus) == 0
            || IsWindowEnabled(focus) == 0
            || IsIconic(window) != 0
            || (focus != window && IsChild(window, focus) == 0)
        {
            return false;
        }
        let (mut window_process, mut focus_process) = (0, 0);
        GetWindowThreadProcessId(window, &mut window_process);
        let focus_thread = GetWindowThreadProcessId(focus, &mut focus_process);
        window_process == self.window_process
            && focus_process == self.focus_process
            && focus_thread == self.focus_thread
    }

    unsafe fn check(
        &self,
        generation: u64,
        latest: &AtomicU64,
        launcher: HWND,
    ) -> Result<(), String> {
        if generation != latest.load(Ordering::Relaxed) || IsWindowVisible(launcher) != 0 {
            return Err("已开始新的操作，已取消自动粘贴；内容保留在剪贴板".into());
        }
        if !self.valid() {
            return Err("原输入窗口已关闭或不可用；内容保留在剪贴板".into());
        }
        let foreground = GetForegroundWindow();
        if !foreground.is_null() && foreground as usize != self.window && foreground != launcher {
            return Err("前台窗口已改变，已取消自动粘贴；内容保留在剪贴板".into());
        }
        if foreground as usize == self.window {
            let focus = gui_focus(self.focus_thread);
            if !focus.is_null() && !self.accepts_focus(focus) {
                return Err("目标输入焦点已改变，已取消自动粘贴；内容保留在剪贴板".into());
            }
        }
        Ok(())
    }

    unsafe fn accepts_focus(&self, focus: HWND) -> bool {
        if self.terminal {
            terminal_focus(self.window as HWND, focus, self.window_process)
        } else {
            focus as usize == self.focus
        }
    }

    unsafe fn wait_for_focus(
        &self,
        generation: u64,
        latest: &AtomicU64,
        launcher: HWND,
    ) -> Result<(), String> {
        let started = Instant::now();
        let mut previous = null_mut();
        loop {
            self.check(generation, latest, launcher)?;
            let focus = gui_focus(self.focus_thread);
            let ready = GetForegroundWindow() as usize == self.window
                && self.accepts_focus(focus)
                && (!self.terminal || focus as usize != self.window);
            // WinUI restores its input site asynchronously and can recreate its HWND.
            // Require a stable, same-process input site instead of forcing the old HWND.
            if ready && (!self.terminal || previous == focus) {
                return Ok(());
            }
            previous = if ready { focus } else { null_mut() };
            if started.elapsed() >= Duration::from_millis(300) {
                return Err("原输入焦点未恢复；内容保留在剪贴板".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

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

pub(super) unsafe fn run(
    target: Target,
    generation: u64,
    latest: &AtomicU64,
    launcher: usize,
) -> Result<(), String> {
    let launcher = launcher as HWND;
    let started = Instant::now();
    loop {
        target.check(generation, latest, launcher)?;
        let held = [
            VK_RETURN, VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_LBUTTON, VK_RBUTTON,
        ]
        .iter()
        .any(|key| GetAsyncKeyState(*key as i32) < 0);
        if !held {
            break;
        }
        if started.elapsed() >= Duration::from_secs(1) {
            return Err("按键尚未释放，已取消自动粘贴；内容保留在剪贴板".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if GetForegroundWindow() as usize != target.window
        && SetForegroundWindow(target.window as HWND) == 0
    {
        return Err("无法恢复原输入窗口；内容保留在剪贴板".into());
    }
    if !target.terminal && gui_focus(target.focus_thread) as usize != target.focus {
        // Attach only while restoring focus; never leave input queues joined.
        let mut message: MSG = zeroed();
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        let current_thread = GetCurrentThreadId();
        if AttachThreadInput(current_thread, target.focus_thread, 1) == 0 {
            return Err("无法恢复原输入焦点；内容保留在剪贴板".into());
        }
        SetFocus(target.focus as HWND);
        AttachThreadInput(current_thread, target.focus_thread, 0);
    }
    target.wait_for_focus(generation, latest, launcher)?;
    let inputs = [
        key(VK_CONTROL, false),
        key(0x56, false),
        key(0x56, true),
        key(VK_CONTROL, true),
    ];
    let sent = SendInput(
        inputs.len() as u32,
        inputs.as_ptr(),
        size_of::<INPUT>() as i32,
    );
    if sent != inputs.len() as u32 {
        if sent != 0 {
            let release = [key(0x56, true), key(VK_CONTROL, true)];
            SendInput(
                release.len() as u32,
                release.as_ptr(),
                size_of::<INPUT>() as i32,
            );
        }
        return Err("系统未能发送粘贴按键；内容保留在剪贴板".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::Shell::{DefSubclassProc, SetWindowSubclass},
    };

    static PASTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn observe(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_KEYDOWN && wparam == 0x56 && GetKeyState(VK_CONTROL as i32) < 0 {
            PASTED.store(true, Ordering::Relaxed);
            return 0;
        }
        if message == WM_CHAR && wparam == 0x16 {
            return 0;
        } // Do not read or modify the user's clipboard.
        DefSubclassProc(hwnd, message, wparam, lparam)
    }

    #[test]
    #[ignore = "requires an interactive foreground desktop; run explicitly with UI-test approval"]
    fn paste_returns_to_captured_edit_and_cancels_changed_focus_or_epoch() {
        unsafe {
            struct Fixture {
                window: HWND,
                previous: HWND,
            }
            impl Drop for Fixture {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.window);
                        if !self.previous.is_null() {
                            SetForegroundWindow(self.previous);
                        }
                    }
                }
            }
            let previous = GetForegroundWindow();
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                super::super::wide("STATIC").as_ptr(),
                super::super::wide("MiniLauncher paste routing check").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                -20000,
                -20000,
                300,
                100,
                null_mut(),
                null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            assert!(!window.is_null());
            let _fixture = Fixture { window, previous };
            let edit = CreateWindowExW(
                0,
                super::super::wide("EDIT").as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                200,
                30,
                window,
                null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            assert!(!edit.is_null());
            SetWindowSubclass(edit, Some(observe), 1, 0);
            SetForegroundWindow(window);
            if GetForegroundWindow() != window && !previous.is_null() {
                let foreground_thread = GetWindowThreadProcessId(previous, std::ptr::null_mut());
                let current_thread = GetCurrentThreadId();
                if foreground_thread != current_thread
                    && AttachThreadInput(current_thread, foreground_thread, 1) != 0
                {
                    SetForegroundWindow(window);
                    AttachThreadInput(current_thread, foreground_thread, 0);
                }
            }
            SetFocus(edit);
            assert_eq!(
                GetForegroundWindow(),
                window,
                "无法激活本次验证的临时输入窗口"
            );
            let target = Target::capture(null_mut()).unwrap();
            assert_eq!(target.focus, edit as usize);
            let epoch = std::sync::Arc::new(AtomicU64::new(1));
            let latest = epoch.clone();
            PASTED.store(false, Ordering::Relaxed);
            let worker = std::thread::spawn(move || run(target, 1, &latest, 0));
            let started = Instant::now();
            while !PASTED.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(2) {
                let mut message: MSG = zeroed();
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(worker.join().unwrap().is_ok());
            assert!(PASTED.load(Ordering::Relaxed), "粘贴按键没有到达原输入框");
            epoch.store(2, Ordering::Relaxed);
            assert!(run(target, 1, &epoch, 0).is_err());
            epoch.store(1, Ordering::Relaxed);
            SetFocus(window);
            assert!(run(target, 1, &epoch, 0).is_err());

            // Windows Terminal changes between the WinUI host and its input site
            // on activation. It can also recreate the site while retaining the same window.
            let instance = GetModuleHandleW(std::ptr::null());
            let terminal_class = super::super::wide("CASCADIA_HOSTING_WINDOW_CLASS");
            let site_class = super::super::wide("Windows.UI.Input.InputSite.WindowClass");
            for class in [&terminal_class, &site_class] {
                let definition = WNDCLASSW {
                    lpfnWndProc: Some(DefWindowProcW),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..zeroed()
                };
                assert_ne!(RegisterClassW(&definition), 0);
            }
            let terminal = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                terminal_class.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                -20000,
                -20000,
                300,
                100,
                null_mut(),
                null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!terminal.is_null());
            let _terminal_fixture = Fixture {
                window: terminal,
                previous: window,
            };
            SetForegroundWindow(terminal);
            SetFocus(terminal);
            let terminal_target = Target::capture(null_mut()).unwrap();
            assert!(terminal_target.terminal);
            assert_eq!(terminal_target.focus, terminal as usize);
            let make_site = || {
                CreateWindowExW(
                    0,
                    site_class.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    200,
                    30,
                    terminal,
                    null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            let site = make_site();
            assert!(!site.is_null());
            SetWindowSubclass(site, Some(observe), 1, 0);
            SetFocus(site);
            assert!(terminal_target.check(1, &epoch, null_mut()).is_ok());
            PASTED.store(false, Ordering::Relaxed);
            let latest = epoch.clone();
            let worker = std::thread::spawn(move || run(terminal_target, 1, &latest, 0));
            let started = Instant::now();
            while !PASTED.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(2) {
                let mut message: MSG = zeroed();
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(worker.join().unwrap().is_ok());
            assert!(PASTED.load(Ordering::Relaxed));
            let site_target = Target::capture(null_mut()).unwrap();
            DestroyWindow(site);
            let replacement = make_site();
            assert!(!replacement.is_null());
            SetFocus(replacement);
            assert!(site_target.valid());
            assert!(site_target.wait_for_focus(1, &epoch, null_mut()).is_ok());
            let wrong_edit = CreateWindowExW(
                0,
                super::super::wide("EDIT").as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                35,
                200,
                30,
                terminal,
                null_mut(),
                instance,
                std::ptr::null(),
            );
            SetFocus(wrong_edit);
            assert!(site_target.check(1, &epoch, null_mut()).is_err());
        }
    }
}

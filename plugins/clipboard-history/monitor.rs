//! Message-driven clipboard listening; the timer only retries a busy clipboard.
use crate::history::{PreparedImage, State, MAX_TEXT};
use std::{
    ptr::{null, null_mut},
    sync::{mpsc, Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*},
    UI::WindowsAndMessaging::*,
};

struct Monitor {
    state: Arc<Mutex<State>>,
    retries: u32,
    excluded: u32,
    ignored: u32,
    history_allowed: u32,
    png: u32,
}

enum Content {
    Text(String),
    Files(Result<Vec<String>, String>),
    Image(Vec<u8>, bool),
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

unsafe fn read_content(hwnd: HWND, monitor: &Monitor) -> Result<Option<Content>, ()> {
    if OpenClipboard(hwnd) == 0 {
        return Err(());
    }
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    let _close = Close;
    if IsClipboardFormatAvailable(monitor.excluded) != 0
        || IsClipboardFormatAvailable(monitor.ignored) != 0
    {
        return Ok(None);
    }
    if IsClipboardFormatAvailable(monitor.history_allowed) != 0 {
        let handle = GetClipboardData(monitor.history_allowed);
        if handle.is_null() || GlobalSize(handle) < 4 {
            return Ok(None);
        }
        let data = GlobalLock(handle);
        if data.is_null() {
            return Ok(None);
        }
        let allowed = std::ptr::read_unaligned(data.cast::<u32>());
        GlobalUnlock(handle);
        if allowed == 0 {
            return Ok(None);
        }
    }
    if IsClipboardFormatAvailable(15) != 0 {
        return Ok(Some(Content::Files(crate::files::read(GetClipboardData(
            15,
        )))));
    }
    for format in [monitor.png, 17, 8] {
        // PNG, CF_DIBV5, CF_DIB (including synthesized CF_BITMAP)
        if IsClipboardFormatAvailable(format) == 0 {
            continue;
        }
        let handle = GetClipboardData(format);
        if handle.is_null() {
            continue;
        }
        let length = GlobalSize(handle);
        if length == 0 || length > mini_launcher::clipboard_image::MAX_ENCODED_BYTES {
            continue;
        }
        let data = GlobalLock(handle);
        if data.is_null() {
            continue;
        }
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), length).to_vec();
        GlobalUnlock(handle);
        return Ok(Some(Content::Image(bytes, format != monitor.png)));
    }
    if IsClipboardFormatAvailable(13) == 0 {
        return Ok(None);
    } // CF_UNICODETEXT
    let handle = GetClipboardData(13);
    if handle.is_null() {
        return Ok(None);
    }
    let bytes = GlobalSize(handle);
    if bytes < 2 {
        return Ok(None);
    }
    let data = GlobalLock(handle);
    if data.is_null() {
        return Ok(None);
    }
    let units = std::slice::from_raw_parts(data.cast::<u16>(), (bytes / 2).min(MAX_TEXT + 1));
    let text = units
        .iter()
        .position(|&c| c == 0)
        .and_then(|end| String::from_utf16(&units[..end]).ok());
    GlobalUnlock(handle);
    Ok(text.filter(|t| t.len() <= MAX_TEXT).map(Content::Text))
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lparam as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Monitor;
    if !pointer.is_null() && (message == WM_CLIPBOARDUPDATE || (message == WM_TIMER && wparam == 1))
    {
        let monitor = &mut *pointer;
        KillTimer(hwnd, 1);
        if message == WM_CLIPBOARDUPDATE {
            monitor.retries = 0;
        }
        if monitor.state.lock().is_ok_and(|state| state.history.paused) {
            return 0;
        }
        match read_content(hwnd, monitor) {
            Ok(Some(content)) => {
                enum PreparedContent {
                    Text(String),
                    Files(Vec<String>),
                    Image(PreparedImage),
                }
                let content = match content {
                    Content::Files(paths) => paths.map(PreparedContent::Files),
                    Content::Text(text) => Ok(PreparedContent::Text(text)),
                    Content::Image(bytes, dib) => {
                        mini_launcher::clipboard_image::decode(&bytes, dib).and_then(|image| {
                            PreparedImage::new(image, if dib { None } else { Some(bytes) })
                                .map(PreparedContent::Image)
                        })
                    }
                };
                if let Ok(mut state) = monitor.state.lock() {
                    let result = match content {
                        Ok(PreparedContent::Files(paths)) => state.capture_files(paths),
                        Ok(PreparedContent::Text(text)) => state.capture(text),
                        Ok(PreparedContent::Image(prepared)) => {
                            state.record_prepared_image(prepared)
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        state.error = Some(error);
                    }
                }
            }
            Err(()) if monitor.retries < 5 => {
                monitor.retries += 1;
                SetTimer(hwnd, 1, 25, None);
            }
            _ => {}
        }
        return 0;
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

pub fn start(state: Arc<Mutex<State>>) -> Result<usize, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || unsafe {
        let class = wide("MiniLauncher.ClipboardHistory.Monitor");
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            let _ = sender.send(Err("注册剪贴板监听窗口失败".into()));
            return;
        }
        let mut monitor = Box::new(Monitor {
            state,
            retries: 0,
            excluded: RegisterClipboardFormatW(
                wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
            ),
            ignored: RegisterClipboardFormatW(wide("Clipboard Viewer Ignore").as_ptr()),
            history_allowed: RegisterClipboardFormatW(
                wide("CanIncludeInClipboardHistory").as_ptr(),
            ),
            png: RegisterClipboardFormatW(wide("PNG").as_ptr()),
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            (&mut *monitor as *mut Monitor).cast(),
        );
        if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
            let _ = sender.send(Err("启动剪贴板监听失败".into()));
            if !hwnd.is_null() {
                DestroyWindow(hwnd);
            }
            return;
        }
        let _ = sender.send(Ok(hwnd as usize));
        PostMessageW(hwnd, WM_CLIPBOARDUPDATE, 0, 0);
        let mut message = MSG::default();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
    });
    receiver
        .recv()
        .map_err(|_| "剪贴板监听线程已退出".to_string())?
}

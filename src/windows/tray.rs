//! Owns the host tray icon registration and its tooltip.
use super::*;

pub(super) struct Icon {
    data: NOTIFYICONDATAW,
    added: bool,
}

impl Icon {
    pub(super) unsafe fn new(hwnd: HWND, instance: HINSTANCE, hotkey: &str) -> Self {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY;
        data.hIcon = LoadIconW(instance, 1 as _);
        let mut icon = Self { data, added: false };
        icon.set_tooltip(hotkey);
        icon
    }

    fn set_tooltip(&mut self, hotkey: &str) {
        self.data.szTip.fill(0);
        // Reserve the last UTF-16 code unit for the terminating NUL.
        let tooltip = format!("MiniLauncher · {hotkey}");
        let capacity = self.data.szTip.len() - 1;
        for (slot, unit) in self.data.szTip.iter_mut().take(capacity).zip(tooltip.encode_utf16()) {
            *slot = unit;
        }
    }

    pub(super) unsafe fn add(&mut self) {
        self.added = Shell_NotifyIconW(NIM_ADD, &self.data) != 0;
    }

    pub(super) unsafe fn notify_error(&self, message: &str) {
        let mut notification = self.data;
        notification.uFlags = NIF_INFO;
        for (slot, value) in notification
            .szInfo
            .iter_mut()
            .take(255)
            .zip(message.encode_utf16())
        {
            *slot = value;
        }
        for (slot, value) in notification
            .szInfoTitle
            .iter_mut()
            .zip("MiniLauncher 启动失败".encode_utf16())
        {
            *slot = value;
        }
        notification.dwInfoFlags = NIIF_ERROR;
        Shell_NotifyIconW(NIM_MODIFY, &notification);
    }

    pub(super) unsafe fn update_hotkey(&mut self, hotkey: &str) {
        self.set_tooltip(hotkey);
        if self.added {
            Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        if self.added {
            unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
        }
    }
}

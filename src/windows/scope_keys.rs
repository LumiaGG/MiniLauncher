//! Foreground-only interception of scope shortcuts, ahead of RegisterHotKey.
//! The callback only checks routing and posts a message; it never touches State.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};

pub(super) const SELECT_SCOPE: u32 = WM_APP + 15;
static OWNER: AtomicUsize = AtomicUsize::new(0);
static ENABLED: AtomicU32 = AtomicU32::new(0);
static COMPOSING: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicU32 = AtomicU32::new(0);

pub(super) const ALT_PREVIEW_TIMER: usize = 3;
const ALT_PREVIEW_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

#[derive(Default)]
pub(super) enum AltPreview {
    #[default]
    Idle,
    Pending(Instant),
    Suppressed,
}

impl AltPreview {
    fn cancel(&mut self, alt_held: bool) {
        *self = if alt_held {
            Self::Suppressed
        } else {
            Self::Idle
        };
    }

    fn press(&mut self, now: Instant) -> bool {
        if !matches!(self, Self::Idle) {
            return false;
        }
        *self = Self::Pending(now + ALT_PREVIEW_DELAY);
        true
    }

    fn ready(&mut self, now: Instant) -> bool {
        if matches!(self, Self::Pending(deadline) if now >= *deadline) {
            *self = Self::Suppressed;
            return true;
        }
        false
    }
}

pub(super) struct Hook(HHOOK);

impl Hook {
    unsafe fn install(hwnd: HWND) -> Result<Self, String> {
        let handle = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_proc),
            GetModuleHandleW(null()),
            0,
        );
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        OWNER.store(hwnd as usize, Ordering::Relaxed);
        CAPTURED.store(0, Ordering::Relaxed);
        Ok(Self(handle))
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        OWNER.store(0, Ordering::Relaxed);
        CAPTURED.store(0, Ordering::Relaxed);
        unsafe {
            UnhookWindowsHookEx(self.0);
        }
    }
}

pub(super) fn set_composing(composing: bool) {
    COMPOSING.store(composing, Ordering::Relaxed);
}

impl State {
    pub(super) unsafe fn cancel_alt_preview(&mut self) {
        KillTimer(self.hwnd, ALT_PREVIEW_TIMER);
        self.alt_preview.cancel(GetAsyncKeyState(VK_MENU as i32) < 0);
    }

    pub(super) unsafe fn release_alt_preview(&mut self) {
        KillTimer(self.hwnd, ALT_PREVIEW_TIMER);
        self.alt_preview = AltPreview::Idle;
    }

    pub(super) unsafe fn schedule_alt_preview(&mut self) {
        if self.alt_preview.press(Instant::now()) {
            if SetTimer(self.hwnd, ALT_PREVIEW_TIMER, 150, None) == 0 {
                self.cancel_alt_preview();
            }
        }
    }

    pub(super) unsafe fn poll_alt_preview(&mut self) {
        if !self.alt_preview.ready(Instant::now()) {
            return;
        }
        KillTimer(self.hwnd, ALT_PREVIEW_TIMER);
        if self.closing.is_none()
            && IsWindowVisible(self.hwnd) != 0
            && GetForegroundWindow() == self.hwnd
            && (GetFocus() == self.hwnd || self.editor.is_focused())
            && !self.editor.is_composing()
            && GetAsyncKeyState(VK_MENU as i32) < 0
            && [VK_CONTROL, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .all(|key| GetAsyncKeyState(*key as i32) >= 0)
        {
            self.begin_scope_alt_selection();
        }
    }

    pub(super) fn configure_scope_keys(&self) {
        let enabled = self.available_scopes().iter().fold(0, |mask, (scope, _)| {
            let shortcut = self
                .settings
                .scope_menu
                .shortcut(&scope.id(), &self.settings.web_search.default_engine);
            mask | shortcut
                .as_bytes()
                .last()
                .map_or(0, |key| shortcut_bit(*key as u32))
        });
        ENABLED.store(enabled, Ordering::Relaxed);
        set_composing(self.editor.is_composing());
    }

    pub(super) unsafe fn activate_scope_keys(&mut self) {
        if self.scope_keys.is_some() || GetForegroundWindow() != self.hwnd {
            return;
        }
        self.configure_scope_keys();
        match Hook::install(self.hwnd) {
            Ok(hook) => self.scope_keys = Some(hook),
            Err(error) => {
                self.status = Some(format!("无法接管范围快捷键：{error}"));
                self.sync_height();
                self.invalidate();
            }
        }
    }

    pub(super) unsafe fn select_scope_shortcut(&mut self, key: u16) -> bool {
        self.cancel_alt_preview();
        let label = format!("Alt+{}", char::from_u32(key as u32).unwrap_or_default());
        let Some((scope, _)) = self.available_scopes().into_iter().find(|(scope, _)| {
            self.settings
                .scope_menu
                .shortcut(&scope.id(), &self.settings.web_search.default_engine)
                == label
        }) else {
            return false;
        };
        self.set_scope(scope);
        true
    }
}

fn shortcut_bit(key: u32) -> u32 {
    if (0x30..=0x39).contains(&key) {
        1 << (key - 0x30)
    } else {
        0
    }
}

fn eligible(
    key: u32,
    enabled: u32,
    alt: bool,
    foreground_input: bool,
    composing: bool,
    other_modifier: bool,
) -> bool {
    alt && foreground_input && !composing && !other_modifier && shortcut_bit(key) & enabled != 0
}

unsafe extern "system" fn keyboard_proc(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = &*(data as *const KBDLLHOOKSTRUCT);
        let bit = shortcut_bit(key.vkCode);
        let up = matches!(message as u32, WM_KEYUP | WM_SYSKEYUP);
        if up && bit != 0 && CAPTURED.fetch_and(!bit, Ordering::Relaxed) & bit != 0 {
            return 1;
        }
        if !up && bit != 0 {
            let hwnd = OWNER.load(Ordering::Relaxed) as HWND;
            // The current key is a digit; these modifier states are already updated.
            let other_modifier = [VK_CONTROL, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| GetAsyncKeyState(*key as i32) < 0);
            let foreground_input = !hwnd.is_null()
                && GetForegroundWindow() == hwnd
                && IsWindowVisible(hwnd) != 0
                && GetFocus() == hwnd;
            if eligible(
                key.vkCode,
                ENABLED.load(Ordering::Relaxed),
                key.flags & LLKHF_ALTDOWN != 0,
                foreground_input,
                COMPOSING.load(Ordering::Relaxed),
                other_modifier,
            ) && PostMessageW(hwnd, SELECT_SCOPE, key.vkCode as usize, 0) != 0
            {
                // This digit will not reach hooks later in the chain.
                hotkey::cancel_pending_double_taps();
                CAPTURED.fetch_or(bit, Ordering::Relaxed);
                return 1;
            }
            // Never leak repeats of a digit whose initial down event we consumed.
            if CAPTURED.load(Ordering::Relaxed) & bit != 0 {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, message, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alt_preview_waits_and_cannot_reopen_after_a_chord_or_stale_timer() {
        let now = Instant::now();
        let mut preview = AltPreview::default();
        assert!(preview.press(now));
        assert!(!preview.press(now));
        assert!(!preview.ready(now + std::time::Duration::from_millis(149)));
        assert!(preview.ready(now + ALT_PREVIEW_DELAY));
        assert!(!preview.ready(now + ALT_PREVIEW_DELAY));
        preview.cancel(true);
        assert!(!preview.press(now));
        assert!(!preview.ready(now + ALT_PREVIEW_DELAY));
        preview.cancel(false);
        assert!(preview.press(now + ALT_PREVIEW_DELAY));
        assert!(!preview.ready(now + ALT_PREVIEW_DELAY));
        assert!(preview.ready(now + ALT_PREVIEW_DELAY * 2));
    }

    #[test]
    fn cancelled_alt_preview_stays_cancelled_until_release() {
        let now = Instant::now();
        let mut preview = AltPreview::default();
        assert!(preview.press(now));
        preview.cancel(true);
        assert!(!preview.ready(now + ALT_PREVIEW_DELAY));
        assert!(!preview.press(now + ALT_PREVIEW_DELAY));
        preview.cancel(false);
        assert!(preview.press(now + ALT_PREVIEW_DELAY));
        // A queued timer from the previous press cannot open the new preview early.
        assert!(!preview.ready(now + ALT_PREVIEW_DELAY));
    }
    #[test]
    fn scope_keys_only_capture_enabled_plain_alt_shortcuts_in_foreground_input() {
        for key in 0x30..=0x39 {
            assert!(eligible(key, 1023, true, true, false, false));
            assert!(!eligible(key, 1023, true, false, false, false));
            assert!(!eligible(key, 1023, false, true, false, false));
            assert!(!eligible(key, 1023, true, true, true, false));
            assert!(!eligible(key, 1023, true, true, false, true));
            assert!(!eligible(key, 0, true, true, false, false));
        }
        assert!(eligible(0x31, 2, true, true, false, false));
        assert!(!eligible(0x32, 2, true, true, false, false));
        assert!(!eligible(0x33, 2, true, true, false, false));
        for key in [VK_TAB as u32, VK_SPACE as u32, 0x2F, 0x3A, 0x41] {
            assert!(!eligible(key, 1023, true, true, false, false));
        }
    }
}

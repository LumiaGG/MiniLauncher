//! Thread-local hook ownership follows the RegisterHotKey calling thread.
//! Callbacks only recognize input and post notifications; they never borrow UI state.
use super::gesture::Detector;
use std::{
    cell::RefCell,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::GetAsyncKeyState, WindowsAndMessaging::*},
};

#[derive(Default)]
struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
    bindings: Vec<(HWND, i32, u32)>,
    detector: Detector,
}

thread_local! {
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
}

fn logical_key(key: u32) -> u32 {
    match key {
        0xa0 | 0xa1 => 0x10,
        0xa2 | 0xa3 => 0x11,
        0xa4 | 0xa5 => 0x12,
        0x5c => 0x5b,
        _ => key,
    }
}

pub(super) fn cancel() {
    HOOKS.with(|hooks| hooks.borrow_mut().detector.cancel());
}

pub(super) unsafe fn register(hwnd: HWND, id: i32, key: u32) -> bool {
    HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        if hooks
            .bindings
            .iter()
            .any(|&(owner, binding, existing)| (owner == hwnd && binding == id) || existing == key)
        {
            return false;
        }
        if hooks.bindings.is_empty() {
            let module = GetModuleHandleW(null());
            hooks.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
            if hooks.keyboard.is_null() {
                return false;
            }
            hooks.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
            if hooks.mouse.is_null() {
                UnhookWindowsHookEx(hooks.keyboard);
                hooks.keyboard = null_mut();
                return false;
            }
            // Include keys already held when observation starts; generic modifier
            // aliases are excluded because hook events report their physical sides.
            hooks.detector.held = (8..=254)
                .filter(|key| !matches!(key, 0x10..=0x12) && GetAsyncKeyState(*key as i32) < 0)
                .collect();
        }
        hooks.detector.cancel();
        hooks.bindings.push((hwnd, id, key));
        true
    })
}

pub(super) unsafe fn unregister(hwnd: HWND, id: i32) -> bool {
    HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        let len = hooks.bindings.len();
        hooks
            .bindings
            .retain(|&(owner, binding, _)| owner != hwnd || binding != id);
        if len == hooks.bindings.len() {
            return false;
        }
        hooks.detector.cancel();
        if hooks.bindings.is_empty() {
            UnhookWindowsHookEx(hooks.keyboard);
            UnhookWindowsHookEx(hooks.mouse);
            *hooks = Hooks::default();
        }
        true
    })
}

unsafe extern "system" fn keyboard_proc(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = &*(data as *const KBDLLHOOKSTRUCT);
        HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            if event.flags & LLKHF_INJECTED != 0 {
                hooks.detector.cancel();
                return;
            }
            let mouse_held = [1, 2, 4, 5, 6]
                .iter()
                .any(|button| GetAsyncKeyState(*button) < 0);
            let up = matches!(message as u32, WM_KEYUP | WM_SYSKEYUP);
            let triggered = hooks.detector.key(event.vkCode, up, event.time);
            if mouse_held {
                hooks.detector.cancel();
            } else if triggered {
                for &(hwnd, id, key) in &hooks.bindings {
                    if logical_key(event.vkCode) == key {
                        PostMessageW(hwnd, WM_HOTKEY, id as usize, 0);
                    }
                }
            }
        });
    }
    CallNextHookEx(null_mut(), code, message, data)
}

unsafe extern "system" fn mouse_proc(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(
            message as u32,
            WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_RBUTTONDOWN
                | WM_RBUTTONUP
                | WM_MBUTTONDOWN
                | WM_MBUTTONUP
                | WM_XBUTTONDOWN
                | WM_XBUTTONUP
                | WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
        )
    {
        HOOKS.with(|hooks| hooks.borrow_mut().detector.cancel());
    }
    CallNextHookEx(null_mut(), code, message, data)
}

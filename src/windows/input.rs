//! Input definitions and IME helpers.
use windows_sys::Win32::Foundation::HWND;

pub const MAX_LENGTH: usize = 65536;

pub unsafe fn complete_composition(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::Ime::*;
    let context = ImmGetContext(hwnd);
    if !context.is_null() {
        if ImmGetCompositionStringW(context, GCS_COMPSTR, std::ptr::null_mut(), 0) > 0 {
            ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
        }
        ImmReleaseContext(hwnd, context);
    }
}

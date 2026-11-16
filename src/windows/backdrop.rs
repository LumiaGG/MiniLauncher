use windows_sys::Win32::{Foundation::HWND, Graphics::Dwm::*, UI::Controls::MARGINS};

/// Keep the DWM frame for materials, while leaving all client painting to us.
/// Menus belong to the active interaction even when they do not take focus.
pub(super) unsafe fn popup_frame_message(hwnd: HWND, message: u32) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    match message {
        WM_NCCALCSIZE | WM_NCPAINT | 0x00AE | 0x00AF => Some(0),
        WM_NCACTIVATE => Some(DefWindowProcW(hwnd, message, 1, -1)),
        _ => None,
    }
}

/// Use the same tint and material visibility for the launcher and its menus.
pub(super) fn background(
    palette: super::Palette,
    mode: &str,
    active: bool,
) -> windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
    let alpha = if !active {
        1.0
    } else if mode == "acrylic" {
        184.0 / 255.0
    } else {
        0.0
    };
    super::gfx::colorref_to_d2d(palette.background, alpha)
}

pub(super) unsafe fn apply_popup(
    hwnd: HWND,
    dark: bool,
    palette: super::Palette,
    mode: &str,
) -> bool {
    let dark = i32::from(dark);
    let rounded = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        &dark as *const _ as _,
        4,
    );
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &rounded as *const _ as _,
        4,
    );
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR as u32,
        &palette.border as *const _ as _,
        4,
    );
    apply(hwnd, mode)
}

pub unsafe fn apply(hwnd: HWND, mode: &str) -> bool {
    let requested: i32 = match mode {
        "mica" => 2,
        "acrylic" => 3,
        _ => 1,
    };
    let supported = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE as u32,
        &requested as *const _ as _,
        4,
    ) >= 0
        && requested != 1;
    // Client alpha belongs to the DirectComposition surface. DWM only supplies
    // the system material and frame extension; no redirection bitmap is used.
    let margin = if supported { -1 } else { 1 };
    let margins = MARGINS {
        cxLeftWidth: margin,
        cxRightWidth: margin,
        cyTopHeight: margin,
        cyBottomHeight: margin,
    };
    let extended = DwmExtendFrameIntoClientArea(hwnd, &margins) >= 0;
    // Do not clear the client to transparent unless both material setup steps
    // succeeded; otherwise the scene must use its opaque background fallback.
    supported && extended
}

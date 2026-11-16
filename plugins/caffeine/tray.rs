//! Active system tray icon and quick-control context menu for the Caffeine plugin.
//! Displays a color-coded status icon while anti-sleep is active, allowing one-click
//! mode/timer switching or double-click deactivation directly from the Windows tray.

use super::{ActiveSession, Controller};
#[cfg(not(test))]
use super::{execute, format_remaining, PowerMode};
#[cfg(not(test))]
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicIsize, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};
#[cfg(not(test))]
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        },
        Gdi::{
            BeginPaint, BitBlt, CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW,
            CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
            EnumFontFamiliesExW, FillRect, GetMonitorInfoW, GetStockObject,
            GetSysColor, InvalidateRect, MonitorFromPoint, RoundRect, SelectObject, SetBkMode,
            SetDCBrushColor, SetDCPenColor, SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS, COLOR_HIGHLIGHT, DC_BRUSH, DC_PEN,
            DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
            DT_SINGLELINE, DT_VCENTER, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN, LOGFONTW,
            MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID,
            SRCCOPY, TEXTMETRICW, TRANSPARENT,
        },
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::MARGINS,
        HiDpi::{
            GetDpiForMonitor, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
        },
        Input::KeyboardAndMouse::{
            GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_APPS, VK_DOWN,
            VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
        },
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{
            CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow,
            DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, GetWindowLongPtrW,
            LoadCursorW, PostMessageW, RegisterClassW, RegisterWindowMessageW, SendMessageW,
            SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, CS_DBLCLKS,
            GWLP_USERDATA, HICON, ICONINFO, IDC_ARROW, MSG, SW_SHOW, WA_INACTIVE, WM_ACTIVATE,
            WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
            WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE,
            WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_RBUTTONDOWN, WM_RBUTTONUP,
            WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Display,
    System,
    Timer,
    Off,
}

#[cfg(not(test))]
const TRAY_CLASS: &str = "MiniLauncher.Caffeine.Tray";
#[cfg(not(test))]
const TRAY_ICON_ID: u32 = 1;
#[cfg(not(test))]
const WM_TRAY_SYNC: u32 = WM_APP + 1;
#[cfg(not(test))]
const WM_TRAY_CALLBACK: u32 = WM_APP + 2;
#[cfg(not(test))]
const WM_TRAY_SHUTDOWN: u32 = WM_APP + 3;

#[cfg(not(test))]
const CMD_SWITCH_MODE: u32 = 1;
#[cfg(not(test))]
const CMD_TURN_OFF: u32 = 2;
#[cfg(not(test))]
const CMD_INDEFINITE: u32 = 100;
#[cfg(not(test))]
const CMD_TIMER_15: u32 = 115;
#[cfg(not(test))]
const CMD_TIMER_30: u32 = 130;
#[cfg(not(test))]
const CMD_TIMER_60: u32 = 160;
#[cfg(not(test))]
const CMD_TIMER_120: u32 = 220;
#[cfg(not(test))]
const CMD_TIMER_240: u32 = 340;

#[cfg(not(test))]
struct SharedTray {
    controller: Controller,
    session: Option<ActiveSession>,
    show_tray_icon: bool,
}

#[cfg(not(test))]
static SHARED_TRAY: OnceLock<Mutex<SharedTray>> = OnceLock::new();
#[cfg(not(test))]
static TRAY_HWND: OnceLock<isize> = OnceLock::new();

#[cfg(test)]
pub fn init(_controller: Controller) {}

#[cfg(not(test))]
pub fn init(controller: Controller) {
    let show_tray_icon = controller
        .load_config()
        .map(|c| c.show_tray_icon)
        .unwrap_or(true);
    let session = controller.snapshot().ok().flatten();
    let _ = SHARED_TRAY.set(Mutex::new(SharedTray {
        controller,
        session,
        show_tray_icon,
    }));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("caffeine-tray".into())
        .spawn(move || tray_thread(ready_tx));
    if let Ok(hwnd) = ready_rx.recv_timeout(Duration::from_millis(500)) {
        if hwnd != 0 {
            let _ = TRAY_HWND.set(hwnd);
            unsafe {
                PostMessageW(hwnd as HWND, WM_TRAY_SYNC, 0, 0);
            }
        }
    }
}

#[cfg(test)]
pub fn sync(_session: Option<ActiveSession>, _show_tray_icon: bool) {}

#[cfg(not(test))]
pub fn sync(session: Option<ActiveSession>, show_tray_icon: bool) {
    if let Some(lock) = SHARED_TRAY.get() {
        if let Ok(mut guard) = lock.lock() {
            guard.session = session;
            guard.show_tray_icon = show_tray_icon;
        }
    }
    if let Some(&hwnd) = TRAY_HWND.get() {
        unsafe {
            PostMessageW(hwnd as HWND, WM_TRAY_SYNC, 0, 0);
        }
    }
}

#[cfg(test)]
pub fn shutdown() {}

#[cfg(not(test))]
pub fn shutdown() {
    if let Some(&hwnd) = TRAY_HWND.get() {
        unsafe {
            SendMessageW(hwnd as HWND, WM_TRAY_SHUTDOWN, 0, 0);
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    let dx = pax - bax * h;
    let dy = pay - bay * h;
    (dx * dx + dy * dy).sqrt()
}

/// Result rows and tray icons share the same coffee-cup silhouette and state colors.
pub fn render_item_icon_rgba(kind: IconKind, size: u32) -> Vec<u8> {
    render_icon_rgba(kind, size)
}

/// Signed distance for the Caffeine coffee cup tray icon in 32x32 centered space (`[-16, 16]`).
/// Returns `(d_cup_handle_saucer, d_steam)`.
fn sd_coffee_cup(nx: f32, ny: f32) -> (f32, f32) {
    // 1. Cup body: centered at (-1.8, 3.6), slightly tapered, rounded mug bottom
    let px = nx + 1.8;
    let py = ny - 3.6;
    let r = if py > 0.0 { 4.5 } else { 1.2 };
    let hx = (6.8 - py * 0.10).max(r + 0.5);
    let hy = 5.5;
    let qx = px.abs() - (hx - r);
    let qy = py.abs() - (hy - r);
    let d_body = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;

    // 2. Cup handle on the right side
    let d_hseg = sd_segment(nx, ny, 4.9, 2.2, 4.9, 4.4);
    let d_handle = ((d_hseg - 3.35).abs() - 1.15).max(3.8 - nx);

    // 3. Saucer / base plate under the cup
    let d_saucer = sd_segment(nx, ny, -9.4, 12.1, 6.2, 12.1) - 1.15;

    let d_cup = d_body.min(d_handle).min(d_saucer);

    // 4. Two rising wavy steam plumes above the cup rim
    let mut d_steam = 99.0f32;
    for x_base in [-4.4f32, 0.8f32] {
        let yc = ny.clamp(-12.2, -4.4);
        let t = (yc + 8.3) / 3.9;
        let wx = x_base + (t * 2.4).sin() * 1.05;
        let d_wisp = ((nx - wx).powi(2) + (ny - yc).powi(2)).sqrt() - 1.08;
        d_steam = d_steam.min(d_wisp);
    }

    (d_cup, d_steam)
}

/// Renders a crisp Caffeine coffee-cup icon in straight RGBA `[r, g, b, a]` for the system tray.
#[allow(dead_code)]
pub fn render_icon_rgba(kind: IconKind, size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let half = s * 0.5;
    let scale = 32.0 / s;
    let aa = 0.72 * scale;

    for y in 0..size {
        for x in 0..size {
            let nx = ((x as f32 + 0.5) - half) * scale;
            let ny = ((y as f32 + 0.5) - half) * scale;

            let (d_cup, d_steam) = sd_coffee_cup(nx, ny);
            let d_all = d_cup.min(d_steam);

            // Subtle dark halo outline around the silhouette so it reads cleanly on any taskbar
            let outline_alpha = (1.0 - smoothstep(-aa, aa, d_all - 1.15)) * 0.82;
            if outline_alpha <= 0.001 {
                continue;
            }

            let cup_alpha = 1.0 - smoothstep(-aa, aa, d_cup);
            let steam_alpha = 1.0 - smoothstep(-aa, aa, d_steam);
            let fg_alpha = cup_alpha.max(steam_alpha);

            let grad_t = ((ny + 2.0) / 14.0).clamp(0.0, 1.0);
            let (cup_r, cup_g, cup_b) = match kind {
                IconKind::Display => {
                    let r = 252.0 * (1.0 - grad_t) + 217.0 * grad_t;
                    let g = 211.0 * (1.0 - grad_t) + 119.0 * grad_t;
                    let b = 77.0 * (1.0 - grad_t) + 6.0 * grad_t;
                    (r, g, b)
                }
                IconKind::System => {
                    let r = 125.0 * (1.0 - grad_t) + 2.0 * grad_t;
                    let g = 211.0 * (1.0 - grad_t) + 132.0 * grad_t;
                    let b = 252.0 * (1.0 - grad_t) + 199.0 * grad_t;
                    (r, g, b)
                }
                IconKind::Timer => {
                    let r = 110.0 * (1.0 - grad_t) + 5.0 * grad_t;
                    let g = 231.0 * (1.0 - grad_t) + 150.0 * grad_t;
                    let b = 183.0 * (1.0 - grad_t) + 105.0 * grad_t;
                    (r, g, b)
                }
                IconKind::Off => {
                    let r = 148.0 * (1.0 - grad_t) + 100.0 * grad_t;
                    let g = 163.0 * (1.0 - grad_t) + 116.0 * grad_t;
                    let b = 184.0 * (1.0 - grad_t) + 139.0 * grad_t;
                    (r, g, b)
                }
            };

            let (steam_r, steam_g, steam_b) = match kind {
                IconKind::Display => (254.0, 243.0, 199.0),
                IconKind::System => (224.0, 242.0, 254.0),
                IconKind::Timer => (209.0, 250.0, 229.0),
                IconKind::Off => (226.0, 232.0, 240.0),
            };

            // Blend steam and cup colors
            let w_steam = if fg_alpha > 0.001 {
                (steam_alpha * (1.0 - cup_alpha * 0.8)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let fill_r = cup_r * (1.0 - w_steam) + steam_r * w_steam;
            let fill_g = cup_g * (1.0 - w_steam) + steam_g * w_steam;
            let fill_b = cup_b * (1.0 - w_steam) + steam_b * w_steam;

            // Composite foreground over dark contour halo (#141418)
            let out_a = fg_alpha + outline_alpha * (1.0 - fg_alpha);
            let r = ((fill_r * fg_alpha + 20.0 * outline_alpha * (1.0 - fg_alpha)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = ((fill_g * fg_alpha + 20.0 * outline_alpha * (1.0 - fg_alpha)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = ((fill_b * fg_alpha + 24.0 * outline_alpha * (1.0 - fg_alpha)) / out_a)
                .round()
                .clamp(0.0, 255.0) as u8;
            let a = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;

            let idx = ((y * size + x) * 4) as usize;
            out[idx] = r;
            out[idx + 1] = g;
            out[idx + 2] = b;
            out[idx + 3] = a;
        }
    }
    out
}

/// Encodes a `size x size` item icon into a standard Windows `.ico` file (32bpp BGRA DIB).
#[allow(dead_code)]
pub fn encode_ico(kind: IconKind, size: u32) -> Vec<u8> {
    let rgba = render_item_icon_rgba(kind, size);
    let xor_bytes = (size * size * 4) as usize;
    let and_row_bytes = ((size + 31) / 32 * 4) as usize;
    let and_bytes = and_row_bytes * (size as usize);
    let dib_size = 40 + xor_bytes + and_bytes;

    let mut ico = Vec::with_capacity(22 + dib_size);
    // ICONDIR
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    // ICONDIRENTRY
    ico.push(if size >= 256 { 0 } else { size as u8 });
    ico.push(if size >= 256 { 0 } else { size as u8 });
    ico.push(0);
    ico.push(0);
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&32u16.to_le_bytes());
    ico.extend_from_slice(&(dib_size as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());

    // BITMAPINFOHEADER
    ico.extend_from_slice(&40u32.to_le_bytes());
    ico.extend_from_slice(&(size as i32).to_le_bytes());
    ico.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&32u16.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());
    ico.extend_from_slice(&((xor_bytes + and_bytes) as u32).to_le_bytes());
    ico.extend_from_slice(&0i32.to_le_bytes());
    ico.extend_from_slice(&0i32.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());

    // Bottom-up BGRA XOR bitmap
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let r = rgba[idx];
            let g = rgba[idx + 1];
            let b = rgba[idx + 2];
            let a = rgba[idx + 3];
            ico.extend_from_slice(&[b, g, r, a]);
        }
    }

    // Bottom-up 1bpp AND mask
    for y in (0..size).rev() {
        let mut row = vec![0u8; and_row_bytes];
        for x in 0..size {
            let a = rgba[((y * size + x) * 4 + 3) as usize];
            if a == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        ico.extend_from_slice(&row);
    }
    ico
}

#[cfg(not(test))]
unsafe fn create_tray_hicon(kind: IconKind) -> HICON {
    let size = 32u32;
    let rgba = render_icon_rgba(kind, size);
    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size as i32;
    bmi.bmiHeader.biHeight = -(size as i32); // top-down DIB
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let hdc = CreateCompatibleDC(null_mut());
    let mut bits: *mut core::ffi::c_void = null_mut();
    let hbm_color = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if !hbm_color.is_null() && !bits.is_null() {
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for i in 0..(size * size) as usize {
            let r = rgba[i * 4] as u32;
            let g = rgba[i * 4 + 1] as u32;
            let b = rgba[i * 4 + 2] as u32;
            let a = rgba[i * 4 + 3] as u32;
            dst[i * 4] = ((b * a + 127) / 255) as u8;
            dst[i * 4 + 1] = ((g * a + 127) / 255) as u8;
            dst[i * 4 + 2] = ((r * a + 127) / 255) as u8;
            dst[i * 4 + 3] = a as u8;
        }
    }
    let hbm_mask = CreateBitmap(size as i32, size as i32, 1, 1, null());
    let icon_info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let hicon = CreateIconIndirect(&icon_info);
    if !hbm_color.is_null() {
        DeleteObject(hbm_color as _);
    }
    if !hbm_mask.is_null() {
        DeleteObject(hbm_mask as _);
    }
    if !hdc.is_null() {
        DeleteDC(hdc);
    }
    hicon
}

#[cfg(not(test))]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(not(test))]
fn copy_tip(dst: &mut [u16; 128], text: &str) {
    dst.fill(0);
    for (slot, ch) in dst.iter_mut().take(127).zip(text.encode_utf16()) {
        *slot = ch;
    }
}

#[cfg(not(test))]
struct TrayWindowState {
    added: bool,
    last_mode: Option<PowerMode>,
    last_tip: String,
    icon_display: isize,
    icon_system: isize,
    taskbar_created_msg: u32,
}

#[cfg(not(test))]
static WINDOW_STATE: OnceLock<Mutex<TrayWindowState>> = OnceLock::new();

#[cfg(not(test))]
unsafe fn sync_tray_icon(hwnd: HWND) {
    let Some(shared_lock) = SHARED_TRAY.get() else {
        return;
    };
    let (session, show_tray_icon) = match shared_lock.lock() {
        Ok(guard) => (guard.session, guard.show_tray_icon),
        Err(_) => return,
    };
    let Some(win_lock) = WINDOW_STATE.get() else {
        return;
    };
    let mut win = match win_lock.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if let (Some(session), true) = (session, show_tray_icon) {
        let timer = match session.deadline {
            Some(deadline) => format_remaining(deadline),
            None => "无限期".into(),
        };
        let tip = format!(
            "防休眠：{}（{timer}）\n双击关闭 · 右键切换模式或定时",
            session.mode.short_label()
        );
        if win.added && win.last_mode == Some(session.mode) && win.last_tip == tip {
            return;
        }
        let mut nid: NOTIFYICONDATAW = zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY_CALLBACK;
        nid.hIcon = match session.mode {
            PowerMode::Display => win.icon_display as HICON,
            PowerMode::System => win.icon_system as HICON,
        };
        copy_tip(&mut nid.szTip, &tip);

        if win.added {
            if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
                win.added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
            }
        } else {
            win.added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
        }
        if win.added {
            win.last_mode = Some(session.mode);
            win.last_tip = tip;
        }
    } else if win.added {
        let menu = CURRENT_MENU.swap(0, Ordering::Relaxed) as HWND;
        if !menu.is_null() {
            DestroyWindow(menu);
        }
        let mut nid: NOTIFYICONDATAW = zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        Shell_NotifyIconW(NIM_DELETE, &nid);
        win.added = false;
        win.last_mode = None;
        win.last_tip.clear();
    }
}

#[cfg(not(test))]
unsafe fn remove_tray_icon(hwnd: HWND) {
    if let Some(win_lock) = WINDOW_STATE.get() {
        if let Ok(mut win) = win_lock.lock() {
            if win.added {
                let mut nid: NOTIFYICONDATAW = zeroed();
                nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = hwnd;
                nid.uID = TRAY_ICON_ID;
                Shell_NotifyIconW(NIM_DELETE, &nid);
                win.added = false;
            }
        }
    }
}

#[cfg(not(test))]
const MENU_CLASS: &str = "MiniLauncher.Caffeine.TrayMenu";
#[cfg(not(test))]
const MENU_WIDTH: i32 = 296;
#[cfg(not(test))]
const MENU_ROW: i32 = 36;
#[cfg(not(test))]
const MENU_PADDING: i32 = 6;

#[cfg(not(test))]
static CURRENT_MENU: AtomicIsize = AtomicIsize::new(0);

#[cfg(not(test))]
fn color(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

#[cfg(not(test))]
fn mix_color(background: u32, accent: u32, percent: u32) -> u32 {
    let channel = |shift: u32| -> u32 {
        let base = (background >> shift) & 255u32;
        let tint = (accent >> shift) & 255u32;
        (base * (100 - percent) + tint * percent + 50) / 100
    };
    channel(0) | (channel(8) << 8) | (channel(16) << 16)
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
struct MenuPalette {
    surface: u32,
    text: u32,
    secondary: u32,
    border: u32,
    selection: u32,
    accent: u32,
}

#[cfg(not(test))]
impl MenuPalette {
    fn current() -> (Self, bool) {
        let (accent, system_dark) = super::runtime::theme_colors();
        let accent = accent
            .map(|(r, g, b)| color(r, g, b))
            .unwrap_or_else(|| unsafe { GetSysColor(COLOR_HIGHLIGHT) });
        let system_dark = system_dark.unwrap_or(true);
        let dark = match mini_launcher::config::load() {
            Ok(cfg) => match cfg.theme.trim().to_ascii_lowercase().as_str() {
                "light" => false,
                "dark" => true,
                _ => system_dark,
            },
            Err(_) => system_dark,
        };
        let palette = if dark {
            Self {
                surface: color(38, 38, 41),
                text: color(247, 247, 247),
                secondary: color(190, 190, 194),
                border: color(48, 48, 52),
                selection: mix_color(color(28, 28, 30), accent, 20),
                accent,
            }
        } else {
            Self {
                surface: color(240, 240, 244),
                text: color(26, 26, 26),
                secondary: color(90, 90, 96),
                border: color(225, 225, 230),
                selection: mix_color(color(250, 250, 252), accent, 12),
                accent,
            }
        };
        (palette, dark)
    }
}

#[cfg(not(test))]
struct MenuFonts {
    body: HFONT,
    symbol: HFONT,
}

#[cfg(not(test))]
impl MenuFonts {
    unsafe fn new(dpi: u32) -> Self {
        unsafe fn make_font(size: i32, weight: i32, name: &str, dpi: u32) -> HFONT {
            CreateFontW(
                -((size as f64 * dpi as f64 / 96.0).round() as i32),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide(name).as_ptr(),
            )
        }
        unsafe extern "system" fn found_family(
            _font: *const LOGFONTW,
            _metrics: *const TEXTMETRICW,
            _kind: u32,
            context: LPARAM,
        ) -> i32 {
            *(context as *mut bool) = true;
            0
        }
        static TEXT_FACE: OnceLock<&'static str> = OnceLock::new();
        let text_face = *TEXT_FACE.get_or_init(|| {
            let mut face = "Microsoft YaHei UI";
            let dc = CreateCompatibleDC(null_mut());
            if !dc.is_null() {
                let mut query: LOGFONTW = zeroed();
                query.lfCharSet = DEFAULT_CHARSET;
                let preferred = wide("Noto Sans SC");
                query.lfFaceName[..preferred.len()].copy_from_slice(&preferred);
                let mut found = false;
                EnumFontFamiliesExW(
                    dc,
                    &query,
                    Some(found_family),
                    &mut found as *mut bool as LPARAM,
                    0,
                );
                DeleteDC(dc);
                if found {
                    face = "Noto Sans SC";
                }
            }
            face
        });
        Self {
            body: make_font(14, 400, text_face, dpi),
            symbol: make_font(19, 400, "Segoe Fluent Icons", dpi),
        }
    }
}

#[cfg(not(test))]
impl Drop for MenuFonts {
    fn drop(&mut self) {
        unsafe {
            for font in [self.body, self.symbol] {
                if !font.is_null() {
                    DeleteObject(font);
                }
            }
        }
    }
}

#[cfg(not(test))]
#[derive(Default)]
struct MenuBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    width: i32,
    height: i32,
}

#[cfg(not(test))]
impl MenuBuffer {
    unsafe fn ensure(&mut self, target: HDC, width: i32, height: i32) -> bool {
        if self.width == width && self.height >= height && !self.dc.is_null() {
            return true;
        }
        let mut next = Self::default();
        next.dc = CreateCompatibleDC(target);
        if next.dc.is_null() {
            return false;
        }
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        next.bitmap = CreateDIBSection(target, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if next.bitmap.is_null() {
            DeleteDC(next.dc);
            return false;
        }
        next.original = SelectObject(next.dc, next.bitmap);
        next.width = width;
        next.height = height;
        *self = next;
        true
    }
}

#[cfg(not(test))]
impl Drop for MenuBuffer {
    fn drop(&mut self) {
        unsafe {
            if !self.original.is_null() {
                SelectObject(self.dc, self.original);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(not(test))]
struct TrayMenuItem {
    id: u32,
    glyph: &'static str,
    label: String,
    accent: Option<u32>,
    separator_above: bool,
}

#[cfg(not(test))]
struct TrayPopup {
    owner: HWND,
    items: Vec<TrayMenuItem>,
    palette: MenuPalette,
    fonts: MenuFonts,
    dpi: u32,
    buffer: MenuBuffer,
    selected: usize,
    mouse_position: Option<(i32, i32)>,
}

#[cfg(not(test))]
impl TrayPopup {
    fn px(&self, value: i32) -> i32 {
        (value as f64 * self.dpi as f64 / 96.0).round() as i32
    }

    fn rect(&self, r: [i32; 4]) -> RECT {
        RECT {
            left: self.px(r[0]),
            top: self.px(r[1]),
            right: self.px(r[2]),
            bottom: self.px(r[3]),
        }
    }

    unsafe fn fill(&self, dc: HDC, r: RECT, color: u32) {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &r, brush);
        DeleteObject(brush);
    }

    unsafe fn round_rect(&self, dc: HDC, r: [i32; 4], radius: i32, background: u32, border: u32) {
        let brush = GetStockObject(DC_BRUSH) as HBRUSH;
        SetDCBrushColor(dc, background);
        let old_brush = SelectObject(dc, brush);
        let (pen, owned_pen) = if background == border || self.px(1) <= 1 {
            let pen = GetStockObject(DC_PEN) as HPEN;
            SetDCPenColor(dc, border);
            (pen, false)
        } else {
            (CreatePen(PS_SOLID, self.px(1).max(1), border), true)
        };
        let old_pen = SelectObject(dc, pen);
        let r = self.rect(r);
        RoundRect(
            dc,
            r.left,
            r.top,
            r.right,
            r.bottom,
            self.px(radius * 2),
            self.px(radius * 2),
        );
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        if owned_pen {
            DeleteObject(pen);
        }
    }

    unsafe fn text(&self, dc: HDC, text: &str, r: [i32; 4], font: HFONT, color: u32) {
        let old = SelectObject(dc, font);
        SetTextColor(dc, color);
        let text = wide(text);
        let mut r = self.rect(r);
        DrawTextW(
            dc,
            text.as_ptr(),
            (text.len() - 1) as i32,
            &mut r,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, old);
    }

    fn select(&mut self, hwnd: HWND, selected: usize) {
        if self.selected != selected {
            self.selected = selected;
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let y = y - self.px(MENU_PADDING);
        let row = y / self.px(MENU_ROW);
        ((0..self.px(MENU_WIDTH)).contains(&x) && y >= 0 && (row as usize) < self.items.len())
            .then_some(row as usize)
    }

    unsafe fn paint(&mut self, hwnd: HWND, print_dc: Option<HDC>) {
        let height = self.items.len() as i32 * MENU_ROW + 2 * MENU_PADDING;
        let (width_px, height_px) = (self.px(MENU_WIDTH), self.px(height));
        let mut paint: PAINTSTRUCT = zeroed();
        let target = print_dc.unwrap_or_else(|| BeginPaint(hwnd, &mut paint));
        let buffered = self.buffer.ensure(target, width_px, height_px);
        let dc = if buffered { self.buffer.dc } else { target };
        let palette = self.palette;
        SetBkMode(dc, TRANSPARENT as i32);
        self.fill(dc, self.rect([0, 0, MENU_WIDTH, height]), palette.surface);
        self.round_rect(
            dc,
            [0, 0, MENU_WIDTH, height],
            8,
            palette.surface,
            palette.border,
        );
        for (index, item) in self.items.iter().enumerate() {
            let y = MENU_PADDING + index as i32 * MENU_ROW;
            if item.separator_above {
                self.fill(
                    dc,
                    self.rect([14, y, MENU_WIDTH - 14, y + 1]),
                    palette.border,
                );
            }
            if index == self.selected {
                self.round_rect(
                    dc,
                    [5, y + 2, MENU_WIDTH - 5, y + MENU_ROW - 2],
                    5,
                    palette.selection,
                    palette.selection,
                );
                self.fill(
                    dc,
                    self.rect([5, y + 11, 8, y + MENU_ROW - 11]),
                    item.accent.unwrap_or(palette.accent),
                );
            }
            self.text(
                dc,
                item.glyph,
                [18, y, 40, y + MENU_ROW],
                self.fonts.symbol,
                item.accent.unwrap_or(palette.secondary),
            );
            self.text(
                dc,
                &item.label,
                [48, y, MENU_WIDTH - 18, y + MENU_ROW],
                self.fonts.body,
                palette.text,
            );
        }
        if buffered {
            BitBlt(target, 0, 0, width_px, height_px, dc, 0, 0, SRCCOPY);
        }
        if print_dc.is_none() {
            EndPaint(hwnd, &paint);
        }
    }
}

#[cfg(not(test))]
unsafe fn execute_menu_item(hwnd: HWND, index: usize) {
    let Some(popup) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopup).as_ref() else {
        return;
    };
    let Some(item) = popup.items.get(index) else {
        return;
    };
    let owner = popup.owner;
    let id = item.id as usize;
    DestroyWindow(hwnd);
    PostMessageW(owner, WM_COMMAND, id, 0);
}

#[cfg(not(test))]
unsafe extern "system" fn menu_popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCDESTROY {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopup;
        if !pointer.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(pointer));
        }
        let _ =
            CURRENT_MENU.compare_exchange(hwnd as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let Some(popup) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopup).as_mut() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };
    let (x, y) = (
        lparam as u16 as i16 as i32,
        (lparam >> 16) as u16 as i16 as i32,
    );
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            popup.paint(hwnd, None);
            0
        }
        WM_PRINTCLIENT => {
            popup.paint(hwnd, Some(wparam as HDC));
            0
        }
        WM_ACTIVATE if wparam & 0xffff == WA_INACTIVE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let mut cursor: POINT = zeroed();
            if GetCursorPos(&mut cursor) != 0 {
                let position = Some((cursor.x, cursor.y));
                if popup.mouse_position == position {
                    return 0;
                }
                popup.mouse_position = position;
            }
            if let Some(row) = popup.row_at(x, y) {
                popup.select(hwnd, row);
            }
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let mut client: RECT = zeroed();
            GetClientRect(hwnd, &mut client);
            if !(0..client.right).contains(&x) || !(0..client.bottom).contains(&y) {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            if let Some(row) = popup.row_at(x, y) {
                execute_menu_item(hwnd, row);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let step = if (wparam >> 16) as u16 as i16 > 0 { -1 } else { 1 };
            let next = (popup.selected as i32 + step).rem_euclid(popup.items.len() as i32);
            popup.select(hwnd, next as usize);
            0
        }
        WM_KEYDOWN => {
            let count = popup.items.len();
            match wparam as u16 {
                VK_UP => popup.select(hwnd, (popup.selected + count - 1) % count),
                VK_DOWN => popup.select(hwnd, (popup.selected + 1) % count),
                VK_TAB => {
                    let next = if GetKeyState(VK_SHIFT as i32) < 0 {
                        (popup.selected + count - 1) % count
                    } else {
                        (popup.selected + 1) % count
                    };
                    popup.select(hwnd, next);
                }
                VK_HOME => popup.select(hwnd, 0),
                VK_END => popup.select(hwnd, count - 1),
                VK_RETURN | VK_SPACE => execute_menu_item(hwnd, popup.selected),
                VK_ESCAPE | VK_LEFT | VK_APPS => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

#[cfg(not(test))]
unsafe fn show_context_menu(hwnd: HWND) {
    let previous = CURRENT_MENU.swap(0, Ordering::Relaxed) as HWND;
    if !previous.is_null() {
        DestroyWindow(previous);
    }

    let Some(shared_lock) = SHARED_TRAY.get() else {
        return;
    };
    let controller = match shared_lock.lock() {
        Ok(guard) => guard.controller.clone(),
        Err(_) => return,
    };
    let Ok(Some(session)) = controller.snapshot() else {
        sync_tray_icon(hwnd);
        return;
    };

    let (palette, dark) = MenuPalette::current();
    let mode_accent = match session.mode {
        PowerMode::Display => {
            if dark {
                color(245, 158, 11)
            } else {
                color(217, 119, 6)
            }
        }
        PowerMode::System => {
            if dark {
                color(56, 189, 248)
            } else {
                color(2, 132, 199)
            }
        }
    };

    let timer = match session.deadline {
        Some(deadline) => format_remaining(deadline),
        None => "无限期".into(),
    };
    let active_mins = session.duration.map(|d| d.as_secs() / 60).unwrap_or(0);
    let opposite = session.mode.opposite();

    let mut items = Vec::with_capacity(8);
    items.push(TrayMenuItem {
        id: CMD_SWITCH_MODE,
        glyph: match opposite {
            PowerMode::Display => "\u{E706}",
            PowerMode::System => "\u{EC46}",
        },
        label: format!("切换为：{}", opposite.label()),
        accent: None,
        separator_above: false,
    });

    let ind_active = session.deadline.is_none();
    items.push(TrayMenuItem {
        id: CMD_INDEFINITE,
        glyph: if ind_active { "\u{E73E}" } else { "\u{E823}" },
        label: if ind_active {
            format!("无限期开启（当前 · {}）", session.mode.short_label())
        } else {
            format!("无限期开启（{}）", session.mode.short_label())
        },
        accent: ind_active.then_some(mode_accent),
        separator_above: true,
    });

    let presets = [
        (15u64, CMD_TIMER_15, "定时 15 分钟"),
        (30, CMD_TIMER_30, "定时 30 分钟"),
        (60, CMD_TIMER_60, "定时 1 小时"),
        (120, CMD_TIMER_120, "定时 2 小时"),
        (240, CMD_TIMER_240, "定时 4 小时"),
    ];
    for (mins, cmd, text) in presets {
        let is_current = session.deadline.is_some() && active_mins == mins;
        items.push(TrayMenuItem {
            id: cmd,
            glyph: if is_current { "\u{E73E}" } else { "\u{E916}" },
            label: if is_current {
                format!("{text}（{timer}）")
            } else {
                text.into()
            },
            accent: is_current.then_some(mode_accent),
            separator_above: false,
        });
    }

    items.push(TrayMenuItem {
        id: CMD_TURN_OFF,
        glyph: "\u{E7E8}",
        label: format!("关闭防休眠（{} · {timer}）", session.mode.short_label()),
        accent: Some(mode_accent),
        separator_above: true,
    });

    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let area = info.rcWork;
    let (mut dpi_x, mut dpi_y) = (0, 0);
    if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) < 0 || dpi_x == 0 {
        dpi_x = 96;
    }
    let dpi = dpi_x;
    let px = |val: i32| (val as f64 * dpi as f64 / 96.0).round() as i32;
    let width = px(MENU_WIDTH);
    let height = px(items.len() as i32 * MENU_ROW + 2 * MENU_PADDING);
    let x = if cursor.x + width > area.right {
        cursor.x - width
    } else {
        cursor.x
    };
    let y = if cursor.y + height > area.bottom {
        cursor.y - height
    } else {
        cursor.y
    };
    let x = x.clamp(area.left, (area.right - width).max(area.left));
    let y = y.clamp(area.top, (area.bottom - height).max(area.top));

    let instance = GetModuleHandleW(null());
    let class_name = wide(MENU_CLASS);
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(menu_popup_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class_name.as_ptr(),
        ..zeroed()
    });
    let menu_hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class_name.as_ptr(),
        wide("Caffeine").as_ptr(),
        WS_POPUP,
        x,
        y,
        width,
        height,
        hwnd,
        null_mut(),
        instance,
        null(),
    );
    if menu_hwnd.is_null() {
        return;
    }
    let popup = Box::new(TrayPopup {
        owner: hwnd,
        items,
        palette,
        fonts: MenuFonts::new(dpi),
        dpi,
        buffer: MenuBuffer::default(),
        selected: 0,
        mouse_position: Some((cursor.x, cursor.y)),
    });
    SetWindowLongPtrW(menu_hwnd, GWLP_USERDATA, Box::into_raw(popup) as isize);
    CURRENT_MENU.store(menu_hwnd as isize, Ordering::Relaxed);
    let dark_flag = i32::from(dark);
    let rounded = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        menu_hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        &dark_flag as *const _ as _,
        4,
    );
    DwmSetWindowAttribute(
        menu_hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &rounded as *const _ as _,
        4,
    );
    let margins = MARGINS {
        cxLeftWidth: 1,
        cxRightWidth: 1,
        cyTopHeight: 1,
        cyBottomHeight: 1,
    };
    DwmExtendFrameIntoClientArea(menu_hwnd, &margins);
    ShowWindow(menu_hwnd, SW_SHOW);
    SetForegroundWindow(menu_hwnd);
    SetFocus(menu_hwnd);
    SetCapture(menu_hwnd);
}

#[cfg(not(test))]
unsafe fn handle_menu_command(cmd: u32) {
    let Some(shared_lock) = SHARED_TRAY.get() else {
        return;
    };
    let controller = match shared_lock.lock() {
        Ok(guard) => guard.controller.clone(),
        Err(_) => return,
    };
    let Ok(Some(session)) = controller.snapshot() else {
        return;
    };
    match cmd {
        CMD_SWITCH_MODE => {
            let _ = execute(&controller, "switch_mode");
        }
        CMD_TURN_OFF => {
            let _ = controller.deactivate();
        }
        CMD_INDEFINITE => {
            let _ = controller.activate(session.mode, None);
        }
        CMD_TIMER_15 => {
            let _ = controller.activate(session.mode, Some(Duration::from_secs(15 * 60)));
        }
        CMD_TIMER_30 => {
            let _ = controller.activate(session.mode, Some(Duration::from_secs(30 * 60)));
        }
        CMD_TIMER_60 => {
            let _ = controller.activate(session.mode, Some(Duration::from_secs(60 * 60)));
        }
        CMD_TIMER_120 => {
            let _ = controller.activate(session.mode, Some(Duration::from_secs(120 * 60)));
        }
        CMD_TIMER_240 => {
            let _ = controller.activate(session.mode, Some(Duration::from_secs(240 * 60)));
        }
        _ => {}
    }
}

#[cfg(not(test))]
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY_SYNC {
        sync_tray_icon(hwnd);
        return 0;
    }
    if msg == WM_TRAY_SHUTDOWN {
        let menu = CURRENT_MENU.swap(0, Ordering::Relaxed) as HWND;
        if !menu.is_null() {
            DestroyWindow(menu);
        }
        remove_tray_icon(hwnd);
        return 0;
    }
    if msg == WM_COMMAND && lparam == 0 {
        handle_menu_command(wparam as u32);
        return 0;
    }
    if msg == WM_TRAY_CALLBACK {
        let event = (lparam as u32) & 0xFFFF;
        match event {
            WM_LBUTTONDBLCLK | WM_MBUTTONUP => {
                if let Some(shared_lock) = SHARED_TRAY.get() {
                    let controller = shared_lock.lock().ok().map(|g| g.controller.clone());
                    if let Some(controller) = controller {
                        let _ = controller.deactivate();
                    }
                }
                return 0;
            }
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                show_context_menu(hwnd);
                return 0;
            }
            _ => {}
        }
    }
    if let Some(win_lock) = WINDOW_STATE.get() {
        if let Ok(mut win) = win_lock.lock() {
            if win.taskbar_created_msg != 0 && msg == win.taskbar_created_msg {
                win.added = false;
                drop(win);
                sync_tray_icon(hwnd);
                return 0;
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(not(test))]
fn tray_thread(ready_tx: std::sync::mpsc::SyncSender<isize>) {
    let _apartment = super::runtime::Apartment::new();
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let class_name = wide(TRAY_CLASS);
        let mut wc: WNDCLASSW = zeroed();
        wc.style = CS_DBLCLKS;
        wc.lpfnWndProc = Some(tray_wnd_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassW(&wc);

        let taskbar_msg = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        let icon_display = create_tray_hicon(IconKind::Display);
        let icon_system = create_tray_hicon(IconKind::System);
        let _ = WINDOW_STATE.set(Mutex::new(TrayWindowState {
            added: false,
            last_mode: None,
            last_tip: String::new(),
            icon_display: icon_display as isize,
            icon_system: icon_system as isize,
            taskbar_created_msg: taskbar_msg,
        }));

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        let _ = ready_tx.send(hwnd as isize);
        if hwnd.is_null() {
            return;
        }

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        remove_tray_icon(hwnd);
        if !icon_display.is_null() {
            DestroyIcon(icon_display);
        }
        if !icon_system.is_null() {
            DestroyIcon(icon_system);
        }
    }
}

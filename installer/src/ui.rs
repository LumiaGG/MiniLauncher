use std::path::PathBuf;
use std::sync::Mutex;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::Vector2;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::dialog::pick_folder;
use crate::payload::extract_payload;
use crate::system::*;

const WM_INSTALL_PROGRESS: u32 = WM_USER + 101;
const WM_INSTALL_FINISHED: u32 = WM_USER + 102;
const WM_INSTALL_FAILED: u32 = WM_USER + 103;

const ID_EDIT_PATH: usize = 1001;

// 标准设计客户区尺寸 (DIP，96 DPI 视口基准)
const BASE_CLIENT_WIDTH: i32 = 600;
const BASE_CLIENT_HEIGHT: i32 = 470;

// 各控件与热区逻辑坐标 (DIP)
const DIP_EDIT_LEFT: f32 = 44.0;
const DIP_EDIT_TOP: f32 = 144.0;
const DIP_EDIT_WIDTH: f32 = 416.0;
const DIP_EDIT_HEIGHT: f32 = 22.0;

const RECT_DIP_BTN_BROWSE: (f32, f32, f32, f32) = (474.0, 138.0, 556.0, 172.0); // left, top, right, bottom
const RECT_DIP_CHK_DESKTOP: (f32, f32, f32, f32) = (42.0, 234.0, 276.0, 262.0);
const RECT_DIP_CHK_STARTMENU: (f32, f32, f32, f32) = (296.0, 234.0, 556.0, 262.0);
const RECT_DIP_CHK_PORTABLE: (f32, f32, f32, f32) = (42.0, 272.0, 556.0, 300.0);
const RECT_DIP_CHK_LAUNCH: (f32, f32, f32, f32) = (42.0, 310.0, 556.0, 338.0);
const RECT_DIP_BTN_INSTALL: (f32, f32, f32, f32) = (440.0, 388.0, 556.0, 428.0);

#[derive(Clone, Copy, Debug)]
pub struct D2dTheme {
    pub is_dark: bool,
    pub window_bg: D2D1_COLOR_F,
    pub card_bg: D2D1_COLOR_F,
    pub card_border: D2D1_COLOR_F,
    pub text_primary: D2D1_COLOR_F,
    pub text_secondary: D2D1_COLOR_F,
    pub text_hint: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub accent_hover: D2D1_COLOR_F,
    pub accent_text: D2D1_COLOR_F,
    pub btn_sec_bg: D2D1_COLOR_F,
    pub btn_sec_hover: D2D1_COLOR_F,
    pub btn_sec_border: D2D1_COLOR_F,
    pub checkbox_border: D2D1_COLOR_F,
    pub progress_track: D2D1_COLOR_F,
    pub edit_bg_color: u32,
    pub edit_text_color: u32,
}

impl D2dTheme {
    pub fn current() -> Self {
        #[cfg(test)]
        if PREVIEW_LIGHT_THEME.load(std::sync::atomic::Ordering::Relaxed) {
            return Self::for_mode(false);
        }
        Self::for_mode(is_system_dark_mode())
    }

    fn for_mode(is_dark: bool) -> Self {
        if is_dark {
            Self {
                is_dark: true,
                window_bg: rgba(26, 26, 26, 1.0),
                card_bg: rgba(36, 36, 36, 0.95),
                card_border: rgba(255, 255, 255, 0.08),
                text_primary: rgba(255, 255, 255, 1.0),
                text_secondary: rgba(215, 215, 215, 0.8),
                text_hint: rgba(180, 180, 180, 0.55),
                accent: rgba(0, 120, 212, 1.0),
                accent_hover: rgba(24, 140, 235, 1.0),
                accent_text: rgba(255, 255, 255, 1.0),
                btn_sec_bg: rgba(48, 48, 48, 0.85),
                btn_sec_hover: rgba(62, 62, 62, 0.95),
                btn_sec_border: rgba(255, 255, 255, 0.12),
                checkbox_border: rgba(255, 255, 255, 0.35),
                progress_track: rgba(55, 55, 55, 1.0),
                edit_bg_color: rgb(36, 36, 36),
                edit_text_color: rgb(255, 255, 255),
            }
        } else {
            Self {
                is_dark: false,
                window_bg: rgba(244, 244, 244, 1.0),
                card_bg: rgba(255, 255, 255, 0.98),
                card_border: rgba(0, 0, 0, 0.08),
                text_primary: rgba(24, 24, 24, 1.0),
                text_secondary: rgba(85, 85, 85, 0.85),
                text_hint: rgba(120, 120, 120, 0.65),
                accent: rgba(0, 103, 192, 1.0),
                accent_hover: rgba(0, 120, 212, 1.0),
                accent_text: rgba(255, 255, 255, 1.0),
                btn_sec_bg: rgba(248, 248, 248, 0.95),
                btn_sec_hover: rgba(238, 238, 238, 1.0),
                btn_sec_border: rgba(0, 0, 0, 0.12),
                checkbox_border: rgba(0, 0, 0, 0.35),
                progress_track: rgba(225, 225, 225, 1.0),
                edit_bg_color: rgb(255, 255, 255),
                edit_text_color: rgb(24, 24, 24),
            }
        }
    }
}

#[cfg(test)]
static PREVIEW_LIGHT_THEME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn rgba(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn is_system_dark_mode() -> bool {
    let subkey: Vec<u16> = "Software\\Microsoft\x5CWindows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let val_name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) == 0 {
            let mut data: u32 = 1;
            let mut size: u32 = std::mem::size_of::<u32>() as u32;
            let status = RegQueryValueExW(
                hkey,
                val_name.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data as *mut u32 as _,
                &mut size,
            );
            RegCloseKey(hkey);
            if status == 0 {
                return data == 0;
            }
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HoverControl {
    None,
    BrowseButton,
    InstallButton,
    ChkDesktop,
    ChkStartMenu,
    ChkPortable,
    ChkLaunch,
}

struct RenderEngine {
    target: ID2D1HwndRenderTarget,
    dwrite: IDWriteFactory,
    fmt_title: IDWriteTextFormat,
    fmt_subtitle: IDWriteTextFormat,
    fmt_card_title: IDWriteTextFormat,
    fmt_normal: IDWriteTextFormat,
    fmt_small: IDWriteTextFormat,
    fmt_btn_bold: IDWriteTextFormat,
}

struct AppState {
    theme: D2dTheme,
    target_dir: PathBuf,
    chk_desktop: bool,
    chk_startmenu: bool,
    chk_portable: bool,
    chk_launch: bool,
    is_installing: bool,
    is_finished: bool,
    progress_percent: u32,
    status_text: String,
    hover: HoverControl,
    edit_hwnd: HWND,
    edit_brush: HBRUSH,
    edit_font: HFONT,
    engine: Option<RenderEngine>,
}

unsafe impl Send for AppState {}
unsafe impl Sync for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

pub fn run_installer_gui() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance =
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let class_name: Vec<u16> = "MiniLauncherInstallerWnd\0".encode_utf16().collect();

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };

        RegisterClassExW(&wnd_class);

        // 计算当前显示器 DPI 并精准计算包含标题栏边框的窗口物理尺寸
        let primary_dpi = GetDpiForSystem().max(96);
        let scale = primary_dpi as f32 / 96.0;
        let client_w = (BASE_CLIENT_WIDTH as f32 * scale).round() as i32;
        let client_h = (BASE_CLIENT_HEIGHT as f32 * scale).round() as i32;

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut rc_window = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        AdjustWindowRectExForDpi(&mut rc_window, style, 0, 0, primary_dpi);

        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as _, 0);
        let win_w = (rc_window.right - rc_window.left).min(work.right - work.left);
        let win_h = (rc_window.bottom - rc_window.top).min(work.bottom - work.top);
        let pos_x = work.left + (work.right - work.left - win_w) / 2;
        let pos_y = work.top + (work.bottom - work.top - win_h) / 2;

        let window_title: Vec<u16> = "MiniLauncher 安装向导\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_title.as_ptr(),
            style,
            pos_x,
            pos_y,
            win_w,
            win_h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );

        if hwnd.is_null() {
            return;
        }

        apply_window_material(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg = MSG {
            hwnd: std::ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: POINT { x: 0, y: 0 },
        };

        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn apply_window_material(hwnd: HWND) {
    unsafe {
        let theme = D2dTheme::current();
        let dark_val: i32 = if theme.is_dark { 1 } else { 0 };
        if DwmSetWindowAttribute(
            hwnd,
            20,
            &dark_val as *const i32 as _,
            std::mem::size_of::<i32>() as u32,
        ) != 0
        {
            let _ = DwmSetWindowAttribute(
                hwnd,
                19,
                &dark_val as *const i32 as _,
                std::mem::size_of::<i32>() as u32,
            );
        }

        // This window uses an opaque D2D HWND target and a GDI edit control,
        // neither of which supplies composition alpha for a full-client glass
        // surface. Extending glass over them washes out light-mode content.
        let backdrop_type: i32 = 1; // DWMSBT_NONE
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop_type as *const i32 as _,
            std::mem::size_of::<i32>() as u32,
        );

        let margins = MARGINS {
            cxLeftWidth: 0,
            cxRightWidth: 0,
            cyTopHeight: 0,
            cyBottomHeight: 0,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

unsafe fn create_render_engine(hwnd: HWND) -> Option<RenderEngine> {
    let d2d_factory: ID2D1Factory =
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
    let dwrite_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;

    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rc);
    let width = (rc.right - rc.left).max(1) as u32;
    let height = (rc.bottom - rc.top).max(1) as u32;

    let dpi = GetDpiForWindow(hwnd).max(96) as f32;

    let render_props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: dpi,
        dpiY: dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };

    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd: windows::Win32::Foundation::HWND(hwnd),
        pixelSize: D2D_SIZE_U { width, height },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };

    let target = d2d_factory
        .CreateHwndRenderTarget(&render_props, &hwnd_props)
        .ok()?;

    target.SetDpi(dpi, dpi);

    let font_family = HSTRING::from("Microsoft YaHei UI");
    let font_ptr = PCWSTR(font_family.as_ptr());
    let locale = HSTRING::from("zh-CN");
    let locale_ptr = PCWSTR(locale.as_ptr());

    let fmt_title = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            20.0,
            locale_ptr,
        )
        .ok()?;
    fmt_title
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .ok()?;

    let fmt_subtitle = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            12.5,
            locale_ptr,
        )
        .ok()?;

    let fmt_card_title = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            14.0,
            locale_ptr,
        )
        .ok()?;

    let fmt_normal = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            13.0,
            locale_ptr,
        )
        .ok()?;
    fmt_normal
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .ok()?;

    let fmt_small = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            12.0,
            locale_ptr,
        )
        .ok()?;
    fmt_small
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .ok()?;

    let fmt_btn_bold = dwrite_factory
        .CreateTextFormat(
            font_ptr,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            14.0,
            locale_ptr,
        )
        .ok()?;
    fmt_btn_bold
        .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)
        .ok()?;
    fmt_btn_bold
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .ok()?;

    Some(RenderEngine {
        target,
        dwrite: dwrite_factory,
        fmt_title,
        fmt_subtitle,
        fmt_card_title,
        fmt_normal,
        fmt_small,
        fmt_btn_bold,
    })
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            init_state(hwnd);
            0
        }
        WM_SETTINGCHANGE => {
            apply_window_material(hwnd);
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    state.theme = D2dTheme::current();
                    DeleteObject(state.edit_brush);
                    state.edit_brush = CreateSolidBrush(state.theme.edit_bg_color);
                }
            }
            RedrawWindow(
                hwnd,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let hdc = wparam as HDC;
            if let Ok(guard) = STATE.lock() {
                if let Some(state) = guard.as_ref() {
                    SetTextColor(hdc, state.theme.edit_text_color);
                    SetBkColor(hdc, state.theme.edit_bg_color);
                    return state.edit_brush as LRESULT;
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEMOVE => {
            let raw_x = (lparam & 0xFFFF) as i16 as i32;
            let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let scale = layout_scale(hwnd);
            let dip_x = raw_x as f32 / scale;
            let dip_y = raw_y as f32 / scale;
            handle_mouse_move(hwnd, dip_x, dip_y);
            0
        }
        WM_LBUTTONDOWN => {
            let raw_x = (lparam & 0xFFFF) as i16 as i32;
            let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let scale = layout_scale(hwnd);
            let dip_x = raw_x as f32 / scale;
            let dip_y = raw_y as f32 / scale;
            handle_click(hwnd, dip_x, dip_y);
            0
        }
        WM_INSTALL_PROGRESS => {
            let percent = wparam as u32;
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    state.progress_percent = percent;
                    state.status_text = format!("正在释放组件... {}%", percent);
                }
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_INSTALL_FINISHED => {
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    state.is_installing = false;
                    state.is_finished = true;
                    state.progress_percent = 100;
                    state.status_text = "安装已完成！".into();
                }
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);

            let (launch, target_dir) = if let Ok(guard) = STATE.lock() {
                if let Some(state) = guard.as_ref() {
                    (state.chk_launch, state.target_dir.clone())
                } else {
                    (false, PathBuf::new())
                }
            } else {
                (false, PathBuf::new())
            };

            if launch {
                let exe = target_dir.join("mini-launcher.exe");
                let _ = launch_app(&exe);
                PostQuitMessage(0);
            }
            0
        }
        WM_INSTALL_FAILED => {
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    state.is_installing = false;
                    state.status_text = "安装失败，请关闭已占用的文件后重试".into();
                }
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DPICHANGED => {
            let suggested = &*(lparam as *const RECT);
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor, &mut info);
            let work = info.rcWork;
            let width = (suggested.right - suggested.left).min(work.right - work.left);
            let height = (suggested.bottom - suggested.top).min(work.bottom - work.top);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left.clamp(work.left, work.right - width),
                suggested.top.clamp(work.top, work.bottom - height),
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            0
        }
        WM_SIZE => {
            if wparam == SIZE_MINIMIZED as usize {
                return 0;
            }
            let width = (lparam & 0xFFFF) as u32;
            let height = ((lparam >> 16) & 0xFFFF) as u32;
            let dpi = layout_scale(hwnd) * 96.0;
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    if let Some(engine) = state.engine.as_ref() {
                        engine.target.SetDpi(dpi, dpi);
                        let _ = engine.target.Resize(&D2D_SIZE_U {
                            width: width.max(1),
                            height: height.max(1),
                        });
                    }
                }
            }
            update_edit_layout(hwnd);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT {
                hdc: std::ptr::null_mut(),
                fErase: 0,
                rcPaint: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                fRestore: 0,
                fIncUpdate: 0,
                rgbReserved: [0; 32],
            };
            BeginPaint(hwnd, &mut ps);
            render_d2d(hwnd);
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.take() {
                    DeleteObject(state.edit_brush);
                    DeleteObject(state.edit_font);
                }
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// One physical scale for drawing, hit testing and the native edit, including small work areas.
unsafe fn layout_scale(hwnd: HWND) -> f32 {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    (GetDpiForWindow(hwnd).max(96) as f32 / 96.0)
        .min(rc.right.max(1) as f32 / BASE_CLIENT_WIDTH as f32)
        .min(rc.bottom.max(1) as f32 / BASE_CLIENT_HEIGHT as f32)
}

unsafe fn update_edit_layout(hwnd: HWND) {
    let scale = layout_scale(hwnd);
    let (edit, old_font) = match STATE.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(state) => (state.edit_hwnd, state.edit_font),
            None => return,
        },
        Err(_) => return,
    };
    let name: Vec<u16> = "Microsoft YaHei UI\0".encode_utf16().collect();
    let font = CreateFontW(
        -(13.0 * scale).round() as i32,
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        name.as_ptr(),
    );
    // Native edit messages can synchronously call the parent: never hold STATE here.
    SendMessageW(edit, WM_SETFONT, font as usize, 0);
    MoveWindow(
        edit,
        (DIP_EDIT_LEFT * scale).round() as i32,
        (DIP_EDIT_TOP * scale).round() as i32,
        (DIP_EDIT_WIDTH * scale).round() as i32,
        (DIP_EDIT_HEIGHT * scale).round() as i32,
        1,
    );
    if let Ok(mut guard) = STATE.lock() {
        if let Some(state) = guard.as_mut() {
            state.edit_font = font;
        }
    }
    DeleteObject(old_font);
}

unsafe fn init_state(hwnd: HWND) {
    let scale = layout_scale(hwnd);

    let font_name: Vec<u16> = "Microsoft YaHei UI\0".encode_utf16().collect();
    let font_size = -(13.0 * scale).round() as i32;
    let edit_font = CreateFontW(
        font_size,
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        font_name.as_ptr(),
    );

    let theme = D2dTheme::current();
    let default_dir = get_default_install_dir();
    let edit_brush = CreateSolidBrush(theme.edit_bg_color);

    let instance = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
    let edit_cls: Vec<u16> = "EDIT\0".encode_utf16().collect();
    let edit_txt: Vec<u16> = default_dir
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();

    // 物理坐标转换
    let edit_x = (DIP_EDIT_LEFT * scale).round() as i32;
    let edit_y = (DIP_EDIT_TOP * scale).round() as i32;
    let edit_w = (DIP_EDIT_WIDTH * scale).round() as i32;
    let edit_h = (DIP_EDIT_HEIGHT * scale).round() as i32;

    let edit_hwnd = CreateWindowExW(
        0,
        edit_cls.as_ptr(),
        edit_txt.as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | (ES_AUTOHSCROLL as u32),
        edit_x,
        edit_y,
        edit_w,
        edit_h,
        hwnd,
        ID_EDIT_PATH as _,
        instance,
        std::ptr::null(),
    );

    SendMessageW(edit_hwnd, WM_SETFONT, edit_font as usize, 1);

    let engine = create_render_engine(hwnd);

    if let Ok(mut guard) = STATE.lock() {
        *guard = Some(AppState {
            theme,
            target_dir: default_dir,
            chk_desktop: true,
            chk_startmenu: true,
            chk_portable: true,
            chk_launch: true,
            is_installing: false,
            is_finished: false,
            progress_percent: 0,
            status_text: "准备就绪".into(),
            hover: HoverControl::None,
            edit_hwnd,
            edit_brush,
            edit_font,
            engine,
        });
    }
}

fn pt_in_dip_rect(rc: &(f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= rc.0 && x <= rc.2 && y >= rc.1 && y <= rc.3
}

unsafe fn handle_mouse_move(hwnd: HWND, x: f32, y: f32) {
    let mut new_hover = HoverControl::None;
    if pt_in_dip_rect(&RECT_DIP_BTN_BROWSE, x, y) {
        new_hover = HoverControl::BrowseButton;
    } else if pt_in_dip_rect(&RECT_DIP_BTN_INSTALL, x, y) {
        new_hover = HoverControl::InstallButton;
    } else if pt_in_dip_rect(&RECT_DIP_CHK_DESKTOP, x, y) {
        new_hover = HoverControl::ChkDesktop;
    } else if pt_in_dip_rect(&RECT_DIP_CHK_STARTMENU, x, y) {
        new_hover = HoverControl::ChkStartMenu;
    } else if pt_in_dip_rect(&RECT_DIP_CHK_PORTABLE, x, y) {
        new_hover = HoverControl::ChkPortable;
    } else if pt_in_dip_rect(&RECT_DIP_CHK_LAUNCH, x, y) {
        new_hover = HoverControl::ChkLaunch;
    }

    let mut need_repaint = false;
    if let Ok(mut guard) = STATE.lock() {
        if let Some(state) = guard.as_mut() {
            if state.hover != new_hover {
                state.hover = new_hover;
                need_repaint = true;
            }
        }
    }
    if need_repaint {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

unsafe fn handle_click(hwnd: HWND, x: f32, y: f32) {
    let mut action_browse = false;
    let mut action_install = false;
    let mut need_repaint = false;

    if let Ok(mut guard) = STATE.lock() {
        if let Some(state) = guard.as_mut() {
            if state.is_installing {
                return;
            }
            if pt_in_dip_rect(&RECT_DIP_BTN_BROWSE, x, y) {
                action_browse = true;
            } else if pt_in_dip_rect(&RECT_DIP_BTN_INSTALL, x, y) {
                if state.is_finished {
                    PostQuitMessage(0);
                    return;
                }
                action_install = true;
            } else if pt_in_dip_rect(&RECT_DIP_CHK_DESKTOP, x, y) {
                state.chk_desktop = !state.chk_desktop;
                need_repaint = true;
            } else if pt_in_dip_rect(&RECT_DIP_CHK_STARTMENU, x, y) {
                state.chk_startmenu = !state.chk_startmenu;
                need_repaint = true;
            } else if pt_in_dip_rect(&RECT_DIP_CHK_PORTABLE, x, y) {
                state.chk_portable = !state.chk_portable;
                need_repaint = true;
            } else if pt_in_dip_rect(&RECT_DIP_CHK_LAUNCH, x, y) {
                state.chk_launch = !state.chk_launch;
                need_repaint = true;
            }
        }
    }

    if need_repaint {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }

    if action_browse {
        let (edit_hwnd, cur_path) = if let Ok(guard) = STATE.lock() {
            if let Some(state) = guard.as_ref() {
                (
                    state.edit_hwnd,
                    state.target_dir.to_string_lossy().to_string(),
                )
            } else {
                return;
            }
        } else {
            return;
        };

        if let Some(picked) = pick_folder(hwnd, Some(&cur_path)) {
            let final_dir = if picked
                .file_name()
                .map(|n| n == "MiniLauncher")
                .unwrap_or(false)
            {
                picked
            } else {
                picked.join("MiniLauncher")
            };
            let text_w: Vec<u16> = final_dir
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            SetWindowTextW(edit_hwnd, text_w.as_ptr());
            if let Ok(mut guard) = STATE.lock() {
                if let Some(state) = guard.as_mut() {
                    state.target_dir = final_dir;
                }
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }

    if action_install {
        start_installation(hwnd);
    }
}

unsafe fn start_installation(hwnd: HWND) {
    let (target_dir, desktop, startmenu, portable, edit_hwnd) = if let Ok(mut guard) = STATE.lock()
    {
        if let Some(state) = guard.as_mut() {
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(state.edit_hwnd, buf.as_mut_ptr(), 512);
            if len > 0 {
                let target_str = String::from_utf16_lossy(&buf[..len as usize]);
                state.target_dir = PathBuf::from(target_str.trim());
            }
            state.is_installing = true;
            state.status_text = "正在检查运行中的实例并写入文件...".into();
            (
                state.target_dir.clone(),
                state.chk_desktop,
                state.chk_startmenu,
                state.chk_portable,
                state.edit_hwnd,
            )
        } else {
            return;
        }
    } else {
        return;
    };

    windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(edit_hwnd, 0);
    InvalidateRect(hwnd, std::ptr::null(), 0);

    let hwnd_usize = hwnd as usize;

    std::thread::spawn(move || {
        let hwnd_target = hwnd_usize as HWND;
        terminate_running_instances(2000);

        let extract_res = extract_payload(&target_dir, None, portable, |cur, total, _name| {
            let percent = ((cur as f64 / total as f64) * 90.0) as usize;
            unsafe {
                PostMessageW(hwnd_target, WM_INSTALL_PROGRESS, percent, 0);
            }
        });

        if let Err(_e) = extract_res {
            unsafe {
                PostMessageW(hwnd_target, WM_INSTALL_FAILED, 0, 0);
            }
            return;
        }

        let exe_path = target_dir.join("mini-launcher.exe");

        if desktop {
            if let Some(desktop_lnk) = get_desktop_shortcut_path() {
                let _ =
                    create_shortcut(&exe_path, &desktop_lnk, &target_dir, "MiniLauncher 启动器");
            }
        }

        if startmenu {
            if let Some(startmenu_lnk) = get_start_menu_shortcut_path() {
                if let Some(p) = startmenu_lnk.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let _ = create_shortcut(
                    &exe_path,
                    &startmenu_lnk,
                    &target_dir,
                    "MiniLauncher 启动器",
                );
            }
        }

        let _ = register_uninstall_entry(&target_dir, "0.1.0");

        unsafe {
            PostMessageW(hwnd_target, WM_INSTALL_PROGRESS, 100, 0);
            PostMessageW(hwnd_target, WM_INSTALL_FINISHED, 0, 0);
        }
    });
}

unsafe fn draw_text_layout(
    rt: &ID2D1HwndRenderTarget,
    dwrite: &IDWriteFactory,
    text: &str,
    format: &IDWriteTextFormat,
    rect: &D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let width = (rect.right - rect.left).max(0.0);
    let height = (rect.bottom - rect.top).max(0.0);
    if let Ok(layout) = dwrite.CreateTextLayout(&utf16, format, width, height) {
        let origin = Vector2 {
            X: rect.left,
            Y: rect.top,
        };
        rt.DrawTextLayout(origin, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
    }
}

unsafe fn render_d2d(hwnd: HWND) {
    let mut guard = match STATE.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    if state.engine.is_none() {
        state.engine = create_render_engine(hwnd);
    }
    let engine = match state.engine.as_ref() {
        Some(e) => e,
        None => return,
    };

    let rt = &engine.target;
    let theme = &state.theme;

    rt.SetDpi(layout_scale(hwnd) * 96.0, layout_scale(hwnd) * 96.0);
    rt.BeginDraw();
    // D2D presentation must not overwrite the GDI edit surface (caret/text flicker).
    let scale = layout_scale(hwnd);
    let x = (DIP_EDIT_LEFT * scale).round() / scale;
    let y = (DIP_EDIT_TOP * scale).round() / scale;
    let right = x + (DIP_EDIT_WIDTH * scale).round() / scale;
    let bottom = y + (DIP_EDIT_HEIGHT * scale).round() / scale;
    let size = rt.GetSize();
    for clip in [
        D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: size.width,
            bottom: y,
        },
        D2D_RECT_F {
            left: 0.0,
            top: bottom,
            right: size.width,
            bottom: size.height,
        },
        D2D_RECT_F {
            left: 0.0,
            top: y,
            right: x,
            bottom,
        },
        D2D_RECT_F {
            left: right,
            top: y,
            right: size.width,
            bottom,
        },
    ] {
        rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        rt.Clear(Some(&theme.window_bg));

        // 1. 顶部 Header 区
        draw_header_d2d(rt, engine, theme);

        // 2. 卡片 1：安装路径选择
        draw_card_path_d2d(rt, engine, theme, state);

        // 3. 卡片 2：安装选项与便携模式
        draw_card_options_d2d(rt, engine, theme, state);

        // 4. 底部状态与操作区
        draw_footer_d2d(rt, engine, theme, state);

        rt.PopAxisAlignedClip();
    }
    if rt.EndDraw(None, None).is_err() {
        state.engine = None;
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

unsafe fn draw_header_d2d(rt: &ID2D1HwndRenderTarget, engine: &RenderEngine, theme: &D2dTheme) {
    // 绘制 Fluent 矢量应用徽标 (48x48)
    let icon_rect = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: 28.0,
            top: 20.0,
            right: 76.0,
            bottom: 68.0,
        },
        radiusX: 12.0,
        radiusY: 12.0,
    };

    let logo_brush = rt
        .CreateSolidColorBrush(
            &if theme.is_dark {
                rgba(0, 114, 206, 1.0)
            } else {
                rgba(0, 120, 215, 1.0)
            },
            None,
        )
        .unwrap();
    rt.FillRoundedRectangle(&icon_rect, &logo_brush);

    // 绘制 4 块半透明微光白磁贴
    let tile_brush = rt
        .CreateSolidColorBrush(&rgba(255, 255, 255, 0.95), None)
        .unwrap();
    let draw_tile = |x: f32, y: f32| {
        let t_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 28.0 + x,
                top: 20.0 + y,
                right: 28.0 + x + 13.0,
                bottom: 20.0 + y + 13.0,
            },
            radiusX: 3.5,
            radiusY: 3.5,
        };
        rt.FillRoundedRectangle(&t_rect, &tile_brush);
    };
    draw_tile(8.0, 8.0);
    draw_tile(27.0, 8.0);
    draw_tile(8.0, 27.0);
    draw_tile(27.0, 27.0);

    // 标题文本 (DirectWrite 次像素全抗锯齿)
    let title_brush = rt.CreateSolidColorBrush(&theme.text_primary, None).unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        "MiniLauncher 安装向导",
        &engine.fmt_title,
        &D2D_RECT_F {
            left: 90.0,
            top: 20.0,
            right: 560.0,
            bottom: 46.0,
        },
        &title_brush,
    );

    // 副标题文本
    let subtitle_brush = rt
        .CreateSolidColorBrush(&theme.text_secondary, None)
        .unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        "面向 Windows 10/11 的极简、原生、高响应轻量桌面启动器",
        &engine.fmt_subtitle,
        &D2D_RECT_F {
            left: 91.0,
            top: 48.0,
            right: 560.0,
            bottom: 68.0,
        },
        &subtitle_brush,
    );
}

unsafe fn draw_card_path_d2d(
    rt: &ID2D1HwndRenderTarget,
    engine: &RenderEngine,
    theme: &D2dTheme,
    state: &AppState,
) {
    let card = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: 28.0,
            top: 84.0,
            right: 572.0,
            bottom: 186.0,
        },
        radiusX: 10.0,
        radiusY: 10.0,
    };

    let card_bg = rt.CreateSolidColorBrush(&theme.card_bg, None).unwrap();
    let card_border = rt.CreateSolidColorBrush(&theme.card_border, None).unwrap();
    rt.FillRoundedRectangle(&card, &card_bg);
    rt.DrawRoundedRectangle(&card, &card_border, 1.0, None);

    // 标题
    let title_brush = rt.CreateSolidColorBrush(&theme.text_primary, None).unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        "安装目标路径",
        &engine.fmt_card_title,
        &D2D_RECT_F {
            left: 44.0,
            top: 96.0,
            right: 556.0,
            bottom: 116.0,
        },
        &title_brush,
    );

    // 说明
    let hint_brush = rt.CreateSolidColorBrush(&theme.text_hint, None).unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        "推荐安装至当前用户目录，免 UAC 提权，绿色便携",
        &engine.fmt_small,
        &D2D_RECT_F {
            left: 44.0,
            top: 116.0,
            right: 556.0,
            bottom: 134.0,
        },
        &hint_brush,
    );

    // Edit 控件外壳圆角矩形
    let edit_box = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: 38.0,
            top: 138.0,
            right: 464.0,
            bottom: 172.0,
        },
        radiusX: 6.0,
        radiusY: 6.0,
    };
    let edit_border_brush = rt
        .CreateSolidColorBrush(
            &if theme.is_dark {
                rgba(255, 255, 255, 0.12)
            } else {
                rgba(0, 0, 0, 0.15)
            },
            None,
        )
        .unwrap();
    rt.DrawRoundedRectangle(&edit_box, &edit_border_brush, 1.0, None);

    // 浏览按钮 (Fluent 二级药丸按钮)
    let is_hover = state.hover == HoverControl::BrowseButton;
    let btn_bg_color = if is_hover {
        theme.btn_sec_hover
    } else {
        theme.btn_sec_bg
    };
    let btn_box = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: RECT_DIP_BTN_BROWSE.0,
            top: RECT_DIP_BTN_BROWSE.1,
            right: RECT_DIP_BTN_BROWSE.2,
            bottom: RECT_DIP_BTN_BROWSE.3,
        },
        radiusX: 6.0,
        radiusY: 6.0,
    };
    let btn_bg = rt.CreateSolidColorBrush(&btn_bg_color, None).unwrap();
    let btn_border = rt
        .CreateSolidColorBrush(&theme.btn_sec_border, None)
        .unwrap();
    rt.FillRoundedRectangle(&btn_box, &btn_bg);
    rt.DrawRoundedRectangle(&btn_box, &btn_border, 1.0, None);

    draw_text_layout(
        rt,
        &engine.dwrite,
        "浏览...",
        &engine.fmt_btn_bold,
        &D2D_RECT_F {
            left: RECT_DIP_BTN_BROWSE.0,
            top: RECT_DIP_BTN_BROWSE.1,
            right: RECT_DIP_BTN_BROWSE.2,
            bottom: RECT_DIP_BTN_BROWSE.3,
        },
        &title_brush,
    );
}

unsafe fn draw_card_options_d2d(
    rt: &ID2D1HwndRenderTarget,
    engine: &RenderEngine,
    theme: &D2dTheme,
    state: &AppState,
) {
    let card = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: 28.0,
            top: 198.0,
            right: 572.0,
            bottom: 356.0,
        },
        radiusX: 10.0,
        radiusY: 10.0,
    };

    let card_bg = rt.CreateSolidColorBrush(&theme.card_bg, None).unwrap();
    let card_border = rt.CreateSolidColorBrush(&theme.card_border, None).unwrap();
    rt.FillRoundedRectangle(&card, &card_bg);
    rt.DrawRoundedRectangle(&card, &card_border, 1.0, None);

    let title_brush = rt.CreateSolidColorBrush(&theme.text_primary, None).unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        "安装与便携选项",
        &engine.fmt_card_title,
        &D2D_RECT_F {
            left: 44.0,
            top: 210.0,
            right: 556.0,
            bottom: 230.0,
        },
        &title_brush,
    );

    let accent_brush = rt.CreateSolidColorBrush(&theme.accent, None).unwrap();
    let white_brush = rt.CreateSolidColorBrush(&theme.accent_text, None).unwrap();

    let draw_chk = |chk_state: bool, label: &str, rc: &(f32, f32, f32, f32), is_hover: bool| {
        if is_hover && !state.is_installing && !state.is_finished {
            let brush = rt
                .CreateSolidColorBrush(&theme.btn_sec_hover, None)
                .unwrap();
            rt.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: rc.0 - 4.0,
                        top: rc.1 - 2.0,
                        right: rc.2,
                        bottom: rc.3 + 2.0,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                },
                &brush,
            );
        }
        let chk_x = rc.0 + 2.0;
        let chk_y = rc.1 + 4.0;
        let chk_size = 18.0;

        let chk_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: chk_x,
                top: chk_y,
                right: chk_x + chk_size,
                bottom: chk_y + chk_size,
            },
            radiusX: 4.0,
            radiusY: 4.0,
        };

        if chk_state {
            rt.FillRoundedRectangle(&chk_rect, &accent_brush);

            let p1 = Vector2 {
                X: chk_x + 4.0,
                Y: chk_y + 9.5,
            };
            let p2 = Vector2 {
                X: chk_x + 7.5,
                Y: chk_y + 13.0,
            };
            let p3 = Vector2 {
                X: chk_x + 13.5,
                Y: chk_y + 5.5,
            };
            rt.DrawLine(p1, p2, &white_brush, 2.0, None);
            rt.DrawLine(p2, p3, &white_brush, 2.0, None);
        } else {
            let border_color = if is_hover {
                theme.text_primary
            } else {
                theme.checkbox_border
            };
            let chk_border = rt.CreateSolidColorBrush(&border_color, None).unwrap();
            rt.DrawRoundedRectangle(&chk_rect, &chk_border, 1.5, None);
        }

        let lbl_rect = D2D_RECT_F {
            left: chk_x + 26.0,
            top: chk_y - 2.0,
            right: rc.2,
            bottom: chk_y + chk_size + 2.0,
        };
        draw_text_layout(
            rt,
            &engine.dwrite,
            label,
            &engine.fmt_normal,
            &lbl_rect,
            &title_brush,
        );
    };

    draw_chk(
        state.chk_desktop,
        "创建桌面快捷方式",
        &RECT_DIP_CHK_DESKTOP,
        state.hover == HoverControl::ChkDesktop,
    );
    draw_chk(
        state.chk_startmenu,
        "添加到开始菜单程序组",
        &RECT_DIP_CHK_STARTMENU,
        state.hover == HoverControl::ChkStartMenu,
    );
    draw_chk(
        state.chk_portable,
        "便携模式（配置与历史记录保存到同级 data/ 目录）",
        &RECT_DIP_CHK_PORTABLE,
        state.hover == HoverControl::ChkPortable,
    );
    draw_chk(
        state.chk_launch,
        "安装完成后立即启动 MiniLauncher",
        &RECT_DIP_CHK_LAUNCH,
        state.hover == HoverControl::ChkLaunch,
    );
}

unsafe fn draw_footer_d2d(
    rt: &ID2D1HwndRenderTarget,
    engine: &RenderEngine,
    theme: &D2dTheme,
    state: &AppState,
) {
    let divider = rt.CreateSolidColorBrush(&theme.card_border, None).unwrap();
    rt.DrawLine(
        Vector2 { X: 28.0, Y: 374.0 },
        Vector2 { X: 572.0, Y: 374.0 },
        &divider,
        1.0,
        None,
    );
    if state.is_installing || state.is_finished {
        // 胶囊进度条
        let track_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 28.0,
                top: 390.0,
                right: 420.0,
                bottom: 396.0,
            },
            radiusX: 3.0,
            radiusY: 3.0,
        };
        let track_brush = rt
            .CreateSolidColorBrush(&theme.progress_track, None)
            .unwrap();
        rt.FillRoundedRectangle(&track_rect, &track_brush);

        let fill_w = (420.0 - 28.0) * (state.progress_percent as f32 / 100.0);
        if fill_w > 4.0 {
            let fill_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 28.0,
                    top: 390.0,
                    right: 28.0 + fill_w,
                    bottom: 396.0,
                },
                radiusX: 3.0,
                radiusY: 3.0,
            };
            let accent_brush = rt.CreateSolidColorBrush(&theme.accent, None).unwrap();
            rt.FillRoundedRectangle(&fill_rect, &accent_brush);
        }

        // 状态文字
        let status_brush = rt
            .CreateSolidColorBrush(&theme.text_secondary, None)
            .unwrap();
        draw_text_layout(
            rt,
            &engine.dwrite,
            &state.status_text,
            &engine.fmt_small,
            &D2D_RECT_F {
                left: 28.0,
                top: 404.0,
                right: 420.0,
                bottom: 424.0,
            },
            &status_brush,
        );
    } else {
        let hint_brush = rt
            .CreateSolidColorBrush(&theme.text_secondary, None)
            .unwrap();
        draw_text_layout(
            rt,
            &engine.dwrite,
            "准备就绪，安装后即可使用",
            &engine.fmt_small,
            &D2D_RECT_F {
                left: 28.0,
                top: 398.0,
                right: 420.0,
                bottom: 418.0,
            },
            &hint_brush,
        );
    }

    // 主操作按钮 (Fluent Accent 胶囊按钮)
    let btn_color = if state.is_installing {
        theme.btn_sec_bg
    } else if state.hover == HoverControl::InstallButton {
        theme.accent_hover
    } else {
        theme.accent
    };

    let btn_box = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: RECT_DIP_BTN_INSTALL.0,
            top: RECT_DIP_BTN_INSTALL.1,
            right: RECT_DIP_BTN_INSTALL.2,
            bottom: RECT_DIP_BTN_INSTALL.3,
        },
        radiusX: 6.0,
        radiusY: 6.0,
    };
    let btn_brush = rt.CreateSolidColorBrush(&btn_color, None).unwrap();
    rt.FillRoundedRectangle(&btn_box, &btn_brush);

    let btn_str = if state.is_finished {
        "完成并退出"
    } else if state.is_installing {
        "正在安装..."
    } else {
        "立即安装"
    };
    let btn_text_brush = rt.CreateSolidColorBrush(&theme.accent_text, None).unwrap();
    draw_text_layout(
        rt,
        &engine.dwrite,
        btn_str,
        &engine.fmt_btn_bold,
        &D2D_RECT_F {
            left: RECT_DIP_BTN_INSTALL.0,
            top: RECT_DIP_BTN_INSTALL.1,
            right: RECT_DIP_BTN_INSTALL.2,
            bottom: RECT_DIP_BTN_INSTALL.3,
        },
        &btn_text_brush,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "interactive installer preview; run only with user authorization"]
    fn preview_light_theme() {
        // Exercise the actual window/rendering code without changing Windows
        // personalization or adding a theme override to the shipped installer.
        PREVIEW_LIGHT_THEME.store(true, std::sync::atomic::Ordering::Relaxed);
        run_installer_gui();
    }
}

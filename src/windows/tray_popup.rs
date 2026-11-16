//! Tray icon menu: a standalone themed popup that owns all of its state, so it
//! works while the launcher window is hidden and never touches the item menu.
use super::*;
use std::sync::atomic::AtomicIsize;

const CLASS: &str = "MiniLauncher.TrayMenu";
const WIDTH: i32 = 288;
const ROW: i32 = 36;
const PADDING: i32 = 6;
pub(super) const OPEN: usize = 1;
pub(super) const SETTINGS: usize = 2;
pub(super) const RELOAD: usize = 3;
pub(super) const QUIT: usize = 4;

/// Command id, Segoe Fluent glyph and label. A rule is drawn above the last row.
const ITEMS: [(usize, &str, &str); 4] = [
    (OPEN, "\u{E8A7}", "打开 MiniLauncher"),
    (SETTINGS, "\u{E713}", "设置"),
    (RELOAD, "\u{E72C}", "重新加载配置 / 扫描应用"),
    (QUIT, "\u{E7E8}", "退出"),
];

/// At most one tray popup exists; opening another replaces it.
static CURRENT: AtomicIsize = AtomicIsize::new(0);

struct Popup {
    owner: HWND,
    palette: Palette,
    background: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F,
    dpi: u32,
    accessibility: accessibility::Tree,
    renderer: gfx::WindowRenderer,
    text_engine: gfx::TextEngine,
    selected: usize,
    mouse_position: Option<(i32, i32)>,
}

impl Popup {
    fn px(&self, value: i32) -> i32 {
        layout::px(value, self.dpi)
    }

    fn select(&mut self, hwnd: HWND, selected: usize) {
        if self.selected != selected {
            self.selected = selected;
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
    }

    /// Row under a client-space point, if any.
    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let y = y - self.px(PADDING);
        let row = y / self.px(ROW);
        ((0..self.px(WIDTH)).contains(&x) && y >= 0 && (row as usize) < ITEMS.len())
            .then_some(row as usize)
    }

    unsafe fn paint(&mut self, hwnd: HWND, print_dc: Option<HDC>) {
        let height = ITEMS.len() as i32 * ROW + 2 * PADDING;
        let width_px = self.px(WIDTH);
        let mut bounds: RECT = zeroed();
        GetWindowRect(hwnd, &mut bounds);
        self.accessibility.view(
            self.selected,
            IsWindowVisible(hwnd) != 0,
            true,
            [
                bounds.left as f64,
                bounds.top as f64,
                (bounds.right - bounds.left) as f64,
                (bounds.bottom - bounds.top) as f64,
            ],
            (0..ITEMS.len())
                .map(|index| {
                    (
                        index,
                        [
                            bounds.left as f64,
                            (bounds.top + self.px(PADDING + index as i32 * ROW)) as f64,
                            width_px as f64,
                            self.px(ROW) as f64,
                        ],
                    )
                })
                .collect(),
        );
        let mut paint: PAINTSTRUCT = zeroed();
        if print_dc.is_none() {
            BeginPaint(hwnd, &mut paint);
        }
        let dpi = self.dpi;
        let selected = self.selected;
        let palette = self.palette;
        let background = self.background;
        let text_engine = &self.text_engine;

        if let Err(error) = self.renderer.render(hwnd, dpi, print_dc, |rt, _| {
            rt.SetDpi(dpi as f32, dpi as f32);
            rt.Clear(Some(
                &windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            ));

            let menu_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: WIDTH as f32,
                bottom: height as f32,
            };
            gfx::fill_round_rect(rt, &menu_rect, 8.0, background)?;
            gfx::stroke_round_rect(
                rt,
                &menu_rect,
                8.0,
                gfx::colorref_to_d2d(palette.border, 1.0),
                1.0,
            )?;

            for (index, (_, glyph, label)) in ITEMS.iter().enumerate() {
                let y = (PADDING + index as i32 * ROW) as f32;
                if index + 1 == ITEMS.len() {
                    gfx::draw_horizontal_line(
                        rt,
                        14.0,
                        (WIDTH - 14) as f32,
                        y,
                        1.0,
                        gfx::colorref_to_d2d(palette.border, 1.0),
                    )?;
                }
                if index == selected {
                    let sel_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: 5.0,
                        top: y + 2.0,
                        right: (WIDTH - 5) as f32,
                        bottom: y + ROW as f32 - 2.0,
                    };
                    gfx::fill_round_rect(
                        rt,
                        &sel_rect,
                        5.0,
                        gfx::colorref_to_d2d(palette.selection, 1.0),
                    )?;
                    let accent_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: 5.0,
                        top: y + 11.0,
                        right: 8.0,
                        bottom: y + ROW as f32 - 11.0,
                    };
                    gfx::fill_round_rect(
                        rt,
                        &accent_rect,
                        1.5,
                        gfx::colorref_to_d2d(palette.accent, 1.0),
                    )?;
                }

                let icon_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 18.0,
                    top: y,
                    right: 40.0,
                    bottom: y + ROW as f32,
                };
                text_engine.draw_text_role(
                    rt,
                    glyph,
                    gfx::FontRole::Symbol,
                    &icon_rect,
                    gfx::colorref_to_d2d(palette.secondary, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                let text_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 48.0,
                    top: y,
                    right: (WIDTH - 18) as f32,
                    bottom: y + ROW as f32,
                };
                text_engine.draw_text_role(
                    rt,
                    label,
                    gfx::FontRole::Body,
                    &text_rect,
                    gfx::colorref_to_d2d(palette.text, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;
            }
            Ok(())
        }) {
            eprintln!("Popup drawing failed: {error}");
        }

        if print_dc.is_none() {
            EndPaint(hwnd, &paint);
        }
    }
}

impl State {
    pub(super) unsafe fn menu(&mut self) {
        self.close_image_preview();
        if let Some(cancel) = &self.drag_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.cancel_pending_drag();
        self.paste_epoch.fetch_add(1, Ordering::Relaxed);
        self.remember_paste_target();
        let previous = CURRENT.swap(0, Ordering::Relaxed) as HWND;
        if !previous.is_null() {
            DestroyWindow(previous);
        }
        let mut cursor: POINT = zeroed();
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let area = info.rcWork;
        let (mut dpi_x, mut dpi_y) = (0, 0);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) < 0 || dpi_x == 0 {
            dpi_x = self.dpi;
        }
        let dpi = dpi_x;
        let width = layout::px(WIDTH, dpi);
        let height = layout::px(ITEMS.len() as i32 * ROW + 2 * PADDING, dpi);
        // Like the shell menus: open towards free space around the pointer.
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
        let class_name = wide(CLASS);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            wide("MiniLauncher").as_ptr(),
            WS_POPUP | WS_THICKFRAME,
            x,
            y,
            width,
            height,
            self.hwnd,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let renderer = match gfx::WindowRenderer::new() {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("Renderer initialization failed: {error}");
                DestroyWindow(hwnd);
                return;
            }
        };
        let text_engine = self.text_engine.clone();
        let active = backdrop::apply_popup(hwnd, self.dark, self.palette, &self.settings.backdrop);
        let popup = Box::new(Popup {
            owner: self.hwnd,
            palette: self.palette,
            background: backdrop::background(self.palette, &self.settings.backdrop, active),
            dpi,
            accessibility: accessibility::Tree::new(hwnd, true),
            renderer,
            text_engine,
            selected: 0,
            mouse_position: Some((cursor.x, cursor.y)),
        });
        popup.accessibility.nodes(
            ITEMS
                .iter()
                .map(|(id, _, label)| accessibility::Node {
                    id: id.to_string(),
                    name: (*label).into(),
                    help: String::new(),
                    bounds: [0.; 4],
                    actionable: true,
                })
                .collect(),
            String::new(),
        );
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(popup) as isize);
        CURRENT.store(hwnd as isize, Ordering::Relaxed);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        // Also catch clicks outside, in case Windows refuses the activation.
        SetCapture(hwnd);
    }
}

pub(super) unsafe fn update_theme(
    owner: HWND,
    dark: bool,
    palette: Palette,
    mode: &str,
    text_engine: &gfx::TextEngine,
) {
    let hwnd = CURRENT.load(Ordering::Relaxed) as HWND;
    if hwnd.is_null() {
        return;
    }
    let Some(popup) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup).as_mut() else {
        return;
    };
    if popup.owner != owner {
        return;
    }
    let active = backdrop::apply_popup(hwnd, dark, palette, mode);
    popup.palette = palette;
    popup.background = backdrop::background(palette, mode, active);
    popup.text_engine = text_engine.clone();
    InvalidateRect(hwnd, null(), 0);
}

/// Closes the popup and asks the launcher window to run the command.
unsafe fn execute(hwnd: HWND, index: usize) {
    let Some(popup) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup).as_ref() else {
        return;
    };
    let owner = popup.owner;
    DestroyWindow(hwnd);
    PostMessageW(owner, WM_COMMAND, ITEMS[index].0, 0);
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = backdrop::popup_frame_message(hwnd, message) {
        return result;
    }
    if message == WM_NCDESTROY {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup;
        if !pointer.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(pointer));
        }
        let _ = CURRENT.compare_exchange(hwnd as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let Some(popup) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup).as_mut() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };
    let (x, y) = (
        lparam as u16 as i16 as i32,
        (lparam >> 16) as u16 as i16 as i32,
    );
    match message {
        WM_GETOBJECT => popup
            .accessibility
            .get_object(wparam, lparam)
            .unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam)),
        accessibility::ACTION => {
            if let Some((Some(id), invoke)) = popup.accessibility.resolve(wparam, lparam) {
                if let Some(index) = ITEMS.iter().position(|item| item.0.to_string() == id) {
                    popup.select(hwnd, index);
                    if invoke {
                        execute(hwnd, index);
                    }
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_DPICHANGED => {
            popup.dpi = (wparam & 0xffff).max(96) as u32;
            let rect = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                rect.left,
                rect.top,
                popup.px(WIDTH),
                popup.px(ITEMS.len() as i32 * ROW + 2 * PADDING),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
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
        WM_ACTIVATEAPP if wparam == 0 => {
            // The owner may already be inactive because this menu had focus.
            PostMessageW(popup.owner, WM_CLOSE, 0, 0);
            DestroyWindow(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let mut cursor: POINT = zeroed();
            if GetCursorPos(&mut cursor) != 0 {
                // Repainting can synthesize a move; keep the keyboard selection
                // until the pointer really moves.
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
                execute(hwnd, row);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let step = if (wparam >> 16) as u16 as i16 > 0 {
                -1
            } else {
                1
            };
            let next = (popup.selected as i32 + step).rem_euclid(ITEMS.len() as i32);
            popup.select(hwnd, next as usize);
            0
        }
        WM_KEYDOWN => {
            let count = ITEMS.len();
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
                VK_RETURN | VK_SPACE => execute(hwnd, popup.selected),
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
            popup.accessibility.hide();
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

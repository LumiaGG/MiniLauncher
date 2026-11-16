//! Small drag visuals built from visible rows, without reading original images.
use super::*;
use mini_launcher::clipboard_image::Image;

pub(super) struct Scene {
    width: i32,
    height: i32,
    dpi: u32,
    palette: Palette,
    image: Option<Arc<Image>>,
    photo: bool,
    summary: String,
}

pub(super) fn snapshot(state: &State, entry: &Entry) -> Scene {
    let photo = entry.preview.as_ref();
    let (width, height) = if let Some(photo) = photo {
        let scale = (176.0 / photo.width.max(1) as f64).min(112.0 / photo.height.max(1) as f64);
        (
            (photo.width as f64 * scale).round().max(40.0) as i32 + 16,
            (photo.height as f64 * scale).round().max(32.0) as i32 + 16,
        )
    } else {
        (272, 72)
    };
    let value = match &entry.action {
        Action::Copy(text) => text,
        _ => &entry.title,
    };
    // Bound the snapshot before joining words, including very large copied values.
    let summary: String = value.chars().take(512).collect();
    let summary = summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(180)
        .collect();
    Scene {
        width,
        height,
        dpi: state.dpi,
        palette: state.palette,
        image: photo
            .and_then(|preview| state.previews.get(preview))
            .and_then(|image| image.as_ref())
            .map(gfx::BitmapImage::source),
        photo: photo.is_some(),
        summary,
    }
}

struct Visual {
    scene: Scene,
    renderer: gfx::WindowRenderer,
    image: Option<gfx::BitmapImage>,
    text_engine: gfx::TextEngine,
}

impl Visual {
    unsafe fn paint(&mut self, hwnd: HWND, print_dc: Option<HDC>) -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct2D::Common::*;
        use windows::Win32::Graphics::DirectWrite::*;
        let mut paint: PAINTSTRUCT = zeroed();
        if print_dc.is_none() {
            BeginPaint(hwnd, &mut paint);
        }
        let scene = &self.scene;
        let image = &self.image;
        let text = &self.text_engine;
        let result = self
            .renderer
            .render(hwnd, scene.dpi, print_dc, |rt, device| {
                let width = scene.width as f32;
                let height = scene.height as f32;
                let rect = D2D_RECT_F {
                    left: 0.5,
                    top: 0.5,
                    right: width - 0.5,
                    bottom: height - 0.5,
                };
                gfx::fill_round_rect(
                    rt,
                    &rect,
                    6.0,
                    gfx::colorref_to_d2d(scene.palette.surface, 0.92),
                )?;
                gfx::stroke_round_rect(
                    rt,
                    &rect,
                    6.0,
                    gfx::colorref_to_d2d(scene.palette.border, 0.92),
                    1.0,
                )?;
                let inner = D2D_RECT_F {
                    left: 8.0,
                    top: 8.0,
                    right: width - 8.0,
                    bottom: height - 8.0,
                };
                if let Some(image) = image {
                    image.draw_contained(device, &inner)?;
                } else if scene.photo {
                    text.draw_text_role(
                        rt,
                        "\u{E91B}",
                        gfx::FontRole::Symbol,
                        &inner,
                        gfx::colorref_to_d2d(scene.palette.accent, 1.0),
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                        DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    )?;
                } else {
                    text.draw_text_wrapped(
                        rt,
                        &scene.summary,
                        gfx::FontRole::Body,
                        &inner,
                        gfx::colorref_to_d2d(scene.palette.text, 1.0),
                        DWRITE_TEXT_ALIGNMENT_LEADING,
                        DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                    )?;
                }
                Ok(())
            });
        if print_dc.is_none() {
            EndPaint(hwnd, &paint);
        }
        result
    }
}

pub(super) struct Overlay(HWND);

thread_local! {
    static LAST_CURSOR: std::cell::Cell<Option<(usize, i32, i32)>> = const { std::cell::Cell::new(None) };
}

impl Overlay {
    pub(super) unsafe fn new(scene: Scene, text_engine: gfx::TextEngine) -> Result<Self, String> {
        let renderer = gfx::WindowRenderer::new().map_err(|e| e.to_string())?;
        let image = scene
            .image
            .clone()
            .map(gfx::BitmapImage::from_shared)
            .transpose()
            .map_err(|e| e.to_string())?;
        let width = layout::px(scene.width, scene.dpi);
        let height = layout::px(scene.height, scene.dpi);
        let class = wide("MiniLauncher.DragImage");
        let instance = GetModuleHandleW(null());
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&window_class);
        // Disabled, transparent and non-activating: never become the OLE drop target.
        // No owner HWND, so hiding the launcher cannot hide this window.
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_TOOLWINDOW
                | WS_EX_TOPMOST
                | WS_EX_NOACTIVATE
                | WS_EX_TRANSPARENT,
            class.as_ptr(),
            wide("拖动预览").as_ptr(),
            WS_POPUP | WS_DISABLED,
            0,
            0,
            width,
            height,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(format!(
                "无法创建拖动图样：{}",
                std::io::Error::last_os_error()
            ));
        }
        let overlay = Self(hwnd);

        let visual = Box::new(Visual {
            scene,
            renderer,
            image,
            text_engine,
        });
        let pointer = Box::into_raw(visual);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        (*pointer).paint(hwnd, None).map_err(|e| e.to_string())?;
        LAST_CURSOR.set(None);
        follow(hwnd as usize);
        Ok(overlay)
    }

    pub(super) fn window(&self) -> usize {
        self.0 as usize
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        LAST_CURSOR.set(None);
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

pub(super) unsafe fn follow(window: usize) {
    let hwnd = window as HWND;
    if hwnd.is_null() {
        return;
    }
    if GetAsyncKeyState(VK_LBUTTON as i32) >= 0 {
        hide(window);
        return;
    }
    let mut cursor: POINT = zeroed();
    if GetCursorPos(&mut cursor) == 0 {
        return;
    }
    if LAST_CURSOR.get() == Some((window, cursor.x, cursor.y)) {
        return;
    }
    let mut rect: RECT = zeroed();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return;
    }
    LAST_CURSOR.set(Some((window, cursor.x, cursor.y)));
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let offset = (GetDpiForWindow(hwnd).max(96) as i32 * 16 / 96).max(12);
    let mut x = cursor.x + offset;
    let mut y = cursor.y + offset;
    let mut monitor: MONITORINFO = zeroed();
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(
        MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) != 0
    {
        let work = monitor.rcWork;
        if x + width > work.right {
            x = cursor.x - width - offset;
        }
        if y + height > work.bottom {
            y = cursor.y - height - offset;
        }
        x = x.clamp(work.left, (work.right - width).max(work.left));
        y = y.clamp(work.top, (work.bottom - height).max(work.top));
    }
    if rect.left != x || rect.top != y || IsWindowVisible(hwnd) == 0 {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
        );
    }
}

pub(super) unsafe fn hide(window: usize) {
    LAST_CURSOR.set(None);
    if window != 0 {
        ShowWindow(window as HWND, SW_HIDE);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCDESTROY {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Visual;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !pointer.is_null() {
            drop(Box::from_raw(pointer));
        }
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Visual;
    match message {
        WM_PAINT | WM_PRINTCLIENT if !pointer.is_null() => {
            let dc = (message == WM_PRINTCLIENT).then_some(wparam as HDC);
            if let Err(error) = (*pointer).paint(hwnd, dc) {
                eprintln!("Drag visual drawing failed: {error}");
            }
            0
        }
        WM_DPICHANGED if !pointer.is_null() => {
            (*pointer).scene.dpi = (wparam & 0xffff).max(96) as u32;
            let rect = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

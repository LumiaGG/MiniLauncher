//! A non-activating original-image viewer; keyboard focus stays in the launcher.
use super::*;
use mini_launcher::image_cache::{self, SharedImage};
use std::sync::atomic::AtomicBool;

pub(super) const CLOSE: u32 = WM_APP + 17;
const LOADED: u32 = WM_APP + 18;
const CLASS: &str = "MiniLauncher.PictureViewer";

struct Viewer {
    _asset: Arc<image_cache::Asset>,
    closed: Arc<AtomicBool>,
    owner: HWND,
    palette: Palette,
    scale: f64,
    title: String,
    receiver: Receiver<Result<SharedImage, String>>,
    image: Option<gfx::BitmapImage>,
    error: Option<String>,
    renderer: gfx::WindowRenderer,
    text_engine: gfx::TextEngine,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl State {
    pub(super) fn can_preview_image(&self) -> bool {
        (self.result_navigation || self.query.query_text.trim().is_empty())
            && self
                .query
                .results
                .get(self.viewport.selected)
                .is_some_and(|e| e.preview.is_some())
    }

    pub(super) unsafe fn close_image_preview(&mut self) {
        let hwnd = std::mem::replace(&mut self.image_viewer, null_mut());
        if !hwnd.is_null() {
            DestroyWindow(hwnd);
            PostMessageW(self.hwnd, EVENT, 0, 0);
        }
    }

    pub(super) unsafe fn image_preview_key(&mut self, key: u16, flags: LPARAM) -> bool {
        if key == VK_ESCAPE && !self.image_viewer.is_null() {
            self.close_image_preview();
            return true;
        }
        if key != VK_SPACE {
            if key == VK_RETURN {
                self.close_image_preview();
            }
            return false;
        }
        if self.item_menu.is_some()
            || [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| GetKeyState(*key as i32) < 0)
            || (self.image_viewer.is_null() && !self.can_preview_image())
        {
            return false;
        }
        self.consume_preview_space = true;
        if flags & (1 << 30) != 0 {
            return true;
        }
        if !self.image_viewer.is_null() {
            self.close_image_preview();
            return true;
        }
        let Some(entry) = self.query.results.get(self.viewport.selected).cloned() else {
            return false;
        };
        self.open_image_preview(&entry);
        true
    }

    pub(super) unsafe fn open_image_preview(&mut self, entry: &Entry) {
        let Some(preview) = entry.preview.clone() else {
            return;
        };
        self.close_image_preview();
        self.text_search_request = self.text_search_request.wrapping_add(1);
        match open(
            self.hwnd,
            self.palette,
            self.text_engine.clone(),
            &entry.title,
            preview,
        ) {
            Ok(hwnd) => self.image_viewer = hwnd,
            Err(error) => {
                self.status = Some(format!("无法预览图片：{error}"));
                self.invalidate();
            }
        }
    }
}

unsafe fn open(
    owner: HWND,
    palette: Palette,
    text_engine: gfx::TextEngine,
    title: &str,
    preview: search::ImagePreview,
) -> Result<HWND, String> {
    let instance = GetModuleHandleW(null());
    let class = wide(CLASS);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&window_class);
    let mut monitor: MONITORINFO = zeroed();
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) == 0
    {
        return Err("无法读取屏幕工作区".into());
    }
    let work = monitor.rcWork;
    let width = (work.right - work.left) * 85 / 100;
    let height = (work.bottom - work.top) * 85 / 100;
    let scale = GetDpiForWindow(owner).max(96) as f64 / 96.0;
    let fit_width = (width - 2 - (40.0 * scale).round() as i32).max(1) as u32;
    let fit_height = (height - 2 - (72.0 * scale).round() as i32).max(1) as u32;
    let path = preview.original_path.as_deref().unwrap_or(&preview.path);
    let asset = image_cache::asset(std::path::Path::new(path))?;
    let ready = asset.cached_fit(fit_width, fit_height);
    let image = ready
        .clone()
        .map(gfx::BitmapImage::from_shared)
        .transpose()
        .map_err(|e| e.to_string())?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let closed = Arc::new(AtomicBool::new(false));
    let worker_asset = asset.clone();
    let renderer = gfx::WindowRenderer::new().map_err(|e| e.to_string())?;
    let viewer = Box::new(Viewer {
        _asset: asset,
        closed: closed.clone(),
        owner,
        palette,
        scale,
        title: format!("{title}  ·  {} × {}", preview.width, preview.height),
        receiver,
        image,
        error: None,
        renderer,
        text_engine,
    });
    let pointer = Box::into_raw(viewer);
    // Install userdata after CreateWindowEx returns so failed creation has one owner.
    let hwnd = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class.as_ptr(),
        wide("图片预览").as_ptr(),
        WS_POPUP | WS_BORDER,
        work.left + (work.right - work.left - width) / 2,
        work.top + (work.bottom - work.top - height) / 2,
        width,
        height,
        owner,
        null_mut(),
        instance,
        null(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(pointer));
        return Err(std::io::Error::last_os_error().to_string());
    }
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    let window = hwnd as usize;
    if ready.is_none() {
        std::thread::spawn(move || {
            if closed.load(Ordering::Relaxed) {
                return;
            }
            let image = worker_asset.image().and_then(|_| {
                // A native decode already in progress may finish after close.
                // Skip the expensive resize and release it without publishing.
                if closed.load(Ordering::Relaxed) {
                    return Err("预览已关闭".into());
                }
                worker_asset.fit(fit_width, fit_height)
            });
            if closed.load(Ordering::Relaxed) {
                return;
            }
            if sender.send(image).is_ok() {
                PostMessageW(window as HWND, LOADED, 0, 0);
            }
        });
    }
    Ok(hwnd)
}

impl Viewer {
    unsafe fn paint(&mut self, hwnd: HWND, target: Option<HDC>) {
        use windows::Win32::Graphics::Direct2D::Common::*;
        use windows::Win32::Graphics::Direct2D::*;

        let mut paint: PAINTSTRUCT = zeroed();
        if target.is_none() {
            BeginPaint(hwnd, &mut paint);
        }
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let width = (client.right.max(1) as f64 / self.scale).round() as i32;
        let height = (client.bottom.max(1) as f64 / self.scale).round() as i32;

        let palette = self.palette;
        let scale = self.scale;
        let padding = 20;
        let title = &self.title;
        let error = &self.error;
        let text_engine = &self.text_engine;
        let image = &self.image;

        if let Err(error) = self.renderer.render(
            hwnd,
            (scale * 96.0).round() as u32,
            target,
            |context, device| {
                let rt: &ID2D1RenderTarget = context;
                rt.Clear(Some(&D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }));

                let bg_rect = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: width as f32,
                    bottom: height as f32,
                };
                gfx::fill_rect(rt, &bg_rect, gfx::colorref_to_d2d(palette.background, 1.0))?;

                let title_rect = D2D_RECT_F {
                    left: padding as f32,
                    top: 0.0,
                    right: (width as i32 - 240) as f32,
                    bottom: 48.0,
                };
                text_engine.draw_text_role(
                    rt,
                    &title,
                    gfx::FontRole::Body,
                    &title_rect,
                    gfx::colorref_to_d2d(palette.secondary, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                let close_rect = D2D_RECT_F {
                    left: (width as i32 - 230) as f32,
                    top: 0.0,
                    right: (width as i32 - padding) as f32,
                    bottom: 48.0,
                };
                text_engine.draw_text_role(
                    rt,
                    "空格 / Esc 关闭    ×",
                    gfx::FontRole::Body,
                    &close_rect,
                    gfx::colorref_to_d2d(palette.secondary, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                let preview_top = 52.0;
                let preview_rect = D2D_RECT_F {
                    left: padding as f32,
                    top: preview_top,
                    right: (width as i32 - padding) as f32,
                    bottom: (height as i32 - padding) as f32,
                };

                if let Some(img) = &image {
                    img.draw_contained(device, &preview_rect)?;
                } else {
                    let msg = error.as_deref().unwrap_or("正在读取原图…");
                    text_engine.draw_text_role(
                        rt,
                        msg,
                        gfx::FontRole::Body,
                        &preview_rect,
                        gfx::colorref_to_d2d(palette.secondary, 1.0),
                        windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
                        windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    )?;
                }

                Ok(())
            },
        ) {
            eprintln!("Image viewer drawing failed: {error}");
        }

        if target.is_none() {
            EndPaint(hwnd, &paint);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Viewer;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    if message == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(pointer));
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let viewer = &mut *pointer;
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT | WM_PRINTCLIENT => {
            viewer.paint(
                hwnd,
                if message == WM_PRINTCLIENT {
                    Some(wparam as HDC)
                } else {
                    None
                },
            );
            0
        }
        LOADED => {
            if let Ok(result) = viewer.receiver.try_recv() {
                match result {
                    Ok(image) => match gfx::BitmapImage::from_shared(image) {
                        Ok(image) => viewer.image = Some(image),
                        Err(error) => viewer.error = Some(error.to_string()),
                    },
                    Err(error) => viewer.error = Some(format!("无法读取原图：{error}")),
                }
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_CLOSE => {
            PostMessageW(viewer.owner, CLOSE, 0, 0);
            0
        }
        WM_DPICHANGED => {
            viewer.scale = (wparam & 0xffff).max(96) as f64 / 96.0;
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
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

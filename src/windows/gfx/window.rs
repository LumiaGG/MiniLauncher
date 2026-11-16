//! One composition target per HWND, with a device shared by the UI thread.
use super::{CompositionSurface, GfxDevice};
use std::rc::Rc;
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::{Common::D2D1_COLOR_F, ID2D1DeviceContext};
use windows_sys::Win32::{Foundation::HWND, Graphics::Gdi::HDC};

pub struct WindowRenderer {
    device: Rc<GfxDevice>,
    surface: Option<CompositionSurface>,
    hwnd: HWND,
}

impl WindowRenderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            device: GfxDevice::shared()?,
            surface: None,
            hwnd: std::ptr::null_mut(),
        })
    }

    pub fn device(&self) -> Rc<GfxDevice> {
        self.device.clone()
    }

    fn prepare(&mut self, hwnd: HWND, dpi: u32) -> Result<()> {
        let current = GfxDevice::shared()?;
        let changed = current.id != self.device.id;
        if changed {
            self.detach();
            self.device = current;
        }
        if self.hwnd != hwnd {
            self.detach();
            GfxDevice::unregister_window(self.hwnd as usize);
            self.hwnd = hwnd;
            GfxDevice::register_window(hwnd as usize);
        }
        let mut rect = windows_sys::Win32::Foundation::RECT::default();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
        }
        let width = rect.right.max(1) as u32;
        let height = rect.bottom.max(1) as u32;
        if let Some(surface) = self.surface.as_mut() {
            surface.update_dpi(&self.device, dpi)?;
            surface.resize(&self.device, width, height)?;
        } else {
            self.surface = Some(CompositionSurface::new(
                windows::Win32::Foundation::HWND(hwnd as _),
                &self.device,
                width,
                height,
                dpi,
            )?);
        }
        Ok(())
    }

    fn begin(&self, color: Option<D2D1_COLOR_F>) {
        self.surface
            .as_ref()
            .expect("prepared surface")
            .begin_draw(&self.device.d2d_context, color);
    }

    fn finish(&self, print_dc: Option<HDC>) -> Result<()> {
        let surface = self.surface.as_ref().expect("prepared surface");
        let context = &self.device.d2d_context;
        let result = surface.end_draw(context).and_then(|()| {
            if let Some(dc) = print_dc {
                surface.copy_to_dc(context, dc)
            } else {
                surface.present(1)
            }
        });
        unsafe {
            context.SetTarget(None);
        }
        result
    }

    pub fn render(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        print_dc: Option<HDC>,
        draw: impl FnOnce(&ID2D1DeviceContext, &GfxDevice) -> Result<()>,
    ) -> Result<()> {
        let result = (|| {
            self.prepare(hwnd, dpi)?;
            self.begin(None);
            let drawn = draw(&self.device.d2d_context, &self.device);
            // Always close BeginDraw, even when scene creation fails.
            let finished = if drawn.is_ok() {
                self.finish(print_dc)
            } else {
                let ended = self
                    .surface
                    .as_ref()
                    .unwrap()
                    .end_draw(&self.device.d2d_context);
                unsafe {
                    self.device.d2d_context.SetTarget(None);
                }
                ended
            };
            finished.and(drawn)
        })();
        if let Err(error) = &result {
            self.handle_error(error);
        }
        result
    }

    fn handle_error(&mut self, error: &windows::core::Error) {
        if GfxDevice::is_device_lost(error.code()) {
            self.device.invalidate();
            self.detach();
        }
    }

    fn detach(&mut self) {
        if let Some(surface) = self.surface.take() {
            unsafe {
                self.device.d2d_context.SetTarget(None);
                // Destructors cannot return errors; removed devices may reject cleanup.
                for result in [
                    surface.dcomp_target.SetRoot(None),
                    self.device.dcomp_device.Commit(),
                ] {
                    if let Err(error) = result {
                        if !GfxDevice::is_device_lost(error.code()) {
                            eprintln!("Composition teardown failed: {error}");
                        }
                    }
                }
            }
        }
    }

    pub fn suspend(&mut self) {
        self.detach();
        self.device.release_idle_resources();
    }
}

impl Drop for WindowRenderer {
    fn drop(&mut self) {
        self.detach();
        GfxDevice::unregister_window(self.hwnd as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn shared_device_keeps_window_targets_independent_and_recovers_together() {
        unsafe {
            let create = || {
                CreateWindowExW(
                    0,
                    windows::core::w!("STATIC").as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    8,
                    8,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            let first = create();
            let second = create();
            assert!(!first.is_null() && !second.is_null());
            let mut a = WindowRenderer::new().unwrap();
            let mut b = WindowRenderer::new().unwrap();
            assert!(Rc::ptr_eq(&a.device, &b.device));
            let mut output = super::super::super::buffer::Buffer::default();
            assert!(output.ensure(std::ptr::null_mut(), 8, 8));
            let red = D2D1_COLOR_F {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
            let green = D2D1_COLOR_F {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            };
            a.render(first, 96, Some(output.dc), |rt, _| {
                rt.Clear(Some(&red));
                Ok(())
            })
            .unwrap();
            assert_eq!(*output.bits, 0xffff0000);
            b.render(second, 144, Some(output.dc), |rt, _| {
                rt.Clear(Some(&green));
                Ok(())
            })
            .unwrap();
            assert_eq!(*output.bits, 0xff00ff00);
            let failed = a.render(first, 96, None, |rt, _| {
                let rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: 1.0,
                    bottom: 1.0,
                };
                let _clip = crate::windows::gfx::shapes::ClipGuard::new(rt, &rect);
                Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_FAIL,
                ))
            });
            assert_eq!(
                failed.unwrap_err().code(),
                windows::Win32::Foundation::E_FAIL
            );
            a.render(first, 96, Some(output.dc), |rt, _| {
                rt.Clear(Some(&red));
                Ok(())
            })
            .unwrap();
            assert_eq!(*output.bits.add(4 * 8 + 4), 0xffff0000);
            let engine =
                crate::windows::gfx::TextEngine::new(&a.device.dwrite_factory, true).unwrap();
            let bounds = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: 100.0,
                bottom: 60.0,
            };
            let mut editor = crate::windows::editor::Editor::new(&engine, 96);
            editor.set_text("中文🙂\nline");
            editor.select_all();
            let mut view = crate::windows::text_panel::TextView::create(
                "panel".into(),
                0,
                "正文🙂\nline".into(),
                100.0,
                &engine,
            )
            .unwrap();
            view.select_all();
            let palette = crate::windows::Palette::new(true, 0);
            a.render(first, 96, None, |rt, _| {
                editor.draw(rt, bounds, &palette)?;
                view.draw(rt, &bounds, palette)?;
                engine.draw_text_role(
                    rt,
                    "\u{E721}",
                    crate::windows::gfx::FontRole::Symbol,
                    &bounds,
                    crate::windows::gfx::colorref_to_d2d(0xffffff, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )
            })
            .unwrap();
            let old_id = a.device.id;
            let lost = windows::core::HRESULT(0x887A0005u32 as i32);
            let failed = a.render(first, 96, None, |_, _| {
                Err(windows::core::Error::from_hresult(lost))
            });
            assert_eq!(failed.unwrap_err().code(), lost);
            assert!(a.surface.is_none());
            a.render(first, 96, Some(output.dc), |rt, _| {
                rt.Clear(Some(&red));
                Ok(())
            })
            .unwrap();
            b.render(second, 144, Some(output.dc), |rt, _| {
                rt.Clear(Some(&green));
                Ok(())
            })
            .unwrap();
            assert_ne!(a.device.id, old_id);
            assert!(Rc::ptr_eq(&a.device, &b.device));
            assert_eq!(*output.bits, 0xff00ff00);
            a.suspend();
            b.render(second, 144, Some(output.dc), |rt, _| {
                rt.Clear(Some(&green));
                Ok(())
            })
            .unwrap();
            assert_eq!(*output.bits, 0xff00ff00);
            drop((a, b));
            DestroyWindow(first);
            DestroyWindow(second);
        }
    }
}

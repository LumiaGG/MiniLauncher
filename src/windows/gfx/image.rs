use super::GfxDevice;
use mini_launcher::clipboard_image::Image;
use std::{cell::RefCell, sync::Arc};
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::HICON;

/// Shared CPU source for recovery, with one lazily uploaded copy per device generation.
pub struct BitmapImage {
    image: Arc<Image>,
    bitmap: RefCell<Option<(u64, ID2D1Bitmap1)>>,
}

impl BitmapImage {
    pub fn from_shared(image: Arc<Image>) -> Result<Self> {
        image.validate().map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_INVALIDARG, error)
        })?;
        Ok(Self {
            image,
            bitmap: RefCell::new(None),
        })
    }

    pub fn source(&self) -> Arc<Image> {
        self.image.clone()
    }

    pub fn release_device_resources(&self) {
        self.bitmap.borrow_mut().take();
    }

    pub fn draw_contained(&self, device: &GfxDevice, dest: &D2D_RECT_F) -> Result<()> {
        let mut cached = self.bitmap.borrow_mut();
        if cached.as_ref().is_none_or(|(id, _)| *id != device.id) {
            *cached = None;
            let mut pixels = self.image.pixels.clone();
            for pixel in pixels.chunks_exact_mut(4) {
                for channel in 0..3 {
                    pixel[channel] = (u16::from(pixel[channel]) * u16::from(pixel[3]) / 255) as u8;
                }
            }
            let bitmap = ImageRenderer::bitmap_from_bgra(
                &device.d2d_context,
                self.image.width,
                self.image.height,
                &pixels,
            )?;
            *cached = Some((device.id, bitmap));
        }
        let bitmap = &cached.as_ref().unwrap().1;
        let width = self.image.width as f32;
        let height = self.image.height as f32;
        let avail_w = (dest.right - dest.left).max(1.0);
        let avail_h = (dest.bottom - dest.top).max(1.0);
        let scale = (avail_w / width).min(avail_h / height);
        let draw_w = (width * scale).round().max(1.0);
        let draw_h = (height * scale).round().max(1.0);
        let left = dest.left + (avail_w - draw_w) / 2.0;
        let top = dest.top + (avail_h - draw_h) / 2.0;
        let rect = D2D_RECT_F {
            left,
            top,
            right: left + draw_w,
            bottom: top + draw_h,
        };
        unsafe {
            let target: &ID2D1RenderTarget = &device.d2d_context;
            target.DrawBitmap(
                bitmap,
                Some(&rect),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        Ok(())
    }
}

pub struct ImageRenderer {
    wic_factory: IWICImagingFactory,
}

impl ImageRenderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let wic_factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            Ok(Self { wic_factory })
        }
    }

    pub fn bitmap_from_hicon(
        &self,
        context: &ID2D1DeviceContext,
        hicon: HICON,
    ) -> Result<ID2D1Bitmap1> {
        unsafe {
            let wic_bitmap = self.wic_factory.CreateBitmapFromHICON(hicon)?;
            let converter = self.wic_factory.CreateFormatConverter()?;
            converter.Initialize(
                &wic_bitmap,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;

            let bitmap_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };

            let d2d_bitmap = context.CreateBitmapFromWicBitmap(&converter, Some(&bitmap_props))?;
            Ok(d2d_bitmap)
        }
    }

    pub fn bitmap_from_bgra(
        context: &ID2D1DeviceContext,
        width: u32,
        height: u32,
        bgra_data: &[u8],
    ) -> Result<ID2D1Bitmap1> {
        unsafe {
            let size = D2D_SIZE_U { width, height };
            let bitmap_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let pitch = width * 4;
            let bitmap =
                context.CreateBitmap(size, Some(bgra_data.as_ptr() as _), pitch, &bitmap_props)?;
            Ok(bitmap)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{buffer::Buffer, gfx::WindowRenderer};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn image_upload_is_premultiplied_reused_and_rebuilt_for_new_device() {
        unsafe {
            let hwnd = CreateWindowExW(
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
            );
            assert!(!hwnd.is_null());
            let source = Arc::new(Image {
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 128],
            });
            let image = BitmapImage::from_shared(source.clone()).unwrap();
            let mut renderer = WindowRenderer::new().unwrap();
            let mut output = Buffer::default();
            assert!(output.ensure(std::ptr::null_mut(), 8, 8));
            let rect = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: 8.0,
                bottom: 8.0,
            };
            renderer
                .render(hwnd, 96, Some(output.dc), |_, device| {
                    image.draw_contained(device, &rect)
                })
                .unwrap();
            assert_eq!(*output.bits.add(4 * 8 + 4), 0x80000080);
            assert_eq!(source.pixels, [255, 0, 0, 128]);
            let bitmap = image.bitmap.borrow().as_ref().unwrap().1.clone();
            renderer
                .render(hwnd, 96, Some(output.dc), |_, device| {
                    image.draw_contained(device, &rect)
                })
                .unwrap();
            use windows::core::Interface;
            assert_eq!(
                bitmap.as_raw(),
                image.bitmap.borrow().as_ref().unwrap().1.as_raw()
            );
            let old_id = renderer.device().id;
            renderer.device().invalidate();
            renderer
                .render(hwnd, 96, Some(output.dc), |_, device| {
                    image.draw_contained(device, &rect)
                })
                .unwrap();
            assert_ne!(old_id, image.bitmap.borrow().as_ref().unwrap().0);
            assert_eq!(*output.bits.add(4 * 8 + 4), 0x80000080);
            renderer
                .render(hwnd, 96, Some(output.dc), |rt, _| {
                    crate::windows::gfx::fill_rect(
                        rt,
                        &rect,
                        crate::windows::gfx::colorref_to_d2d(0x0000ff, 0.5),
                    )?;
                    Ok(())
                })
                .unwrap();
            assert_eq!(*output.bits.add(4 * 8 + 4), 0x80800000);
            drop((image, bitmap, renderer));
            DestroyWindow(hwnd);
        }
    }
}

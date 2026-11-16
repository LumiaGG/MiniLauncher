use std::mem::ManuallyDrop;
use windows::core::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use super::device::GfxDevice;

pub struct CompositionSurface {
    pub swap_chain: IDXGISwapChain1,
    pub target_bitmap: Option<ID2D1Bitmap1>,
    pub dcomp_target: IDCompositionTarget,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
}

impl CompositionSurface {
    pub fn new(hwnd: HWND, device: &GfxDevice, width: u32, height: u32, dpi: u32) -> Result<Self> {
        unsafe {
            let width = width.max(1);
            let height = height.max(1);

            let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                // Composition swap chains require FLIP_SEQUENTIAL. Keep this
                // independent of how often the desktop behind us is recomposed.
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };

            let swap_chain = device.dxgi_factory.CreateSwapChainForComposition(
                &device.dxgi_device,
                &swap_chain_desc,
                None,
            )?;

            // Setup DirectComposition Visual Tree
            let dcomp_target = device.dcomp_device.CreateTargetForHwnd(hwnd, true)?;
            let dcomp_visual = device.dcomp_device.CreateVisual()?;
            dcomp_visual.SetContent(&swap_chain)?;
            dcomp_target.SetRoot(&dcomp_visual)?;
            device.dcomp_device.Commit()?;

            // Create Direct2D target bitmap from SwapChain back buffer
            let target_bitmap = create_target_bitmap(&swap_chain, &device.d2d_context, dpi)?;
            device.d2d_context.SetTarget(&target_bitmap);
            device.d2d_context.SetDpi(dpi as f32, dpi as f32);

            Ok(Self {
                swap_chain,
                target_bitmap: Some(target_bitmap),
                dcomp_target,
                width,
                height,
                dpi,
            })
        }
    }

    pub fn resize(&mut self, device: &GfxDevice, width: u32, height: u32) -> Result<()> {
        let width = width.max(1);
        let height = height.max(1);

        if self.width == width && self.height == height && self.target_bitmap.is_some() {
            return Ok(());
        }

        unsafe {
            // Unbind target and release the old target bitmap before resizing swapchain buffers
            device.d2d_context.SetTarget(None);
            self.target_bitmap = None;

            self.swap_chain.ResizeBuffers(
                0, // Keep buffer count
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;

            let target = create_target_bitmap(&self.swap_chain, &device.d2d_context, self.dpi)?;
            device.d2d_context.SetTarget(&target);
            device.d2d_context.SetDpi(self.dpi as f32, self.dpi as f32);
            self.target_bitmap = Some(target);

            self.width = width;
            self.height = height;

            device.dcomp_device.Commit()?;
            Ok(())
        }
    }

    pub fn update_dpi(&mut self, device: &GfxDevice, dpi: u32) -> Result<()> {
        if self.dpi == dpi && self.target_bitmap.is_some() {
            return Ok(());
        }
        self.dpi = dpi;
        unsafe {
            device.d2d_context.SetTarget(None);
            self.target_bitmap = None;
            let target = create_target_bitmap(&self.swap_chain, &device.d2d_context, dpi)?;
            device.d2d_context.SetTarget(&target);
            device.d2d_context.SetDpi(dpi as f32, dpi as f32);
            self.target_bitmap = Some(target);
            Ok(())
        }
    }

    pub fn begin_draw(&self, context: &ID2D1DeviceContext, clear_color: Option<D2D1_COLOR_F>) {
        unsafe {
            if let Some(bitmap) = self.target_bitmap.as_ref() {
                context.SetTarget(bitmap);
            }
            context.SetDpi(self.dpi as f32, self.dpi as f32);
            context.BeginDraw();
            // Grayscale text antialiasing produces crisp premultiplied alpha on translucent backdrops
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let clear = clear_color.unwrap_or(D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            });
            context.Clear(Some(&clear));
        }
    }

    pub fn end_draw(&self, context: &ID2D1DeviceContext) -> Result<()> {
        unsafe { context.EndDraw(None, None) }
    }

    pub fn present(&self, sync_interval: u32) -> Result<()> {
        unsafe {
            self.swap_chain
                .Present(sync_interval, DXGI_PRESENT(0))
                .ok()?;
            Ok(())
        }
    }

    /// Read back the rendered scene only for explicit client-print requests.
    pub fn copy_to_dc(
        &self,
        context: &ID2D1DeviceContext,
        dc: windows_sys::Win32::Graphics::Gdi::HDC,
    ) -> Result<()> {
        unsafe {
            let properties = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi as f32,
                dpiY: self.dpi as f32,
                bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let staging = context.CreateBitmap(
                D2D_SIZE_U {
                    width: self.width,
                    height: self.height,
                },
                None,
                0,
                &properties,
            )?;
            context.SetTarget(None);
            let target = self
                .target_bitmap
                .as_ref()
                .ok_or_else(windows::core::Error::from_thread)?;
            staging.CopyFromBitmap(None, target, None)?;
            let mut buffer = super::super::buffer::Buffer::default();
            if !buffer.ensure(dc, self.width as i32, self.height as i32) {
                return Err(windows::core::Error::from_thread());
            }
            let mapped = staging.Map(D2D1_MAP_OPTIONS_READ)?;
            for row in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(
                    mapped.bits.add(row * mapped.pitch as usize),
                    buffer.bits.cast::<u8>().add(row * self.width as usize * 4),
                    self.width as usize * 4,
                );
            }
            staging.Unmap()?;
            use windows_sys::Win32::Graphics::Gdi::{BitBlt, SRCCOPY};
            if BitBlt(
                dc,
                0,
                0,
                self.width as i32,
                self.height as i32,
                buffer.dc,
                0,
                0,
                SRCCOPY,
            ) == 0
            {
                return Err(windows::core::Error::from_thread());
            }
            Ok(())
        }
    }
}

unsafe fn create_target_bitmap(
    swap_chain: &IDXGISwapChain1,
    context: &ID2D1DeviceContext,
    dpi: u32,
) -> Result<ID2D1Bitmap1> {
    let back_buffer: IDXGISurface = swap_chain.GetBuffer(0)?;
    let dpi_f = dpi as f32;
    let bitmap_props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi_f,
        dpiY: dpi_f,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: ManuallyDrop::new(None),
    };
    context.CreateBitmapFromDxgiSurface(&back_buffer, Some(&bitmap_props))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_surface_readback_and_target_recreation() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                windows::core::w!("STATIC").as_ptr(),
                std::ptr::null(),
                0,
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
            let device = GfxDevice::shared().unwrap();
            let mut surface = CompositionSurface::new(HWND(hwnd as _), &device, 8, 8, 144).unwrap();
            device.d2d_context.SetTarget(None);
            surface.target_bitmap = None;
            surface.resize(&device, 8, 8).unwrap();
            surface.begin_draw(
                &device.d2d_context,
                Some(D2D1_COLOR_F {
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }),
            );
            surface.end_draw(&device.d2d_context).unwrap();
            let mut output = super::super::super::buffer::Buffer::default();
            assert!(output.ensure(std::ptr::null_mut(), 8, 8));
            surface.copy_to_dc(&device.d2d_context, output.dc).unwrap();
            assert_eq!(*output.bits, 0xffff0000);
            surface.present(1).unwrap();
            device.dcomp_device.Commit().unwrap();
            // Readback unbinds the target; the next frame must bind it again.
            surface.begin_draw(
                &device.d2d_context,
                Some(D2D1_COLOR_F {
                    r: 0.0,
                    g: 1.0,
                    b: 0.0,
                    a: 1.0,
                }),
            );
            surface.end_draw(&device.d2d_context).unwrap();
            surface.copy_to_dc(&device.d2d_context, output.dc).unwrap();
            assert_eq!(*output.bits, 0xff00ff00);
            surface.present(1).unwrap();
            // Exercise repeated translucent frames, buffer rotation and resizing
            // while the composition tree is committed between presentations.
            for size in [8, 12, 8] {
                surface.resize(&device, size, size).unwrap();
                assert!(output.ensure(std::ptr::null_mut(), size as i32, size as i32));
                for _ in 0..3 {
                    surface.begin_draw(
                        &device.d2d_context,
                        Some(D2D1_COLOR_F {
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.5,
                        }),
                    );
                    surface.end_draw(&device.d2d_context).unwrap();
                    surface.copy_to_dc(&device.d2d_context, output.dc).unwrap();
                    let pixel = *output.bits;
                    assert_eq!(pixel & 0xffff, 0);
                    assert!((127..=128).contains(&(pixel >> 24)));
                    assert!((127..=128).contains(&((pixel >> 16) & 255)));
                    surface.present(1).unwrap();
                    device.dcomp_device.Commit().unwrap();
                }
            }
            drop(surface);
            drop(device);
            DestroyWindow(hwnd);
        }
    }
}

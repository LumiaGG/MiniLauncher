use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};
use windows::core::{Interface, Result, HRESULT};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::{Common::D2D1_COLOR_F, *};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::*;

thread_local! {
    static SHARED: RefCell<Shared> = RefCell::new(Shared::default());
}

#[derive(Default)]
struct Shared {
    device: Weak<GfxDevice>,
    windows: HashSet<usize>,
}

const MAX_SOLID_BRUSHES: usize = 64;

pub struct GfxDevice {
    pub id: u64,
    pub dxgi_device: IDXGIDevice,
    pub dxgi_factory: IDXGIFactory2,
    pub d2d_device: ID2D1Device,
    pub d2d_context: ID2D1DeviceContext,
    pub dwrite_factory: IDWriteFactory,
    pub dcomp_device: IDCompositionDevice,
    solid_brushes: RefCell<HashMap<[u32; 4], ID2D1SolidColorBrush>>,
}

impl GfxDevice {
    /// Brushes stay immutable and belong to this thread's shared drawing context.
    /// Other render targets get independent brushes rather than crossing resource domains.
    pub unsafe fn solid_brush(
        target: &ID2D1RenderTarget,
        color: D2D1_COLOR_F,
    ) -> Result<ID2D1SolidColorBrush> {
        let device = SHARED.with(|slot| slot.borrow().device.upgrade());
        if let Some(device) = device.filter(|device| target.as_raw() == device.d2d_context.as_raw())
        {
            let key = [
                color.r.to_bits(),
                color.g.to_bits(),
                color.b.to_bits(),
                color.a.to_bits(),
            ];
            let mut brushes = device.solid_brushes.borrow_mut();
            if let Some(brush) = brushes.get(&key) {
                return Ok(brush.clone());
            }
            let brush = target.CreateSolidColorBrush(&color, None)?;
            if brushes.len() >= MAX_SOLID_BRUSHES {
                brushes.clear();
            }
            brushes.insert(key, brush.clone());
            Ok(brush)
        } else {
            target.CreateSolidColorBrush(&color, None)
        }
    }

    pub fn release_idle_resources(&self) {
        self.solid_brushes.borrow_mut().clear();
        unsafe {
            self.d2d_context.SetTarget(None);
            self.d2d_device.ClearResources(0);
            if let Ok(device) = self.dxgi_device.cast::<IDXGIDevice3>() {
                device.Trim();
            }
        }
    }

    pub fn shared() -> Result<Rc<Self>> {
        SHARED.with(|slot| {
            let mut shared = slot.borrow_mut();
            if let Some(device) = shared.device.upgrade() {
                return Ok(device);
            }
            let device = Rc::new(Self::new()?);
            shared.device = Rc::downgrade(&device);
            Ok(device)
        })
    }

    pub fn invalidate(&self) {
        self.solid_brushes.borrow_mut().clear();
        SHARED.with(|slot| {
            let mut shared = slot.borrow_mut();
            if shared
                .device
                .upgrade()
                .is_some_and(|device| device.id == self.id)
            {
                shared.device = Weak::new();
            }
            for hwnd in &shared.windows {
                unsafe {
                    windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                        *hwnd as _,
                        std::ptr::null(),
                        0,
                    );
                }
            }
        });
    }

    pub fn register_window(hwnd: usize) {
        SHARED.with(|slot| {
            slot.borrow_mut().windows.insert(hwnd);
        });
    }

    pub fn unregister_window(hwnd: usize) {
        SHARED.with(|slot| {
            slot.borrow_mut().windows.remove(&hwnd);
        });
    }

    fn new() -> Result<Self> {
        unsafe {
            // 1. Try Hardware D3D11 device first, fallback to WARP if hardware unavailable
            let d3d_device = match create_d3d_device(D3D_DRIVER_TYPE_HARDWARE) {
                Ok(device) => device,
                Err(_) => create_d3d_device(D3D_DRIVER_TYPE_WARP)?,
            };

            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let dxgi_adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = dxgi_adapter.GetParent()?;

            // 2. Direct2D Factory, Device, DeviceContext
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
            let d2d_context = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;

            // 3. DirectWrite Factory
            let dwrite_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            // 4. DirectComposition Device
            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;

            Ok(Self {
                id: {
                    static NEXT: AtomicU64 = AtomicU64::new(1);
                    NEXT.fetch_add(1, Ordering::Relaxed)
                },
                dxgi_device,
                dxgi_factory,
                d2d_device,
                d2d_context,
                dwrite_factory,
                dcomp_device,
                solid_brushes: RefCell::new(HashMap::new()),
            })
        }
    }

    pub fn is_device_lost(hr: HRESULT) -> bool {
        // DXGI_ERROR_DEVICE_REMOVED (0x887A0005) or DXGI_ERROR_DEVICE_RESET (0x887A0007)
        let code = hr.0 as u32;
        code == 0x887A0005 || code == 0x887A0007 || code == 0x8899000C // D2DERR_RECREATE_TARGET
    }
}

unsafe fn create_d3d_device(driver_type: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let mut device: Option<ID3D11Device> = None;
    let mut feature_level = D3D_FEATURE_LEVEL_11_0;

    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;

    D3D11CreateDevice(
        None,
        driver_type,
        HMODULE::default(),
        flags,
        Some(&[
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
        ]),
        D3D11_SDK_VERSION,
        Some(&mut device),
        Some(&mut feature_level),
        None,
    )?;

    Ok(device.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_brushes_reuse_colors_without_crossing_contexts_or_device_lifetimes() {
        unsafe {
            let device = GfxDevice::shared().unwrap();
            let color = D2D1_COLOR_F {
                r: 0.2,
                g: 0.3,
                b: 0.4,
                a: 0.6,
            };
            let first = GfxDevice::solid_brush(&device.d2d_context, color).unwrap();
            let reused = GfxDevice::solid_brush(&device.d2d_context, color).unwrap();
            assert_eq!(first.as_raw(), reused.as_raw());
            let other_color = D2D1_COLOR_F { a: 1.0, ..color };
            let opaque = GfxDevice::solid_brush(&device.d2d_context, other_color).unwrap();
            assert_ne!(first.as_raw(), opaque.as_raw());
            assert_eq!(
                first.GetColor().a,
                0.6,
                "later colors must not mutate retained brushes"
            );
            let other_context = device
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .unwrap();
            let independent = GfxDevice::solid_brush(&other_context, color).unwrap();
            assert_ne!(first.as_raw(), independent.as_raw());
            device.release_idle_resources();
            assert!(device.solid_brushes.borrow().is_empty());
            let refreshed = GfxDevice::solid_brush(&device.d2d_context, color).unwrap();
            assert_ne!(first.as_raw(), refreshed.as_raw());
            for i in 0..=MAX_SOLID_BRUSHES {
                GfxDevice::solid_brush(
                    &device.d2d_context,
                    D2D1_COLOR_F {
                        r: i as f32 / MAX_SOLID_BRUSHES as f32,
                        ..color
                    },
                )
                .unwrap();
                assert!(device.solid_brushes.borrow().len() <= MAX_SOLID_BRUSHES);
            }
            device.invalidate();
            assert!(device.solid_brushes.borrow().is_empty());
            let recovered = GfxDevice::shared().unwrap();
            assert_ne!(device.id, recovered.id);
            let recovered_brush = GfxDevice::solid_brush(&recovered.d2d_context, color).unwrap();
            assert_ne!(first.as_raw(), recovered_brush.as_raw());
        }
    }
}

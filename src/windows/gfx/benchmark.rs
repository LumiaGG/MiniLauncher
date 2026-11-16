//! Opt-in, isolated renderer measurements. Every HWND remains hidden.
use super::*;
use serde_json::{json, Value};
use std::{mem::size_of, ptr::null_mut, sync::Arc};
use windows::{
    core::Interface,
    Win32::Graphics::{Direct2D::Common::D2D_RECT_F, DirectWrite::*, Dxgi::*},
};
use windows_sys::Win32::{
    System::{ProcessStatus::*, Threading::GetCurrentProcess},
    UI::WindowsAndMessaging::*,
};

fn sample(stage: &str, adapter: Option<&IDXGIAdapter3>) -> Value {
    // Let submission/resource retirement reach the driver without forcing a working-set trim.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut counters = PROCESS_MEMORY_COUNTERS_EX2::default();
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    unsafe {
        assert_ne!(
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),
                counters.cb
            ),
            0
        );
    }
    let gpu = |segment| {
        adapter.and_then(|adapter| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe {
                adapter.QueryVideoMemoryInfo(0, segment, &mut info).ok()?;
            }
            Some(info.CurrentUsage)
        })
    };
    json!({"stage": stage, "private_bytes": counters.PrivateUsage,
        "working_set_bytes": counters.WorkingSetSize,
        "private_working_set_bytes": counters.PrivateWorkingSetSize,
        "gpu_local_bytes": gpu(DXGI_MEMORY_SEGMENT_GROUP_LOCAL),
        "gpu_nonlocal_bytes": gpu(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)})
}

struct HiddenWindow(windows_sys::Win32::Foundation::HWND);
impl HiddenWindow {
    fn new(width: i32, height: i32) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                windows::core::w!("STATIC").as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                width,
                height,
                null_mut(),
                null_mut(),
                null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        Self(hwnd)
    }
}
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

#[test]
#[ignore = "isolated memory measurement; run scripts/measure-render-memory.ps1"]
fn hidden_render_memory() {
    let mut samples = vec![sample("before_device", None)];
    let device = GfxDevice::shared().unwrap();
    let adapter = unsafe {
        device
            .dxgi_device
            .GetAdapter()
            .unwrap()
            .cast::<IDXGIAdapter3>()
            .ok()
    };
    samples.push(sample("device_created", adapter.as_ref()));
    let text = TextEngine::new(&device.dwrite_factory, true).unwrap();
    samples.push(sample("text_formats_created", adapter.as_ref()));
    let main = HiddenWindow::new(1284, 870);
    let mut main_renderer = WindowRenderer::new().unwrap();
    let draw_text = |rt: &windows::Win32::Graphics::Direct2D::ID2D1DeviceContext, _: &GfxDevice| unsafe {
        text.draw_text_role(
            rt,
            "搜索内容 MiniLauncher 中文测试",
            FontRole::Body,
            &D2D_RECT_F {
                left: 20.0,
                top: 20.0,
                right: 500.0,
                bottom: 60.0,
            },
            colorref_to_d2d(0xffffff, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        text.draw_text_role(
            rt,
            "\u{E721}",
            FontRole::Symbol,
            &D2D_RECT_F {
                left: 20.0,
                top: 60.0,
                right: 50.0,
                bottom: 90.0,
            },
            colorref_to_d2d(0xffffff, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )
    };
    main_renderer.render(main.0, 144, None, draw_text).unwrap();
    samples.push(sample("main_text_frame", adapter.as_ref()));
    let mut secondary = Vec::new();
    for (width, height) in [(432, 234), (384, 450), (1800, 1200), (408, 108)] {
        let window = HiddenWindow::new(width, height);
        let mut renderer = WindowRenderer::new().unwrap();
        assert!(std::rc::Rc::ptr_eq(&device, &renderer.device()));
        renderer.render(window.0, 144, None, draw_text).unwrap();
        secondary.push((renderer, window));
    }
    samples.push(sample("all_five_surfaces", adapter.as_ref()));
    let image = BitmapImage::from_shared(Arc::new(mini_launcher::clipboard_image::Image {
        width: 640,
        height: 400,
        pixels: vec![255; 640 * 400 * 4],
    }))
    .unwrap();
    let rect = D2D_RECT_F {
        left: 0.0,
        top: 0.0,
        right: 640.0,
        bottom: 400.0,
    };
    main_renderer
        .render(main.0, 144, None, |_, device| {
            image.draw_contained(device, &rect)
        })
        .unwrap();
    samples.push(sample("image_uploaded", adapter.as_ref()));
    drop(image);
    for (renderer, _) in &mut secondary {
        renderer.suspend();
    }
    main_renderer.suspend();
    samples.push(sample("all_surfaces_suspended", adapter.as_ref()));
    drop((secondary, main_renderer, main, text, device));
    samples.push(sample("renderer_resources_dropped", adapter.as_ref()));
    let report = json!({"kind": "isolated_hidden_renderer", "debug_build": cfg!(debug_assertions),
        "notes": "No launcher providers, plugins, result caches or visible-window interaction. GPU and process bytes are separate metrics and must not be added.",
        "samples": samples});
    let path = std::env::var_os("MINILAUNCHER_RENDER_MEMORY_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts/render-memory.json")
        });
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report);
}

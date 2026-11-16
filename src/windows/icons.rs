use super::{size_of, wide, zeroed};
use windows_sys::Win32::UI::{Controls::*, Shell::*, WindowsAndMessaging::*};

const SOURCE_SIZE: i32 = 128;

thread_local! {
    static FACTORY: std::cell::RefCell<Option<windows::Win32::Graphics::Imaging::IWICImagingFactory>> =
        const { std::cell::RefCell::new(None) };
}

pub unsafe fn clear_thread_cache() {
    FACTORY.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Resample once on the icon worker, then draw at 1:1 physical pixels.
pub unsafe fn load_scaled(path: &str, size: i32) -> HICON {
    let source = load(path);
    if source.is_null() {
        return source;
    }
    let scaled = resample(source, size).unwrap_or(std::ptr::null_mut());
    if scaled.is_null() {
        source
    } else {
        DestroyIcon(source);
        scaled
    }
}

unsafe fn resample(source: HICON, size: i32) -> windows::core::Result<HICON> {
    use windows::Win32::{
        Graphics::Imaging::*,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::WindowsAndMessaging::HICON as Icon,
    };
    use windows_sys::Win32::Graphics::Gdi::*;
    let factory: IWICImagingFactory =
        FACTORY.with(|slot| -> windows::core::Result<IWICImagingFactory> {
            if let Some(factory) = slot.borrow().as_ref() {
                return Ok(factory.clone());
            }
            let created: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            *slot.borrow_mut() = Some(created.clone());
            Ok(created)
        })?;
    let bitmap = factory.CreateBitmapFromHICON(Icon(source))?;
    // Filter premultiplied pixels so transparent edges do not gain dark halos.
    let converter = factory.CreateFormatConverter()?;
    converter.Initialize(
        &bitmap,
        &GUID_WICPixelFormat32bppPBGRA,
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeCustom,
    )?;
    let scaler = factory.CreateBitmapScaler()?;
    scaler.Initialize(
        &converter,
        size as u32,
        size as u32,
        WICBitmapInterpolationModeFant,
    )?;
    let mut buffer = super::buffer::Buffer::default();
    if !buffer.ensure(std::ptr::null_mut(), size, size) {
        return Ok(std::ptr::null_mut());
    }
    let pixels =
        std::slice::from_raw_parts_mut(buffer.bits.cast::<u8>(), (size * size * 4) as usize);
    scaler.CopyPixels(std::ptr::null(), (size * 4) as u32, pixels)?;
    let mask_bits = vec![0u8; (((size + 15) / 16) * 2 * size) as usize];
    let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
    if mask.is_null() {
        return Ok(std::ptr::null_mut());
    }
    let mut info: ICONINFO = zeroed();
    info.fIcon = 1;
    info.hbmColor = GetCurrentObject(buffer.dc, OBJ_BITMAP as u32);
    info.hbmMask = mask;
    let icon = CreateIconIndirect(&info);
    DeleteObject(mask);
    Ok(icon)
}

/// Returns an owned icon. Explicit ICO files bypass Shell file associations.
pub unsafe fn load(path: &str) -> HICON {
    if std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ico"))
    {
        // Preserve the source resolution for monitors above 100% DPI.
        return LoadImageW(
            std::ptr::null_mut(),
            wide(path).as_ptr(),
            IMAGE_ICON,
            SOURCE_SIZE,
            SOURCE_SIZE,
            LR_LOADFROMFILE,
        ) as HICON;
    }
    let path = wide(path);
    // Resolve the actual icon resource, including a shortcut's custom icon.
    // The normal system image list only supplies a screen-sized raster, which
    // loses detail when the launcher is shown on a higher-DPI monitor.
    if let Some(icon) = extract(&path) {
        return icon;
    }
    // Keep compatibility with handlers that cannot extract at the requested size.
    let mut info: SHFILEINFOW = zeroed();
    let image_list = SHGetFileInfoW(
        path.as_ptr(),
        0,
        &mut info,
        size_of::<SHFILEINFOW>() as u32,
        SHGFI_SYSICONINDEX | SHGFI_LARGEICON,
    ) as HIMAGELIST;
    // Copy the base image only, preserving shortcut-specific icons without arrows.
    if image_list == 0 {
        std::ptr::null_mut()
    } else {
        ImageList_GetIcon(image_list, info.iIcon, ILD_NORMAL)
    }
}

unsafe fn extract(path: &[u16]) -> Option<HICON> {
    use windows::{
        core::PCWSTR,
        Win32::UI::Shell::{BHID_SFUIObject, IExtractIconW, IShellItem},
    };
    let item: IShellItem =
        windows::Win32::UI::Shell::SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
    let extractor: IExtractIconW = item.BindToHandler(None, &BHID_SFUIObject).ok()?;
    let mut location = [0u16; 32768];
    let mut index = 0;
    let mut flags = 0;
    extractor
        .GetIconLocation(GIL_FORSHELL, &mut location, &mut index, &mut flags)
        .ok()?;
    let mut icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
    let result = extractor.Extract(
        PCWSTR(location.as_ptr()),
        index as u32,
        Some(&mut icon),
        None,
        SOURCE_SIZE as u32,
    );
    if result.is_ok() && !icon.0.is_null() {
        return Some(icon.0);
    }
    if !icon.0.is_null() {
        DestroyIcon(icon.0);
    }
    if flags & GIL_NOTFILENAME != 0 {
        return None;
    }
    let mut icon = std::ptr::null_mut();
    let result = SHDefExtractIconW(
        location.as_ptr(),
        index,
        0,
        &mut icon,
        std::ptr::null_mut(),
        SOURCE_SIZE as u32,
    );
    if result >= 0 && !icon.is_null() {
        Some(icon)
    } else {
        if !icon.is_null() {
            DestroyIcon(icon);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        path::Path,
    };
    use windows_sys::Win32::Graphics::Gdi::*;

    #[test]
    fn shell_application_icon_preserves_high_resolution() {
        let explorer = Path::new(&std::env::var_os("WINDIR").unwrap()).join("explorer.exe");
        unsafe {
            let initialized = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32,
            );
            assert!(initialized >= 0);
            let icon = load(explorer.to_str().unwrap());
            assert!(!icon.is_null());
            let mut info: ICONINFO = zeroed();
            assert_ne!(GetIconInfo(icon, &mut info), 0);
            let mut bitmap: BITMAP = zeroed();
            let read = GetObjectW(
                info.hbmColor,
                size_of::<BITMAP>() as i32,
                (&mut bitmap as *mut BITMAP).cast(),
            );
            DeleteObject(info.hbmColor);
            DeleteObject(info.hbmMask);
            DestroyIcon(icon);
            assert_ne!(read, 0);
            assert_eq!(
                (bitmap.bmWidth, bitmap.bmHeight),
                (SOURCE_SIZE, SOURCE_SIZE)
            );

            // A shortcut must retain its custom icon without an arrow overlay.
            use windows::{
                core::{Interface, PCWSTR},
                Win32::{
                    System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER},
                    UI::Shell::{IShellLinkW, ShellLink},
                },
            };
            let shortcut = std::env::temp_dir().join(format!(
                "mini-launcher-icon-{}-{}.lnk",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/test.ico");
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(PCWSTR(wide(explorer.to_str().unwrap()).as_ptr()))
                .unwrap();
            link.SetIconLocation(PCWSTR(wide(source.to_str().unwrap()).as_ptr()), 0)
                .unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Save(PCWSTR(wide(shortcut.to_str().unwrap()).as_ptr()), true)
                .unwrap();
            let actual = fingerprint(&shortcut);
            std::fs::remove_file(&shortcut).unwrap();
            let expected = fingerprint(&source);
            for size in [32, 40, 48, 64, 96] {
                let scaled = load_scaled(source.to_str().unwrap(), size);
                assert!(!scaled.is_null());
                let mut info: ICONINFO = zeroed();
                assert_ne!(GetIconInfo(scaled, &mut info), 0);
                let mut bitmap: BITMAP = zeroed();
                assert_ne!(
                    GetObjectW(
                        info.hbmColor,
                        size_of::<BITMAP>() as i32,
                        (&mut bitmap as *mut BITMAP).cast()
                    ),
                    0
                );
                assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (size, size));
                let mut header: BITMAPINFO = zeroed();
                header.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
                header.bmiHeader.biWidth = size;
                header.bmiHeader.biHeight = -size;
                header.bmiHeader.biPlanes = 1;
                header.bmiHeader.biBitCount = 32;
                let dc = CreateCompatibleDC(std::ptr::null_mut());
                let mut pixels = vec![0u8; (size * size * 4) as usize];
                assert_eq!(
                    GetDIBits(
                        dc,
                        info.hbmColor,
                        0,
                        size as u32,
                        pixels.as_mut_ptr().cast(),
                        &mut header,
                        DIB_RGB_COLORS
                    ),
                    size
                );
                DeleteDC(dc);
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
                DestroyIcon(scaled);
                assert!(pixels.chunks_exact(4).any(|p| p[3] == 0), "保留透明背景");
                assert!(
                    pixels.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255),
                    "保留平滑边缘"
                );
            }
            drop(file);
            drop(link);
            clear_thread_cache();
            windows_sys::Win32::System::Com::CoUninitialize();
            assert_eq!(actual, expected, "快捷方式应保留自定义图标且不叠加角标");
        }
    }

    unsafe fn fingerprint(path: &Path) -> u64 {
        let icon = load(path.to_str().unwrap());
        assert!(!icon.is_null(), "无法加载 {}", path.display());
        let mut info: ICONINFO = zeroed();
        assert_ne!(GetIconInfo(icon, &mut info), 0);
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut header: BITMAPINFO = zeroed();
        header.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        header.bmiHeader.biWidth = 128;
        header.bmiHeader.biHeight = -128;
        header.bmiHeader.biPlanes = 1;
        header.bmiHeader.biBitCount = 32;
        header.bmiHeader.biCompression = BI_RGB;
        let mut pixels = vec![0u8; 128 * 128 * 4];
        let copied = GetDIBits(
            dc,
            info.hbmColor,
            0,
            128,
            pixels.as_mut_ptr().cast(),
            &mut header,
            DIB_RGB_COLORS,
        );
        DeleteDC(dc);
        DeleteObject(info.hbmColor);
        DeleteObject(info.hbmMask);
        DestroyIcon(icon);
        assert_eq!(copied, 128);
        let mut hash = DefaultHasher::new();
        pixels.hash(&mut hash);
        hash.finish()
    }
}

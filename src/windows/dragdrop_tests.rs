use super::*;
use mini_launcher::clipboard_image::Image;
use windows::Win32::System::Ole::ReleaseStgMedium;

unsafe fn bytes(data: &IDataObject, id: u16) -> Vec<u8> {
    let mut medium = data.GetData(&format(id)).unwrap();
    let handle = medium.u.hGlobal.0;
    let pointer = GlobalLock(handle);
    assert!(!pointer.is_null());
    let result = std::slice::from_raw_parts(pointer.cast::<u8>(), GlobalSize(handle)).to_vec();
    GlobalUnlock(handle);
    ReleaseStgMedium(&mut medium);
    result
}

#[test]
fn native_drag_formats_preserve_unicode_images_and_file_drop_paths() {
    let _ole = Ole::initialize().unwrap();
    let text = "完整文本\r\n中文 🙂";
    let mut prepared = prepare(DragData::Text { text: text.into() }).unwrap();
    let data = unsafe { data_object(&mut prepared).unwrap() };
    assert!(unsafe { data.QueryGetData(&format(13)) }.is_ok());
    let text_bytes = unsafe { bytes(&data, 13) };
    let units: Vec<u16> = text_bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|u| *u != 0)
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), text);
    drop(data);

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("拖放 验证-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let image = Image {
        width: 2,
        height: 1,
        pixels: vec![0, 0, 255, 255, 255, 0, 0, 128],
    };
    let source = root.join("source.png");
    image.write_png(&source).unwrap();
    let mut prepared = prepare_at(
        &mut DragData::Image {
            path: source.clone(),
        },
        &root,
    )
    .unwrap();
    let export = prepared.export.as_ref().unwrap().path.clone();
    let data = unsafe { data_object(&mut prepared).unwrap() };
    for id in [15, 17, registered("PNG").unwrap()] {
        assert!(unsafe { data.QueryGetData(&format(id)) }.is_ok());
    }
    let dib = unsafe { bytes(&data, 17) };
    assert_eq!(
        clipboard_image::decode(&dib, true).unwrap().pixels,
        image.pixels
    );
    let png = unsafe { bytes(&data, registered("PNG").unwrap()) };
    assert_eq!(
        clipboard_image::decode(&png, false).unwrap().pixels,
        image.pixels
    );
    let paths = unsafe { bytes(&data, 15) };
    assert_eq!(u32::from_le_bytes(paths[0..4].try_into().unwrap()), 20);
    assert_eq!(u32::from_le_bytes(paths[16..20].try_into().unwrap()), 1);
    let units: Vec<u16> = paths[20..]
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|u| *u != 0)
        .collect();
    assert_eq!(PathBuf::from(String::from_utf16(&units).unwrap()), export);
    assert!(export.is_file());
    drop(data);
    drop(prepared);
    assert!(!export.exists(), "取消拖动后应清理 PNG 导出");
    fs::remove_file(source).unwrap();
    fs::remove_dir(root).unwrap();

    let cancel = Arc::new(AtomicBool::new(false));
    let source: IDropSource = Source {
        cancelled: cancel.clone(),
        visual_window: 0,
        window: unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() } as usize,
    }
    .into();
    assert_eq!(
        unsafe { source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(1)) },
        S_OK
    );
    assert_eq!(
        unsafe { source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(0)) },
        DRAGDROP_S_DROP
    );
    assert_eq!(
        unsafe { source.QueryContinueDrag(true, MODIFIERKEYS_FLAGS(1)) },
        DRAGDROP_S_CANCEL
    );
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        unsafe { source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(1)) },
        DRAGDROP_S_CANCEL
    );
}

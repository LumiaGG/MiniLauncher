//! Native OLE drag source. Data preparation does not touch the system clipboard.
#[cfg(test)]
#[path = "dragdrop_tests.rs"]
mod tests;
#[cfg(test)]
use mini_launcher::clipboard_image;
use mini_launcher::{
    drag_data::DragData,
    image_cache::{self, Asset},
};
use std::{
    fs,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{implement, BOOL, HRESULT},
    Win32::{
        Foundation::{
            DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
            DV_E_DVASPECT, DV_E_FORMATETC, DV_E_TYMED, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER,
            HGLOBAL, OLE_E_ADVISENOTSUPPORTED, S_OK,
        },
        System::{
            Com::{
                IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
                DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
            },
            Ole::*,
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::Shell::SHCreateStdEnumFmtEtc,
    },
};
use windows_sys::Win32::{
    Foundation::GlobalFree,
    System::{DataExchange::RegisterClipboardFormatW, Memory::*},
    UI::WindowsAndMessaging::IsWindow,
};

pub(super) struct Ole;
impl Ole {
    pub(super) fn initialize() -> Result<Self, String> {
        unsafe {
            OleInitialize(None).map_err(|e| e.to_string())?;
        }
        Ok(Self)
    }
}
impl Drop for Ole {
    fn drop(&mut self) {
        unsafe {
            OleUninitialize();
        }
    }
}

struct Export {
    path: PathBuf,
    keep: bool,
}
impl Drop for Export {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) struct Prepared {
    formats: Vec<(u16, Data)>,
    export: Option<Export>,
    visual: Option<super::drag_image::Scene>,
}

enum Data {
    Bytes(Arc<Vec<u8>>),
    // File targets never need a decoded bitmap. Render it only if requested.
    Dib {
        asset: Arc<Asset>,
        bytes: OnceLock<Result<Arc<Vec<u8>>, String>>,
    },
}
impl From<Vec<u8>> for Data {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(Arc::new(value))
    }
}
impl Data {
    fn bytes(&self) -> Result<Arc<Vec<u8>>, String> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.clone()),
            Self::Dib { asset, bytes } => bytes
                .get_or_init(|| asset.image()?.dib().map(Arc::new))
                .clone(),
        }
    }
}

fn registered(name: &str) -> Result<u16, String> {
    let format = unsafe { RegisterClipboardFormatW(super::wide(name).as_ptr()) };
    if format == 0 {
        Err("注册拖动格式失败".into())
    } else {
        Ok(format as u16)
    }
}

fn file_drop(paths: &[PathBuf]) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut bytes = Vec::new();
    for number in [20u32, 0, 0, 0, 1] {
        bytes.extend(number.to_le_bytes());
    } // DROPFILES, Unicode
    for path in paths {
        for unit in path.as_os_str().encode_wide().chain(Some(0)) {
            bytes.extend(unit.to_le_bytes());
        }
    }
    bytes.extend([0, 0]);
    bytes
}

fn export_root() -> PathBuf {
    std::env::temp_dir()
        .join("MiniLauncher")
        .join("drag-export")
}

fn clean_exports(root: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(root)
        .map_err(|e| e.to_string())?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err("图片拖动临时目录不能是目录联接或符号链接".into());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(parts) = name
            .strip_prefix("drag-")
            .and_then(|s| s.strip_suffix(".png"))
        else {
            continue;
        };
        if parts.is_empty() || !parts.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::now());
        files.push((modified, metadata.len(), entry.path()));
    }
    files.sort_by_key(|f| f.0);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    // Leave room for this drag and for targets that read the PNG after Drop returns.
    for (modified, size, path) in files {
        if total <= 210 * 1024 * 1024
            && modified.elapsed().unwrap_or_default() < Duration::from_secs(86400)
        {
            continue;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    if total > 210 * 1024 * 1024 {
        return Err("无法清理过期的图片拖动临时文件".into());
    }
    Ok(())
}

pub(super) fn prepare(mut data: DragData) -> Result<Prepared, String> {
    prepare_at(&mut data, &export_root())
}

fn prepare_at(data: &mut DragData, root: &Path) -> Result<Prepared, String> {
    data.resolve(&std::env::current_dir().map_err(|e| e.to_string())?)?;
    let mut prepared = Prepared {
        formats: Vec::new(),
        export: None,
        visual: None,
    };
    match data {
        DragData::Text { text } => {
            let bytes: Vec<u8> = text
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            prepared.formats.push((13, Data::from(bytes)));
        }
        DragData::Files { paths } => prepared.formats.push((15, file_drop(paths).into())),
        DragData::Image { path } => {
            let asset = image_cache::asset(path)?;
            let png = asset.png()?;
            clean_exports(root)?;
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let path = root.join(format!(
                "drag-{stamp}-{}-{}.png",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            prepared.export = Some(Export {
                path: path.clone(),
                keep: false,
            });
            fs::write(&path, png.as_slice()).map_err(|e| e.to_string())?;
            prepared
                .formats
                .push((15, file_drop(std::slice::from_ref(&path)).into()));
            prepared
                .formats
                .push((registered("PNG")?, Data::Bytes(png)));
            prepared.formats.push((
                17,
                Data::Dib {
                    asset,
                    bytes: OnceLock::new(),
                },
            ));
        }
    }
    prepared.formats.push((
        registered("Preferred DropEffect")?,
        1u32.to_le_bytes().to_vec().into(),
    ));
    Ok(prepared)
}

fn format(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

#[implement(IDataObject)]
struct DataObject {
    formats: Vec<(u16, Data)>,
}

impl DataObject {
    unsafe fn format_id(request: *const FORMATETC) -> windows::core::Result<u16> {
        if request.is_null() {
            return Err(E_POINTER.into());
        }
        let request = &*request;
        if request.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
            return Err(DV_E_TYMED.into());
        }
        if request.dwAspect != DVASPECT_CONTENT.0 {
            return Err(DV_E_DVASPECT.into());
        }
        Ok(request.cfFormat)
    }

    unsafe fn bytes(&self, request: *const FORMATETC) -> windows::core::Result<Arc<Vec<u8>>> {
        let id = Self::format_id(request)?;
        if let Some((_, data)) = self.formats.iter().find(|f| f.0 == id) {
            return data.bytes().map_err(|e| {
                windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), e)
            });
        }
        Err(DV_E_FORMATETC.into())
    }
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, request: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        unsafe {
            let bytes = self.bytes(request)?;
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if memory.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            let pointer = GlobalLock(memory);
            if pointer.is_null() {
                GlobalFree(memory);
                return Err(E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
            GlobalUnlock(memory);
            Ok(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 {
                    hGlobal: HGLOBAL(memory),
                },
                pUnkForRelease: ManuallyDrop::new(None),
            })
        }
    }
    fn GetDataHere(
        &self,
        _request: *const FORMATETC,
        _medium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryGetData(&self, request: *const FORMATETC) -> HRESULT {
        unsafe {
            match DataObject::format_id(request) {
                Err(error) => error.code(),
                Ok(id) => {
                    if self.formats.iter().any(|f| f.0 == id) {
                        S_OK
                    } else {
                        DV_E_FORMATETC
                    }
                }
            }
        }
    }
    fn GetCanonicalFormatEtc(&self, input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
        if input.is_null() || output.is_null() {
            return E_POINTER;
        }
        unsafe {
            *output = *input;
            (*output).ptd = null_mut();
        }
        DATA_S_SAMEFORMATETC
    }
    fn SetData(
        &self,
        request: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> windows::core::Result<()> {
        if request.is_null() || medium.is_null() {
            return Err(E_POINTER.into());
        }
        // Accept completion feedback without retaining data that the source never reads.
        unsafe {
            let id = DataObject::format_id(request)?;
            if ![
                "Performed DropEffect",
                "Logical Performed DropEffect",
                "Paste Succeeded",
            ]
            .iter()
            .any(|name| registered(name).is_ok_and(|format| format == id))
            {
                return Err(E_NOTIMPL.into());
            }
            if (*medium).tymed != TYMED_HGLOBAL.0 as u32 {
                return Err(DV_E_TYMED.into());
            }
            if release.as_bool() {
                let mut owned = (*medium).clone();
                ReleaseStgMedium(&mut owned);
            }
        }
        Ok(())
    }
    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let formats: Vec<_> = self.formats.iter().map(|f| format(f.0)).collect();
        unsafe { SHCreateStdEnumFmtEtc(&formats) }
    }
    fn DAdvise(
        &self,
        _request: *const FORMATETC,
        _flags: u32,
        _sink: windows::core::Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

unsafe fn data_object(prepared: &mut Prepared) -> Result<IDataObject, String> {
    Ok(DataObject {
        formats: std::mem::take(&mut prepared.formats),
    }
    .into())
}

#[implement(IDropSource)]
struct Source {
    cancelled: Arc<AtomicBool>,
    window: usize,
    visual_window: usize,
}
impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool()
            || self.cancelled.load(Ordering::Relaxed)
            || unsafe { IsWindow(self.window as _) } == 0
        {
            unsafe {
                super::drag_image::hide(self.visual_window);
            }
            DRAGDROP_S_CANCEL
        } else if keys.0 & 1 == 0 {
            unsafe {
                super::drag_image::hide(self.visual_window);
            }
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        unsafe {
            super::drag_image::follow(self.visual_window);
        }
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

pub(super) unsafe fn run(
    mut prepared: Prepared,
    cancelled: Arc<AtomicBool>,
    window: usize,
    text_engine: super::gfx::TextEngine,
) -> Result<bool, String> {
    let data = data_object(&mut prepared)?;
    let scene = prepared.visual.take().ok_or("无法生成拖动图样")?;
    let visual = super::drag_image::Overlay::new(scene, text_engine)?;
    let source: IDropSource = Source {
        cancelled,
        window,
        visual_window: visual.window(),
    }
    .into();
    let mut effect = DROPEFFECT_NONE;
    let result = DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect);
    drop(visual);
    if result == DRAGDROP_S_CANCEL {
        return Ok(false);
    }
    result.ok().map_err(|e| e.to_string())?;
    let dropped = result == DRAGDROP_S_DROP && effect.0 & DROPEFFECT_COPY.0 != 0;
    if dropped {
        if let Some(export) = &mut prepared.export {
            export.keep = true;
        }
    }
    Ok(dropped)
}

impl super::State {
    pub(super) unsafe fn cancel_pending_drag(&mut self) {
        if self.drag_press.take().is_some() {
            self.drag_release_consumed = true;
        }
        if self.drag_pending.is_some() {
            self.status = self
                .config_error
                .clone()
                .or_else(|| self.execution.execution_error.clone())
                .or_else(|| self.query.groups.status());
        }
        self.drag_pending = None;
        self.prepared_drag = None;
        self.drag_request = self.drag_request.wrapping_add(1);
        if !self.dragging {
            if let Some(cancel) = self.drag_cancel.take() {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() == self.hwnd
            && self.scroll_drag.is_none()
        {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
    }

    pub(super) unsafe fn prepare_drag(&mut self) {
        use mini_launcher::drag_data::DragSource;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        let Some((_, entry)) = self.drag_press.take() else {
            return;
        };
        if let Some(DragSource::Plugin { plugin_id, action_id }) = &entry.drag {
            if !self.validate_plugin_callback(plugin_id, action_id, true) {
                return;
            }
        }
        let visual = super::drag_image::snapshot(self, &entry);
        let Some(source) = entry.drag else {
            return;
        };
        self.drag_release_consumed = true;
        self.drag_request = self.drag_request.wrapping_add(1);
        let id = self.drag_request;
        self.drag_pending = Some(id);
        self.status = Some("正在准备拖动…".into());
        self.invalidate();
        let cancel = Arc::new(AtomicBool::new(false));
        self.drag_cancel = Some(cancel.clone());
        let receiver = match source {
            DragSource::Plugin {
                plugin_id,
                action_id,
            } => self.plugins.prepare_drag(plugin_id, action_id),
            DragSource::Data(data) => {
                let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                let _ = sender.send(Ok(data));
                Ok(receiver)
            }
        };
        let sender = self.event_sender.clone();
        let hwnd = self.hwnd as usize;
        std::thread::spawn(move || {
            let result = receiver
                .and_then(|r| r.recv().map_err(|_| "插件拖动请求已中止".to_string())?)
                .and_then(|data| {
                    if cancel.load(Ordering::Relaxed)
                        || unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } >= 0
                    {
                        return Err("拖动已取消".into());
                    }
                    let mut prepared = prepare(data)?;
                    prepared.visual = Some(visual);
                    Ok(prepared)
                });
            super::send_event(&sender, hwnd, super::UiEvent::DragPrepared { id, result });
        });
    }
}

//! Everything's documented Unicode IPC v1, without a bundled SDK DLL.
use crate::{
    config::EverythingSettings,
    providers::{self, Emit, Source, Update},
    search::{Action, Entry},
};
use std::{
    mem::zeroed,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::COPYDATASTRUCT, LibraryLoader::*},
    UI::WindowsAndMessaging::*,
};

const WORK: u32 = WM_APP + 1;
const TIMER: usize = 1;
pub struct Query {
    pub scoped: bool,
    pub generation: u64,
    pub text: String,
    pub settings: EverythingSettings,
}
pub struct Client {
    sender: Sender<Query>,
    window: usize,
}
impl Client {
    pub fn start(emit: Emit, latest: Arc<AtomicU64>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (ready, window) = mpsc::sync_channel(1);
        std::thread::spawn(move || unsafe {
            let class_name = wide("MiniLauncher.Everything.IPC");
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..zeroed()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            let mut host = Box::new(Host {
                receiver,
                emit,
                latest,
                current: None,
                reply_id: 1,
            });
            if hwnd.is_null() {
                let _ = ready.send(0);
                return;
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *host as *mut Host as isize);
            let _ = ready.send(hwnd as usize);
            let mut message: MSG = zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
        });
        Self {
            sender,
            window: window.recv().unwrap_or(0),
        }
    }
    pub fn query(&self, query: Query) {
        if self.sender.send(query).is_ok() && self.window != 0 {
            unsafe {
                PostMessageW(self.window as HWND, WORK, 0, 0);
            }
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        if self.window != 0 {
            unsafe {
                PostMessageW(self.window as HWND, WM_CLOSE, 0, 0);
            }
        }
    }
}

struct Pending {
    generation: u64,
    exclusive: bool,
    max_results: usize,
}
struct Host {
    receiver: Receiver<Query>,
    emit: Emit,
    latest: Arc<AtomicU64>,
    current: Option<Pending>,
    reply_id: u32,
}
impl Host {
    fn finish(&mut self, entries: Vec<Entry>, status: Option<String>) {
        if let Some(pending) = self.current.take() {
            if pending.generation == self.latest.load(Ordering::Relaxed) {
                (self.emit)(Update {
                    kind: crate::providers::UpdateKind::Results,
                    generation: pending.generation,
                    source: Source::Files,
                    entries,
                    exclusive: pending.exclusive,
                    pending: false,
                    status,
                });
            }
        }
    }
    unsafe fn query(&mut self, hwnd: HWND) {
        let Ok(mut request) = self.receiver.try_recv() else {
            return;
        };
        while let Ok(next) = self.receiver.try_recv() {
            request = next;
        }
        KillTimer(hwnd, TIMER);
        self.current = None;
        if request.generation != self.latest.load(Ordering::Relaxed) {
            return;
        }
        let route = if request.scoped && request.settings.enabled {
            Some((request.text.trim(), true))
        } else {
            providers::files_query(&request.text, &request.settings)
        };
        let (text, exclusive) = route.unwrap_or(("", false));
        self.current = Some(Pending {
            generation: request.generation,
            exclusive,
            max_results: request.settings.max_results,
        });
        if route.is_none() {
            self.finish(Vec::new(), None);
            return;
        }
        if text.is_empty() {
            self.finish(
                Vec::new(),
                Some(format!(
                    "输入 {} 文件名，支持 Everything 搜索语法",
                    request.settings.keyword
                )),
            );
            return;
        }
        let everything = FindWindowW(wide("EVERYTHING_TASKBAR_NOTIFICATION").as_ptr(), null());
        if everything.is_null() {
            self.finish(
                Vec::new(),
                Some("未检测到 Everything 客户端，请先启动 Everything".into()),
            );
            return;
        }
        self.reply_id = self.reply_id.wrapping_add(1).max(1);
        let mut payload = Vec::new();
        for value in [
            hwnd as usize as u32,
            self.reply_id,
            0,
            0,
            request.settings.max_results as u32,
        ] {
            payload.extend(value.to_le_bytes());
        }
        for unit in text.encode_utf16().chain(Some(0)) {
            payload.extend(unit.to_le_bytes());
        }
        let copy = COPYDATASTRUCT {
            dwData: 2,
            cbData: payload.len() as u32,
            lpData: payload.as_mut_ptr() as _,
        };
        (self.emit)(Update {
            kind: crate::providers::UpdateKind::State,
            generation: request.generation,
            source: Source::Files,
            entries: Vec::new(),
            exclusive,
            pending: true,
            status: None,
        });
        let mut result = 0usize;
        if SendMessageTimeoutW(
            everything,
            WM_COPYDATA,
            hwnd as usize,
            &copy as *const _ as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            200,
            &mut result,
        ) == 0
            || result == 0
        {
            self.finish(Vec::new(), Some("Everything IPC 不可用或没有响应".into()));
        } else {
            SetTimer(hwnd, TIMER, request.settings.timeout_ms as u32, None);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let host = &mut *pointer;
    match message {
        WORK => {
            host.query(hwnd);
            0
        }
        WM_COPYDATA if lparam != 0 => {
            let copy = &*(lparam as *const COPYDATASTRUCT);
            if copy.dwData != host.reply_id as usize || host.current.is_none() {
                return 0;
            }
            KillTimer(hwnd, TIMER);
            if copy.lpData.is_null() || copy.cbData > 8 * 1024 * 1024 {
                host.finish(Vec::new(), Some("Everything 返回了无效数据".into()));
                return 1;
            }
            let bytes = std::slice::from_raw_parts(copy.lpData as *const u8, copy.cbData as usize);
            let limit = host.current.as_ref().unwrap().max_results;
            match parse_reply(bytes, limit) {
                Ok((entries, total)) => {
                    let status = (total > entries.len()).then(|| {
                        format!(
                            "显示前 {} / {} 个文件结果，请补充关键词缩小范围",
                            entries.len(),
                            total
                        )
                    });
                    host.finish(entries, status);
                }
                Err(error) => host.finish(Vec::new(), Some(error)),
            }
            1
        }
        WM_TIMER => {
            KillTimer(hwnd, TIMER);
            host.finish(Vec::new(), Some("Everything 查询超时，请重试".into()));
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn dword(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or("Everything 返回的数据被截断")?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}
fn string(bytes: &[u8], offset: usize, minimum: usize) -> Result<String, String> {
    if offset < minimum || !offset.is_multiple_of(2) {
        return Err("Everything 返回的字符串偏移无效".into());
    }
    let mut units = Vec::new();
    for chunk in bytes
        .get(offset..)
        .ok_or("Everything 字符串越界")?
        .as_chunks::<2>()
        .0
    {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| "Everything 返回了无效 Unicode".into());
        }
        units.push(unit);
        if units.len() > 32768 {
            break;
        }
    }
    Err("Everything 字符串缺少终止符".into())
}
fn parse_reply(bytes: &[u8], limit: usize) -> Result<(Vec<Entry>, usize), String> {
    let total = dword(bytes, 8)? as usize;
    let count = dword(bytes, 20)? as usize;
    if count > limit || count > 1000 {
        return Err("Everything 返回了过多结果".into());
    }
    let strings_start = 28 + count * 12;
    if bytes.len() < strings_start {
        return Err("Everything 返回的数据被截断".into());
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let item = 28 + index * 12;
        let name = string(bytes, dword(bytes, item + 4)? as usize, strings_start)?;
        let directory = string(bytes, dword(bytes, item + 8)? as usize, strings_start)?;
        let path = PathBuf::from(directory)
            .join(&name)
            .to_string_lossy()
            .into_owned();
        let kind = if dword(bytes, item)? & 1 != 0 {
            "文件夹"
        } else {
            "文件"
        };
        entries.push(Entry::new(name, path.clone(), Action::Open(path), kind));
    }
    Ok((entries, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_reply_is_decoded_and_invalid_offsets_are_rejected() {
        let mut bytes = Vec::new();
        let name: Vec<_> = "测试.txt"
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        for value in [0u32, 1, 1, 0, 1, 1, 0, 0, 40, 40 + name.len() as u32] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(name);
        bytes.extend(
            "C:\\文档"
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes),
        );
        let (items, total) = parse_reply(&bytes, 200).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].title, "测试.txt");
        bytes[32..36].copy_from_slice(&0xfffffff0u32.to_le_bytes());
        assert!(parse_reply(&bytes, 200).is_err());
        assert!(parse_reply(&[0u8; 16], 200).is_err());
    }
}

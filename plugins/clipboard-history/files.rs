//! File history holds references only; replay always requests a copy.
use std::path::Path;
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, Memory::*},
    UI::Shell::DragQueryFileW,
};

pub const MAX_FILES: usize = 64;
pub const MAX_PATH: usize = 32768;
pub const MAX_TOTAL: usize = 64 * 1024;

pub fn validate(paths: &[String]) -> Result<(), String> {
    if paths.is_empty()
        || paths.len() > MAX_FILES
        || paths.iter().map(String::len).sum::<usize>() > MAX_TOTAL
        || paths
            .iter()
            .any(|p| p.len() > MAX_PATH || p.contains('\0') || !Path::new(p).is_absolute())
    {
        return Err("文件列表无效：最多 64 项，路径总量不超过 64 KiB".into());
    }
    Ok(())
}

pub fn available(paths: &[String]) -> Result<(), String> {
    validate(paths)?;
    for path in paths {
        std::fs::metadata(path).map_err(|e| format!("文件不存在或无法访问：{path}（{e}）"))?;
    }
    Ok(())
}

/// Caller keeps the clipboard open for the lifetime of the handle.
pub unsafe fn read(handle: HANDLE) -> Result<Vec<String>, String> {
    if handle.is_null() || GlobalSize(handle) < 20 || GlobalSize(handle) > 2 * MAX_TOTAL + 1024 {
        return Err("剪贴板文件列表过大或无效".into());
    }
    // Validate the bounds and terminators before passing external memory to Shell.
    let pointer = GlobalLock(handle);
    if pointer.is_null() {
        return Err("锁定剪贴板文件列表失败".into());
    }
    let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), GlobalSize(handle));
    let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    let valid = offset >= 20
        && offset < bytes.len()
        && if wide {
            offset % 2 == 0
                && bytes[offset..]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .scan(false, |previous_zero, unit| {
                        let end = *previous_zero && unit == 0;
                        *previous_zero = unit == 0;
                        Some(end)
                    })
                    .any(|end| end)
        } else {
            bytes[offset..].windows(2).any(|b| b == [0, 0])
        };
    GlobalUnlock(handle);
    if !valid {
        return Err("剪贴板文件列表头或结束标记无效".into());
    }
    let count = DragQueryFileW(handle, u32::MAX, std::ptr::null_mut(), 0) as usize;
    if count == 0 || count > MAX_FILES {
        return Err("剪贴板文件列表为空或超过 64 项".into());
    }
    let mut paths = Vec::with_capacity(count);
    let mut total = 0;
    for index in 0..count as u32 {
        let length = DragQueryFileW(handle, index, std::ptr::null_mut(), 0) as usize;
        if length == 0 || length > MAX_PATH {
            return Err("剪贴板文件路径过长或无效".into());
        }
        let mut units = vec![0u16; length + 1];
        if DragQueryFileW(handle, index, units.as_mut_ptr(), units.len() as u32) as usize != length
        {
            return Err("读取剪贴板文件路径失败".into());
        }
        let path = String::from_utf16(&units[..length]).map_err(|_| "文件路径编码无效")?;
        total += path.len();
        if total > MAX_TOTAL {
            return Err("剪贴板文件路径总量超过 64 KiB".into());
        }
        paths.push(path);
    }
    validate(&paths)?;
    Ok(paths)
}

pub fn encode(paths: &[String]) -> Result<Vec<u8>, String> {
    validate(paths)?;
    let mut bytes = Vec::new();
    bytes.extend(20u32.to_le_bytes()); // DROPFILES.pFiles
    bytes.extend([0u8; 12]); // point and fNC
    bytes.extend(1u32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.encode_utf16().chain(Some(0)) {
            bytes.extend(unit.to_le_bytes());
        }
    }
    bytes.extend(0u16.to_le_bytes());
    Ok(bytes)
}

struct Memory(HGLOBAL);
impl Memory {
    unsafe fn new(bytes: &[u8]) -> Result<Self, String> {
        let memory = Self(GlobalAlloc(GMEM_MOVEABLE, bytes.len()));
        if memory.0.is_null() {
            return Err("分配剪贴板文件内存失败".into());
        }
        let pointer = GlobalLock(memory.0);
        if pointer.is_null() {
            return Err("锁定剪贴板文件内存失败".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
        GlobalUnlock(memory.0);
        Ok(memory)
    }
    unsafe fn publish(&mut self, format: u32) -> Result<(), String> {
        if SetClipboardData(format, self.0).is_null() {
            return Err("复制文件到剪贴板失败".into());
        }
        self.0 = std::ptr::null_mut(); // ownership transferred to Windows
        Ok(())
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}

pub unsafe fn copy(paths: &[String], owner: HWND) -> Result<(), String> {
    available(paths)?; // Reject the entire group before changing the clipboard.
    let mut drop = Memory::new(&encode(paths)?)?;
    let mut effect = Memory::new(&1u32.to_le_bytes())?; // DROPEFFECT_COPY
    let name: Vec<u16> = "Preferred DropEffect"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let format = RegisterClipboardFormatW(name.as_ptr());
    if format == 0 {
        return Err("注册文件复制格式失败".into());
    }
    if owner.is_null() || OpenClipboard(owner) == 0 {
        return Err("剪贴板被占用，请重试".into());
    }
    let result = (|| {
        if EmptyClipboard() == 0 {
            return Err("清空系统剪贴板失败".into());
        }
        effect.publish(format)?;
        drop.publish(15) // CF_HDROP
    })();
    CloseClipboard();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_file_list_roundtrips_unicode_and_multiple_paths() {
        let paths = vec![r"C:\文件\报告.txt".into(), r"\\server\share\folder".into()];
        unsafe {
            let memory = Memory::new(&encode(&paths).unwrap()).unwrap();
            assert_eq!(read(memory.0).unwrap(), paths);
            let mut malformed = encode(&paths).unwrap();
            malformed[..4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(read(Memory::new(&malformed).unwrap().0).is_err());
            let mut unterminated = vec![1u8; 24];
            unterminated[..4].copy_from_slice(&20u32.to_le_bytes());
            assert!(read(Memory::new(&unterminated).unwrap().0).is_err());
        }
        assert!(validate(&vec![r"C:\x".into(); 65]).is_err());
        assert!(validate(&["relative.txt".into()]).is_err());
        assert!(validate(&["C:\\bad\0path".into()]).is_err());
        assert!(validate(&vec![format!("C:\\{}", "x".repeat(2000)); 64]).is_err());
    }
}

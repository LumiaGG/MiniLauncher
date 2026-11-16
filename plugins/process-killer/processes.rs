use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FALSE, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    PROCESS_NAME_NATIVE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
};

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub executable_path: Option<String>,
    pub memory_bytes: u64,
    pub is_protected: bool,
    pub protection_reason: Option<String>,
}

const CRITICAL_SYSTEM_NAMES: &[&str] = &[
    "system",
    "system idle process",
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "dwm.exe",
    "fontdrvhost.exe",
    "mini-launcher.exe",
    "process-killer.exe",
];

pub fn is_protected_process(pid: u32, name: &str) -> Option<String> {
    if pid == 0 {
        return Some("系统空闲进程 (System Idle Process)".into());
    }
    if pid == 4 {
        return Some("Windows 内核系统进程 (System)".into());
    }
    unsafe {
        if pid == GetCurrentProcessId() {
            return Some("插件自身进程".into());
        }
    }
    let lower_name = name.to_ascii_lowercase();
    for &critical in CRITICAL_SYSTEM_NAMES {
        if lower_name == critical {
            return Some(format!("关键系统/宿主服务 ({critical})"));
        }
    }
    None
}

/// Enumerates all accessible processes on the system.
pub fn get_all_processes() -> Vec<ProcessInfo> {
    let mut list = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return list;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let name = decode_wide(&entry.szExeFile);
                let pid = entry.th32ProcessID;
                let parent_pid = entry.th32ParentProcessID;

                let protection = is_protected_process(pid, &name);
                let is_protected = protection.is_some();

                let (executable_path, memory_bytes) = get_process_details(pid);

                list.push(ProcessInfo {
                    pid,
                    parent_pid,
                    name,
                    executable_path,
                    memory_bytes,
                    is_protected,
                    protection_reason: protection,
                });

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    list
}

unsafe fn get_process_details(pid: u32) -> (Option<String>, u64) {
    if pid == 0 || pid == 4 {
        return (None, 0);
    }
    let handle = OpenProcess(
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
        FALSE,
        pid,
    );
    let handle = if handle.is_null() {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)
    } else {
        handle
    };

    if handle.is_null() {
        return (None, 0);
    }

    let path = get_executable_path(handle);
    let memory = get_memory_usage(handle);
    CloseHandle(handle);
    (path, memory)
}

unsafe fn get_executable_path(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    if QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) != 0 && size > 0 {
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        return Some(path);
    }
    // Fallback to native path if Win32 path fails
    size = buffer.len() as u32;
    if QueryFullProcessImageNameW(handle, PROCESS_NAME_NATIVE, buffer.as_mut_ptr(), &mut size) != 0
        && size > 0
    {
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        return Some(path);
    }
    None
}

unsafe fn get_memory_usage(handle: HANDLE) -> u64 {
    let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if K32GetProcessMemoryInfo(
        handle,
        &mut counters,
        std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
    ) != 0
    {
        counters.WorkingSetSize as u64
    } else {
        0
    }
}

pub fn format_memory(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else if bytes > 0 {
        format!("{bytes} B")
    } else {
        "--".into()
    }
}

pub fn terminate_process(pid: u32) -> Result<(), String> {
    if let Some(reason) = is_protected_process(pid, "") {
        return Err(format!("受保护进程（{reason}），禁止终止"));
    }

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, FALSE, pid);
        if handle.is_null() {
            let err = GetLastError();
            return match err {
                ERROR_ACCESS_DENIED => Err(
                    "拒绝访问：权限不足，请尝试以管理员身份运行 MiniLauncher 或在 UAC 提权后重试"
                        .into(),
                ),
                ERROR_INVALID_PARAMETER => Err("进程不存在或已提前退出".into()),
                _ => Err(format!("无法打开进程 (PID {pid})，错误码: {err}")),
            };
        }

        let ret = TerminateProcess(handle, 1);
        let err = GetLastError();
        CloseHandle(handle);

        if ret == 0 {
            return Err(format!("终止进程失败，错误码: {err}"));
        }
        Ok(())
    }
}

fn decode_wide(slice: &[u16]) -> String {
    let len = slice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..len])
}

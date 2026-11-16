use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, LPARAM, S_OK, WPARAM,
};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_COMMAND,
};

/// 检查是否有 MiniLauncher 实例正在运行，如果有，通知其优雅退出并等待
pub fn terminate_running_instances(timeout_ms: u32) -> bool {
    unsafe {
        let class_name: Vec<u16> = "MiniLauncher.Native.Window\0".encode_utf16().collect();
        let hwnd = FindWindowW(class_name.as_ptr(), std::ptr::null());
        if !hwnd.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);

            // 发送 QUIT 命令通知主程序优雅释放资源
            PostMessageW(hwnd, WM_COMMAND, 4 as WPARAM, 0 as LPARAM);

            if pid != 0 {
                let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
                if !process.is_null() {
                    let wait = WaitForSingleObject(process, timeout_ms);
                    CloseHandle(process);
                    if wait == 0 {
                        return true;
                    }
                }
            }
        }

        // 双重检查互斥体 Local\MiniLauncher.SingleInstance
        let mutex_name: Vec<u16> = "Local\\MiniLauncher.SingleInstance\0".encode_utf16().collect();
        let h_mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
        if !h_mutex.is_null() {
            let already_exists = GetLastError() == ERROR_ALREADY_EXISTS;
            CloseHandle(h_mutex);
            if already_exists {
                // 等待短暂释放
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    true
}

/// 创建 Windows 快捷方式 (.lnk)
pub fn create_shortcut(
    target_exe: &Path,
    shortcut_path: &Path,
    working_dir: &Path,
    description: &str,
) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);

        // CLSID_ShellLink: 00021401-0000-0000-C000-000000000046
        const CLSID_SHELL_LINK: windows_sys::core::GUID = windows_sys::core::GUID {
            data1: 0x00021401,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
        };
        // IID_IShellLinkW: 000214F9-0000-0000-C000-000000000046
        const IID_ISHELL_LINK_W: windows_sys::core::GUID = windows_sys::core::GUID {
            data1: 0x000214F9,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
        };
        // IID_IPersistFile: 0000010b-0000-0000-C000-000000000046
        const IID_IPERSIST_FILE: windows_sys::core::GUID = windows_sys::core::GUID {
            data1: 0x0000010B,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
        };

        let mut sl: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_SHELL_LINK,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ISHELL_LINK_W,
            &mut sl,
        );
        if hr != S_OK || sl.is_null() {
            CoUninitialize();
            return Err("Failed to create IShellLink instance".into());
        }

        type Vtbl = *const usize;
        let vtbl = *(sl as *const Vtbl);
        let query_interface: unsafe extern "system" fn(
            *mut std::ffi::c_void,
            *const windows_sys::core::GUID,
            *mut *mut std::ffi::c_void,
        ) -> i32 = std::mem::transmute(*vtbl.add(0));
        let set_path: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16) -> i32 =
            std::mem::transmute(*vtbl.add(20));
        let set_working_dir: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16) -> i32 =
            std::mem::transmute(*vtbl.add(9));
        let set_description: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16) -> i32 =
            std::mem::transmute(*vtbl.add(7));
        let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));

        let target_w: Vec<u16> = target_exe
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let work_w: Vec<u16> = working_dir
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let desc_w: Vec<u16> = description.encode_utf16().chain(Some(0)).collect();

        set_path(sl, target_w.as_ptr());
        set_working_dir(sl, work_w.as_ptr());
        set_description(sl, desc_w.as_ptr());

        let mut pf: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr_qi = query_interface(sl, &IID_IPERSIST_FILE, &mut pf);
        if hr_qi == S_OK && !pf.is_null() {
            let pf_vtbl = *(pf as *const Vtbl);
            let pf_save: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16, i32) -> i32 =
                std::mem::transmute(*pf_vtbl.add(6));
            let pf_release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
                std::mem::transmute(*pf_vtbl.add(2));

            let sc_w: Vec<u16> = shortcut_path
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let hr_save = pf_save(pf, sc_w.as_ptr(), 1);
            pf_release(pf);
            release(sl);
            CoUninitialize();

            if hr_save == S_OK {
                Ok(())
            } else {
                Err(format!("IPersistFile::Save failed with code {hr_save}"))
            }
        } else {
            release(sl);
            CoUninitialize();
            Err("Failed to query IPersistFile".into())
        }
    }
}

/// 注册 Windows 控制面板“应用和功能”卸载项（无需管理员权限，写入 HKCU）
pub fn register_uninstall_entry(install_dir: &Path, app_version: &str) -> Result<(), String> {
    let subkey = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MiniLauncher";
    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey_w.as_ptr(),
            0,
            std::ptr::null_mut(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );

        if status != 0 {
            return Err(format!("RegCreateKeyExW failed: {status}"));
        }

        let exe_path = install_dir.join("mini-launcher.exe");
        let uninst_path = install_dir.join("uninstall.exe");
        let uninst_cmd = format!("\"{}\" --uninstall", uninst_path.display());

        let set_string = |name: &str, val: &str| {
            let name_w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let val_w: Vec<u16> = val.encode_utf16().chain(Some(0)).collect();
            RegSetValueExW(
                hkey,
                name_w.as_ptr(),
                0,
                REG_SZ,
                val_w.as_ptr() as _,
                (val_w.len() * 2) as u32,
            );
        };

        set_string("DisplayName", "MiniLauncher");
        set_string("DisplayVersion", app_version);
        set_string("Publisher", "LumiaGG & Contributors");
        set_string("DisplayIcon", &format!("\"{}\",0", exe_path.display()));
        set_string("InstallLocation", &install_dir.display().to_string());
        set_string("UninstallString", &uninst_cmd);
        set_string("QuietUninstallString", &format!("{} --silent", uninst_cmd));

        RegCloseKey(hkey);
    }
    Ok(())
}

/// 清除 Windows 控制面板“应用和功能”卸载项
pub fn unregister_uninstall_entry() {
    let subkey = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MiniLauncher";
    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, subkey_w.as_ptr());
    }
}

/// 获取默认安装路径：%LOCALAPPDATA%\Programs\MiniLauncher
pub fn get_default_install_dir() -> PathBuf {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local_appdata)
            .join("Programs")
            .join("MiniLauncher")
    } else {
        PathBuf::from("C:\\MiniLauncher")
    }
}

/// 获取开始菜单快捷方式路径
pub fn get_start_menu_shortcut_path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|appdata| {
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("MiniLauncher.lnk")
    })
}

/// 获取桌面快捷方式路径
pub fn get_desktop_shortcut_path() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(|user| PathBuf::from(user).join("Desktop").join("MiniLauncher.lnk"))
}

/// 启动 MiniLauncher 进程
pub fn launch_app(exe_path: &Path) -> Result<(), String> {
    std::process::Command::new(exe_path)
        .current_dir(exe_path.parent().unwrap_or(Path::new(".")))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

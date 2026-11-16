#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod applications;
mod gui;
mod model;
mod packages;

use std::{io, path::PathBuf};

fn main() {
    let cli = std::env::args().any(|a| matches!(a.as_str(), "--check" | "--help" | "-h"));
    #[cfg(windows)]
    if cli {
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    if let Err(error) = run() {
        if cli {
            eprintln!("MiniLauncher 设置：{error}");
        } else {
            rfd::MessageDialog::new()
                .set_title("MiniLauncher 设置")
                .set_description(error)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut path = None;
    let mut plugins = false;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--plugins" => plugins = true,
            "--check" => check = true,
            "--from-launcher" => {}
            "--settings" => {
                path = Some(PathBuf::from(args.next().ok_or("--settings 缺少文件路径")?))
            }
            "--help" | "-h" => {
                println!("MiniLauncher 设置 GUI\n\nmini-launcher-config [--plugins] [--settings <文件>] [--check]\n\n--plugins  直接打开插件管理\n--settings 使用指定配置文件\n--check    仅校验配置，不启动图形界面、不保存\n\n修改后自动保存；关闭设置后进程退出。");
                return Ok(());
            }
            _ => return Err(format!("未知参数：{arg}")),
        }
    }
    if check {
        let text =
            std::fs::read_to_string(path.unwrap_or_else(mini_launcher::config::settings_path))
                .map_err(|e| e.to_string())?;
        let (settings, _) = mini_launcher::config::parse_and_complete(&text)?;
        mini_launcher::plugin_catalog::validate_web_keywords(
            &settings.plugins,
            &settings.web_search,
        )?;
        model::validate_plugin_schemas(&settings.plugins)?;
        println!("配置校验通过");
        return Ok(());
    }
    let startup_error = if path.is_none() {
        mini_launcher::config::load().err()
    } else {
        None
    };
    let path = path.unwrap_or_else(mini_launcher::config::settings_path);
    let Some(instance) = Instance::acquire(&path)? else {
        return Ok(());
    };
    let draft = model::Draft::load(path)?;
    gui::run(draft, plugins, instance, startup_error);
    Ok(())
}

// One interactive editor per settings file. Three-way merging still protects
// against external text editors and the launcher's default completion watcher.
struct Instance(#[cfg(windows)] windows_sys::Win32::Foundation::HANDLE);
impl Instance {
    fn acquire(path: &std::path::Path) -> Result<Option<Self>, String> {
        #[cfg(windows)]
        unsafe {
            use std::hash::{Hash, Hasher};
            use windows_sys::Win32::{
                Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS},
                System::Threading::CreateMutexW,
                UI::WindowsAndMessaging::{
                    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
                },
            };
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::fs::canonicalize(path)
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_lowercase()
                .hash(&mut hasher);
            let name: Vec<_> = format!("Local\\MiniLauncher.Settings.{:x}", hasher.finish())
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(io::Error::last_os_error().to_string());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                let title: Vec<_> = "MiniLauncher 设置".encode_utf16().chain(Some(0)).collect();
                let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                if !hwnd.is_null() {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
                eprintln!("该配置的设置管理器已打开，请切换到已有设置窗口。");
                return Ok(None);
            }
            Ok(Some(Self(handle)))
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            Ok(Some(Self()))
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

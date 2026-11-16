#![windows_subsystem = "windows"]

mod dialog;
mod payload;
mod system;
mod ui;

use std::env;
use std::path::{Path, PathBuf};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO,
};

fn main() {
    let args: Vec<String> = env::args().collect();

    let exe_name = env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .unwrap_or_default();

    // 卸载模式：显式带有 --uninstall 参数，或者自身文件名以 uninstall 开头
    if args.iter().any(|a| a == "--uninstall") || exe_name.starts_with("uninstall") {
        let silent = args.iter().any(|a| a == "--silent" || a == "-s");
        handle_uninstall(silent);
        return;
    }

    // 更新模式: --update --target <dir> [--source <zip>]
    if let Some(pos) = args.iter().position(|a| a == "--update") {
        let mut target_dir = None;
        let mut source_zip = None;
        let mut i = pos + 1;
        while i < args.len() {
            if args[i] == "--target" && i + 1 < args.len() {
                target_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else if args[i] == "--source" && i + 1 < args.len() {
                source_zip = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else {
                i += 1;
            }
        }
        handle_update(target_dir, source_zip);
        return;
    }

    // 静默安装模式: --silent [-d <path>]
    if args.iter().any(|a| a == "--silent" || a == "-s") {
        let mut install_path = system::get_default_install_dir();
        for i in 1..args.len() {
            if (args[i] == "-d" || args[i] == "--dir") && i + 1 < args.len() {
                install_path = PathBuf::from(&args[i + 1]);
                break;
            }
        }
        handle_silent_install(&install_path);
        return;
    }

    // 默认图形向导模式
    ui::run_installer_gui();
}

fn handle_silent_install(target_dir: &Path) {
    system::terminate_running_instances(2000);
    let _ = payload::extract_payload(target_dir, None, true, |_, _, _| {});

    let exe_path = target_dir.join("mini-launcher.exe");
    if let Some(desktop_lnk) = system::get_desktop_shortcut_path() {
        let _ = system::create_shortcut(&exe_path, &desktop_lnk, target_dir, "MiniLauncher 启动器");
    }
    if let Some(startmenu_lnk) = system::get_start_menu_shortcut_path() {
        if let Some(p) = startmenu_lnk.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = system::create_shortcut(
            &exe_path,
            &startmenu_lnk,
            target_dir,
            "MiniLauncher 启动器",
        );
    }
    let _ = system::register_uninstall_entry(target_dir, "0.1.0");
}

fn handle_update(target_dir: Option<PathBuf>, source_zip: Option<PathBuf>) {
    let target = match target_dir {
        Some(t) => t,
        None => system::get_default_install_dir(),
    };

    // 1. 关闭正在运行的旧版本实例并等待释放
    system::terminate_running_instances(3000);

    // 2. 解压覆盖
    let is_portable = target.join("portable.toml").exists();
    let _ = payload::extract_payload(&target, source_zip.as_deref(), is_portable, |_, _, _| {});

    // 3. 重新唤出新版本
    let exe = target.join("mini-launcher.exe");
    let _ = system::launch_app(&exe);
}

fn handle_uninstall(silent: bool) {
    let install_dir = match env::current_exe() {
        Ok(exe) => exe.parent().unwrap_or(Path::new(".")).to_path_buf(),
        Err(_) => system::get_default_install_dir(),
    };

    let mut keep_user_data = true;

    if !silent {
        unsafe {
            let title: Vec<u16> = "卸载 MiniLauncher\0".encode_utf16().collect();
            let prompt: Vec<u16> = "确定要卸载 MiniLauncher 吗？\0".encode_utf16().collect();
            let confirm = MessageBoxW(
                std::ptr::null_mut(),
                prompt.as_ptr(),
                title.as_ptr(),
                MB_ICONQUESTION | MB_YESNO,
            );
            if confirm != IDYES {
                return;
            }

            let data_prompt: Vec<u16> =
                "是否保留您的自定义设置和历史数据（data/ 目录与配置文件）？\n点击【是】保留个人数据，点击【否】全部彻底清除。\0"
                    .encode_utf16()
                    .collect();
            let data_choice = MessageBoxW(
                std::ptr::null_mut(),
                data_prompt.as_ptr(),
                title.as_ptr(),
                MB_ICONQUESTION | MB_YESNO,
            );
            keep_user_data = data_choice == IDYES;
        }
    }

    // 1. 退出正在运行的实例
    system::terminate_running_instances(2000);

    // 2. 删除快捷方式
    if let Some(desktop) = system::get_desktop_shortcut_path() {
        if desktop.exists() {
            let _ = std::fs::remove_file(desktop);
        }
    }
    if let Some(startmenu) = system::get_start_menu_shortcut_path() {
        if startmenu.exists() {
            let _ = std::fs::remove_file(startmenu);
        }
    }

    // 3. 注销控制面板
    system::unregister_uninstall_entry();

    // 4. 清理安装目录
    if let Ok(entries) = std::fs::read_dir(&install_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.eq_ignore_ascii_case("uninstall.exe") {
                    continue; // 自身稍后自删除
                }
                if keep_user_data
                    && (name.eq_ignore_ascii_case("data")
                        || name.eq_ignore_ascii_case("portable.toml"))
                {
                    continue;
                }
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    // 5. 提示完成
    if !silent {
        unsafe {
            let title: Vec<u16> = "MiniLauncher 卸载完成\0".encode_utf16().collect();
            let msg: Vec<u16> = "MiniLauncher 已成功从您的电脑中移除。\0"
                .encode_utf16()
                .collect();
            MessageBoxW(
                std::ptr::null_mut(),
                msg.as_ptr(),
                title.as_ptr(),
                MB_ICONINFORMATION | MB_OK,
            );
        }
    }

    // 6. 自删除批处理机制退出
    if let Ok(current_exe) = env::current_exe() {
        let exe_str = current_exe.to_string_lossy().to_string();
        let cmd = format!("timeout /t 1 /nobreak > NUL & del /f /q \"{}\"", exe_str);
        let _ = std::process::Command::new("cmd")
            .args(["/c", &cmd])
            .spawn();
    }
}

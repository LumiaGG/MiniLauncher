#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mini_launcher::{config, hotkey, plugin_config, providers, search};
#[cfg(windows)]
mod everything;
mod list;
#[cfg(windows)]
use mini_launcher::plugins;
#[cfg(windows)]
mod windows;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--initialize-data") {
        if let Err(error) = config::load() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--register-plugin") {
        let result = std::env::args_os()
            .nth(2)
            .ok_or_else(|| "缺少插件目录".to_string())
            .and_then(|directory| plugin_config::register(&std::path::PathBuf::from(directory)));
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if let Err(error) = windows::run() {
        windows::show_error(&error);
    }
    #[cfg(not(windows))]
    eprintln!("MiniLauncher currently supports Windows only.");
}

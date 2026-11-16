use super::wide;
use std::{
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn command(executable: &Path) -> Vec<u16> {
    let mut value = vec![b'"' as u16];
    value.extend(executable.as_os_str().encode_wide());
    value.extend(wide("\" --background"));
    value
}

/// Only owns the MiniLauncher value in the current user's login startup list.
pub(super) fn sync(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        Some(command(
            &std::env::current_exe().map_err(|e| e.to_string())?,
        ))
    } else {
        None
    };
    unsafe {
        let mut key = null_mut();
        let status = if enabled {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut key,
                null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if !enabled && status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err(format!(
                "无法更新开机启动：{}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        let name = wide("MiniLauncher");
        let status = match command {
            Some(command) => RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            ),
            None => RegDeleteValueW(key, name.as_ptr()),
        };
        RegCloseKey(key);
        if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
            Ok(())
        } else {
            Err(format!(
                "无法更新开机启动：{}",
                std::io::Error::from_raw_os_error(status as i32)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_command_quotes_unicode_and_spaces_and_starts_in_background() {
        let value = command(Path::new(r"C:\我的应用\Mini Launcher\mini-launcher.exe"));
        assert_eq!(value.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&value[..value.len() - 1]).unwrap(),
            "\"C:\\我的应用\\Mini Launcher\\mini-launcher.exe\" --background"
        );
    }
}

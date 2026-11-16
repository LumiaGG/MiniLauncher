use crate::config::{self, Settings};
use std::{fs, ptr::null, sync::Arc, thread::JoinHandle};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::Threading::*};

pub struct Watcher {
    stop: HANDLE,
    worker: Option<JoinHandle<()>>,
}
impl Watcher {
    pub fn start(
        emit: Arc<dyn Fn(Result<Prepared, String>) + Send + Sync>,
    ) -> Result<Self, String> {
        unsafe {
            let stop = CreateEventW(null(), 1, 0, null());
            if stop.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let stop_value = stop as usize;
            let worker = std::thread::spawn(move || {
                let directory: Vec<_> = config::data_dir()
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                let change = FindFirstChangeNotificationW(
                    directory.as_ptr(),
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE,
                );
                if change == INVALID_HANDLE_VALUE {
                    emit(Err("无法监控配置目录，请使用托盘手动重载".into()));
                    return;
                }
                let stop = stop_value as HANDLE;
                let handles = [stop, change];
                let mut previous = fs::read(config::settings_path()).ok();
                loop {
                    let event = WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
                    if event != WAIT_OBJECT_0 + 1 {
                        break;
                    }
                    if FindNextChangeNotification(change) == 0 {
                        break;
                    }
                    // Editors may save via several writes or an atomic rename.
                    if WaitForSingleObject(stop, 150) == WAIT_OBJECT_0 {
                        break;
                    }
                    let contents = fs::read(config::settings_path()).ok();
                    if contents == previous {
                        continue;
                    }
                    previous = contents;
                    emit(Prepared::load(false));
                }
                FindCloseChangeNotification(change);
            });
            Ok(Self {
                stop,
                worker: Some(worker),
            })
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            SetEvent(self.stop);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            CloseHandle(self.stop);
        }
    }
}
use std::os::windows::ffi::OsStrExt;

pub(super) struct Prepared {
    pub settings: Settings,
    pub catalog: mini_launcher::plugin_catalog::RuntimeCatalog,
    pub force: bool,
}
impl Prepared {
    pub fn load(force: bool) -> Result<Self, String> {
        let settings = config::load()?;
        let catalog = mini_launcher::plugin_catalog::RuntimeCatalog::read(&settings.plugins)?;
        Ok(Self {
            settings,
            catalog,
            force,
        })
    }
}

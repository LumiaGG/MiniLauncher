use super::{send_event, wide, UiEvent};
use std::{
    ptr::null,
    sync::mpsc::{self, Sender},
};
use windows_sys::Win32::{
    System::Com::*,
    UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
};

pub enum Task {
    Paste {
        target: super::paste::Target,
        generation: u64,
        latest: std::sync::Arc<std::sync::atomic::AtomicU64>,
        launcher: usize,
    },
    Open {
        target: String,
        verb: String,
        arguments: Option<String>,
    },
    Launch {
        executable: String,
        args: Vec<String>,
    },
}

pub fn start(events: Sender<UiEvent>, hwnd: usize) -> Sender<(u64, Task)> {
    let (sender, receiver) = mpsc::channel::<(u64, Task)>();
    std::thread::spawn(move || unsafe {
        let initialized = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        while let Ok((id, task)) = receiver.recv() {
            let result = if initialized < 0 {
                Err(format!("无法初始化启动线程：0x{:08X}", initialized as u32))
            } else {
                match task {
                    Task::Paste {
                        target,
                        generation,
                        latest,
                        launcher,
                    } => super::paste::run(target, generation, &latest, launcher),
                    Task::Launch { executable, args } => super::launch_program(&executable, &args),
                    Task::Open {
                        target,
                        verb,
                        arguments,
                    } => {
                        let target = wide(&target);
                        let verb = wide(&verb);
                        let arguments = arguments.map(|value| wide(&value));
                        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
                        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
                        // Finish Shell dispatch on this COM thread; suppress generic Shell error dialogs.
                        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
                        info.lpVerb = verb.as_ptr();
                        info.lpFile = target.as_ptr();
                        info.lpParameters =
                            arguments.as_ref().map_or(null(), |value| value.as_ptr());
                        info.nShow = SW_SHOWNORMAL;
                        if ShellExecuteExW(&mut info) != 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error().to_string())
                        }
                    }
                }
            };
            send_event(&events, hwnd, UiEvent::Launched { id, result });
        }
        if initialized >= 0 {
            CoUninitialize();
        }
    });
    sender
}

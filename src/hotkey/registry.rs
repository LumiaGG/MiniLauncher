//! Plugin global shortcuts, kept in sync with the enabled plugin catalog.
use super::{unregister, Hotkey, PluginShortcut, Prefill};
use std::collections::HashSet;
use windows_sys::Win32::Foundation::HWND;

struct Binding {
    id: i32,
    plugin: String,
    prefill: Prefill,
    hotkey: String,
}

/// Shortcuts a plugin registered system-wide through the host window.
pub struct Registry {
    hwnd: HWND,
    bindings: Vec<Binding>,
    next_id: i32,
}

impl Registry {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            bindings: Vec::new(),
            next_id: 100,
        }
    }

    pub fn binding(&self, id: i32) -> Option<(&str, Prefill)> {
        self.bindings
            .iter()
            .find(|binding| binding.id == id)
            .map(|binding| (binding.plugin.as_str(), binding.prefill))
    }

    /// Registers every desired shortcut and releases shortcuts that are no
    /// longer declared. A conflict leaves the previous bindings registered and
    /// releases only the shortcuts added by this call.
    pub fn configure(
        &mut self,
        desired: Vec<PluginShortcut>,
        launcher: &str,
    ) -> Result<(), String> {
        let launcher = Hotkey::parse(launcher)?.label;
        let mut seen = HashSet::new();
        for shortcut in &desired {
            let label = &shortcut.hotkey.label;
            if *label == launcher || !seen.insert(label.clone()) {
                return Err(format!("插件全局快捷键 {label} 与其他快捷键冲突"));
            }
        }
        let mut next = Vec::with_capacity(desired.len());
        let mut added = Vec::new();
        for PluginShortcut {
            plugin,
            prefill,
            hotkey,
        } in desired
        {
            if let Some(old) = self
                .bindings
                .iter()
                .find(|binding| binding.hotkey == hotkey.label)
            {
                next.push(Binding {
                    id: old.id,
                    plugin,
                    prefill,
                    hotkey: hotkey.label,
                });
                continue;
            }
            let id = self.next_id;
            self.next_id = if id >= 0xbffe { 100 } else { id + 1 };
            // Safety: the registry only registers for the window it was created with.
            let registered = unsafe { hotkey.register(self.hwnd, id) };
            if !registered {
                for id in added {
                    unsafe { unregister(self.hwnd, id) };
                }
                return Err(format!(
                    "插件 {plugin} 的全局快捷键 {} 被占用，原插件快捷键保持有效",
                    hotkey.label
                ));
            }
            added.push(id);
            next.push(Binding {
                id,
                plugin,
                prefill,
                hotkey: hotkey.label,
            });
        }
        for old in &self.bindings {
            if !next.iter().any(|binding| binding.id == old.id) {
                unsafe { unregister(self.hwnd, old.id) };
            }
        }
        self.bindings = next;
        Ok(())
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        for binding in &self.bindings {
            unsafe { unregister(self.hwnd, binding.id) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn shortcut(plugin: &str, prefill: Prefill, text: &str) -> PluginShortcut {
        PluginShortcut {
            plugin: plugin.into(),
            prefill,
            hotkey: Hotkey::parse(text).unwrap(),
        }
    }

    // A separate thread represents another application owning the combination.
    fn available(text: &str) -> bool {
        let hotkey = Hotkey::parse(text).unwrap();
        std::thread::spawn(move || unsafe {
            let registered = hotkey.register(null_mut(), 1);
            if registered {
                unregister(null_mut(), 1);
            }
            registered
        })
        .join()
        .unwrap()
    }

    #[test]
    fn plugin_shortcuts_roll_back_reassign_and_unregister() {
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("shortcut-test").as_ptr(),
                0,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            assert!(!hwnd.is_null());
            let mut registry = Registry::new(hwnd);
            registry
                .configure(
                    vec![PluginShortcut {
                        plugin: "test".into(),
                        prefill: Prefill::Selection,
                        hotkey: Hotkey::parse("Ctrl+Alt+Shift+F23").unwrap(),
                    }],
                    "Alt+Space",
                )
                .unwrap();
            let id = registry.bindings[0].id;
            assert_eq!(registry.binding(id), Some(("test", Prefill::Selection)));

            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let blocker = std::thread::spawn(move || {
                let hotkey = Hotkey::parse("Ctrl+Alt+Shift+F20").unwrap();
                let registered = hotkey.register(null_mut(), 1);
                ready_tx.send(registered).unwrap();
                let _ = release_rx.recv();
                if registered {
                    unregister(null_mut(), 1);
                }
            });
            assert!(ready_rx.recv().unwrap());
            assert!(registry
                .configure(
                    vec![
                        shortcut("replacement", Prefill::None, "Ctrl+Alt+Shift+F23"),
                        shortcut("new-a", Prefill::None, "Ctrl+Alt+Shift+F22"),
                        shortcut("new-b", Prefill::Selection, "Ctrl+Alt+Shift+F21"),
                        shortcut("blocked", Prefill::None, "Ctrl+Alt+Shift+F20"),
                    ],
                    "Alt+Space"
                )
                .is_err());
            assert_eq!(registry.bindings.len(), 1);
            assert_eq!(registry.binding(id), Some(("test", Prefill::Selection)));
            assert!(!available("Ctrl+Alt+Shift+F23"));
            assert!(available("Ctrl+Alt+Shift+F22"));
            assert!(available("Ctrl+Alt+Shift+F21"));
            drop(release_tx);
            blocker.join().unwrap();

            registry
                .configure(
                    vec![shortcut("replacement", Prefill::None, "Ctrl+Alt+Shift+F23")],
                    "Alt+Space",
                )
                .unwrap();
            assert_eq!(registry.bindings.len(), 1);
            assert_eq!(registry.binding(id), Some(("replacement", Prefill::None)));
            assert!(!available("Ctrl+Alt+Shift+F23"));
            // Empty plugin catalog disables registrations; no optional package required.
            registry.configure(Vec::new(), "Alt+Space").unwrap();
            assert!(registry.binding(id).is_none());
            // The released combination can be registered again by another id.
            let hotkey = Hotkey::parse("Ctrl+Alt+Shift+F23").unwrap();
            assert!(hotkey.register(hwnd, 101));
            unregister(hwnd, 101);

            // Double taps use the same routing, conflict checks and lifecycle.
            registry
                .configure(
                    vec![shortcut("double", Prefill::None, "Ctrl+Ctrl")],
                    "Alt+Space",
                )
                .unwrap();
            let double_id = registry.bindings[0].id;
            assert_eq!(registry.binding(double_id), Some(("double", Prefill::None)));
            assert!(registry
                .configure(
                    vec![shortcut("double", Prefill::None, "Ctrl+Ctrl")],
                    "Ctrl+Ctrl"
                )
                .is_err());
            assert_eq!(registry.binding(double_id), Some(("double", Prefill::None)));
            // Failure after adding a gesture must release the temporary hook binding.
            assert!(Hotkey::parse("Alt+Alt").unwrap().register(hwnd, 102));
            assert!(registry
                .configure(
                    vec![
                        shortcut("new", Prefill::None, "Shift+Shift"),
                        shortcut("collision", Prefill::None, "Alt+Alt"),
                    ],
                    "Alt+Space"
                )
                .is_err());
            assert_eq!(registry.binding(double_id), Some(("double", Prefill::None)));
            assert!(Hotkey::parse("Shift+Shift").unwrap().register(hwnd, 103));
            unregister(hwnd, 103);
            unregister(hwnd, 102);
            // Release and re-add to check last-binding hook teardown as well.
            registry.configure(Vec::new(), "Alt+Space").unwrap();
            registry
                .configure(
                    vec![shortcut("double", Prefill::Selection, "Shift+Shift")],
                    "Win+Win",
                )
                .unwrap();
            registry.configure(Vec::new(), "Alt+Space").unwrap();
            DestroyWindow(hwnd);
        }
    }
}

//! Global combinations and same-key double taps shared by launcher and plugins.

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT},
};

#[cfg(windows)]
mod double_tap;
#[cfg(any(windows, test))]
mod gesture;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
pub use registry::Registry;

/// What a plugin shortcut puts in the search box after opening the plugin scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prefill {
    /// Empty query; pressing the shortcut again hides the launcher.
    #[default]
    None,
    /// Copy the selection of the foreground app and query it.
    Selection,
}

/// A resolved plugin shortcut ready to be registered system-wide.
#[derive(Clone)]
pub struct PluginShortcut {
    pub plugin: String,
    pub prefill: Prefill,
    pub hotkey: Hotkey,
}

/// A parsed shortcut: gesture kind, modifiers, virtual key and canonical label.
#[derive(Clone)]
pub struct Hotkey {
    pub double_tap: bool,
    pub modifiers: u32,
    pub key: u32,
    pub label: String,
}

impl Hotkey {
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<_> = text.split('+').map(str::trim).collect();
        if parts.len() == 2 {
            let single = |text: &str| -> Result<Self, String> {
                let (key, label) = match text.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => (0x11, "Ctrl"),
                    "alt" => (0x12, "Alt"),
                    "shift" => (0x10, "Shift"),
                    "win" | "windows" => (0x5b, "Win"),
                    _ => return Self::parse(text),
                };
                Ok(Self {
                    double_tap: false,
                    modifiers: 0,
                    key,
                    label: label.into(),
                })
            };
            if let (Ok(first), Ok(second)) = (single(parts[0]), single(parts[1])) {
                if first.key == second.key {
                    return Ok(Self {
                        double_tap: true,
                        label: format!("{}+{}", first.label, first.label),
                        ..first
                    });
                }
            }
        }
        if parts.iter().any(|part| part.is_empty()) {
            return Err("快捷键格式应为 Alt+Q 或 Ctrl+Shift+Space".into());
        }
        let mut modifiers = 0;
        for modifier in &parts[..parts.len().saturating_sub(1)] {
            let flag = match modifier.to_ascii_lowercase().as_str() {
                "alt" => 1,
                "ctrl" | "control" => 2,
                "shift" => 4,
                "win" | "windows" => 8,
                _ => {
                    return Err(format!(
                        "未知快捷键修饰键：{modifier}，支持 Ctrl、Alt、Shift、Win"
                    ))
                }
            };
            if modifiers & flag != 0 {
                return Err(format!("快捷键修饰键重复：{modifier}"));
            }
            modifiers |= flag;
        }
        let name = parts.last().unwrap_or(&"").to_ascii_uppercase();
        let (key, label) = match name.as_str() {
            "SPACE" | "空格" => (0x20, "Space".to_string()),
            "ENTER" | "RETURN" => (0x0d, "Enter".into()),
            "TAB" => (0x09, "Tab".into()),
            "ESC" | "ESCAPE" => (0x1b, "Escape".into()),
            "UP" => (0x26, "Up".into()),
            "DOWN" => (0x28, "Down".into()),
            "LEFT" => (0x25, "Left".into()),
            "RIGHT" => (0x27, "Right".into()),
            "HOME" => (0x24, "Home".into()),
            "END" => (0x23, "End".into()),
            "PAGEUP" => (0x21, "PageUp".into()),
            "PAGEDOWN" => (0x22, "PageDown".into()),
            "INSERT" => (0x2d, "Insert".into()),
            "DELETE" | "DEL" => (0x2e, "Delete".into()),
            "BACKSPACE" => (0x08, "Backspace".into()),
            _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => {
                (name.as_bytes()[0] as u32, name.clone())
            }
            _ if name.starts_with('F') => {
                let number = name[1..]
                    .parse::<u32>()
                    .ok()
                    .filter(|number| (1..=24).contains(number))
                    .ok_or("功能键必须为 F1–F24")?;
                (0x70 + number - 1, format!("F{number}"))
            }
            _ => {
                return Err(format!(
                    "不支持的快捷键：{text}，例如 Alt+Q、Ctrl+Shift+Space、Win+F2"
                ))
            }
        };
        let mut labels = Vec::new();
        for (flag, label) in [(2, "Ctrl"), (1, "Alt"), (4, "Shift"), (8, "Win")] {
            if modifiers & flag != 0 {
                labels.push(label.to_string());
            }
        }
        labels.push(label);
        Ok(Self {
            double_tap: false,
            modifiers,
            key,
            label: labels.join("+"),
        })
    }
}

#[cfg(windows)]
impl Hotkey {
    /// Registers a combination or observes double taps on the calling UI thread.
    ///
    /// # Safety
    /// `hwnd` must stay live until [`unregister`] is called on this same thread.
    /// The calling thread must pump messages for double-tap observation.
    pub unsafe fn register(&self, hwnd: HWND, id: i32) -> bool {
        if self.double_tap {
            double_tap::register(hwnd, id, self.key)
        } else {
            RegisterHotKey(hwnd, id, self.modifiers | MOD_NOREPEAT, self.key) != 0
        }
    }
}

/// Releases a registration created by [`Hotkey::register`].
///
/// # Safety
/// `hwnd` must be the window the shortcut was registered for.
#[cfg(windows)]
pub unsafe fn unregister(hwnd: HWND, id: i32) {
    if !double_tap::unregister(hwnd, id) {
        UnregisterHotKey(hwnd, id);
    }
}

/// Cancel a pending gesture when a host input handler consumes keyboard input.
#[cfg(windows)]
pub fn cancel_pending_double_taps() {
    double_tap::cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_hotkey_accepts_user_alt_q_and_modifier_combinations() {
        let hotkey = Hotkey::parse("Alt+q").unwrap();
        assert_eq!(
            (hotkey.modifiers, hotkey.key, hotkey.label.as_str()),
            (1, 0x51, "Alt+Q")
        );
        let hotkey = Hotkey::parse(" control + SHIFT + space ").unwrap();
        assert_eq!(
            (hotkey.modifiers, hotkey.key, hotkey.label.as_str()),
            (6, 0x20, "Ctrl+Shift+Space")
        );
        assert_eq!(Hotkey::parse("win+F24").unwrap().key, 0x87);
    }
    #[test]
    fn config_hotkey_rejects_ambiguous_and_invalid_combinations() {
        for text in ["", "Alt+", "Ctrl+Ctrl+Q", "Alt+F25", "Alt+Potato", "Q+Alt"] {
            assert!(Hotkey::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn double_keys_accept_aliases_and_all_supported_single_keys() {
        for (text, label, key) in [
            (" control + CTRL ", "Ctrl+Ctrl", 0x11),
            ("alt+Alt", "Alt+Alt", 0x12),
            ("shift+SHIFT", "Shift+Shift", 0x10),
            ("windows+win", "Win+Win", 0x5b),
            ("a+A", "A+A", 0x41),
            ("space+空格", "Space+Space", 0x20),
            ("F24+f24", "F24+F24", 0x87),
            ("return+enter", "Enter+Enter", 0x0d),
        ] {
            let parsed = Hotkey::parse(text).unwrap();
            assert!(parsed.double_tap);
            assert_eq!(parsed.label, label);
            assert_eq!(parsed.key, key);
            assert_eq!(Hotkey::parse(label).unwrap().label, label);
        }
        assert!(!Hotkey::parse("Ctrl+Space").unwrap().double_tap);
        for text in ["Ctrl+Ctrl+Q", "A+B", "Ctrl+Alt+Alt", "Ctrl", "++"] {
            assert!(Hotkey::parse(text).is_err(), "{text}");
        }
    }
}

use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED};

/// Keep the apartment alive for the entire tray thread: windows-rs caches
/// activation factories across menu openings, beyond any one UISettings object.
pub struct Apartment(bool);

impl Apartment {
    pub fn new() -> Self {
        Self(unsafe { RoInitialize(RO_INIT_SINGLETHREADED).is_ok() })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

pub fn theme_colors() -> (Option<(u8, u8, u8)>, Option<bool>) {
    let settings = UISettings::new().ok();
    let accent = settings
        .as_ref()
        .and_then(|s| s.GetColorValue(UIColorType::Accent).ok())
        .map(|c| (c.R, c.G, c.B));
    let dark = settings
        .as_ref()
        .and_then(|s| s.GetColorValue(UIColorType::Background).ok())
        .map(|c| (c.R as u32 + c.G as u32 + c.B as u32) < 384);
    (accent, dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_theme_reads_share_the_tray_apartment() {
        std::thread::spawn(|| {
            let _apartment = Apartment::new();
            for _ in 0..8 {
                let (accent, dark) = theme_colors();
                assert!(accent.is_some());
                assert!(dark.is_some());
            }
        })
        .join()
        .unwrap();
    }
}

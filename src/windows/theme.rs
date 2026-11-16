use super::color;
use windows::Win32::System::WinRT::*;
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows_sys::Win32::Graphics::Gdi::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Appearance {
    pub dark: bool,
    pub accent: u32,
    pub backdrop: String,
}

pub(super) struct Changes {
    pub colors: bool,
    pub frame: bool,
    pub fonts: bool,
}

impl Appearance {
    pub fn changes(&self, previous: Option<&Self>, force_frame: bool) -> Changes {
        Changes {
            colors: previous != Some(self),
            frame: force_frame
                || previous
                    .is_none_or(|old| old.dark != self.dark || old.backdrop != self.backdrop),
            fonts: previous.is_none_or(|old| old.dark != self.dark),
        }
    }
}

pub(super) fn accent() -> u32 {
    unsafe {
        let initialized = RoInitialize(RO_INIT_SINGLETHREADED).is_ok();
        // UISettings supplies the actual shell accent. DWM's glass color and
        // cached registry values can differ from the current accent palette.
        let accent = UISettings::new()
            .and_then(|settings| settings.GetColorValue(UIColorType::Accent))
            .ok()
            .map(|accent| color(accent.R, accent.G, accent.B));
        if initialized {
            RoUninitialize();
        }
        accent.unwrap_or_else(|| GetSysColor(COLOR_HIGHLIGHT))
    }
}

pub(super) fn mix(background: u32, accent: u32, percent: u32) -> u32 {
    let channel = |shift: u32| -> u32 {
        let base = (background >> shift) & 255u32;
        let tint = (accent >> shift) & 255u32;
        (base * (100 - percent) + tint * percent + 50) / 100
    };
    channel(0) | channel(8) << 8 | channel(16) << 16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_notifications_and_accent_changes_do_not_rebuild_material_or_fonts() {
        let initial = Appearance {
            dark: true,
            accent: 0x112233,
            backdrop: "mica".into(),
        };
        let first = initial.changes(None, false);
        assert!(first.colors && first.frame && first.fonts);
        for _ in 0..100 {
            let unchanged = initial.changes(Some(&initial), false);
            assert!(!unchanged.colors && !unchanged.frame && !unchanged.fonts);
        }
        let accent = Appearance {
            accent: 0x334455,
            ..initial.clone()
        };
        let changed = accent.changes(Some(&initial), false);
        assert!(changed.colors && !changed.frame && !changed.fonts);
        let backdrop = Appearance {
            backdrop: "acrylic".into(),
            ..initial.clone()
        };
        let changed = backdrop.changes(Some(&initial), false);
        assert!(changed.colors && changed.frame && !changed.fonts);
        let light = Appearance {
            dark: false,
            ..initial.clone()
        };
        let changed = light.changes(Some(&initial), false);
        assert!(changed.colors && changed.frame && changed.fonts);
        let reset = initial.changes(Some(&initial), true);
        assert!(!reset.colors && reset.frame && !reset.fonts);
    }

    #[test]
    fn selection_tracks_the_accent_in_both_themes() {
        use super::super::Palette;
        for dark in [false, true] {
            let red = Palette::new(dark, color(220, 50, 30));
            let green = Palette::new(dark, color(30, 160, 80));
            assert_eq!(red.accent, color(220, 50, 30));
            assert_eq!(green.accent, color(30, 160, 80));
            assert_ne!(red.selection, green.selection);
            assert_ne!(red.selection, red.background);
            assert_eq!(red.text, green.text);
            assert_eq!(red.background, green.background);
            assert!((red.selection & 255) > ((red.selection >> 8) & 255));
            assert!(((green.selection >> 8) & 255) > (green.selection & 255));
        }
    }
}

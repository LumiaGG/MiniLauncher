//! Bounded native text measurements shared by every variable-height result type.
use super::super::*;
use crate::windows::gfx::{FontRole, TextEngine};

const MAX_ENTRIES: usize = 2048;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Style {
    width: i32,
    role: FontRole,
    dpi: u32,
}

#[derive(Default)]
pub(in crate::windows) struct Measurements {
    // Nested maps permit borrowed text lookups without allocating on cache hits.
    styles: HashMap<Style, HashMap<String, i32>>,
    subtitles: HashMap<Style, HashMap<String, String>>,
    entries: usize,
    text_bytes: usize,
    #[cfg(test)]
    pub(crate) measured: usize,
}

impl Measurements {
    pub(in crate::windows) fn clear(&mut self) {
        self.styles.clear();
        self.subtitles.clear();
        self.entries = 0;
        self.text_bytes = 0;
    }

    fn cached(&mut self, style: Style, text: &str, measure: impl FnOnce() -> i32) -> i32 {
        if let Some(height) = self.styles.get(&style).and_then(|texts| texts.get(text)) {
            return *height;
        }
        #[cfg(test)]
        {
            self.measured += 1;
        }
        let height = measure();
        if text.len() > MAX_TEXT_BYTES {
            return height;
        }
        if self.entries >= MAX_ENTRIES || self.text_bytes + text.len() > MAX_TEXT_BYTES {
            self.clear();
        }
        self.styles
            .entry(style)
            .or_default()
            .insert(text.into(), height);
        self.entries += 1;
        self.text_bytes += text.len();
        height
    }

    pub(in crate::windows) fn subtitle(
        &mut self,
        text_engine: &TextEngine,
        text: &str,
        width: i32,
        dpi: u32,
    ) -> String {
        let style = Style { width, role: FontRole::Small, dpi };
        if let Some(value) = self.subtitles.get(&style).and_then(|texts| texts.get(text)) {
            return value.clone();
        }
        let shortened = presentation::compact_path(text, |candidate| {
            text_engine.measure_width(candidate, FontRole::Small)
                .is_ok_and(|measured| measured <= width as f32)
        });
        let bytes = text.len() + shortened.len();
        if bytes <= MAX_TEXT_BYTES {
            if self.entries >= MAX_ENTRIES || self.text_bytes + bytes > MAX_TEXT_BYTES { self.clear(); }
            self.subtitles.entry(style).or_default().insert(text.into(), shortened.clone());
            self.entries += 1;
            self.text_bytes += bytes;
        }
        shortened
    }

    pub(in crate::windows) fn text_height(
        &mut self,
        text_engine: &TextEngine,
        text: &str,
        width: i32,
        role: FontRole,
        dpi: u32,
    ) -> i32 {
        let style = Style { width, role, dpi };
        self.cached(style, text, || {
            let format = text_engine.format_for(role);
            text_engine
                .measure_multiline_height(text, format, width.max(1) as f32)
                .map(|h| h.ceil() as i32)
                .unwrap_or(20)
        })
    }

    pub(in crate::windows) fn panel_height(
        &mut self,
        text_engine: &TextEngine,
        panel: &search::TextPanel,
        width: i32,
        capacity: i32,
        dpi: u32,
    ) -> i32 {
        let geometry = text_panel::Geometry::for_panel(width, 0, capacity, panel);
        let heights = panel
            .blocks
            .iter()
            .zip(&geometry.areas)
            .map(|(block, area)| {
                self.text_height(
                    text_engine,
                    &block.text,
                    (area[2] - area[0] - 14).max(1),
                    FontRole::Body,
                    dpi,
                )
                .max(20)
            })
            .collect::<Vec<_>>();
        // Recompute layout chrome cheaply: state, retry, columns and viewport
        // capacity can change even while the measured text is unchanged.
        text_panel::Geometry::preferred_height(width, capacity, panel, &heights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_heights_preserve_wrapping_and_remeasure_only_changed_blocks() {
        let text_engine = TextEngine::create(true).unwrap();
        let mut cache = Measurements::default();
        let mut panel: search::TextPanel = serde_json::from_value(serde_json::json!({
            "blocks": [
                {"id":"a","title":"One","text":"混合 Unicode 😀 and wrapped text ".repeat(20)},
                {"id":"b","title":"Two","text":"second block\n".repeat(10)}
            ]
        }))
        .unwrap();
        let first = cache.panel_height(&text_engine, &panel, 856, 600, 144);
        assert_eq!(cache.measured, 2);
        assert_eq!(
            cache.panel_height(&text_engine, &panel.clone(), 856, 600, 144),
            first
        );
        assert_eq!(
            cache.measured, 2,
            "freshly deserialized content can reuse measurements"
        );
        panel.blocks[0].text.push_str("\nchanged");
        cache.panel_height(&text_engine, &panel, 856, 600, 144);
        assert_eq!(cache.measured, 3);
        panel.retry = true;
        cache.panel_height(&text_engine, &panel, 856, 300, 144);
        assert_eq!(
            cache.measured, 3,
            "viewport capacity and chrome do not invalidate text metrics"
        );
        for width in [160, 400] {
            let text = &panel.blocks[0].text;
            let measured = cache.text_height(&text_engine, text, width, FontRole::Title, 144);
            let format = text_engine.format_for(FontRole::Title);
            let direct = text_engine
                .measure_multiline_height(text, format, width as f32)
                .map(|h| h.ceil() as i32)
                .unwrap_or(20);
            assert_eq!(measured, direct);
        }
    }

    #[test]
    fn reuse_tracks_text_width_font_dpi_and_font_lifetime() {
        let mut cache = Measurements::default();
        let style = Style {
            width: 200,
            role: FontRole::Body,
            dpi: 96,
        };
        assert_eq!(cache.cached(style, "unchanged", || 20), 20);
        assert_eq!(
            cache.cached(style, "unchanged", || panic!("must reuse")),
            20
        );
        for (style, text) in [
            (style, "changed"),
            (
                Style {
                    width: 100,
                    ..style
                },
                "unchanged",
            ),
            (Style { role: FontRole::Title, ..style }, "unchanged"),
            (Style { dpi: 144, ..style }, "unchanged"),
        ] {
            assert_eq!(cache.cached(style, text, || 40), 40);
        }
        assert_eq!(cache.measured, 5);
        cache.clear(); // Font recreation can recycle the same native handle.
        assert_eq!(cache.cached(style, "unchanged", || 60), 60);
    }

    #[test]
    fn cache_is_bounded_and_oversized_text_preserves_existing_entries() {
        let mut cache = Measurements::default();
        let style = Style {
            width: 200,
            role: FontRole::Body,
            dpi: 96,
        };
        for n in 0..=MAX_ENTRIES {
            cache.cached(style, &n.to_string(), || 20);
        }
        assert!(cache.entries <= MAX_ENTRIES);
        let large = "x".repeat(MAX_TEXT_BYTES / 2 + 1);
        cache.cached(style, &large, || 20);
        cache.cached(
            Style {
                width: 100,
                ..style
            },
            &large,
            || 30,
        );
        assert!(cache.text_bytes <= MAX_TEXT_BYTES);
        let entries = cache.entries;
        cache.cached(style, &"x".repeat(MAX_TEXT_BYTES + 1), || 40);
        assert_eq!(cache.entries, entries);
    }
}

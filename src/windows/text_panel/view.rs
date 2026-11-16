use crate::windows::gfx::TextEngine;
use crate::windows::Palette;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

pub fn d2d_color(c: u32, alpha: f32) -> D2D1_COLOR_F {
    crate::windows::gfx::colorref_to_d2d(c, alpha)
}

pub struct TextView {
    pub key: String,
    pub block_index: usize,
    pub block_id: String,
    pub text: String,
    pub layout: IDWriteTextLayout,
    pub scroll_y: f32,
    pub content_height: f32,
    pub selection: Option<std::ops::Range<usize>>,
    pub rect: D2D_RECT_F,
    pub(crate) drag_anchor: Option<usize>,
}

impl TextView {
    pub fn new(key: String, block_index: usize, text: String, layout: IDWriteTextLayout) -> Self {
        let mut metrics = DWRITE_TEXT_METRICS::default();
        let content_height = unsafe {
            if layout.GetMetrics(&mut metrics).is_ok() {
                metrics.height
            } else {
                0.0
            }
        };
        Self {
            key,
            block_index,
            block_id: String::new(),
            text,
            layout,
            scroll_y: 0.0,
            content_height,
            selection: None,
            rect: D2D_RECT_F::default(),
            drag_anchor: None,
        }
    }

    pub fn create(
        key: String,
        block_index: usize,
        text: String,
        width: f32,
        engine: &TextEngine,
    ) -> windows_core::Result<Self> {
        let layout = engine.create_layout(&text, &engine.body_format, width.max(1.0), f32::MAX)?;
        Ok(Self::new(key, block_index, text, layout))
    }

    pub fn preferred_height(&self) -> f32 {
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            if self.layout.GetMetrics(&mut metrics).is_ok() {
                metrics.height
            } else {
                self.content_height
            }
        }
    }

    pub fn scroll_by(&mut self, delta: f32, viewport_height: f32) {
        let max_scroll = (self.content_height - viewport_height).max(0.0);
        self.scroll_y = (self.scroll_y + delta).clamp(0.0, max_scroll);
    }

    pub fn hit_test(&self, x: f32, y: f32) -> usize {
        if self.text.is_empty() {
            return 0;
        }
        let mut trailing = windows_core::BOOL::default();
        let mut inside = windows_core::BOOL::default();
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            if self
                .layout
                .HitTestPoint(x, y, &mut trailing, &mut inside, &mut metrics)
                .is_ok()
            {
                if trailing.as_bool() {
                    (metrics.textPosition + metrics.length) as usize
                } else {
                    metrics.textPosition as usize
                }
            } else {
                0
            }
        }
    }

    pub fn handle_mouse_down(&mut self, x: f32, y: f32) {
        if self.text.is_empty() {
            self.selection = None;
            self.drag_anchor = None;
            return;
        }
        let pos = self.hit_test(x, y + self.scroll_y);
        self.drag_anchor = Some(pos);
        self.selection = None;
    }

    pub fn handle_mouse_move(&mut self, x: f32, y: f32) {
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        if self.text.is_empty() {
            return;
        }
        let pos = self.hit_test(x, y + self.scroll_y);
        let start = anchor.min(pos);
        let end = anchor.max(pos);
        if start == end {
            self.selection = None;
        } else {
            self.selection = Some(start..end);
        }
    }

    pub fn handle_mouse_up(&mut self, x: f32, y: f32) {
        self.handle_mouse_move(x, y);
        self.drag_anchor = None;
    }

    pub fn select_all(&mut self) {
        let len = self.text.encode_utf16().count();
        self.selection = if len > 0 { Some(0..len) } else { None };
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.drag_anchor = None;
    }

    pub fn has_selection(&self) -> bool {
        self.selection.as_ref().is_some_and(|r| !r.is_empty())
    }

    pub fn copy_selection(&self) -> Option<String> {
        let range = self.selection.as_ref()?;
        if range.is_empty() {
            return None;
        }
        let u16_chars: Vec<u16> = self.text.encode_utf16().collect();
        let start = range.start.min(u16_chars.len());
        let end = range.end.min(u16_chars.len());
        if start >= end {
            return None;
        }
        String::from_utf16(&u16_chars[start..end]).ok()
    }

    pub fn set_text(
        &mut self,
        text: String,
        width: f32,
        engine: &TextEngine,
    ) -> windows_core::Result<()> {
        let layout = engine.create_layout(&text, &engine.body_format, width.max(1.0), f32::MAX)?;
        // DirectWrite selection offsets are UTF-16 units, not UTF-8 bytes.
        // Keep a selection only if its text still exists at the same offsets.
        self.selection = self.selection.take().filter(|range| {
            !range.is_empty()
                && text.encode_utf16().count() >= range.end
                && self
                    .text
                    .encode_utf16()
                    .skip(range.start)
                    .take(range.len())
                    .eq(text.encode_utf16().skip(range.start).take(range.len()))
        });
        self.drag_anchor = None;
        self.layout = layout;
        self.text = text;
        self.content_height = self.preferred_height();
        self.scroll_by(0.0, self.rect.bottom - self.rect.top);
        Ok(())
    }

    pub fn set_width(&mut self, width: f32) -> windows_core::Result<()> {
        unsafe {
            self.layout.SetMaxWidth(width.max(1.0))?;
        }
        self.content_height = self.preferred_height();
        Ok(())
    }

    pub unsafe fn draw(
        &self,
        context: &ID2D1DeviceContext,
        rect: &D2D_RECT_F,
        palette: Palette,
    ) -> windows_core::Result<()> {
        let _clip = crate::windows::gfx::shapes::ClipGuard::new(context, rect);

        if let Some(range) = &self.selection {
            if !range.is_empty() {
                let metrics = crate::windows::gfx::text::hit_test_range(
                    &self.layout,
                    range.start as u32,
                    range.len() as u32,
                    rect.left,
                    rect.top - self.scroll_y,
                )?;
                let brush = crate::windows::gfx::GfxDevice::solid_brush(
                    context,
                    d2d_color(palette.selection, 1.0),
                )?;
                for hit in metrics {
                    context.FillRectangle(
                        &D2D_RECT_F {
                            left: hit.left,
                            top: hit.top,
                            right: hit.left + hit.width,
                            bottom: hit.top + hit.height,
                        },
                        &brush,
                    );
                }
            }
        }

        // Direct2D text drawing
        let text_color = d2d_color(palette.text, 1.0);
        let text_brush = crate::windows::gfx::GfxDevice::solid_brush(context, text_color)?;
        let origin = windows_numerics::Vector2 {
            X: rect.left,
            Y: rect.top - self.scroll_y,
        };
        context.DrawTextLayout(
            origin,
            &self.layout,
            &text_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );

        // Draw custom scrollbar thumb if content_height > viewport_height
        let viewport_height = rect.bottom - rect.top;
        if self.content_height > viewport_height && viewport_height > 1.0 {
            let scroll_range = (self.content_height - viewport_height).max(1.0);
            let thumb_height = ((viewport_height / self.content_height) * viewport_height)
                .clamp(1.0, viewport_height)
                .min(viewport_height);
            let thumb_height = if viewport_height >= 16.0 {
                thumb_height.clamp(16.0, viewport_height)
            } else {
                thumb_height
            };
            let available_track = (viewport_height - thumb_height).max(0.0);
            let thumb_top = rect.top + (self.scroll_y / scroll_range) * available_track;
            let thumb_width = 4.0;
            let thumb_margin = 2.0;
            let thumb_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: rect.right - thumb_width - thumb_margin,
                    top: thumb_top,
                    right: rect.right - thumb_margin,
                    bottom: thumb_top + thumb_height,
                },
                radiusX: 2.0,
                radiusY: 2.0,
            };
            let thumb_color = d2d_color(palette.secondary, 0.6);
            let thumb_brush = crate::windows::gfx::GfxDevice::solid_brush(context, thumb_color)?;
            context.FillRoundedRectangle(&thumb_rect, &thumb_brush);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_view_preferred_height_and_scroll() {
        let text = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8";
        let mut view = TextView::create(
            "test_key".into(),
            0,
            text.into(),
            300.0,
            &TextEngine::create(true).unwrap(),
        )
        .unwrap();

        assert!(view.preferred_height() > 50.0);
        assert_eq!(view.preferred_height(), view.content_height);

        // scroll_by with viewport smaller than content
        view.scroll_by(20.0, 50.0);
        assert_eq!(view.scroll_y, 20.0);

        // Clamp at max_scroll
        view.scroll_by(10000.0, 50.0);
        assert_eq!(view.scroll_y, view.content_height - 50.0);

        // Scroll back up and clamp at 0
        view.scroll_by(-10000.0, 50.0);
        assert_eq!(view.scroll_y, 0.0);
    }

    #[test]
    fn text_view_drag_selection_and_copy() {
        let text = "Hello 😀 world!";
        let mut view = TextView::create(
            "test_key".into(),
            0,
            text.into(),
            300.0,
            &TextEngine::create(true).unwrap(),
        )
        .unwrap();

        assert!(view.copy_selection().is_none());

        // Select all
        view.select_all();
        assert_eq!(view.copy_selection(), Some(text.into()));

        // Clear selection
        view.clear_selection();
        assert!(view.copy_selection().is_none());

        // Mouse drag selection
        view.handle_mouse_down(0.0, 5.0);
        view.handle_mouse_move(40.0, 5.0);
        view.handle_mouse_up(40.0, 5.0);

        assert!(view.has_selection());
        let copied = view.copy_selection().unwrap();
        assert!(!copied.is_empty());
        assert!(text.starts_with(&copied));
    }

    #[test]
    fn updates_preserve_valid_utf16_selection_and_scroll_but_clear_replaced_text() {
        let engine = TextEngine::create(true).unwrap();
        let original = format!("前缀😀固定内容\n{}", "正文 line\n".repeat(80));
        let mut view =
            TextView::create("stable".into(), 0, original.clone(), 300.0, &engine).unwrap();
        view.rect.bottom = 100.0;
        view.selection = Some(2..4); // The emoji occupies two UTF-16 units.
        view.scroll_by(50.0, 100.0);
        view.set_text(format!("{original}新增行\n"), 300.0, &engine)
            .unwrap();
        assert_eq!(view.selection, Some(2..4));
        assert_eq!(view.copy_selection().as_deref(), Some("😀"));
        assert_eq!(view.scroll_y, 50.0);

        // Unselected text can change without invalidating the selected range.
        view.set_text(original.replacen("前缀", "说明", 1), 300.0, &engine)
            .unwrap();
        assert_eq!(view.copy_selection().as_deref(), Some("😀"));
        view.set_text(original.replace('😀', "😃"), 300.0, &engine)
            .unwrap();
        assert!(view.selection.is_none());
        assert_eq!(view.scroll_y, 50.0);

        view.select_all();
        view.set_text("短".into(), 300.0, &engine).unwrap();
        assert!(view.selection.is_none());
        assert_eq!(view.scroll_y, 0.0);
        view.select_all();
        view.set_text(String::new(), 300.0, &engine).unwrap();
        view.select_all();
        assert!(view.copy_selection().is_none());
        assert!(view.text.is_empty());
    }
}

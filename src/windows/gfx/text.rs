use std::{cell::RefCell, collections::HashMap, rc::Rc};
use windows::core::{Result, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

/// DirectWrite reports the required capacity with an expected insufficient-buffer result.
pub fn hit_test_range(
    layout: &IDWriteTextLayout,
    start: u32,
    length: u32,
    x: f32,
    y: f32,
) -> Result<Vec<DWRITE_HIT_TEST_METRICS>> {
    unsafe {
        let mut count = 0;
        if let Err(error) = layout.HitTestTextRange(start, length, x, y, None, &mut count) {
            if error.code()
                != windows::core::HRESULT::from_win32(
                    windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER.0,
                )
            {
                return Err(error);
            }
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut metrics = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        layout.HitTestTextRange(start, length, x, y, Some(&mut metrics), &mut count)?;
        metrics.truncate(count as usize);
        Ok(metrics)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    Input,
    Title,
    Body,
    Small,
    Symbol,
    SymbolSmall,
}

const MAX_DRAW_LAYOUTS: usize = 256;
const MAX_DRAW_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct DrawStyle {
    role: FontRole,
    width: u32,
    height: u32,
    horizontal: i32,
    vertical: i32,
    wrapping: i32,
}

#[derive(Default)]
struct DrawLayouts {
    styles: HashMap<DrawStyle, HashMap<String, IDWriteTextLayout>>,
    entries: usize,
    text_bytes: usize,
}

#[derive(Clone)]
pub struct TextEngine {
    pub factory: IDWriteFactory,
    pub input_format: IDWriteTextFormat,
    pub title_format: IDWriteTextFormat,
    pub body_format: IDWriteTextFormat,
    pub small_format: IDWriteTextFormat,
    pub symbol_format: IDWriteTextFormat,
    pub symbol_small_format: IDWriteTextFormat,
    draw_layouts: Rc<RefCell<DrawLayouts>>,
}

fn resolve_symbol_font(factory: &IDWriteFactory) -> HSTRING {
    unsafe {
        let mut collection: Option<IDWriteFontCollection> = None;
        if factory.GetSystemFontCollection(&mut collection, false).is_ok() {
            if let Some(collection) = collection {
                let fluent = HSTRING::from("Segoe Fluent Icons");
                let mut index = 0;
                let mut exists = windows::core::BOOL(0);
                if collection
                    .FindFamilyName(
                        windows::core::PCWSTR(fluent.as_ptr()),
                        &mut index,
                        &mut exists,
                    )
                    .is_ok()
                    && exists.as_bool()
                {
                    return fluent;
                }
            }
        }
        HSTRING::from("Segoe MDL2 Assets")
    }
}

impl TextEngine {
    #[cfg(test)]
    pub fn create(dark_mode: bool) -> Result<Self> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Self::new(&factory, dark_mode)
        }
    }

    pub fn new(factory: &IDWriteFactory, dark_mode: bool) -> Result<Self> {
        unsafe {
            let yahei = HSTRING::from("Microsoft YaHei UI");
            let yahei_ptr = windows::core::PCWSTR(yahei.as_ptr());
            let symbol_font = resolve_symbol_font(factory);
            let symbol_ptr = windows::core::PCWSTR(symbol_font.as_ptr());
            let locale = HSTRING::from("zh-CN");
            let locale_ptr = windows::core::PCWSTR(locale.as_ptr());

            // Light mode has slightly higher weights for contrast against bright backdrops
            let title_weight = if dark_mode {
                DWRITE_FONT_WEIGHT_NORMAL
            } else {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            };
            let body_weight = if dark_mode {
                DWRITE_FONT_WEIGHT_NORMAL
            } else {
                DWRITE_FONT_WEIGHT_MEDIUM
            };

            let create_format = |font, weight, size| {
                factory.CreateTextFormat(
                    font,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    locale_ptr,
                )
            };
            let input_format = create_format(yahei_ptr, body_weight, 16.0)?;
            // Share vertical alignment across editor text, placeholder, and hit testing.
            input_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let title_format = create_format(yahei_ptr, title_weight, 16.0)?;
            let body_format = create_format(yahei_ptr, body_weight, 14.0)?;
            let small_format = create_format(yahei_ptr, body_weight, 13.0)?;
            let symbol_format = create_format(symbol_ptr, DWRITE_FONT_WEIGHT_NORMAL, 19.0)?;
            let symbol_small_format = create_format(symbol_ptr, DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;

            Ok(Self {
                factory: factory.clone(),
                input_format,
                title_format,
                body_format,
                small_format,
                symbol_format,
                symbol_small_format,
                draw_layouts: Rc::new(RefCell::new(DrawLayouts::default())),
            })
        }
    }

    pub fn format_for(&self, role: FontRole) -> &IDWriteTextFormat {
        match role {
            FontRole::Input => &self.input_format,
            FontRole::Title => &self.title_format,
            FontRole::Body => &self.body_format,
            FontRole::Small => &self.small_format,
            FontRole::Symbol => &self.symbol_format,
            FontRole::SymbolSmall => &self.symbol_small_format,
        }
    }

    pub fn measure_width(&self, text: &str, role: FontRole) -> Result<f32> {
        let layout = self.draw_layout(text, DrawStyle {
            role,
            width: f32::MAX.to_bits(),
            height: f32::MAX.to_bits(),
            horizontal: DWRITE_TEXT_ALIGNMENT_LEADING.0,
            vertical: DWRITE_PARAGRAPH_ALIGNMENT_NEAR.0,
            wrapping: DWRITE_WORD_WRAPPING_NO_WRAP.0,
        })?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics)?;
        }
        Ok(metrics.widthIncludingTrailingWhitespace)
    }

    pub fn create_layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_width: f32,
        max_height: f32,
    ) -> Result<IDWriteTextLayout> {
        let text_utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.factory.CreateTextLayout(
                &text_utf16,
                format,
                max_width.max(0.0),
                max_height.max(0.0),
            )?;
            Ok(layout)
        }
    }

    pub fn measure_multiline_height(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_width: f32,
    ) -> Result<f32> {
        let layout = self.create_layout(text, format, max_width, 10000.0)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics)?;
        }
        Ok(metrics.height)
    }

    // Cached layouts are private and immutable once configured. Public create_layout
    // still returns a fresh layout for editors and panels that modify their formatting.
    fn draw_layout(&self, text: &str, style: DrawStyle) -> Result<IDWriteTextLayout> {
        if let Some(layout) = self
            .draw_layouts
            .borrow()
            .styles
            .get(&style)
            .and_then(|texts| texts.get(text))
        {
            return Ok(layout.clone());
        }
        let format = self.format_for(style.role);
        let layout = self.create_layout(
            text,
            format,
            f32::from_bits(style.width),
            f32::from_bits(style.height),
        )?;
        unsafe {
            layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT(style.horizontal))?;
            layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT(style.vertical))?;
            layout.SetWordWrapping(DWRITE_WORD_WRAPPING(style.wrapping))?;
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let ellipsis = self.factory.CreateEllipsisTrimmingSign(format)?;
            layout.SetTrimming(&trimming, Some(&ellipsis))?;
        }
        if text.len() <= MAX_DRAW_TEXT_BYTES {
            let mut cache = self.draw_layouts.borrow_mut();
            if cache.entries >= MAX_DRAW_LAYOUTS
                || cache.text_bytes + text.len() > MAX_DRAW_TEXT_BYTES
            {
                *cache = DrawLayouts::default();
            }
            cache
                .styles
                .entry(style)
                .or_default()
                .insert(text.into(), layout.clone());
            cache.entries += 1;
            cache.text_bytes += text.len();
        }
        Ok(layout)
    }

    unsafe fn draw_text(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        role: FontRole,
        rect: &D2D_RECT_F,
        color: D2D1_COLOR_F,
        h_align: DWRITE_TEXT_ALIGNMENT,
        v_align: DWRITE_PARAGRAPH_ALIGNMENT,
        wrapping: DWRITE_WORD_WRAPPING,
    ) -> Result<()> {
        let style = DrawStyle {
            role,
            width: (rect.right - rect.left).max(0.0).to_bits(),
            height: (rect.bottom - rect.top).max(0.0).to_bits(),
            horizontal: h_align.0,
            vertical: v_align.0,
            wrapping: wrapping.0,
        };
        let layout = self.draw_layout(text, style)?;
        let origin = Self::draw_origin(&layout, role, rect, h_align, v_align)?;
        let brush = crate::windows::gfx::GfxDevice::solid_brush(target, color)?;
        target.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        Ok(())
    }

    fn draw_origin(
        layout: &IDWriteTextLayout,
        role: FontRole,
        rect: &D2D_RECT_F,
        h_align: DWRITE_TEXT_ALIGNMENT,
        v_align: DWRITE_PARAGRAPH_ALIGNMENT,
    ) -> Result<windows_numerics::Vector2> {
        let mut origin = windows_numerics::Vector2 {
            X: rect.left,
            Y: rect.top,
        };
        if matches!(role, FontRole::Symbol | FontRole::SymbolSmall) {
            // Center the visible glyph, excluding font side bearings and line spacing.
            let overhang = unsafe { layout.GetOverhangMetrics()? };
            if h_align == DWRITE_TEXT_ALIGNMENT_CENTER {
                origin.X += (overhang.left - overhang.right) / 2.0;
            }
            if v_align == DWRITE_PARAGRAPH_ALIGNMENT_CENTER {
                origin.Y += (overhang.top - overhang.bottom) / 2.0;
            }
        }
        Ok(origin)
    }

    pub unsafe fn draw_text_role(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        role: FontRole,
        rect: &D2D_RECT_F,
        color: D2D1_COLOR_F,
        h_align: DWRITE_TEXT_ALIGNMENT,
        v_align: DWRITE_PARAGRAPH_ALIGNMENT,
    ) -> Result<()> {
        self.draw_text(
            target,
            text,
            role,
            rect,
            color,
            h_align,
            v_align,
            DWRITE_WORD_WRAPPING_NO_WRAP,
        )
    }

    pub unsafe fn draw_highlighted_title(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        rect: &D2D_RECT_F,
        color: D2D1_COLOR_F,
        highlight: D2D1_COLOR_F,
        ranges: &[std::ops::Range<u32>],
        wrapped: bool,
    ) -> Result<()> {
        let layout = self.draw_layout(text, DrawStyle {
            role: FontRole::Title,
            width: (rect.right - rect.left).max(0.0).to_bits(),
            height: (rect.bottom - rect.top).max(0.0).to_bits(),
            horizontal: DWRITE_TEXT_ALIGNMENT_LEADING.0,
            vertical: if wrapped { DWRITE_PARAGRAPH_ALIGNMENT_NEAR.0 } else { DWRITE_PARAGRAPH_ALIGNMENT_CENTER.0 },
            wrapping: if wrapped { DWRITE_WORD_WRAPPING_WRAP.0 } else { DWRITE_WORD_WRAPPING_NO_WRAP.0 },
        })?;
        let _clip = super::shapes::ClipGuard::new(target, rect);
        for range in ranges {
            for hit in hit_test_range(&layout, range.start, range.end - range.start, rect.left, rect.top)? {
                if hit.isTrimmed.as_bool() { continue; }
                super::fill_round_rect(target, &D2D_RECT_F {
                    left: hit.left, top: hit.top, right: hit.left + hit.width, bottom: hit.top + hit.height,
                }, 2.0, highlight)?;
            }
        }
        let brush = super::GfxDevice::solid_brush(target, color)?;
        target.DrawTextLayout(windows_numerics::Vector2 { X: rect.left, Y: rect.top }, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        Ok(())
    }

    pub unsafe fn draw_text_wrapped(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        role: FontRole,
        rect: &D2D_RECT_F,
        color: D2D1_COLOR_F,
        h_align: DWRITE_TEXT_ALIGNMENT,
        v_align: DWRITE_PARAGRAPH_ALIGNMENT,
    ) -> Result<()> {
        self.draw_text(
            target,
            text,
            role,
            rect,
            color,
            h_align,
            v_align,
            DWRITE_WORD_WRAPPING_WRAP,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::Interface;

    fn style() -> DrawStyle {
        DrawStyle {
            role: FontRole::Body,
            width: 200.0f32.to_bits(),
            height: 60.0f32.to_bits(),
            horizontal: DWRITE_TEXT_ALIGNMENT_LEADING.0,
            vertical: DWRITE_PARAGRAPH_ALIGNMENT_NEAR.0,
            wrapping: DWRITE_WORD_WRAPPING_NO_WRAP.0,
        }
    }

    #[test]
    fn draw_layouts_reuse_native_objects_and_isolate_formatting() {
        let engine = TextEngine::create(true).unwrap();
        let text = "中文😀 wrapped text ".repeat(8);
        let first = engine.draw_layout(&text, style()).unwrap();
        let reused = engine.clone().draw_layout(&text, style()).unwrap();
        assert_eq!(first.as_raw(), reused.as_raw());
        for changed in [
            DrawStyle {
                role: FontRole::Title,
                ..style()
            },
            DrawStyle {
                width: 100.0f32.to_bits(),
                ..style()
            },
            DrawStyle {
                height: 30.0f32.to_bits(),
                ..style()
            },
            DrawStyle {
                horizontal: DWRITE_TEXT_ALIGNMENT_CENTER.0,
                ..style()
            },
            DrawStyle {
                vertical: DWRITE_PARAGRAPH_ALIGNMENT_CENTER.0,
                ..style()
            },
            DrawStyle {
                wrapping: DWRITE_WORD_WRAPPING_WRAP.0,
                ..style()
            },
        ] {
            let layout = engine.draw_layout(&text, changed).unwrap();
            assert_ne!(first.as_raw(), layout.as_raw());
            unsafe {
                assert_eq!(layout.GetTextAlignment().0, changed.horizontal);
                assert_eq!(layout.GetParagraphAlignment().0, changed.vertical);
                assert_eq!(layout.GetWordWrapping().0, changed.wrapping);
            }
        }
        let independent = engine
            .create_layout(&text, engine.format_for(FontRole::Body), 200.0, 60.0)
            .unwrap();
        assert_ne!(first.as_raw(), independent.as_raw());
        let themed = TextEngine::create(false).unwrap();
        assert_ne!(
            first.as_raw(),
            themed.draw_layout(&text, style()).unwrap().as_raw()
        );
    }

    #[test]
    fn draw_cache_bounds_entries_and_retained_text() {
        let engine = TextEngine::create(true).unwrap();
        for i in 0..=MAX_DRAW_LAYOUTS {
            engine.draw_layout(&format!("row {i}"), style()).unwrap();
        }
        assert_eq!(engine.draw_layouts.borrow().entries, 1);
        let large = "x".repeat(MAX_DRAW_TEXT_BYTES / 2 + 1);
        engine.draw_layout(&large, style()).unwrap();
        engine
            .draw_layout(
                &large,
                DrawStyle {
                    role: FontRole::Title,
                    ..style()
                },
            )
            .unwrap();
        let cache = engine.draw_layouts.borrow();
        assert_eq!(cache.entries, 1);
        assert!(cache.text_bytes <= MAX_DRAW_TEXT_BYTES);
    }
}

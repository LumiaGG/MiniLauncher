use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectWrite::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitTestPointResult {
    pub text_position: usize,
    pub is_trailing: bool,
    pub is_inside: bool,
}

/// Manages DirectWrite `IDWriteTextLayout` and hit-testing for the text editor.
pub struct EditorLayout {
    factory: IDWriteFactory,
    format: IDWriteTextFormat,
    layout: Option<IDWriteTextLayout>,
    default_line_height: f32,
    display_text: String,
    composition: Option<(usize, usize)>,
    size: Option<(f32, f32)>,
}

impl EditorLayout {
    pub fn new(factory: &IDWriteFactory, format: &IDWriteTextFormat) -> Self {
        // Measure default line height using format metrics
        let mut default_line_height = 20.0;
        let sample = [0x41u16]; // 'A'
        if let Ok(temp_layout) =
            unsafe { factory.CreateTextLayout(&sample, format, 1000.0, 1000.0) }
        {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if unsafe { temp_layout.GetMetrics(&mut metrics) }.is_ok() && metrics.height > 0.0 {
                default_line_height = metrics.height;
            }
        }

        Self {
            factory: factory.clone(),
            format: format.clone(),
            layout: None,
            default_line_height,
            display_text: String::new(),
            composition: None,
            size: None,
        }
    }

    #[inline]
    pub fn raw_layout(&self) -> Option<&IDWriteTextLayout> {
        self.layout.as_ref()
    }

    #[inline]
    pub fn default_line_height(&self) -> f32 {
        self.default_line_height
    }

    /// Rebuilds the DirectWrite text layout.
    /// If `comp_range` is Some((start_u16, len_u16)), underlines the composition text.
    pub fn update(
        &mut self,
        display_text: &str,
        comp_range: Option<(usize, usize)>,
        max_width: f32,
        max_height: f32,
    ) -> Result<()> {
        let size = (max_width.max(1.0), max_height.max(1.0));
        if self.size == Some(size)
            && self.composition == comp_range
            && self.display_text == display_text
        {
            return Ok(());
        }
        let utf16: Vec<u16> = display_text.encode_utf16().collect();
        unsafe {
            let layout = self
                .factory
                .CreateTextLayout(&utf16, &self.format, size.0, size.1)?;

            // If composition string is present, apply underline
            if let Some((start, len)) = comp_range {
                if len > 0 {
                    let range = DWRITE_TEXT_RANGE {
                        startPosition: start as u32,
                        length: len as u32,
                    };
                    layout.SetUnderline(true, range)?;
                }
            }

            self.layout = Some(layout);
            self.display_text.clear();
            self.display_text.push_str(display_text);
            self.composition = comp_range;
            self.size = Some(size);
        }
        Ok(())
    }

    /// Hit-tests a point (relative to text layout top-left) for mouse clicks.
    pub fn hit_test_point(&self, point_x: f32, point_y: f32) -> Result<HitTestPointResult> {
        let layout = match &self.layout {
            Some(l) => l,
            None => {
                return Ok(HitTestPointResult {
                    text_position: 0,
                    is_trailing: false,
                    is_inside: false,
                })
            }
        };

        let mut is_trailing_hit = windows::core::BOOL(0);
        let mut is_inside = windows::core::BOOL(0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();

        unsafe {
            layout.HitTestPoint(
                point_x.max(0.0),
                point_y.max(0.0),
                &mut is_trailing_hit,
                &mut is_inside,
                &mut metrics,
            )?;
        }

        let offset = if is_trailing_hit.as_bool() {
            metrics.textPosition as usize + metrics.length as usize
        } else {
            metrics.textPosition as usize
        };

        Ok(HitTestPointResult {
            text_position: offset,
            is_trailing: is_trailing_hit.as_bool(),
            is_inside: is_inside.as_bool(),
        })
    }

    /// Hit-tests a UTF-16 text position to obtain the pixel bounds of the caret.
    pub fn hit_test_text_position(&self, text_position: usize) -> Result<CaretBounds> {
        let layout = match &self.layout {
            Some(l) => l,
            None => {
                return Ok(CaretBounds {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: self.default_line_height,
                })
            }
        };

        let mut point_x = 0.0f32;
        let mut point_y = 0.0f32;
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();

        unsafe {
            layout.HitTestTextPosition(
                text_position as u32,
                false,
                &mut point_x,
                &mut point_y,
                &mut metrics,
            )?;
        }

        let height = if metrics.height > 0.0 {
            metrics.height
        } else {
            self.default_line_height
        };

        Ok(CaretBounds {
            x: point_x,
            y: point_y,
            width: metrics.width,
            height,
        })
    }

    /// Hit-tests a UTF-16 text range to obtain selection highlight rectangles.
    pub fn hit_test_text_range(&self, start: usize, length: usize) -> Result<Vec<D2D_RECT_F>> {
        if length == 0 {
            return Ok(Vec::new());
        }

        let layout = match &self.layout {
            Some(l) => l,
            None => return Ok(Vec::new()),
        };

        let metrics = crate::windows::gfx::text::hit_test_range(
            layout,
            start as u32,
            length as u32,
            0.0,
            0.0,
        )?;
        Ok(metrics
            .into_iter()
            .map(|hit| D2D_RECT_F {
                left: hit.left,
                top: hit.top,
                right: hit.left + hit.width,
                bottom: hit.top + hit.height,
            })
            .collect())
    }

    /// Retrieves overall text metrics (width, height, line count, etc.).
    pub fn get_metrics(&self) -> Result<DWRITE_TEXT_METRICS> {
        if let Some(layout) = &self.layout {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            unsafe {
                layout.GetMetrics(&mut metrics)?;
            }
            Ok(metrics)
        } else {
            Ok(DWRITE_TEXT_METRICS::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::gfx::{FontRole, TextEngine};
    use windows::core::Interface;

    #[test]
    fn unchanged_layout_reuses_native_text_and_composition_and_size_invalidate_it() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = EditorLayout::new(&engine.factory, engine.format_for(FontRole::Input));
        editor.update("中文😀", None, 400.0, 80.0).unwrap();
        let first = editor.raw_layout().unwrap().clone();
        editor.update("中文😀", None, 400.0, 80.0).unwrap();
        assert_eq!(first.as_raw(), editor.raw_layout().unwrap().as_raw());
        let mut previous = first;
        for (text, composition, width, height) in [
            ("中文😀x", None, 400.0, 80.0),
            ("中文😀x", Some((2, 2)), 400.0, 80.0),
            ("中文😀x", None, 400.0, 80.0),
            ("中文😀x", None, 200.0, 80.0),
            ("中文😀x", None, 200.0, 40.0),
        ] {
            editor.update(text, composition, width, height).unwrap();
            let current = editor.raw_layout().unwrap().clone();
            assert_ne!(previous.as_raw(), current.as_raw());
            editor.update(text, composition, width, height).unwrap();
            assert_eq!(current.as_raw(), editor.raw_layout().unwrap().as_raw());
            previous = current;
        }
    }
}

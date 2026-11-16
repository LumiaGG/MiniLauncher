use super::ime::ImeHandler;
use super::layout::EditorLayout;
use super::model::TextModel;
use super::selection::Selection;
use crate::windows::gfx::colorref_to_d2d;
use crate::windows::Palette;
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows_numerics::Vector2;

pub struct EditorView;

impl EditorView {
    pub unsafe fn draw(
        context: &ID2D1DeviceContext,
        bounds: D2D_RECT_F,
        palette: &Palette,
        model: &TextModel,
        selection: &Selection,
        layout: &EditorLayout,
        ime: &ImeHandler,
        placeholder_layout: Option<&IDWriteTextLayout>,
        is_focused: bool,
        show_caret: bool,
    ) -> Result<()> {
        // Clip drawing strictly to the editor bounds
        let _clip = crate::windows::gfx::shapes::ClipGuard::new(context, &bounds);

        let origin = Vector2 {
            X: bounds.left,
            Y: bounds.top,
        };

        // 1. Draw placeholder if text is empty and not composing
        if model.is_empty() && !ime.is_composing() {
            if let Some(placeholder) = placeholder_layout {
                let hint_brush = crate::windows::gfx::GfxDevice::solid_brush(
                    context,
                    colorref_to_d2d(palette.hint, 1.0),
                )?;
                context.DrawTextLayout(
                    origin,
                    placeholder,
                    &hint_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        } else {
            // 2. Draw selection highlights (when not composing)
            if !selection.is_empty() && !ime.is_composing() {
                let rects = layout.hit_test_text_range(selection.start(), selection.len())?;
                {
                    let sel_brush = crate::windows::gfx::GfxDevice::solid_brush(
                        context,
                        colorref_to_d2d(palette.selection, 1.0),
                    )?;
                    for r in rects {
                        let sel_rect = D2D_RECT_F {
                            left: bounds.left + r.left,
                            top: bounds.top + r.top,
                            right: bounds.left + r.right,
                            bottom: bounds.top + r.bottom,
                        };
                        context.FillRectangle(&sel_rect, &sel_brush);
                    }
                }
            }

            // 3. Draw text layout
            if let Some(raw_layout) = layout.raw_layout() {
                let text_brush = crate::windows::gfx::GfxDevice::solid_brush(
                    context,
                    colorref_to_d2d(palette.text, 1.0),
                )?;
                context.DrawTextLayout(
                    origin,
                    raw_layout,
                    &text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        // 4. Draw caret if focused and blinking on
        if is_focused && show_caret {
            let caret_offset = if ime.is_composing() {
                // If composing, place caret after the composition text
                selection.start() + ime.composition_string().encode_utf16().count()
            } else {
                selection.caret
            };

            let caret_bounds = layout.hit_test_text_position(caret_offset)?;
            {
                let caret_x = bounds.left + caret_bounds.x;
                let caret_y = bounds.top + caret_bounds.y;
                let caret_width = 1.5f32;
                let caret_rect = D2D_RECT_F {
                    left: caret_x,
                    top: caret_y,
                    right: caret_x + caret_width,
                    bottom: caret_y + caret_bounds.height,
                };

                let caret_brush = crate::windows::gfx::GfxDevice::solid_brush(
                    context,
                    colorref_to_d2d(palette.accent, 1.0),
                )?;
                context.FillRectangle(&caret_rect, &caret_brush);
            }
        }

        Ok(())
    }
}

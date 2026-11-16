//! Shared geometry for variable-height rows and bounded, asynchronous thumbnails.
use super::*;
mod measure;
use crate::windows::gfx::{self, FontRole};
pub(super) use measure::Measurements;
use mini_launcher::{clipboard_image::Image, search::ImagePreview};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectWrite::*;

pub(super) type Thumbnail = gfx::BitmapImage;

// Match the largest consumer: the 176 x 112 DIP drag preview.
pub(super) struct PreviewRequest {
    pub preview: ImagePreview,
    pub dpi: u32,
}

pub(super) fn load(request: &PreviewRequest) -> Option<Arc<Image>> {
    mini_launcher::image_cache::asset(std::path::Path::new(&request.preview.path))
        .ok()?
        .fit(
            layout::px(176, request.dpi).max(1) as u32,
            layout::px(112, request.dpi).max(1) as u32,
        )
        .ok()
}

impl State {
    pub(super) unsafe fn reflow_rows(&mut self) {
        self.close_image_preview();
        let width = self.width();
        let capacity = self.viewport.capacity().max(200);
        let dpi = self.dpi;
        let heights = self
            .query
            .results
            .iter()
            .map(|entry| {
                if let Some(panel) = &entry.text_panel {
                    return self.row_measurements.panel_height(
                        &self.text_engine,
                        panel,
                        width,
                        capacity,
                        dpi,
                    );
                }
                if entry.preview.is_some() {
                    return 84;
                }
                if !entry.adaptive_height {
                    return ROW_HEIGHT;
                }
                let height = self.row_measurements.text_height(
                    &self.text_engine,
                    &entry.title,
                    width - 211,
                    FontRole::Title,
                    dpi,
                );
                (height + 36).clamp(60, 112)
            })
            .collect();
        self.viewport.set_heights(heights);
    }

    pub(super) fn queue_visible_previews(&mut self) {
        if unsafe { IsWindowVisible(self.hwnd) } == 0 {
            return;
        }
        for entry in &self.query.results[self.viewport.range()] {
            if let Some(preview) = &entry.preview {
                if !self.previews.contains_key(preview)
                    && self.pending_previews.len() < 8
                    && self.pending_previews.insert((preview.clone(), self.dpi))
                {
                    let _ = self.preview_sender.send(PreviewRequest {
                        preview: preview.clone(),
                        dpi: self.dpi,
                    });
                }
            }
        }
    }

    pub(super) unsafe fn draw_image_row(
        &self,
        context: &ID2D1DeviceContext,
        device: &gfx::GfxDevice,
        entry: &Entry,
        top: i32,
        height: i32,
    ) -> windows::core::Result<()> {
        let Some(preview) = &entry.preview else {
            return Ok(());
        };
        let text_top = top + (height - 48) / 2;
        let text_right = (self.width() - 164) as f32;
        let title_rect = D2D_RECT_F {
            left: 86.0,
            top: text_top as f32,
            right: text_right,
            bottom: (text_top + 23) as f32,
        };
        self.text_engine.draw_text_role(
            context,
            &entry.title,
            FontRole::Title,
            &title_rect,
            gfx::colorref_to_d2d(self.palette.text, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        let mut details = format!("{} × {}", preview.width, preview.height);
        if !entry.subtitle.is_empty() {
            details.push_str(" · ");
            details.push_str(&entry.subtitle);
        }
        let subtitle_rect = D2D_RECT_F {
            left: 86.0,
            top: (text_top + 26) as f32,
            right: text_right,
            bottom: (text_top + 46) as f32,
        };
        self.text_engine.draw_text_role(
            context,
            &details,
            FontRole::Small,
            &subtitle_rect,
            gfx::colorref_to_d2d(self.palette.secondary, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;

        let image_top = top + (height - 4 - 60) / 2;
        let thumb_rect = D2D_RECT_F {
            left: (self.width() - 152) as f32,
            top: image_top as f32,
            right: (self.width() - 36) as f32,
            bottom: (image_top + 60) as f32,
        };
        let surface = theme::mix(self.palette.background, self.palette.text, 2);
        gfx::fill_round_rect(
            context,
            &thumb_rect,
            4.0,
            gfx::colorref_to_d2d(surface, 1.0),
        )?;
        if let Some(Some(image)) = self.previews.get(preview) {
            image.draw_contained(device, &thumb_rect)?;
        } else {
            let msg = if self.previews.contains_key(preview) {
                "预览失败"
            } else {
                "加载中…"
            };
            self.text_engine.draw_text_role(
                context,
                msg,
                FontRole::Small,
                &thumb_rect,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
        }
        Ok(())
    }

    pub(super) unsafe fn draw_row_title(
        &self,
        context: &ID2D1DeviceContext,
        entry: &Entry,
        top: i32,
        height: i32,
    ) -> windows::core::Result<()> {
        let ranges = presentation::literal_ranges(&entry.title, &self.query.query_text);
        if !entry.adaptive_height {
            let (rect_top, rect_bottom) = if entry.subtitle.is_empty() {
                (top + 8, top + height - 12)
            } else {
                (top + 7, top + 28)
            };
            let rect = D2D_RECT_F {
                left: 86.0,
                top: rect_top as f32,
                right: (self.width() - 125) as f32,
                bottom: rect_bottom as f32,
            };
            self.text_engine.draw_highlighted_title(
                context,
                &entry.title,
                &rect,
                gfx::colorref_to_d2d(self.palette.text, 1.0),
                gfx::colorref_to_d2d(self.palette.accent, 0.20),
                &ranges,
                false,
            )?;
            return Ok(());
        }
        let rect = D2D_RECT_F {
            left: 86.0,
            top: (top + 5) as f32,
            right: (self.width() - 125) as f32,
            bottom: (top + height - 31) as f32,
        };
        self.text_engine.draw_highlighted_title(
            context,
            &entry.title,
            &rect,
            gfx::colorref_to_d2d(self.palette.text, 1.0),
            gfx::colorref_to_d2d(self.palette.accent, 0.20),
            &ranges,
            true,
        )?;
        Ok(())
    }
}

impl State {
    pub(super) unsafe fn draw_text_panel(
        &self,
        context: &ID2D1DeviceContext,
        entry: &Entry,
        top: i32,
        panel: &mini_launcher::result::TextPanel,
        geometry: &text_panel::Geometry,
    ) -> windows::core::Result<()> {
        let loading_badge =
            panel.state == mini_launcher::result::PanelState::Loading && geometry.status.is_none();
        let title_right = if loading_badge {
            (self.width() - 130) as f32
        } else {
            (self.width() - 36) as f32
        };
        let title_rect = D2D_RECT_F {
            left: 36.0,
            top: (top + 3) as f32,
            right: title_right,
            bottom: (top + 30) as f32,
        };
        self.text_engine.draw_text_role(
            context,
            &entry.title,
            FontRole::Title,
            &title_rect,
            gfx::colorref_to_d2d(self.palette.text, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        if loading_badge {
            let badge_rect = D2D_RECT_F {
                left: (self.width() - 126) as f32,
                top: (top + 5) as f32,
                right: (self.width() - 36) as f32,
                bottom: (top + 29) as f32,
            };
            self.text_engine.draw_text_role(
                context,
                "正在查询…",
                FontRole::Small,
                &badge_rect,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
        }
        if let Some(status) = geometry.status {
            let mut area = status;
            if let Some(retry) = geometry.retry {
                area[2] = retry[0] - 12;
                let retry_rect = D2D_RECT_F {
                    left: retry[0] as f32,
                    top: retry[1] as f32,
                    right: retry[2] as f32,
                    bottom: retry[3] as f32,
                };
                gfx::fill_round_rect(
                    context,
                    &retry_rect,
                    4.0,
                    gfx::colorref_to_d2d(self.palette.surface, 1.0),
                )?;
                gfx::stroke_round_rect(
                    context,
                    &retry_rect,
                    4.0,
                    gfx::colorref_to_d2d(self.palette.border, 1.0),
                    1.0,
                )?;
                self.text_engine.draw_text_role(
                    context,
                    "重试",
                    FontRole::Small,
                    &retry_rect,
                    gfx::colorref_to_d2d(self.palette.accent, 1.0),
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;
            }
            let color = if panel.state == mini_launcher::result::PanelState::Error {
                self.palette.accent
            } else {
                self.palette.secondary
            };
            let status_rect = D2D_RECT_F {
                left: area[0] as f32,
                top: area[1] as f32,
                right: area[2] as f32,
                bottom: area[3] as f32,
            };
            self.text_engine.draw_text_role(
                context,
                panel.status(),
                FontRole::Small,
                &status_rect,
                gfx::colorref_to_d2d(color, 1.0),
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
        }
        // Keep text cards opaque for readability, with a subtle theme tint.
        let background = theme::mix(
            self.palette.background,
            self.palette.accent,
            if self.dark { 8 } else { 4 },
        );
        let border = theme::mix(self.palette.border, self.palette.accent, 20);
        for (index, frame) in geometry.frames.iter().copied().enumerate() {
            let block = &panel.blocks[index];
            let frame_rect = D2D_RECT_F {
                left: frame[0] as f32,
                top: frame[1] as f32,
                right: frame[2] as f32,
                bottom: frame[3] as f32,
            };
            gfx::fill_round_rect(
                context,
                &frame_rect,
                7.0,
                gfx::colorref_to_d2d(background, 1.0),
            )?;
            gfx::stroke_round_rect(
                context,
                &frame_rect,
                7.0,
                gfx::colorref_to_d2d(border, 1.0),
                1.0,
            )?;
            let block_title_rect = D2D_RECT_F {
                left: (frame[0] + 12) as f32,
                top: (frame[1] + 5) as f32,
                right: (frame[2] - 12) as f32,
                bottom: (frame[1] + 29) as f32,
            };
            self.text_engine.draw_text_role(
                context,
                &block.title,
                FontRole::Small,
                &block_title_rect,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
            // Draw the placeholder outside the selectable text model so an
            // empty body's display hint can never become copied content.
            if !block.selectable || block.text.is_empty() {
                let area = geometry.areas[index];
                let content_rect = D2D_RECT_F {
                    left: area[0] as f32,
                    top: area[1] as f32,
                    right: area[2] as f32,
                    bottom: area[3] as f32,
                };
                let text_color = if block.text.is_empty() {
                    self.palette.hint
                } else {
                    self.palette.text
                };
                let placeholder = if panel.state == mini_launcher::result::PanelState::Loading
                {
                    "正在查询…"
                } else {
                    "暂无内容"
                };
                let display_text = if block.text.is_empty() {
                    placeholder
                } else {
                    &block.text
                };
                self.text_engine.draw_text_wrapped(
                    context,
                    display_text,
                    FontRole::Body,
                    &content_rect,
                    gfx::colorref_to_d2d(text_color, 1.0),
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                    DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn preview_decodes_to_display_bounds_instead_of_metadata_dimensions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("thumbnail-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.png");
        Image {
            width: 1024,
            height: 512,
            pixels: vec![255; 1024 * 512 * 4],
        }
        .write_png(&path)
        .unwrap();
        let image = load(&PreviewRequest {
            preview: ImagePreview {
                path: path.to_string_lossy().into(),
                original_path: None,
                width: 1024,
                height: 512,
            },
            dpi: 144,
        })
        .unwrap();
        assert_eq!((image.width, image.height), (264, 132));
        assert!(image.pixels.len() <= 264 * 168 * 4);
        drop(image);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}

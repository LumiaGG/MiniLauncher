use super::*;
use crate::windows::gfx::{self, FontRole};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

impl State {
    pub(super) unsafe fn paint(&mut self, print_dc: Option<HDC>) {
        // Reuse the last composed scene until the exit finishes.
        if self.closing.is_some() {
            if print_dc.is_none() {
                let mut paint: PAINTSTRUCT = zeroed();
                BeginPaint(self.hwnd, &mut paint);
                EndPaint(self.hwnd, &paint);
            }
            return;
        }
        self.sync_accessibility(false);
        let mut paint: PAINTSTRUCT = zeroed();
        if print_dc.is_none() {
            BeginPaint(self.hwnd, &mut paint);
        }
        let renderer = self.renderer.clone();
        let result =
            renderer
                .borrow_mut()
                .render(self.hwnd, self.dpi, print_dc, |context, device| {
                    self.paint_scene(context, device)
                });
        if let Err(error) = result {
            eprintln!("Launcher drawing failed: {error}");
        }
        if print_dc.is_none() {
            EndPaint(self.hwnd, &paint);
        }
    }

    unsafe fn paint_scene(
        &mut self,
        context: &ID2D1DeviceContext,
        device: &gfx::GfxDevice,
    ) -> windows::core::Result<()> {
        let mut client: RECT = zeroed();
        GetClientRect(self.hwnd, &mut client);
        if self.render_device_id != device.id {
            self.d2d_icons.clear();
            for image in self.previews.values().flatten() {
                image.release_device_resources();
            }
            self.render_device_id = device.id;
        }
        // GPU copies are only needed for visible rows, not every past search.
        self.d2d_icons.retain(|path, _| {
            self.query.results[self.viewport.range()]
                .iter()
                .any(|entry| entry.icon_path.as_ref() == Some(path))
        });

        let clear_color = backdrop::background(
            self.palette,
            &self.settings.backdrop,
            self.backdrop_active,
        );

        context.Clear(Some(&clear_color));

        // Draw scope capsule and status capsule
        self.paint_scope(context)?;
        self.paint_status_capsule(context)?;

        // Draw self-drawing editor
        let editor_bounds = self.editor_bounds();
        self.editor.draw(context, editor_bounds, &self.palette)?;

        // Header divider line if query is active
        if self.query.query_active {
            let line_y = (IDLE_HEIGHT + self.input_extra() - 1) as f32;
            gfx::draw_horizontal_line(
                context,
                20.0,
                (self.width() - 20) as f32,
                line_y,
                1.0,
                gfx::colorref_to_d2d(self.palette.border, 1.0),
            )?;
            if let Some(phase) = self.query.progress_phase(Instant::now()) {
                let width = (self.width() - 40) as f32;
                let length = width * 0.24;
                let left = phase * (width + length) - length;
                gfx::draw_horizontal_line(
                    context,
                    20.0 + left.max(0.0),
                    20.0 + (left + length).min(width),
                    line_y,
                    2.0,
                    gfx::colorref_to_d2d(self.palette.accent, 1.0),
                )?;
            }
        }

        let now = Instant::now();
        let footer = self.logical(client.bottom)
            - if self.footer_visible() {
                FOOTER_HEIGHT
            } else {
                0
            };

        // Result clipping region
        let result_bottom = (self.row_top() + self.viewport.capacity()).min(footer);
        let clip_rect = D2D_RECT_F {
            left: 0.0,
            top: self.row_top() as f32,
            right: self.width() as f32,
            bottom: result_bottom as f32,
        };
        let clip = gfx::shapes::ClipGuard::new(context, &clip_rect);

        let animated_panel_height = |top: i32, height: i32| -> i32 {
            if self.height_animation.is_some() && !self.viewport.scrollable() {
                (footer - 8 - top).clamp(116.min(height), height)
            } else {
                height
            }
        };

        if self.viewport.range().contains(&self.viewport.selected) {
            let top = self.selection.value(now);
            let raw_height = self.viewport.row_height(self.viewport.selected);
            let has_panel = self.query.results[self.viewport.selected]
                .text_panel
                .is_some();
            let height = if has_panel {
                animated_panel_height(top, raw_height)
            } else {
                raw_height
            };
            let background = if has_panel {
                theme::mix(
                    self.palette.background,
                    self.palette.accent,
                    if self.dark { 18 } else { 10 },
                )
            } else {
                self.palette.selection
            };
            let sel_rect = D2D_RECT_F {
                left: 20.0,
                top: top as f32,
                right: (self.width() - 20) as f32,
                bottom: (top + height - 4) as f32,
            };
            gfx::fill_round_rect(
                context,
                &sel_rect,
                6.0,
                gfx::colorref_to_d2d(background, 1.0),
            )?;
            let accent_rect = D2D_RECT_F {
                left: 21.0,
                top: (top + (height - 22) / 2) as f32,
                right: 24.0,
                bottom: (top + (height + 22) / 2) as f32,
            };
            gfx::fill_round_rect(
                context,
                &accent_rect,
                2.0,
                gfx::colorref_to_d2d(self.palette.accent, 1.0),
            )?;
        }

        let pin_query = &self.query.pin_query;
        self.panels.begin_layout();
        for index in self.viewport.range() {
            let entry = &self.query.results[index];
            let top = self.row_top() + self.viewport.row_top(index);
            let height = self.viewport.row_height(index);
            let selected = index == self.viewport.selected;
            if !selected && self.hovered == Some(index) && !self.selection.active(Instant::now()) {
                let hover_rect = D2D_RECT_F {
                    left: 20.0,
                    top: top as f32,
                    right: (self.width() - 20) as f32,
                    bottom: (top + height - 4) as f32,
                };
                gfx::fill_round_rect(
                    context,
                    &hover_rect,
                    6.0,
                    gfx::colorref_to_d2d(self.palette.hover, 1.0),
                )?;
            }
            if let Some(panel) = &entry.text_panel {
                let draw_height = animated_panel_height(top, height);
                let geometry =
                    text_panel::Geometry::for_panel(self.width(), top, draw_height, panel);
                if panel.blocks.iter().any(|b| b.selectable) {
                    self.panels
                        .layout(&entry.key, panel, &geometry, &self.text_engine)?;
                }
                self.draw_text_panel(context, entry, top, panel, &geometry)?;
                continue;
            }
            // Preview rows use the same plugin icon pipeline as ordinary rows.
            let icon_top = top + (height - 4 - 32) / 2;
            // Icon or fallback symbol
            let d2d_icon = entry.icon_path.as_ref().and_then(|p| self.d2d_icons.get(p));
            if let Some(bitmap) = d2d_icon {
                let icon_rect = D2D_RECT_F {
                    left: 36.0,
                    top: icon_top as f32,
                    right: 68.0,
                    bottom: (icon_top + 32) as f32,
                };
                context.DrawBitmap(
                    bitmap,
                    Some(&icon_rect),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            } else if let Some(hicon) = entry
                .icon_path
                .as_ref()
                .and_then(|p| self.icons.get(p))
                .copied()
                .filter(|&h| h != 0)
            {
                let bitmap = self.image_renderer.bitmap_from_hicon(
                    context,
                    windows::Win32::UI::WindowsAndMessaging::HICON(hicon as _),
                )?;
                let icon_rect = D2D_RECT_F {
                    left: 36.0,
                    top: icon_top as f32,
                    right: 68.0,
                    bottom: (icon_top + 32) as f32,
                };
                context.DrawBitmap(
                    &bitmap,
                    Some(&icon_rect),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                if let Some(p) = &entry.icon_path {
                    self.d2d_icons.insert(p.clone(), bitmap);
                }
            } else {
                let pill_rect = D2D_RECT_F {
                    left: 33.0,
                    top: (icon_top - 3) as f32,
                    right: 71.0,
                    bottom: (icon_top + 35) as f32,
                };
                gfx::fill_round_rect(
                    context,
                    &pill_rect,
                    6.0,
                    gfx::colorref_to_d2d(self.palette.surface, 1.0),
                )?;
                let symbol = if entry.preview.is_some() {
                    "\u{E91B}"
                } else {
                    match entry.kind {
                        mini_launcher::result::EntryKind::Calculator => "\u{E8EF}",
                        mini_launcher::result::EntryKind::Web => "\u{E774}",
                        mini_launcher::result::EntryKind::Settings => "\u{E713}",
                        _ => "\u{E71D}",
                    }
                };
                self.text_engine.draw_text_role(
                    context,
                    symbol,
                    FontRole::Symbol,
                    &pill_rect,
                    gfx::colorref_to_d2d(self.palette.accent, 1.0),
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;
            }
            if entry.preview.is_some() {
                self.draw_image_row(context, device, entry, top, height)?;
                continue;
            }
            self.draw_row_title(context, entry, top, height)?;
            let trailing = if entry.is_app()
                && self
                    .history
                    .get(&entry.key)
                    .is_some_and(|u| u.pinned_queries.contains(pin_query))
            {
                "置顶"
            } else if selected {
                "↵"
            } else {
                ""
            };
            let subtitle_top = (top + 29) as f32;
            let sub_right = (self.width() - if trailing.is_empty() { 60 } else { 120 }) as f32;
            let sub_rect = D2D_RECT_F {
                left: 86.0,
                top: subtitle_top,
                right: sub_right,
                bottom: (top + height - 12) as f32,
            };
            let subtitle = self.row_measurements.subtitle(&self.text_engine, &entry.subtitle,
                (sub_rect.right - sub_rect.left) as i32, self.dpi);
            self.text_engine.draw_text_role(
                context,
                &subtitle,
                FontRole::Small,
                &sub_rect,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
            if !trailing.is_empty() {
                let trail_rect = D2D_RECT_F {
                    left: (self.width() - 110) as f32,
                    top: (top + 13) as f32,
                    right: (self.width() - 41) as f32,
                    bottom: (top + 42) as f32,
                };
                self.text_engine.draw_text_role(
                    context,
                    trailing,
                    FontRole::Body,
                    &trail_rect,
                    gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                    DWRITE_TEXT_ALIGNMENT_TRAILING,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;
            }
        }
        self.panels.draw(context, self.palette)?;
        if let Some(thumb) = self
            .viewport
            .thumb(self.row_top(), self.scroll_track_height())
        {
            let thumb_rect = D2D_RECT_F {
                left: (self.width() - 14) as f32,
                top: thumb.top as f32,
                right: (self.width() - 11) as f32,
                bottom: (thumb.top + thumb.height) as f32,
            };
            gfx::fill_round_rect(
                context,
                &thumb_rect,
                2.0,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
            )?;
        }
        if self.query.empty_message_visible() {
            let text = self.empty_title();
            let title_rect = D2D_RECT_F {
                left: 40.0,
                top: (self.row_top() + 8) as f32,
                right: (self.width() - 40) as f32,
                bottom: (self.row_top() + 32) as f32,
            };
            self.text_engine.draw_text_role(
                context,
                text,
                FontRole::Title,
                &title_rect,
                gfx::colorref_to_d2d(self.palette.text, 1.0),
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
            let (primary, secondary) = self.empty_actions();
            for (index, label) in [primary, secondary].into_iter().enumerate() {
                let sub_rect = D2D_RECT_F {
                    left: if index == 0 { 40.0 } else { self.width() as f32 / 2.0 + 4.0 },
                    top: (self.row_top() + 32) as f32,
                    right: if index == 0 { self.width() as f32 / 2.0 - 4.0 } else { (self.width() - 40) as f32 },
                    bottom: (self.row_top() + 56) as f32,
                };
                gfx::fill_round_rect(context, &sub_rect, 4.0,
                    gfx::colorref_to_d2d(self.palette.hover, 1.0))?;
                self.text_engine.draw_text_role(context, label, FontRole::Small, &sub_rect,
                    gfx::colorref_to_d2d(self.palette.accent, 1.0),
                    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
        }
        self.panels.end_layout(self.hwnd);
        drop(clip);
        if self.footer_visible() && footer >= IDLE_HEIGHT {
            let footer_line_rect = D2D_RECT_F {
                left: 20.0,
                top: footer as f32,
                right: (self.width() - 20) as f32,
                bottom: (footer + 1) as f32,
            };
            gfx::fill_rect(
                context,
                &footer_line_rect,
                gfx::colorref_to_d2d(self.palette.border, 1.0),
            )?;
            let hint = self.result_hint();
            let feedback = self.status.as_ref().map(|text| {
                format!("详情 › {}", text.lines().next().unwrap_or(text))
            });
            let status = feedback.as_deref().unwrap_or(if self.capsule_hovered {
                if let Some(capsule) = &self.capsule {
                    if self.scope == scope::Scope::Plugin(capsule.plugin_id.clone()) {
                        &capsule.hint_scoped
                    } else {
                        &capsule.hint_normal
                    }
                } else {
                    ""
                }
            } else if self.panels.focused() {
                "拖选文字    Ctrl+C 复制选区    Esc / Tab 返回搜索"
            } else {
                &hint
            });
            let status_rect = D2D_RECT_F {
                left: 24.0,
                top: (footer + 6) as f32,
                right: (self.width() - 180) as f32,
                bottom: (footer + 29) as f32,
            };
            self.text_engine.draw_text_role(
                context,
                status,
                FontRole::Small,
                &status_rect,
                gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
            if self.query.query_active {
                let count = if self.application_index_pending() {
                    "正在索引…".to_string()
                } else if self.viewport.scrollable() {
                    let range = self.viewport.range();
                    format!(
                        "{}–{} / {}",
                        range.start + 1,
                        range.end,
                        self.query.results.len()
                    )
                } else {
                    format!("{} 个结果", self.query.results.len())
                };
                let count_rect = D2D_RECT_F {
                    left: (self.width() - 172) as f32,
                    top: (footer + 6) as f32,
                    right: (self.width() - 24) as f32,
                    bottom: (footer + 29) as f32,
                };
                self.text_engine.draw_text_role(
                    context,
                    &count,
                    FontRole::Small,
                    &count_rect,
                    gfx::colorref_to_d2d(self.palette.secondary, 1.0),
                    DWRITE_TEXT_ALIGNMENT_TRAILING,
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;
            }
        }
        Ok(())
    }
}

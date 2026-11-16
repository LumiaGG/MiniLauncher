use super::gfx::{self, FontRole};
use super::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;
use windows::Win32::Graphics::DirectWrite::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Scope {
    #[default]
    All,
    Files,
    Web(String),
    Plugin(String),
}

impl Scope {
    pub(super) fn id(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Files => "files".into(),
            Self::Web(id) => format!("web:{id}"),
            Self::Plugin(id) => format!("plugin:{id}"),
        }
    }
    fn group(&self) -> u8 {
        match self {
            Self::All | Self::Files => 0,
            Self::Web(_) => 1,
            Self::Plugin(_) => 2,
        }
    }
    pub(super) fn symbol(&self) -> &'static str {
        match self {
            Self::All => "\u{E721}",
            Self::Files => "\u{E8B7}",
            Self::Web(_) => "\u{E774}",
            Self::Plugin(_) => "\u{EA86}",
        }
    }
}

impl State {
    pub(super) fn scope_name(&self) -> &str {
        if let Some(item) = self
            .settings
            .scope_menu
            .items
            .iter()
            .find(|item| item.id == self.scope.id() && !item.name.is_empty())
        {
            return &item.name;
        }
        match &self.scope {
            Scope::All => "综合",
            Scope::Files => "文件",
            Scope::Web(id) => self
                .settings
                .web_search
                .engines
                .iter()
                .find(|e| &e.id == id)
                .map_or("网页", |e| e.name.as_str()),
            Scope::Plugin(id) => self
                .plugin_scopes
                .iter()
                .find(|(key, _)| key == id)
                .map_or("插件", |(_, name)| name.as_str()),
        }
    }

    pub(super) fn validate_scope(&mut self) {
        let valid = match &self.scope {
            Scope::All => true,
            Scope::Files => self.settings.everything.enabled,
            Scope::Web(id) => {
                self.settings.web_search.enabled
                    && self.settings.web_search.engines.iter().any(|e| &e.id == id)
            }
            Scope::Plugin(id) => self.plugin_scopes.iter().any(|(key, _)| key == id),
        };
        if !valid {
            self.scope = Scope::All;
        }
    }

    pub(super) unsafe fn set_scope(&mut self, scope: Scope) {
        if self.editor.is_composing() {
            return;
        }
        if self.scope == scope {
            self.close_item_menu();
            return;
        }
        self.scope = scope;
        self.layout_input();
        self.execution.pending_execute = None;
        // Until this generation returns, Enter must not launch an old result.
        self.query.clear_results();
        self.viewport.reset(0, self.visible_rows);
        self.request_search();
        self.sync_height();
        self.invalidate();
        self.focus_input();
    }

    pub(super) unsafe fn paint_scope(
        &self,
        target: &ID2D1RenderTarget,
    ) -> windows::core::Result<()> {
        let menu_open = self.scope_menu_open();
        if self.scope_hovered || menu_open {
            let hover_rect = D2D_RECT_F {
                left: 12.0,
                top: 13.0,
                right: self.scope_right() as f32,
                bottom: 45.0,
            };
            // A translucent overlay preserves the material underneath instead
            // of replacing it with an opaque gray patch.
            let (tint, opacity, border_opacity) = if menu_open {
                (self.palette.accent, if self.dark { 0.18 } else { 0.12 }, 0.30)
            } else {
                (self.palette.text, if self.dark { 0.08 } else { 0.06 }, 0.10)
            };
            gfx::fill_round_rect(
                target,
                &hover_rect,
                6.0,
                gfx::colorref_to_d2d(tint, opacity),
            )?;
            gfx::stroke_round_rect(
                target,
                &hover_rect,
                6.0,
                gfx::colorref_to_d2d(tint, border_opacity),
                1.0,
            )?;
        }
        let symbol_rect = D2D_RECT_F {
            left: 18.0,
            top: 14.0,
            right: 38.0,
            bottom: 44.0,
        };
        self.text_engine.draw_text_role(
            target,
            self.scope.symbol(),
            FontRole::Symbol,
            &symbol_rect,
            gfx::colorref_to_d2d(self.palette.secondary, 1.0),
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        let name_rect = D2D_RECT_F {
            left: 44.0,
            top: 14.0,
            right: (self.scope_right() - 21) as f32,
            bottom: 44.0,
        };
        self.text_engine.draw_text_role(
            target,
            self.scope_name(),
            FontRole::Small,
            &name_rect,
            gfx::colorref_to_d2d(self.palette.text, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        let arrow_rect = D2D_RECT_F {
            left: (self.scope_right() - 18) as f32,
            top: 14.0,
            right: (self.scope_right() - 6) as f32,
            bottom: 44.0,
        };
        self.text_engine.draw_text_role(
            target,
            "▾",
            FontRole::Small,
            &arrow_rect,
            gfx::colorref_to_d2d(self.palette.secondary, 1.0),
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        let divider_rect = D2D_RECT_F {
            left: (self.scope_right() + 4) as f32,
            top: 20.0,
            right: (self.scope_right() + 5) as f32,
            bottom: 38.0,
        };
        gfx::fill_rect(
            target,
            &divider_rect,
            gfx::colorref_to_d2d(self.palette.border, 1.0),
        )?;
        Ok(())
    }

    pub(super) fn scope_right(&self) -> i32 {
        let name = self.text_engine.measure_width(self.scope_name(), FontRole::Small)
            .unwrap_or(47.0).ceil() as i32;
        // Leave room for the editor even at the minimum window width.
        let editor_limit = self.capsule_rect().map_or(self.width() - 16, |[left, ..]| left - 8);
        let maximum = (self.width() / 3).min(220).min(editor_limit - 96).max(112);
        (name + 65).clamp(112, maximum)
    }

    pub(super) fn hit_scope(&self, x: i32, y: i32) -> bool {
        (12..self.scope_right()).contains(&x) && (13..45).contains(&y)
    }

    pub(super) unsafe fn paint_status_capsule(
        &self,
        target: &ID2D1RenderTarget,
    ) -> windows::core::Result<()> {
        let Some(capsule) = &self.capsule else {
            return Ok(());
        };
        let Some([left, top, right, bottom]) = self.capsule_rect() else {
            return Ok(());
        };
        let badge_color = capsule.badge_color;
        let bg = theme::mix(
            self.palette.background,
            badge_color,
            if self.capsule_hovered { 24 } else { 14 },
        );
        let border = theme::mix(
            self.palette.border,
            badge_color,
            if self.capsule_hovered { 62 } else { 36 },
        );
        let rect = D2D_RECT_F {
            left: left as f32,
            top: top as f32,
            right: right as f32,
            bottom: bottom as f32,
        };
        gfx::fill_round_rect(target, &rect, 7.0, gfx::colorref_to_d2d(bg, 1.0))?;
        gfx::stroke_round_rect(target, &rect, 7.0, gfx::colorref_to_d2d(border, 1.0), 1.0)?;

        if let Some(glyph) = &capsule.glyph {
            let glyph_rect = D2D_RECT_F {
                left: (left + 9) as f32,
                top: top as f32,
                right: (left + 25) as f32,
                bottom: bottom as f32,
            };
            self.text_engine.draw_text_role(
                target,
                glyph,
                FontRole::SymbolSmall,
                &glyph_rect,
                gfx::colorref_to_d2d(badge_color, 1.0),
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?;
        }

        let text_color = theme::mix(
            self.palette.text,
            badge_color,
            if self.capsule_hovered { 8 } else { 16 },
        );
        let text_left = if capsule.glyph.is_some() {
            left + 27
        } else {
            left + 10
        };
        let text_rect = D2D_RECT_F {
            left: text_left as f32,
            top: top as f32,
            right: (right - 9) as f32,
            bottom: bottom as f32,
        };
        self.text_engine.draw_text_role(
            target,
            &capsule.text,
            FontRole::Small,
            &text_rect,
            gfx::colorref_to_d2d(text_color, 1.0),
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
        )?;
        Ok(())
    }

    pub(super) fn available_scopes(&self) -> Vec<(Scope, String)> {
        let mut choices = vec![(Scope::All, "综合搜索".into())];
        if self.settings.everything.enabled {
            choices.push((Scope::Files, "文件搜索".into()));
        }
        if self.settings.web_search.enabled {
            choices.extend(self.settings.web_search.engines.iter().map(|engine| {
                (Scope::Web(engine.id.clone()), engine.name.clone())
            }));
        }
        choices.extend(self.plugin_scopes.iter().map(|(id, name)| {
            (Scope::Plugin(id.clone()), name.clone())
        }));
        choices
    }

    pub(super) unsafe fn scope_menu(&mut self) {
        if self.editor.is_composing() || self.closing.is_some() || IsWindowVisible(self.hwnd) == 0 {
            return;
        }
        if self.scope_menu_open() {
            self.close_item_menu();
            return;
        }
        self.close_item_menu();
        let choices = self.available_scopes();
        let available: Vec<_> = choices
            .iter()
            .map(|(scope, name)| mini_launcher::scope_menu::Item::new(scope.id(), name))
            .collect();
        let ordered = self.settings.scope_menu.resolve(&available);
        let mut previous_group = None;
        let choices = ordered
            .into_iter()
            .filter_map(|item| {
                let (scope, _) = choices
                    .iter()
                    .find(|(scope, _)| scope.id() == item.id)?;
                let group = scope.group();
                let separator = previous_group.is_some_and(|previous| previous != group);
                previous_group = Some(group);
                let shortcut = self.settings.scope_menu.shortcut(
                    &item.id, &self.settings.web_search.default_engine,
                );
                Some((scope.clone(), item.name, shortcut, separator))
            })
            .collect();
        let mut point = POINT {
            x: self.px(12),
            y: self.px(54),
        };
        ClientToScreen(self.hwnd, &mut point);
        self.open_scope_menu(choices, point);
    }
}

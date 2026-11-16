//! Host-owned selectable text. Pointer motion, scrolling and copying never cross RPC.
#[cfg(test)]
mod benchmark;
mod layout;
pub mod view;

pub(super) use layout::{Geometry, Hit};
pub use view::TextView;

use super::Palette;
use crate::windows::gfx::TextEngine;
use mini_launcher::{providers::Update, result::TextPanel};
use std::{collections::HashSet, sync::Arc};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows_sys::Win32::{Foundation::*, UI::Input::KeyboardAndMouse::*};

#[derive(Default)]
pub(super) struct Panels {
    pub(crate) views: Vec<TextView>,
    visible: HashSet<String>,
    deferred: Vec<Update>,
    focused: Option<(String, usize)>,
}

impl Panels {
    pub fn focused(&self) -> bool {
        self.focused.is_some()
    }

    pub fn defer(&mut self, update: Update) {
        // Keep at most one latest snapshot per provider, regardless of plugin activity.
        self.deferred.retain(|old| old.source != update.source);
        self.deferred.push(update);
    }

    pub fn take_deferred(&mut self) -> Vec<Update> {
        std::mem::take(&mut self.deferred)
    }

    pub unsafe fn clear(&mut self, parent: HWND) {
        if self.focused() {
            SetFocus(parent);
        }
        self.views.clear();
        self.visible.clear();
        self.deferred.clear();
        self.focused = None;
    }

    pub fn begin_layout(&mut self) {
        self.visible.clear();
    }

    pub unsafe fn end_layout(&mut self, parent: HWND) {
        if let Some((focused_key, _)) = &self.focused {
            if !self.visible.contains(focused_key) {
                SetFocus(parent);
                self.focused = None;
            }
        }
        self.views.retain(|v| self.visible.contains(&v.key));
    }

    pub fn cancel_drag(&mut self) {
        for view in &mut self.views { view.drag_anchor = None; }
    }

    pub fn select_focused(&mut self) {
        if let Some((key, block)) = self.focused.clone() { self.select_all(&key, Some(block)); }
    }

    pub fn blur(&mut self) {
        self.focused = None;
    }

    pub unsafe fn focus_entry(&mut self, parent: HWND, key: &str, backwards: bool) -> bool {
        let matching: Vec<usize> = self
            .views
            .iter()
            .filter(|v| v.key == key)
            .map(|v| v.block_index)
            .collect();
        if matching.is_empty() {
            SetFocus(parent);
            self.focused = None;
            false
        } else {
            let block = if backwards {
                *matching.last().unwrap()
            } else {
                *matching.first().unwrap()
            };
            self.focused = Some((key.to_string(), block));
            true
        }
    }

    pub unsafe fn cycle_focus(&mut self, parent: HWND, backwards: bool) {
        if let Some((key, block)) = &self.focused.clone() {
            let matching: Vec<usize> = self
                .views
                .iter()
                .filter(|v| &v.key == key)
                .map(|v| v.block_index)
                .collect();
            if let Some(pos) = matching.iter().position(|&b| b == *block) {
                let next = if backwards {
                    pos.checked_sub(1)
                } else {
                    (pos + 1 < matching.len()).then_some(pos + 1)
                };
                if let Some(next_idx) = next {
                    self.focused = Some((key.clone(), matching[next_idx]));
                    return;
                }
            }
        }
        self.focused = None;
        SetFocus(parent);
    }

    pub unsafe fn focus_block(&mut self, key: &str, block: usize) {
        if self
            .views
            .iter()
            .any(|v| v.key == key && v.block_index == block)
        {
            self.focused = Some((key.to_string(), block));
        }
    }

    pub fn has_selection(&self, key: &str, block: Option<usize>) -> bool {
        self.views
            .iter()
            .any(|v| v.key == key && block.is_none_or(|b| b == v.block_index) && v.has_selection())
    }

    pub fn copy_selection(&self, key: &str, block: Option<usize>) -> Option<String> {
        self.views
            .iter()
            .find(|v| v.key == key && block.is_none_or(|b| b == v.block_index) && v.has_selection())
            .and_then(|v| v.copy_selection())
    }

    pub fn copy_focused(&self) -> Option<String> {
        let (key, block) = self.focused.as_ref()?;
        self.copy_selection(key, Some(*block))
    }

    pub fn select_all(&mut self, key: &str, block: Option<usize>) {
        for v in &mut self.views {
            if v.key == key && block.is_none_or(|b| b == v.block_index) {
                v.select_all();
            }
        }
    }

    pub fn clear_selection(&mut self) {
        for v in &mut self.views {
            v.clear_selection();
        }
    }

    pub unsafe fn layout(
        &mut self,
        key: &str,
        content: &Arc<TextPanel>,
        geometry: &Geometry,
        engine: &TextEngine,
    ) -> windows::core::Result<()> {
        self.visible.insert(key.to_owned());
        self.views.retain(|view| view.key != key || content.blocks.get(view.block_index).is_some_and(|block| block.selectable && block.id == view.block_id));
        if self.focused.as_ref().is_some_and(|(focused_key, index)| focused_key == key && !content.blocks.get(*index).is_some_and(|block| block.selectable)) {
            self.focused = None;
        }
        for (block_index, block) in content.blocks.iter().enumerate() {
            if !block.selectable {
                continue;
            }
            if block_index >= geometry.areas.len() {
                continue;
            }
            let area = geometry.areas[block_index];
            // Reserve the same scrollbar space used by row-height measurement.
            let width = (area[2] - area[0] - 14).max(1) as f32;
            let rect = D2D_RECT_F {
                left: area[0] as f32,
                top: area[1] as f32,
                right: area[2] as f32,
                bottom: area[3] as f32,
            };

            if let Some(view) = self
                .views
                .iter_mut()
                .find(|v| v.key == key && v.block_index == block_index)
            {
                view.rect = rect;
                if view.text != block.text {
                    view.set_text(block.text.clone(), width, engine)?;
                } else {
                    view.set_width(width)?;
                }
                view.scroll_by(0.0, rect.bottom - rect.top);
            } else {
                match TextView::create(
                    key.to_string(),
                    block_index,
                    block.text.clone(),
                    width,
                    engine,
                ) {
                    Ok(mut view) => {
                        view.rect = rect;
                        view.block_id = block.id.clone();
                        self.views.push(view);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    pub unsafe fn draw(
        &self,
        context: &ID2D1DeviceContext,
        palette: Palette,
    ) -> windows_core::Result<()> {
        for view in &self.views {
            if self.visible.contains(&view.key) {
                view.draw(context, &view.rect, palette)?;
            }
        }
        Ok(())
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        for (i, view) in self.views.iter().enumerate() {
            if self.visible.contains(&view.key)
                && x >= view.rect.left
                && x < view.rect.right
                && y >= view.rect.top
                && y < view.rect.bottom
            {
                return Some((i, x - view.rect.left, y - view.rect.top));
            }
        }
        None
    }

    pub fn handle_mouse_down(&mut self, x: f32, y: f32) -> bool {
        if let Some((idx, lx, ly)) = self.hit_test(x, y) {
            let view = &mut self.views[idx];
            self.focused = Some((view.key.clone(), view.block_index));
            view.handle_mouse_down(lx, ly);
            true
        } else {
            false
        }
    }

    pub fn handle_mouse_move(&mut self, x: f32, y: f32) -> bool {
        if let Some((key, block)) = &self.focused {
            if let Some(view) = self
                .views
                .iter_mut()
                .find(|v| &v.key == key && v.block_index == *block)
            {
                let lx = x - view.rect.left;
                let ly = y - view.rect.top;
                view.handle_mouse_move(lx, ly);
                return true;
            }
        }
        false
    }

    pub fn handle_mouse_up(&mut self, x: f32, y: f32) -> bool {
        if let Some((key, block)) = &self.focused {
            if let Some(view) = self
                .views
                .iter_mut()
                .find(|v| &v.key == key && v.block_index == *block)
            {
                let lx = x - view.rect.left;
                let ly = y - view.rect.top;
                view.handle_mouse_up(lx, ly);
                return true;
            }
        }
        false
    }

    pub fn scroll(&mut self, x: f32, y: f32, delta: f32) -> bool {
        if let Some((idx, _, _)) = self.hit_test(x, y) {
            let view = &mut self.views[idx];
            let v_height = view.rect.bottom - view.rect.top;
            view.scroll_by(delta, v_height);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;

    fn panel() -> TextPanel {
        serde_json::from_value(serde_json::json!({"blocks":[
            {"id":"source","title":"原文","text":"原文 hello 😀"},
            {"id":"definition","title":"释义与例句","text":"长文本 line\n".repeat(100)}
        ]}))
        .unwrap()
    }

    #[test]
    fn blocks_reconcile_by_id_keep_selection_scroll_and_tab_order_without_visible_windows() {
        unsafe {
            let parent = null_mut();
            let mut panels = Panels::default();
            let content = Arc::new(panel());
            let engine = TextEngine::create(true).unwrap();
            let geometry = Geometry::for_panel(856, 68, 360, &content);

            panels.begin_layout();
            panels
                .layout("result-id", &content, &geometry, &engine)
                .unwrap();
            panels.end_layout(parent);

            assert_eq!(panels.views.len(), 2);
            assert_eq!(panels.views[0].key, "result-id");
            assert_eq!(panels.views[0].block_index, 0);
            assert_eq!(panels.views[1].block_index, 1);

            // Test focus
            assert!(panels.focus_entry(parent, "result-id", false));
            assert!(panels.focused());
            assert_eq!(panels.focused, Some(("result-id".into(), 0)));
            panels.select_focused();
            assert_eq!(
                panels.copy_focused().as_deref(),
                Some(content.blocks[0].text.as_str())
            );

            // Test scroll
            panels.views[1].scroll_by(50.0, 100.0);
            assert_eq!(panels.views[1].scroll_y, 50.0);

            // Relayout retains scroll
            panels.begin_layout();
            panels
                .layout("result-id", &content, &geometry, &engine)
                .unwrap();
            panels.end_layout(parent);

            assert_eq!(panels.views[1].scroll_y, 50.0);
            assert_eq!(panels.views.len(), 2);
            panels.cycle_focus(parent, false);
            assert_eq!(panels.focused, Some(("result-id".into(), 1)));
            panels.select_focused();
            assert_eq!(
                panels.copy_focused().as_deref(),
                Some(content.blocks[1].text.as_str())
            );
            panels.cycle_focus(parent, true);
            assert_eq!(panels.focused, Some(("result-id".into(), 0)));
            assert_eq!(
                panels.copy_focused().as_deref(),
                Some(content.blocks[0].text.as_str())
            );
            panels.cycle_focus(parent, true);
            assert!(!panels.focused());
            panels.focus_entry(parent, "result-id", false);
            panels.blur();
            assert!(!panels.focused());
            let mut changed = (*content).clone();
            changed.blocks.truncate(1);
            let changed = Arc::new(changed);
            panels.begin_layout();
            panels.layout("result-id", &changed, &geometry, &engine).unwrap();
            panels.end_layout(parent);
            assert_eq!(panels.views.len(), 1);
            let mut changed = (*changed).clone();
            changed.blocks[0].selectable = false;
            panels.begin_layout();
            panels.layout("result-id", &Arc::new(changed), &geometry, &engine).unwrap();
            panels.end_layout(parent);
            assert!(panels.views.is_empty());
        }
    }
}

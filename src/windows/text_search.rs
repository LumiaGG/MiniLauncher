//! Reuse a text item's read-only payload to populate the launcher query.
use super::*;
use mini_launcher::drag_data::{DragData, DragSource};

pub(super) fn source(entry: &Entry) -> Option<DragSource> {
    if entry.preview.is_some()
        || !(matches!(&entry.action, Action::Copy(_)) || entry.paste_after_execute)
    {
        return None;
    }
    let source = entry.drag_source()?;
    match source {
        DragSource::Data(DragData::Text { .. }) | DragSource::Plugin { .. } => Some(source),
        _ => None,
    }
}

impl State {
    pub(super) unsafe fn search_entry_text(&mut self, entry: &Entry) {
        let Some(source) = source(entry) else { return };
        self.text_search_request = self.text_search_request.wrapping_add(1);
        let request = self.text_search_request;
        match source {
            DragSource::Data(DragData::Text { text }) => self.fill_search_text(text),
            DragSource::Plugin {
                plugin_id,
                action_id,
            } => {
                if !self.validate_plugin_callback(&plugin_id, &action_id, true) {
                    return;
                }
                // prepare_drag only retrieves the stable item's data; it never copies or executes.
                let receiver = self.plugins.prepare_drag(plugin_id, action_id);
                let generation = self.query.generation;
                let hwnd = self.hwnd as usize;
                let sender = self.event_sender.clone();
                self.status = Some("正在读取文本…".into());
                std::thread::spawn(move || {
                    let result = receiver
                        .and_then(|r| r.recv().map_err(|_| "插件请求已中止".to_string())?)
                        .and_then(|data| match data {
                            DragData::Text { text } => Ok(text),
                            _ => Err("此条目没有提供文本内容".into()),
                        });
                    send_event(
                        &sender,
                        hwnd,
                        UiEvent::SearchText {
                            request,
                            generation,
                            result,
                        },
                    );
                });
            }
            _ => {}
        }
    }

    pub(super) unsafe fn fill_search_text(&mut self, text: String) {
        if text.len() > input::MAX_LENGTH || text.contains('\0') {
            self.status = Some("搜索文本过长或包含无效字符".into());
            return;
        }
        self.close_image_preview();
        self.cancel_pending_drag();
        self.close_item_menu();
        self.execution.pending_execute = None;
        self.scope = scope::Scope::All;
        self.query.clear_results();
        self.viewport.reset(0, self.visible_rows);
        self.editor.set_text(&text);
        self.search_if_changed();
        self.focus_input();
        self.sync_height();
        self.invalidate();
    }
}

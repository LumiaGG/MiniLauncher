use super::*;
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::component::{
    input::{Input, InputEvent, InputState},
    tag::Tag,
    Icon, Sizable,
};

pub(super) struct ScopeRow {
    id: String,
    source: String,
    kind: &'static str,
    available: bool,
    name: Entity<InputState>,
    original_name: String,
    shortcut: Entity<InputState>,
    original_shortcut: String,
    shortcut_focus: FocusHandle,
    recording: bool,
    recording_error: Option<String>,
    error: Option<String>,
    anchor: ScrollAnchor,
}

fn update_scope_item(
    item: &mut Value,
    name: Option<String>,
    shortcut: Option<String>,
) -> Result<(), String> {
    let table = item.as_table_mut().ok_or("范围菜单项必须为表")?;
    if let Some(name) = name {
        table.insert("name".into(), Value::String(name));
    }
    if let Some(shortcut) = shortcut {
        table.insert("shortcut".into(), Value::String(shortcut));
    }
    Ok(())
}

fn recorded_scope_shortcut(stroke: &Keystroke) -> Result<Option<String>, String> {
    let Some(label) = super::fields::recorded_hotkey(stroke)? else {
        return Ok(None);
    };
    if label.strip_prefix("Alt+").is_some_and(|key| {
        key.len() == 1 && key.as_bytes()[0].is_ascii_digit()
    }) {
        Ok(Some(label))
    } else {
        Err("请按 Alt+0–9；Esc 取消录入。".into())
    }
}
#[derive(Clone)]
struct ScopeDrag {
    id: String,
    title: String,
}
impl Render for ScopeDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_4()
            .py_2()
            .rounded_lg()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .child(self.title.clone())
    }
}

impl Settings {
    pub(super) fn build_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(settings) = self.draft.value.clone().try_into::<config::Settings>() else {
            return;
        };
        let mut sources = vec![
            ("all".to_string(), "综合搜索".to_string(), "内置", true),
            (
                "files".into(),
                "文件搜索".into(),
                "文件",
                settings.everything.enabled,
            ),
        ];
        sources.extend(settings.web_search.engines.iter().map(|e| {
            (
                format!("web:{}", e.id),
                e.name.clone(),
                "网页",
                settings.web_search.enabled,
            )
        }));
        let active_plugins = plugin_catalog::search_scopes(&settings.plugins)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<std::collections::HashSet<_>>();
        sources.extend(self.catalog.iter().filter(|p| !p.id.is_empty()).map(|p| {
            (
                format!("plugin:{}", p.id),
                p.name.clone(),
                "插件",
                settings.plugins.enabled && active_plugins.contains(&p.id),
            )
        }));
        let Some(Value::Array(items)) =
            at_mut(&mut self.draft.value, &[key("scope_menu"), key("items")])
        else {
            return;
        };
        for (id, _, _, _) in &sources {
            if !items
                .iter()
                .any(|e| e.get("id").and_then(Value::as_str) == Some(id))
            {
                if let Ok(item) = Value::try_from(mini_launcher::scope_menu::Item::new(id, "")) {
                    items.push(item);
                }
            }
        }
        self.scope_rows = items
            .iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?.to_string();
                let source = sources.iter().find(|s| s.0 == id);
                let (title, kind, available) = source.map(|s| (s.1.clone(), s.2, s.3)).unwrap_or((
                    format!(
                        "已移除：{}",
                        id.split_once(':').map_or(id.as_str(), |(_, name)| name)
                    ),
                    "未安装",
                    false,
                ));
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(name.clone())
                        .placeholder(format!("沿用“{title}”"))
                });
                self.subscriptions
                    .push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.queue_save(cx);
                            cx.notify();
                        }
                    }));
                let shortcut = settings.scope_menu.shortcut(&id, &settings.web_search.default_engine);
                let shortcut_input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(shortcut.clone())
                        .placeholder("无快捷键")
                });
                self.subscriptions.push(cx.subscribe(&shortcut_input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.queue_save(cx);
                        cx.notify();
                    }
                }));
                let shortcut_focus = cx.focus_handle();
                let blur_id = id.clone();
                self.subscriptions.push(cx.on_blur(&shortcut_focus, window, move |this, _, cx| {
                    if let Some(row) = this.scope_rows.iter_mut().find(|row| row.id == blur_id) {
                        row.recording = false;
                        row.recording_error = None;
                        cx.notify();
                    }
                }));
                Some(ScopeRow {
                    id,
                    source: title,
                    kind,
                    available,
                    name: input,
                    original_name: name,
                    shortcut: shortcut_input,
                    original_shortcut: shortcut,
                    shortcut_focus,
                    recording: false,
                    recording_error: None,
                    error: None,
                    anchor: ScrollAnchor::for_handle(self.scroll_handle.clone()),
                })
            })
            .collect();
    }

    pub(super) fn collect_scope(&mut self, cx: &App) -> Result<Value, String> {
        let mut seen = std::collections::HashMap::<String, usize>::new();
        for index in 0..self.scope_rows.len() { self.scope_rows[index].error = None; }
        for index in 0..self.scope_rows.len() {
            let row = &self.scope_rows[index];
            let name = row.name.read(cx).value().trim().to_string();
            let shortcut = row.shortcut.read(cx).value().trim().to_string();
            if name.chars().count() > 80 || name.chars().any(char::is_control) {
                self.scope_rows[index].error = Some("名称最多 80 字，不能包含控制字符".into());
            }
            if shortcut.is_empty() { continue; }
            match mini_launcher::hotkey::Hotkey::parse(&shortcut) {
                Ok(key) if key.modifiers == 1 && (0x30..=0x39).contains(&key.key) => {
                    if let Some(previous) = seen.insert(key.label.clone(), index) {
                        let error = format!("快捷键 {} 与其他范围重复", key.label);
                        self.scope_rows[previous].error = Some(error.clone());
                        self.scope_rows[index].error = Some(error);
                    }
                }
                _ => self.scope_rows[index].error = Some("请使用 Alt+0–9，留空禁用".into()),
            }
        }
        if let Some(error) = self.scope_rows.iter().find_map(|row| row.error.clone()) {
            return Err(error);
        }
        let mut items = at(&self.draft.value, &[key("scope_menu"), key("items")])
            .cloned()
            .ok_or("缺少范围菜单配置")?;
        if let Value::Array(items) = &mut items {
            for item in items {
                if let Some(row) = self
                    .scope_rows
                    .iter()
                    .find(|r| item.get("id").and_then(Value::as_str) == Some(&r.id))
                {
                    let name = row.name.read(cx).value().trim().to_string();
                    let shortcut = row.shortcut.read(cx).value().trim().to_string();
                    update_scope_item(
                        item,
                        (name != row.original_name).then_some(name),
                        (shortcut != row.original_shortcut).then_some(shortcut),
                    )?;
                }
            }
        }
        let mut menu: mini_launcher::scope_menu::MenuSettings =
            Value::Table([("items".to_string(), items.clone())].into_iter().collect())
                .try_into()
                .map_err(|e: toml::de::Error| e.to_string())?;
        menu.validate()?;
        let default_engine = at(&self.draft.value, &[key("web_search"), key("default_engine")])
            .and_then(Value::as_str).unwrap_or("");
        menu.validate_shortcuts(default_engine)?;
        Ok(items)
    }

    pub(super) fn focus_scope_error(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.scope_rows.iter().find(|row| row.error.is_some()) {
            row.anchor.scroll_to(window, cx);
            let name = row.name.read(cx).value().trim().to_string();
            let input = if name.chars().count() > 80 || name.chars().any(char::is_control) {
                &row.name
            } else {
                &row.shortcut
            };
            input.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    pub(super) fn mark_scope_saved(&mut self, cx: &App) {
        for row in &mut self.scope_rows {
            row.original_name = row.name.read(cx).value().trim().to_string();
            row.original_shortcut = row.shortcut.read(cx).value().trim().to_string();
        }
    }

    fn show_scope(&mut self, id: &str, show: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(Value::Array(items)) =
            at_mut(&mut self.draft.value, &[key("scope_menu"), key("items")])
        {
            if let Some(item) = items
                .iter_mut()
                .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
            {
                if let Some(table) = item.as_table_mut() {
                    table.insert("visible".into(), Value::Boolean(show));
                }
            }
        }
        self.queue_save(cx);
    }

    fn move_scope(&mut self, id: &str, target: &str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let result = self
            .collect_scope(cx)
            .and_then(|items| self.draft.set(&[key("scope_menu"), key("items")], items));
        if let Err(e) = result {
            self.error = Some(e);
            cx.notify();
            return;
        }
        if let Some(Value::Array(items)) =
            at_mut(&mut self.draft.value, &[key("scope_menu"), key("items")])
        {
            if page_model::move_id(items, id, target) {
                let from = self.scope_rows.iter().position(|r| r.id == id).unwrap();
                let to = self.scope_rows.iter().position(|r| r.id == target).unwrap();
                let row = self.scope_rows.remove(from);
                self.scope_rows.insert(to, row);
            }
        }
        self.error = None;
        self.queue_save(cx);
        cx.notify();
    }

    fn visible(&self, id: &str) -> bool {
        at(&self.draft.value, &[key("scope_menu"), key("items")])
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
            })
            .and_then(|item| item.get("visible"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub(super) fn render_scope(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let accent = cx.theme().primary;
        let shown = self
            .scope_rows
            .iter()
            .filter(|r| self.visible(&r.id) && r.available)
            .collect::<Vec<_>>();
        div().flex().flex_col().gap_2()
            .child(div().text_sm().text_color(muted).child("拖动手柄或使用箭头调整顺序。名称留空时沿用来源名称；快捷键支持 Alt+0–9，留空禁用，仅在启动器内生效。开关只控制菜单显示。"))
            .child(div().p_4().rounded_lg().border_1().border_color(border).flex().flex_col().gap_3()
                .child(div().flex().items_center().gap_2().child("菜单预览").child(Tag::secondary().small().child(format!("{} 项可用", shown.len()))))
                .child(div().flex().flex_wrap().gap_2().children(shown.iter().map(|row| {
                    let name = row.name.read(cx).value();
                    Tag::secondary().outline().child(if name.trim().is_empty() { row.source.clone() } else { name.trim().to_string() })
                })))
                .when(shown.is_empty(), |el| el.child(div().text_sm().text_color(muted).child("当前没有可显示的来源。"))))
            .children(self.scope_rows.iter().enumerate().map(|(index, row)| {
                let id = row.id.clone(); let toggle_id = id.clone(); let drop_id = id.clone(); let up_id = id.clone(); let down_id = id.clone();
                let previous = index.checked_sub(1).map(|i| self.scope_rows[i].id.clone());
                let next = self.scope_rows.get(index + 1).map(|r| r.id.clone());
                let visible = self.visible(&id);
                let title = row.name.read(cx).value();
                let drag = ScopeDrag { id: id.clone(), title: if title.trim().is_empty() { row.source.clone() } else { title.to_string() } };
                div().id(SharedString::from(format!("scope-row-{id}"))).anchor_scroll(Some(row.anchor.clone())).px_3().py_2().rounded_lg().border_1().border_color(if row.error.is_some() { cx.theme().danger } else { border }).flex().items_center().gap_3()
                    .drag_over::<ScopeDrag>(move |style, _, _, _| style.border_color(accent).bg(accent.opacity(0.08)))
                    .on_drop(cx.listener(move |this, drag: &ScopeDrag, _, cx| this.move_scope(&drag.id, &drop_id, cx)))
                    .child(div().id(SharedString::from(format!("scope-drag-{id}"))).cursor_grab().p_1().text_color(muted)
                        .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())).child(Icon::new(Glyph::GripVertical)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().flex().items_center().gap_2().child(Icon::new(scope_icon(row.kind)).small()).child(row.source.clone())
                            .child(Tag::secondary().small().child(row.kind))
                            .when(!row.available, |el| el.child(Tag::warning().small().child("暂不可用"))))
                        .child(Input::new(&row.name).small().disabled(self.busy))
                        .when_some(row.error.clone(), |el, error| el.child(div().text_sm().text_color(cx.theme().danger).child(error))))
                    .child(div().w(px(180.)).flex_shrink_0().flex().flex_col().gap_1()
                        .child(div().text_xs().text_color(muted).child("范围快捷键"))
                        .child(Input::new(&row.shortcut).small().disabled(self.busy))
                        .child(div().flex().gap_1()
                            .child(Button::new(("scope-record", index)).label("按键录入").small().disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let row = &mut this.scope_rows[index];
                                    row.recording = true;
                                    row.recording_error = None;
                                    row.shortcut_focus.focus(window, cx);
                                    cx.notify();
                                })))
                            .child(Button::new(("scope-clear", index)).label("清空").small().ghost().disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let row = &mut this.scope_rows[index];
                                    row.recording = false;
                                    row.recording_error = None;
                                    row.shortcut.update(cx, |state, cx| state.set_value("", window, cx));
                                    this.queue_save(cx);
                                    cx.notify();
                                }))))
                        .when(row.recording && !self.busy, |el| el.child(
                            div().id(("scope-capture", index)).track_focus(&row.shortcut_focus)
                                .text_xs().text_color(muted)
                                .child(row.recording_error.clone().unwrap_or_else(|| "请按 Alt+0–9；Esc 取消。".into()))
                                .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    if event.is_held || this.busy { return; }
                                    let row = &mut this.scope_rows[index];
                                    if !row.recording { return; }
                                    if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                                        row.recording = false;
                                        row.recording_error = None;
                                    } else {
                                        match recorded_scope_shortcut(&event.keystroke) {
                                            Ok(Some(text)) => {
                                                row.shortcut.update(cx, |state, cx| state.set_value(text, window, cx));
                                                row.recording = false;
                                                row.recording_error = None;
                                                this.queue_save(cx);
                                            }
                                            Ok(None) => {},
                                            Err(error) => row.recording_error = Some(error),
                                        }
                                    }
                                    cx.notify();
                                })))))
                    .child(div().flex().gap_1()
                        .child(Button::new(("scope-up", index)).icon(Glyph::ChevronUp).ghost().small().tooltip("上移").disabled(self.busy || previous.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| { if let Some(target) = &previous { this.move_scope(&up_id, target, cx); } })))
                        .child(Button::new(("scope-down", index)).icon(Glyph::ChevronDown).ghost().small().tooltip("下移").disabled(self.busy || next.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| { if let Some(target) = &next { this.move_scope(&down_id, target, cx); } }))))
                    .child(Switch::new(("scope-shown", index)).label("显示").checked(visible)
                        .on_click(cx.listener(move |this, checked, _, cx| this.show_scope(&toggle_id, *checked, cx))))
            })).into_any_element()
    }
}
fn scope_icon(kind: &str) -> Glyph {
    match kind {
        "网页" => Glyph::Globe,
        "文件" => Glyph::Folder,
        "插件" => Glyph::Blocks,
        _ => Glyph::Search,
    }
}

#[cfg(test)]
mod tests {
    use super::{recorded_scope_shortcut, update_scope_item};
    use crate::model::{key, Draft};
    use gpui_kit::{Keystroke, Modifiers};
    use mini_launcher::config;
    use toml::Value;

    #[test]
    fn saving_legacy_scope_items_inserts_and_clears_shortcuts_without_losing_preferences() {
        let path = std::env::temp_dir().join(format!(
            "mini-launcher-scope-save-{}-{}.toml", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        let mut initial: Value = toml::from_str(config::DEFAULT_SETTINGS).unwrap();
        initial["plugins"]["enabled"] = Value::Boolean(false);
        initial["plugins"]["directories"] = Value::Array(vec![]);
        initial["scope_menu"]["items"] = toml::from_str::<Value>(
            "items=[{id='all',name='综合',visible=false},{id='files'}]"
        ).unwrap()["items"].clone();
        std::fs::write(&path, format!("# preserve comment\n{}", toml::to_string(&initial).unwrap())).unwrap();
        let mut draft = Draft::load(path.clone()).unwrap();
        let mut items = draft.value["scope_menu"]["items"].clone();
        let rows = items.as_array_mut().unwrap();
        assert!(rows[0].get("shortcut").is_none());
        update_scope_item(&mut rows[0], None, Some("Alt+9".into())).unwrap();
        update_scope_item(&mut rows[1], Some("我的文件".into()), None).unwrap();
        assert!(rows[1].get("shortcut").is_none());
        draft.set(&[key("scope_menu"), key("items")], items).unwrap();
        draft.save(false).unwrap();
        let mut reloaded = Draft::load(path.clone()).unwrap();
        let rows = reloaded.value["scope_menu"]["items"].as_array_mut().unwrap();
        assert_eq!(rows[0]["shortcut"].as_str(), Some("Alt+9"));
        assert_eq!(rows[0]["name"].as_str(), Some("综合"));
        assert_eq!(rows[0]["visible"].as_bool(), Some(false));
        assert_eq!(rows[1]["name"].as_str(), Some("我的文件"));
        update_scope_item(&mut rows[0], None, Some(String::new())).unwrap();
        reloaded.save(false).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# preserve comment"));
        let (settings, _) = config::parse_and_complete(&saved).unwrap();
        assert_eq!(settings.scope_menu.shortcut("all", &settings.web_search.default_engine), "");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn scope_recording_accepts_alt_digits_and_ignores_modifier_keys() {
        let stroke = |key: &str, modifiers| Keystroke {
            key: key.into(), modifiers, key_char: None,
        };
        for key in ["0", "1", "9"] {
            assert_eq!(recorded_scope_shortcut(&stroke(key, Modifiers {
                alt: true, ..Default::default()
            })).unwrap(), Some(format!("Alt+{key}")));
        }
        assert_eq!(recorded_scope_shortcut(&stroke("alt", Modifiers::default())).unwrap(), None);
        for (key, modifiers) in [
            ("1", Modifiers::default()),
            ("a", Modifiers { alt: true, ..Default::default() }),
            ("1", Modifiers { alt: true, control: true, ..Default::default() }),
        ] {
            assert!(recorded_scope_shortcut(&stroke(key, modifiers)).is_err());
        }
    }
}

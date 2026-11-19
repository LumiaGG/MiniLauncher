use super::*;
use crate::model::FieldPath;
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::component::{
    input::{Input, InputEvent, InputState, NumberInput, Textarea, TextareaState},
    select::{Select, SelectEvent, SelectItem, SelectState},
    tab::{Tab, TabBar},
    IndexPath, Sizable,
};

#[derive(Clone)]
struct Choice {
    value: String,
    title: String,
}
impl SelectItem for Choice {
    type Value = String;
    fn value(&self) -> &String {
        &self.value
    }
    fn title(&self) -> SharedString {
        self.title.clone().into()
    }
}

/// Convert physical key events to the same labels accepted by the host parser.
pub(super) fn recorded_hotkey(stroke: &Keystroke) -> Result<Option<String>, String> {
    if matches!(
        stroke.key.as_str(),
        "control" | "ctrl" | "alt" | "shift" | "win" | "super" | "platform"
    ) {
        return Ok(None);
    }
    let mut parts = Vec::new();
    for (pressed, name) in [
        (stroke.modifiers.control, "Ctrl"),
        (stroke.modifiers.alt, "Alt"),
        (stroke.modifiers.shift, "Shift"),
        (stroke.modifiers.platform, "Win"),
    ] {
        if pressed {
            parts.push(name);
        }
    }
    parts.push(&stroke.key);
    mini_launcher::hotkey::Hotkey::parse(&parts.join("+"))
        .map(|hotkey| Some(hotkey.label))
        .map_err(|_| "不支持此按键，请使用字母、数字、F1–F24、空格或导航键。".into())
}

enum Editor {
    Input(Entity<InputState>),
    Number(Entity<InputState>),
    /// Free-form lines, only for arrays that are not plain string lists.
    Lines(Entity<TextareaState>),
    /// One input per item for string lists (paths, keywords).
    List(Vec<Entity<InputState>>),
    Select(Entity<SelectState<Vec<Choice>>>),
    /// Short enums: written to the draft on click, nothing to collect.
    Segment(Vec<Choice>),
}
pub(super) struct Field {
    pub path: FieldPath,
    pub title: String,
    pub error: Option<String>,
    anchor: ScrollAnchor,
    help: String,
    original: Value,
    editor: Option<Editor>,
    browse: bool,
    secret: bool,
    hotkey_focus: Option<FocusHandle>,
    recording: bool,
    recording_error: Option<String>,
    group: &'static str,
    /// The `enabled` switch of a section; rendered first, in its own card.
    master: bool,
    visible_when: serde_json::Value,
}
impl Field {
    pub fn changed_value(&self, cx: &App) -> Result<Option<Value>, String> {
        Ok(self.value(cx)?.filter(|value| value != &self.original))
    }

    pub fn mark_saved(&mut self, cx: &App) {
        if self.error.is_some() { return; }
        if let Ok(Some(value)) = self.value(cx) {
            self.original = value;
        }
    }

    pub fn value(&self, cx: &App) -> Result<Option<Value>, String> {
        let text = match &self.editor {
            Some(Editor::Input(state) | Editor::Number(state)) => {
                state.read(cx).value().to_string()
            }
            Some(Editor::Lines(state)) => state.read(cx).value().to_string(),
            Some(Editor::List(rows)) => {
                return Ok(Some(Value::Array(
                    rows.iter()
                        .map(|row| row.read(cx).value().trim().to_string())
                        .filter(|text| !text.is_empty())
                        .map(Value::String)
                        .collect(),
                )))
            }
            Some(Editor::Select(state)) => {
                return Ok(state.read(cx).selected_value().cloned().map(Value::String))
            }
            Some(Editor::Segment(_)) | None => return Ok(None),
        };
        parse(&self.original, &text)
            .map(Some)
            .map_err(|e| format!("{}：{e}", self.title))
    }
}

impl Settings {
    pub(super) fn plugin_priority_editor(&self, config_key: &str, cx: &App) -> AnyElement {
        let path = vec![key("plugins"), key(config_key), key("priority")];
        let editor = self
            .fields
            .iter()
            .find(|field| field.path == path)
            .and_then(|field| field.editor.as_ref());
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("优先级"),
            )
            .when_some(editor, |row, editor| {
                if let Editor::Number(state) = editor {
                    row.child(
                        div().w(px(120.)).child(
                            NumberInput::new(state)
                                .small()
                                .disabled(self.busy)
                                .accessibility_label(format!(
                                    "{config_key} 插件优先级，数值越大越靠前"
                                )),
                        ),
                    )
                } else {
                    row
                }
            })
            .into_any_element()
    }

    fn schema_order(&self, path: &[Part]) -> Vec<String> {
        let [Part::Key(section), Part::Key(plugin)] = path else {
            return vec![];
        };
        if section != "plugins" {
            return vec![];
        }
        let Some(info) = self.catalog.iter().find(|p| &p.config_key == plugin) else {
            return vec![];
        };
        model::schema(&info.directory)
            .ok()
            .and_then(|spec| {
                spec["fields"]
                    .as_object()
                    .map(|fields| fields.keys().cloned().collect())
            })
            .unwrap_or_default()
    }

    fn row_input(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let state = cx.new(|cx| InputState::new(window, cx).default_value(text));
        self.subscriptions
            .push(cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.queue_save(cx);
                    cx.notify();
                }
            }));
        state
    }

    pub(super) fn add_fields(
        &mut self,
        path: FieldPath,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Value::Table(table) = &value {
            let schema_order = self.schema_order(&path);
            let mut entries = table.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(name, _)| {
                let mut child = path.clone();
                child.push(key(name.as_str()));
                (
                    layout(&model::path_name(&child)).1,
                    schema_order
                        .iter()
                        .position(|s| s == *name)
                        .unwrap_or(usize::MAX),
                )
            });
            for (name, value) in entries {
                if matches!(path.as_slice(), [Part::Key(section), Part::Key(_)] if section == "plugins")
                    && self.plugin_credentials.as_ref().is_some_and(|editor| &editor.target_field == name)
                {
                    continue;
                }
                let mut child = path.clone();
                child.push(key(name));
                self.add_fields(child, value.clone(), window, cx);
            }
            return;
        }
        if let Value::Array(array) = &value {
            if array.iter().any(Value::is_table) {
                for (index, value) in array.iter().enumerate() {
                    let mut child = path.clone();
                    child.push(Part::Index(index));
                    self.add_fields(child, value.clone(), window, cx);
                }
                return;
            }
        }
        let name = model::path_name(&path);
        let leaf = path
            .last()
            .and_then(|p| {
                if let Part::Key(k) = p {
                    Some(k.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");
        let plugin_field = matches!(
            path.as_slice(),
            [Part::Key(section), Part::Key(_), Part::Key(_)] if section == "plugins"
        );
        let alias = matches!(path.first(), Some(Part::Key(k)) if k == "aliases");
        let master = leaf == "enabled" && value.is_bool();
        let mut title = label(leaf).to_string();
        let mut help = hint(&name).to_string();
        let mut choices = choices(&name);
        let mut browse = alias || matches!(leaf, "search_paths" | "directories");
        let mut visible_when = serde_json::Value::Null;
        let mut hotkey = name == "hotkey";
        if let Some((text, note)) = describe(&name) {
            title = text.into();
            help = note.into();
        }
        if let [Part::Key(section), Part::Key(plugin), Part::Key(field)] = path.as_slice() {
            if section == "plugins" {
                if let Some(info) = self.catalog.iter().find(|p| &p.config_key == plugin) {
                    if let Ok(spec) = model::schema(&info.directory) {
                        let rule = &spec["fields"][field];
                        hotkey = rule["format"].as_str() == Some("hotkey");
                        visible_when = rule["visible_when"].clone();
                        if let Some(text) = rule["title"].as_str() {
                            title = text.into();
                        }
                        if let Some(text) = rule["description"].as_str() {
                            help = text.into();
                        }
                        browse |= rule["type"].as_str() == Some("path");
                        if let Some(values) = rule["enum"].as_array() {
                            choices = values
                                .iter()
                                .filter_map(|v| v.as_str().map(|s| (s.into(), s.into())))
                                .collect();
                        }
                    }
                }
            }
        }
        if master && plugin_field {
            title = "启用插件".into();
            help = String::new();
        }
        if name == "web_search.default_engine" {
            choices = self
                .draft
                .value
                .get("web_search")
                .and_then(|v| v.get("engines"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    Some((
                        e.get("id")?.as_str()?.into(),
                        e.get("name")?.as_str()?.into(),
                    ))
                })
                .collect();
        }
        if let Some(index) = path.iter().find_map(|p| {
            if let Part::Index(i) = p {
                Some(*i)
            } else {
                None
            }
        }) {
            title = format!("第 {} 项 · {title}", index + 1);
        }
        let secret = leaf.contains("token") || leaf.contains("password") || leaf.contains("secret");
        let editor = if value.is_bool() {
            None
        } else if !choices.is_empty() {
            let items = choices
                .into_iter()
                .map(|(value, title)| Choice { value, title })
                .collect::<Vec<_>>();
            if items.len() <= 3 && name != "web_search.default_engine" {
                Some(Editor::Segment(items))
            } else {
                let selected = items.iter().position(|c| value.as_str() == Some(&c.value));
                let state = cx.new(|cx| {
                    SelectState::new(items, selected.map(IndexPath::new), window, cx)
                        .searchable(name == "web_search.default_engine")
                });
                let field_index = self.fields.len();
                self.subscriptions.push(cx.subscribe(
                    &state,
                    move |this, _, event: &SelectEvent<Vec<Choice>>, cx| {
                        if let SelectEvent::Confirm(Some(value)) = event {
                            this.set_field(field_index, Value::String(value.clone()), cx);
                        }
                    },
                ));
                Some(Editor::Select(state))
            }
        } else if matches!(&value, Value::Array(items) if items.iter().all(Value::is_str)) {
            let texts = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            let rows = texts
                .into_iter()
                .map(|text| self.row_input(text, window, cx))
                .collect();
            Some(Editor::List(rows))
        } else if value.is_array() {
            let text = display(&value);
            let state = cx.new(|cx| TextareaState::new(window, cx).rows(4).default_value(text));
            self.subscriptions
                .push(cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.queue_save(cx);
                        cx.notify();
                    }
                }));
            Some(Editor::Lines(state))
        } else {
            let text = display(&value);
            let state = cx.new(|cx| {
                let state = InputState::new(window, cx)
                    .default_value(text)
                    .masked(secret);
                if value.is_integer() {
                    let mut state = state.step(1.0);
                    let bounds = match name.as_str() {
                        "application_priority" | "everything.priority" => {
                            Some((i32::MIN as f64, i32::MAX as f64))
                        }
                        _ if plugin_field && leaf == "priority" => {
                            Some((i32::MIN as f64, i32::MAX as f64))
                        }
                        "window_width" => Some((480., 1440.)),
                        "max_results" => Some((1., 8.)),
                        "plugins.max_processes" => Some((1., 4.)),
                        "everything.max_results" => Some((1., 1000.)),
                        "everything.timeout_ms" => Some((100., 5000.)),
                        _ => None,
                    };
                    if let Some((min, max)) = bounds {
                        state = state.min(min).max(max);
                    }
                    state
                } else {
                    state
                }
            });
            self.subscriptions
                .push(cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.queue_save(cx);
                        cx.notify();
                    }
                }));
            Some(if value.is_integer() {
                Editor::Number(state)
            } else {
                Editor::Input(state)
            })
        };
        let hotkey_focus = (hotkey && value.is_str()).then(|| cx.focus_handle());
        if let Some(focus) = &hotkey_focus {
            let index = self.fields.len();
            self.subscriptions
                .push(cx.on_blur(focus, window, move |this, _, cx| {
                    if let Some(field) = this.fields.get_mut(index) {
                        field.recording = false;
                        field.recording_error = None;
                        cx.notify();
                    }
                }));
        }
        self.fields.push(Field {
            path,
            title,
            error: None,
            anchor: ScrollAnchor::for_handle(self.scroll_handle.clone()),
            help,
            original: value,
            editor,
            browse,
            secret,
            hotkey_focus,
            recording: false,
            recording_error: None,
            group: layout(&name).0,
            master,
            visible_when,
        });
    }

    fn set_field(&mut self, index: usize, value: Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let path = self.fields[index].path.clone();
        match self
            .draft
            .validate_field(&path, &value)
            .and_then(|_| self.draft.set(&path, value))
        {
            Ok(()) => {
                self.error = None;
                self.queue_save(cx);
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    fn list_add(
        &mut self,
        index: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let state = self.row_input(text, window, cx);
        if let Some(Editor::List(rows)) = &mut self.fields[index].editor {
            rows.push(state);
        }
        self.queue_save(cx);
        cx.notify();
    }

    fn list_remove(&mut self, index: usize, row: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(Editor::List(rows)) = &mut self.fields[index].editor {
            if row < rows.len() {
                rows.remove(row);
            }
        }
        self.queue_save(cx);
        cx.notify();
    }

    fn browse(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let field = &self.fields[index];
        let name = model::path_name(&field.path);
        let directory = field.original.is_array() || name.ends_with("directory");
        self.pick(Picker::Field(index, directory), window, cx);
    }

    pub(super) fn apply_chosen(
        &mut self,
        index: usize,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = path.to_string_lossy().to_string();
        if matches!(self.fields[index].editor, Some(Editor::List(_))) {
            self.list_add(index, text, window, cx);
            return;
        }
        match &self.fields[index].editor {
            Some(Editor::Input(state) | Editor::Number(state)) => {
                state.update(cx, |s, cx| s.set_value(text, window, cx))
            }
            Some(Editor::Lines(state)) => state.update(cx, |s, cx| {
                let old = s.value();
                let text = if old.is_empty() {
                    text
                } else {
                    format!("{}\n{text}", old.trim_end())
                };
                s.set_value(text, window, cx);
            }),
            _ => {}
        }
        self.queue_save(cx);
        cx.notify();
    }

    /// A field is dimmed and read-only while the `enabled` switch of its section is off.
    fn gated(&self, path: &[Part]) -> bool {
        let Some((Part::Key(leaf), parent)) = path.split_last() else {
            return false;
        };
        if leaf == "enabled" || parent.is_empty() {
            return false;
        }
        let mut master = parent.to_vec();
        master.push(key("enabled"));
        at(&self.draft.value, &master).and_then(Value::as_bool) == Some(false)
    }

    /// Whether the `enabled` switch of a top-level section is off.
    pub(super) fn section_off(&self, section: &str) -> bool {
        at(&self.draft.value, &[key(section), key("enabled")]).and_then(Value::as_bool)
            == Some(false)
    }

    fn render_field(
        &self,
        index: usize,
        field: &Field,
        muted: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gated = self.gated(&field.path);
        let off = self.busy || gated;
        let value = at(&self.draft.value, &field.path).unwrap_or(&field.original);
        let title = field.title.clone();
        let alias = matches!(field.path.first(), Some(Part::Key(k)) if k == "aliases");
        let mut wide = false;
        let browse_button = |label: &'static str, icon: Option<Glyph>, cx: &mut Context<Self>| {
            let button = Button::new(("browse", index))
                .label(label)
                .small()
                .disabled(off)
                .on_click(cx.listener(move |this, _, window, cx| this.browse(index, window, cx)));
            match icon {
                Some(icon) => button.icon(icon),
                None => button,
            }
        };
        let control: AnyElement = if let Some(on) = value.as_bool() {
            Switch::new(("field", index))
                .checked(on)
                .accessibility_label(title.clone())
                .disabled(off)
                .on_click(cx.listener(move |this, checked, _, cx| {
                    this.set_field(index, Value::Boolean(*checked), cx)
                }))
                .into_any_element()
        } else {
            match &field.editor {
                Some(Editor::Input(state)) if field.hotkey_focus.is_some() => {
                    wide = true;
                    let focus = field.hotkey_focus.as_ref().unwrap();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(240.)).child(Input::new(state).disabled(off)))
                                .child(
                                    Button::new(("record-hotkey", index))
                                        .label("按键录入")
                                        .small()
                                        .disabled(off)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let field = &mut this.fields[index];
                                            field.recording = true;
                                            field.recording_error = None;
                                            field.hotkey_focus.as_ref().unwrap().focus(window, cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div().flex().items_center().gap_2()
                                .child(div().text_sm().text_color(muted).child("双击按键"))
                                .children(["Ctrl", "Alt", "Shift", "Win"].into_iter().map(|key| {
                                    Button::new((key, index))
                                        .label(format!("双击 {key}"))
                                        .small()
                                        .disabled(off)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let field = &mut this.fields[index];
                                            field.recording = false;
                                            field.recording_error = None;
                                            if let Some(Editor::Input(state)) = &field.editor {
                                                state.update(cx, |s, cx| {
                                                    s.set_value(format!("{key}+{key}"), window, cx)
                                                });
                                            }
                                            this.queue_save(cx);
                                            cx.notify();
                                        }))
                                })),
                        )
                        .child(div().text_xs().text_color(muted).child(
                            "其他双击可直接输入 A+A、Space+Space 等。原按键行为仍会发生，Win 可能打开开始菜单，Shift 可能切换输入法。"
                        ))
                        .when(field.recording && !off, |el| {
                            el.child(
                                div()
                                    .id(("hotkey-capture", index))
                                    .track_focus(focus)
                                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                        this.fields[index].recording = false;
                                        this.fields[index].recording_error = None;
                                        cx.notify();
                                    }))
                                    .text_sm()
                                    .text_color(muted)
                                    .child(field.recording_error.clone().unwrap_or_else(|| {
                                        "请按下组合键；Esc 取消，点击其他位置退出。".into()
                                    }))
                                    .capture_key_down(cx.listener(
                                        move |this, event: &KeyDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            window.prevent_default();
                                            if event.is_held
                                                || this.busy
                                                || this.gated(&this.fields[index].path)
                                            {
                                                return;
                                            }
                                            let field = &mut this.fields[index];
                                            if event.keystroke.key == "escape"
                                                && !event.keystroke.modifiers.modified()
                                            {
                                                field.recording = false;
                                                field.recording_error = None;
                                            } else {
                                                match recorded_hotkey(&event.keystroke) {
                                                    Ok(Some(text)) => {
                                                        if let Some(Editor::Input(state)) =
                                                            &field.editor
                                                        {
                                                            state.update(cx, |s, cx| {
                                                                s.set_value(text, window, cx)
                                                            });
                                                        }
                                                        field.recording = false;
                                                        field.recording_error = None;
                                                        this.queue_save(cx);
                                                    }
                                                    Ok(None) => {}
                                                    Err(error) => {
                                                        field.recording_error = Some(error)
                                                    }
                                                }
                                            }
                                            cx.notify();
                                        },
                                    )),
                            )
                        })
                        .into_any_element()
                }
                Some(Editor::Input(state)) if field.browse => {
                    wide = true;
                    div()
                        .flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(state).small().disabled(off)),
                        )
                        .child(browse_button("浏览…", None, cx))
                        .into_any_element()
                }
                Some(Editor::Input(state)) => div()
                    .w(px(280.))
                    .child(
                        Input::new(state)
                            .disabled(off)
                            .when(field.secret, |input| input.mask_toggle()),
                    )
                    .into_any_element(),
                Some(Editor::Number(state)) => div()
                    .w(px(160.))
                    .child(
                        NumberInput::new(state)
                            .disabled(off)
                            .accessibility_label(title.clone()),
                    )
                    .into_any_element(),
                Some(Editor::Lines(state)) => {
                    wide = true;
                    Textarea::new(state)
                        .disabled(off)
                        .h(px(104.))
                        .into_any_element()
                }
                Some(Editor::List(rows)) => {
                    wide = true;
                    let mut list = div().flex().flex_col().gap_1().w_full();
                    for (row, state) in rows.iter().enumerate() {
                        list = list.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Input::new(state).small().disabled(off)),
                                )
                                .child(
                                    Button::new(("list-del", index * 1000 + row))
                                        .icon(Glyph::Close)
                                        .ghost()
                                        .small()
                                        .tooltip("移除")
                                        .disabled(off)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.list_remove(index, row, cx)
                                        })),
                                ),
                        );
                    }
                    if rows.is_empty() {
                        list = list.child(div().text_sm().text_color(muted).child("未设置"));
                    }
                    list.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new(("list-add", index))
                                    .icon(Glyph::Plus)
                                    .label("添加")
                                    .small()
                                    .disabled(off)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.list_add(index, String::new(), window, cx)
                                    })),
                            )
                            .when(field.browse, |el| {
                                el.child(browse_button("浏览添加…", Some(Glyph::FolderOpen), cx))
                            }),
                    )
                    .into_any_element()
                }
                Some(Editor::Select(state)) => div()
                    .w(px(220.))
                    .child(
                        Select::new(state)
                            .disabled(off)
                            .accessibility_label(title.clone()),
                    )
                    .into_any_element(),
                Some(Editor::Segment(choices)) => {
                    let current = value
                        .as_str()
                        .and_then(|v| choices.iter().position(|c| c.value == v))
                        .unwrap_or(0);
                    let values = choices.iter().map(|c| c.value.clone()).collect::<Vec<_>>();
                    let owner = cx.entity().downgrade();
                    TabBar::new(("segment", index))
                        .segmented()
                        .selected_index(current)
                        .children(
                            choices
                                .iter()
                                .map(|c| Tab::new().label(c.title.clone()).disabled(off)),
                        )
                        .on_click(move |i, _, cx| {
                            if let Some(v) = values.get(*i).cloned() {
                                let _ = owner.update(cx, |this, cx| {
                                    this.set_field(index, Value::String(v), cx)
                                });
                            }
                        })
                        .into_any_element()
                }
                None => div().into_any_element(),
            }
        };
        let help = if field.help.is_empty() && matches!(field.editor, Some(Editor::Lines(_))) {
            "每行一项；清空表示不设置".to_string()
        } else {
            field.help.clone()
        };
        let info = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().child(field.title.clone()))
            .when_some(field.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .when(!help.is_empty(), |el| {
                el.child(div().text_sm().text_color(muted).child(help))
            });
        if alias {
            return div()
                .id(("alias-field", index))
                .anchor_scroll(Some(field.anchor.clone()))
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(110.))
                        .flex_shrink_0()
                        .truncate()
                        .child(field.title.clone()),
                )
                .child(div().flex_1().min_w_0().child(control)
                    .when_some(field.error.clone(), |el, error| el.child(div().text_sm().text_color(cx.theme().danger).child(error))))
                .into_any_element();
        }
        if wide {
            div()
                .id(("wide-field", index))
                .anchor_scroll(Some(field.anchor.clone()))
                .flex()
                .flex_col()
                .gap_2()
                .child(info)
                .child(control)
                .when(gated, |el| el.opacity(0.5))
                .into_any_element()
        } else {
            div()
                .id(("setting-field", index))
                .anchor_scroll(Some(field.anchor.clone()))
                .flex()
                .items_center()
                .gap_4()
                .child(info.flex_1().min_w_0())
                .child(div().flex_shrink_0().child(control))
                .when(gated, |el| el.opacity(0.5))
                .into_any_element()
        }
    }

    pub(super) fn focus_first_error(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.fields.iter().find(|field| field.error.is_some()) {
            field.anchor.scroll_to(window, cx);
            match &field.editor {
                Some(Editor::Input(input) | Editor::Number(input)) => input.update(cx, |state, cx| state.focus(window, cx)),
                Some(Editor::List(rows)) => {
                    if let Some(input) = rows.first() { input.update(cx, |state, cx| state.focus(window, cx)); }
                }
                Some(Editor::Lines(input)) => input.update(cx, |state, cx| state.focus(window, cx)),
                _ => {}
            }
        } else if self.scope_error.is_some() {
            self.focus_scope_error(window, cx);
        }
    }

    /// Fields grouped into titled cards; every `enabled` switch gets a card of its own.
    pub(super) fn render_fields(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.render_field_groups(false, cx)
    }

    pub(super) fn render_alias_fields(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.render_field_groups(true, cx)
    }

    fn render_field_groups(&self, aliases: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, border, surface, radius) = (
            theme.muted_foreground,
            theme.border,
            theme.tokens.group_box,
            theme.radius,
        );
        let mut groups: Vec<(&'static str, bool, Vec<AnyElement>)> = vec![];
        for (index, field) in self.fields.iter().enumerate() {
            if self.page == 5
                && self.plugin.is_none()
                && matches!(field.path.as_slice(), [Part::Key(section), Part::Key(_), Part::Key(leaf)] if section == "plugins" && leaf == "priority")
            {
                continue;
            }
            let is_alias = matches!(field.path.first(), Some(Part::Key(k)) if k == "aliases");
            if is_alias != aliases {
                continue;
            }
            if !model::field_visible(&field.visible_when, |name| {
                let mut sibling = field.path.clone();
                *sibling.last_mut()? = key(name);
                self.fields
                    .iter()
                    .find(|candidate| candidate.path == sibling)
                    .and_then(|candidate| candidate.value(cx).ok().flatten())
                    .or_else(|| model::at(&self.draft.value, &sibling).cloned())
            }) {
                continue;
            }
            let row = self.render_field(index, field, muted, cx);
            match groups.last_mut() {
                Some((group, master, rows))
                    if !field.master && !*master && *group == field.group =>
                {
                    rows.push(row)
                }
                _ => groups.push((field.group, field.master, vec![row])),
            }
        }
        groups
            .into_iter()
            .map(|(title, _, rows)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(!title.is_empty(), |el| {
                        el.child(div().text_sm().text_color(muted).child(title))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(surface)
                            .children(rows.into_iter().enumerate().map(|(i, row)| {
                                div()
                                    .px_3()
                                    .py_2()
                                    .when(i > 0, |el| el.border_t_1().border_color(border))
                                    .child(row)
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) if items.iter().all(Value::is_str) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => model::literal(value).unwrap_or_default(),
    }
}
fn parse(original: &Value, text: &str) -> Result<Value, String> {
    match original {
        Value::String(_) => Ok(Value::String(text.into())),
        Value::Array(items) if items.iter().all(Value::is_str) => Ok(Value::Array(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|s| Value::String(s.into()))
                .collect(),
        )),
        _ => model::parse_value(text),
    }
}

/// Display order and card title of known fields. Unknown fields sort last, alphabetically.
const LAYOUT: &[(&str, &str)] = &[
    ("hotkey", "启动与唤出"),
    ("start_at_login", "启动与唤出"),
    ("portable_mode", "数据"),
    ("theme", "主题"),
    ("backdrop", "主题"),
    ("window_width", "搜索栏"),
    ("max_results", "搜索栏"),
    ("search_paths", ""),
    ("application_priority", "综合搜索"),
    ("everything.enabled", ""),
    ("everything.keyword", "触发方式"),
    ("everything.global_search", "触发方式"),
    ("everything.priority", "综合搜索"),
    ("everything.max_results", "查询选项"),
    ("everything.timeout_ms", "查询选项"),
    ("web_search.enabled", ""),
    ("web_search.mode", "展示"),
    ("web_search.default_engine", "展示"),
    ("plugins.enabled", ""),
    ("plugins.max_processes", "运行"),
    ("plugins.directories", "运行"),
];
fn layout(name: &str) -> (&'static str, usize) {
    if let Some(index) = LAYOUT.iter().position(|(known, _)| *known == name) {
        return (LAYOUT[index].1, index);
    }
    if let ["plugins", _, leaf] = name.split('.').collect::<Vec<_>>().as_slice() {
        return match *leaf {
            "enabled" => ("", 0),
            "keywords" => ("触发与显示", 1),
            "direct_display" => ("触发与显示", 2),
            "global_search" => ("触发与显示", 3),
            _ => ("插件选项", 10),
        };
    }
    ("", usize::MAX)
}
fn describe(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "application_priority" => (
            "应用搜索优先级",
            "与文件搜索、插件一起排序，数值越大越靠前；默认 0。",
        ),
        "everything.priority" => (
            "文件搜索优先级",
            "与应用搜索、插件一起排序，数值越大越靠前；默认 0。",
        ),
        "everything.enabled" => ("启用文件搜索", "关闭后文件搜索不可用。"),
        "everything.keyword" => ("触发关键词", "输入此关键词后进入文件搜索。"),
        "everything.global_search" => ("参与综合搜索", "在综合搜索结果中包含文件。"),
        "everything.max_results" => ("文件结果数量", "1–1000 条。"),
        "everything.timeout_ms" => ("查询超时", "100–5000 毫秒。"),
        "web_search.enabled" => ("启用网页搜索", "关闭后所有网页搜索引擎不可用。"),
        "web_search.mode" => ("结果展示方式", "决定网页搜索结果何时出现在综合搜索中。"),
        "web_search.default_engine" => ("默认搜索引擎", "综合搜索时使用的引擎。"),
        "plugins.enabled" => ("启用插件系统", "关闭后所有插件停用，不会启动任何插件进程。"),
        _ => return None,
    })
}
fn label(name: &str) -> &str {
    match name {
        "hotkey" => "唤出快捷键",
        "start_at_login" => "开机启动",
        "portable_mode" => "便携模式",
        "theme" => "主题",
        "backdrop" => "背景材质",
        "window_width" => "搜索栏宽度",
        "max_results" => "显示结果数量",
        "search_paths" => "应用扫描目录",
        "enabled" => "启用",
        "keyword" => "触发关键词",
        "keywords" => "触发关键词",
        "direct_display" => "空输入时直接显示",
        "priority" => "优先级",
        "global_search" => "参与全局搜索",
        "timeout_ms" => "查询超时（毫秒）",
        "mode" => "显示方式",
        "default_engine" => "默认搜索引擎",
        "id" => "标识",
        "name" => "名称",
        "url" => "搜索地址",
        "directories" => "插件搜索目录",
        "max_processes" => "插件进程上限",
        "visible" => "在范围菜单中显示",
        _ => name,
    }
}
fn hint(name: &str) -> &str {
    match name {
        "hotkey" => {
            "组合键如 Alt+Space；同键双击如 Ctrl+Ctrl、Win+Win、A+A。双击无法检测其他软件的占用。"
        }
        "portable_mode" => {
            "开启后数据保存在安装目录 data；切换后退出设置与宿主并重新启动，完成数据迁移。"
        }
        "window_width" => "480–1440 个逻辑像素，按屏幕缩放比例显示。",
        "max_results" => "1–8 行；多行文本和图片可能占据多行空间。",
        "search_paths" => "添加额外的应用或快捷方式目录。",
        "plugins.directories" => "留空使用数据目录下的 plugins；自定义列表会替代默认目录。",
        "plugins.max_processes" => "1–4 个保温插件进程。",
        _ => "",
    }
}
fn choices(name: &str) -> Vec<(String, String)> {
    let values: &[(&str, &str)] = match name {
        "theme" => &[("system", "跟随系统"), ("light", "浅色"), ("dark", "深色")],
        "backdrop" => &[("none", "实色"), ("mica", "云母"), ("acrylic", "透明模糊")],
        "web_search.mode" => &[
            ("fallback", "无匹配时"),
            ("always", "始终显示"),
            ("keyword_only", "仅关键词"),
        ],
        _ => &[],
    };
    values
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{display, layout, parse, recorded_hotkey, Keystroke, Modifiers};
    use toml::Value;
    #[test]
    fn recorded_keys_match_host_hotkey_syntax() {
        for (key, modifiers, expected) in [
            (
                "q",
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
                "Alt+Q",
            ),
            (
                "space",
                Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                "Ctrl+Shift+Space",
            ),
            (
                "f24",
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
                "Win+F24",
            ),
            ("pageup", Modifiers::default(), "PageUp"),
            (
                "tab",
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
                "Alt+Tab",
            ),
            (
                "escape",
                Modifiers {
                    control: true,
                    ..Default::default()
                },
                "Ctrl+Escape",
            ),
        ] {
            let stroke = Keystroke {
                key: key.into(),
                modifiers,
                key_char: None,
            };
            assert_eq!(recorded_hotkey(&stroke).unwrap().as_deref(), Some(expected));
        }
        for key in ["control", "alt", "shift", "platform"] {
            let stroke = Keystroke {
                key: key.into(),
                ..Default::default()
            };
            assert_eq!(recorded_hotkey(&stroke).unwrap(), None);
        }
        let unsupported = Keystroke {
            key: "/".into(),
            ..Default::default()
        };
        assert!(recorded_hotkey(&unsupported).is_err());
    }
    #[test]
    fn form_values_preserve_windows_paths_and_numeric_types() {
        let paths = Value::Array(vec![
            Value::String(r"D:\应用\工具".into()),
            Value::String(r"C:\Program Files".into()),
        ]);
        assert_eq!(parse(&paths, &display(&paths)).unwrap(), paths);
        assert_eq!(parse(&Value::Integer(8), "6").unwrap(), Value::Integer(6));
        assert!(parse(&Value::Integer(8), "abc").is_err());
        assert_eq!(
            parse(&Value::String(String::new()), "中文\"\\").unwrap(),
            Value::String("中文\"\\".into())
        );
    }
    #[test]
    fn enabled_switches_sort_before_the_rest_of_their_section() {
        for section in ["everything", "web_search", "plugins"] {
            let master = layout(&format!("{section}.enabled")).1;
            assert!(LAYOUT_NAMES
                .iter()
                .filter(|n| n.starts_with(section))
                .all(|n| layout(n).1 >= master));
        }
        assert_eq!(layout("plugins.demo.enabled").1, 0);
        assert!(layout("plugins.demo.keywords").1 < layout("plugins.demo.other").1);
    }
    const LAYOUT_NAMES: &[&str] = &[
        "everything.enabled",
        "everything.keyword",
        "everything.timeout_ms",
        "web_search.enabled",
        "web_search.mode",
        "plugins.enabled",
        "plugins.directories",
    ];
}

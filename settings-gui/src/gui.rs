use crate::{
    model::{self, at, at_mut, key, Draft, Part},
    packages, Instance,
};
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    switch::Switch,
    tag::Tag,
    ActiveTheme, Disableable, Sizable, Theme, ThemeMode, WindowExt,
};
use gpui_kit::{prelude::*, *};
use mini_launcher::{config, plugin_catalog};
use std::path::PathBuf;
use toml::Value;
mod about;
mod appearance;
mod applications;
mod collection;
mod credentials;
mod fields;
mod page_model;
mod scope;
mod two_factor;
mod web;
use fields::Field;
use gpui_kit::component::input::{Input, InputEvent, InputState};

const PAGES: [&str; 8] = [
    "常规",
    "外观",
    "应用搜索",
    "文件搜索",
    "网页搜索",
    "插件",
    "范围菜单",
    "关于",
];

// The default bundle lacks the icons used by the navigation and drag handles.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [GripVertical, Blocks, FolderSearch, ListOrdered]
);
struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn run(draft: Draft, plugins: bool, instance: Instance, startup_error: Option<String>) {
    let _instance = instance;
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::install(cx);
            let mode = match draft.value.get("theme").and_then(Value::as_str) {
                Some("dark") => ThemeMode::Dark,
                Some("light") => ThemeMode::Light,
                _ => cx.window_appearance().into(),
            };
            Theme::change(mode, None, cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1020.), px(760.)), cx);
            if let Err(error) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(760.), px(540.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("MiniLauncher 设置".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let view = cx.new(|cx| {
                        let mut view =
                            Settings::new(draft, if plugins { 5 } else { 0 }, window, cx);
                        view.error = startup_error;
                        view
                    });
                    let weak = view.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        weak.update(cx, |view, cx| view.confirm_close(window, cx))
                            .unwrap_or(true)
                    });
                    view
                },
            ) {
                rfd::MessageDialog::new()
                    .set_title("无法打开设置")
                    .set_description(error.to_string())
                    .show();
                cx.quit();
            }
            cx.activate(true);
        });
}

#[derive(Clone, Copy)]
enum Picker {
    InstallZip,
    InstallDirectory,
    Register,
    Field(usize, bool),
}
enum Question {
    Uninstall(PathBuf, String),
}

struct Settings {
    draft: Draft,
    app_rules: crate::applications::Rules,
    app_rules_error: Option<String>,
    page: usize,
    plugin: Option<String>,
    catalog: Vec<plugin_catalog::PluginInfo>,
    fields: Vec<Field>,
    plugin_credentials: Option<credentials::CredentialEditor>,
    scope_rows: Vec<scope::ScopeRow>,
    tfa: Vec<two_factor::TfaAccount>,
    tfa_error: Option<String>,
    subscriptions: Vec<Subscription>,
    edited: bool,
    autosave_task: Option<Task<()>>,
    window: AnyWindowHandle,
    busy: bool,
    status: String,
    error: Option<String>,
    list_filter: Entity<InputState>,
    _filter_subscription: Option<Subscription>,
    plugin_filter: usize,
    app_names_key: Option<(Value, Value)>,
    app_names_generation: u64,
    app_names_loading: bool,
    app_names_task: Option<Task<()>>,
    scroll_handle: ScrollHandle,
    scope_error: Option<String>,
}

impl Settings {
    fn new(draft: Draft, page: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rules = crate::applications::Rules::load(&draft.path);
        let app_rules_error = rules.as_ref().err().cloned();
        let list_filter = cx.new(|cx| InputState::new(window, cx).placeholder("搜索名称、路径或关键词"));
        let mut view = Self {
            draft,
            app_rules: rules.unwrap_or_default(),
            app_rules_error,
            page,
            plugin: None,
            catalog: vec![],
            fields: vec![],
            plugin_credentials: None,
            scope_rows: vec![],
            tfa: vec![],
            tfa_error: None,
            subscriptions: vec![],
            edited: false,
            autosave_task: None,
            window: window.window_handle(),
            busy: false,
            status: String::new(),
            error: None,
            list_filter: list_filter.clone(),
            _filter_subscription: Some(cx.subscribe(&list_filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) { cx.notify(); }
            })),
            plugin_filter: 0,
            app_names_key: None,
            app_names_generation: 0,
            app_names_loading: false,
            app_names_task: None,
            scroll_handle: ScrollHandle::new(),
            scope_error: None,
        };
        view.rebuild(window, cx);
        view
    }

    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fields.clear();
        self.scope_rows.clear();
        self.subscriptions.clear();
        if let Ok(settings) = self.draft.value.clone().try_into::<config::Settings>() {
            self.catalog = plugin_catalog::inspect(&settings.plugins);
        }
        self.plugin_credentials = self.current_plugin().and_then(|plugin| {
            credentials::CredentialEditor::new(plugin, &self.draft.value, window, cx)
        });
        let roots = if let Some(plugin) = &self.plugin {
            vec![vec![key("plugins"), key(plugin)]]
        } else {
            match self.page {
                0 => ["hotkey", "start_at_login", "portable_mode"]
                    .iter()
                    .map(|k| vec![key(k)])
                    .collect(),
                1 => ["theme", "backdrop", "window_width", "max_results"]
                    .iter()
                    .map(|k| vec![key(k)])
                    .collect(),
                2 => ["application_priority", "search_paths", "aliases"]
                    .iter()
                    .map(|k| vec![key(k)])
                    .collect(),
                3 => vec![vec![key("everything")]],
                4 => ["enabled", "mode", "default_engine"]
                    .iter()
                    .map(|k| vec![key("web_search"), key(k)])
                    .collect(),
                5 => ["enabled", "max_processes", "directories"]
                    .iter()
                    .map(|k| vec![key("plugins"), key(k)])
                    .collect(),
                _ => vec![],
            }
        };
        for path in roots {
            if let Some(value) = at(&self.draft.value, &path).cloned() {
                self.add_fields(path, value, window, cx);
            }
        }
        if self.page == 5 && self.plugin.is_none() {
            let priorities: Vec<_> = self
                .catalog
                .iter()
                .filter(|plugin| !plugin.config_key.is_empty())
                .map(|plugin| (plugin.config_key.clone(), plugin.priority))
                .collect();
            for (config_key, priority) in priorities {
                self.add_fields(
                    vec![key("plugins"), key(&config_key), key("priority")],
                    Value::Integer(priority.into()),
                    window,
                    cx,
                );
            }
        }
        if self.page == 6 {
            self.build_scope(window, cx);
        }
        if self.page == 2 && self.plugin.is_none() {
            self.build_app_names(cx);
        }
        if let Some(plugin) = self.current_plugin() {
            if matches!(
                &plugin.settings_extension,
                Some(plugin_catalog::SettingsExtension::TwoFactorAccounts { .. })
            ) {
                self.refresh_tfa();
            }
        }
        self.edited = false;
        cx.notify();
    }

    fn collect(&mut self, cx: &App) -> Result<(), String> {
        let mut changes = vec![];
        let mut first_error = None;
        for field in &mut self.fields {
            field.error = None;
            let result = field.changed_value(cx).and_then(|value| {
                if let Some(value) = value {
                    if at(&self.draft.value, &field.path) != Some(&value) {
                        self.draft.validate_field(&field.path, &value)
                            .map_err(|e| format!("{}：{e}", field.title))?;
                        changes.push((field.path.clone(), value));
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                first_error.get_or_insert(error.clone());
                field.error = Some(error);
            }
        }
        self.scope_error = None;
        if self.page == 6 {
            match self.collect_scope(cx) {
                Ok(items) => changes.push((vec![key("scope_menu"), key("items")], items)),
                Err(error) => {
                    first_error.get_or_insert(error.clone());
                    self.scope_error = Some(error);
                }
            }
        }
        for (path, value) in changes {
            self.draft.set(&path, value)?;
        }
        self.edited = first_error.is_some();
        first_error.map_or(Ok(()), Err)
    }

    fn queue_save(&mut self, cx: &mut Context<Self>) {
        self.edited = true;
        self.status = "正在自动保存…".into();
        let window = self.window;
        self.autosave_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let done = cx
                .update_window(window, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        if this.busy {
                            return false;
                        }
                        this.flush(window, cx);
                        true
                    })
                    .unwrap_or(true)
                })
                .unwrap_or(true);
            if done {
                break;
            }
        }));
        cx.notify();
    }

    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.autosave_task = None;
        if !self.edited && !self.draft.dirty() && !self.app_rules.dirty() {
            return true;
        }
        let validation = self.collect(cx);
        match self.draft.save(false)
            .and_then(|_| self.app_rules.save(&self.draft.path))
        {
            Ok(()) => {
                for field in &mut self.fields {
                    field.mark_saved(cx);
                }
                if self.scope_error.is_none() { self.mark_scope_saved(cx); }
                if let Ok(settings) = self.draft.value.clone().try_into::<config::Settings>() {
                    self.catalog = plugin_catalog::inspect(&settings.plugins);
                }
                let mode = match self.draft.value.get("theme").and_then(Value::as_str) {
                    Some("dark") => ThemeMode::Dark,
                    Some("light") => ThemeMode::Light,
                    _ => cx.window_appearance().into(),
                };
                Theme::change(mode, Some(window), cx);
                self.error = validation.as_ref().err().map(|error| format!("请修正标记的字段：{error}"));
                self.status = if validation.is_ok() { "已自动保存" } else { "有效字段已保存，其余修改待修正" }.into();
                cx.notify();
                validation.is_ok()
            }
            Err(error) => {
                self.error = Some(format!("未能自动保存：{error}"));
                cx.notify();
                false
            }
        }
    }

    fn navigate(
        &mut self,
        page: usize,
        plugin: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if !self.flush(window, cx) {
            self.focus_first_error(window, cx);
            return;
        }
        self.page = page;
        self.plugin = plugin;
        if page == 2 && !self.app_rules.dirty() {
            self.reload_app_rules();
        }
        self.error = None;
        self.scope_error = None;
        self.list_filter.update(cx, |state, cx| state.set_value("", window, cx));
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
        self.rebuild(window, cx);
    }

    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.busy { return false; }
        let saved = self.flush(window, cx);
        if !saved { self.focus_first_error(window, cx); }
        saved
    }

    fn ask(&mut self, question: Question, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let submitted = std::rc::Rc::new(std::cell::Cell::new(false));
        let close_submitted = submitted.clone();
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let submitted = submitted.clone();
            let Question::Uninstall(path, key) = &question;
            let path = path.clone();
            let key = key.clone();
            dialog.confirm().title("卸载插件")
                .description("插件将停用。安装目录内的数据保留到 plugin-backups；外部登记目录保留原文件。是否继续？")
                .ok_text("卸载").cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    owner.update(cx, |this, cx| {
                        submitted.set(true);
                        this.busy = false;
                        this.package(None, Some((path.clone(), key.clone())), window, cx);
                    }).is_ok()
                })
                .on_close({
                    let owner = close_owner.clone();
                    let submitted = close_submitted.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if !submitted.get() {
                                this.busy = false;
                            }
                            cx.notify();
                        });
                    }
                })
        });
        cx.notify();
    }

    fn pick(&mut self, action: Picker, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.flush(window, cx) {
            return;
        }
        self.busy = true;
        self.status = "请选择文件或目录".into();
        cx.notify();
        let dialog = rfd::AsyncFileDialog::new().set_parent(window);
        cx.spawn_in(window, async move |this, cx| {
            let choice = match action {
                Picker::InstallZip => dialog.add_filter("插件包", &["zip"]).pick_file().await,
                Picker::InstallDirectory | Picker::Register | Picker::Field(_, true) => {
                    dialog.pick_folder().await
                }
                Picker::Field(_, false) => dialog.pick_file().await,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.status.clear();
                if let Some(file) = choice {
                    let path = file.path().to_path_buf();
                    match action {
                        Picker::InstallZip | Picker::InstallDirectory => {
                            this.package(Some(path), None, window, cx)
                        }
                        Picker::Register => {
                            match this.collect(cx).and_then(|_| this.draft.register(&path)) {
                                Ok(()) => {
                                    this.queue_save(cx);
                                    this.flush(window, cx);
                                    this.rebuild(window, cx);
                                }
                                Err(e) => this.error = Some(e),
                            }
                        }
                        Picker::Field(index, _) => this.apply_chosen(index, path, window, cx),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn package(
        &mut self,
        source: Option<PathBuf>,
        uninstall: Option<(PathBuf, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if !self.flush(window, cx) {
            return;
        }
        self.busy = true;
        self.error = None;
        self.status = "正在处理插件，请稍候…".into();
        cx.notify();
        let mut draft = self.draft.clone();
        if let Some((path, key)) = uninstall {
            let task = cx.background_executor().spawn(async move {
                let result = packages::prepare_uninstall(&path, &key, &draft)
                    .and_then(|plan| packages::uninstall(plan, &mut draft));
                (draft, result)
            });
            self.await_package(task, window, cx);
        } else if let Some(source) = source {
            let task = cx
                .background_executor()
                .spawn(async move { packages::prepare(&source, &draft) });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |this, window, cx| match result {
                    Ok(plan) => this.confirm_install(plan, window, cx),
                    Err(error) => {
                        this.busy = false;
                        this.status.clear();
                        this.error = Some(error);
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.busy = false;
            self.error = Some("未选择插件包".into());
            cx.notify();
        }
    }

    fn await_package(
        &mut self,
        task: Task<(Draft, Result<String, String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let (draft, result) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.draft = draft;
                match result {
                    Ok(text) => this.status = text,
                    Err(error) => this.error = Some(error),
                }
                this.rebuild(window, cx);
            });
        })
        .detach();
    }

    fn confirm_install(
        &mut self,
        plan: packages::InstallPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = format!(
            "{}（{}）\n{} 个文件，共 {} KiB\n安装到：{}",
            plan.name,
            plan.id,
            plan.entries,
            plan.bytes.div_ceil(1024),
            plan.target.to_string_lossy().trim_start_matches(r"\\?\")
        );
        let pending = std::rc::Rc::new(std::cell::RefCell::new(Some(plan)));
        let close_pending = pending.clone();
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        self.status = "请确认安装插件".into();
        window.activate_window();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let pending = pending.clone();
            dialog
                .confirm()
                .title("安装插件")
                .width(px(620.))
                .description(description.clone())
                .ok_text("安装")
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    owner
                        .update(cx, |this, cx| {
                            let Some(plan) = pending.borrow_mut().take() else {
                                return false;
                            };
                            this.status = "正在处理插件，请稍候…".into();
                            let mut draft = this.draft.clone();
                            let task = cx.background_executor().spawn(async move {
                                let result = packages::install(plan, &mut draft);
                                (draft, result)
                            });
                            this.await_package(task, window, cx);
                            cx.notify();
                            true
                        })
                        .unwrap_or(true)
                })
                .on_close({
                    let owner = close_owner.clone();
                    let pending = close_pending.clone();
                    move |_, _, cx| {
                        if pending.borrow_mut().take().is_some() {
                            let _ = owner.update(cx, |this, cx| {
                                this.busy = false;
                                this.status = "已取消安装".into();
                                cx.notify();
                            });
                        }
                    }
                })
        });
        cx.notify();
    }

    fn plugin_cards(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.catalog
            .iter()
            .enumerate()
            .filter(|(_, plugin)| {
                model::matches_filter(&self.list_filter.read(cx).value(),
                    &[&plugin.name, &plugin.directory.to_string_lossy(), &plugin.keywords.join(" ")])
                    && match self.plugin_filter { 1 => plugin.enabled, 2 => !plugin.enabled, _ => true }
            })
            .map(|(index, plugin)| {
                let config_key = plugin.config_key.clone();
                let toggle_key = config_key.clone();
                let uninstall_key = config_key.clone();
                let directory = plugin.directory.clone();
                let open_dir = directory.clone();
                let valid = !plugin.config_key.is_empty();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_3()
                    .py_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tokens.group_box)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(plugin.name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("启用"),
                                    )
                                    .child(
                                        Switch::new(("plugin-enabled", index))
                                            .small()
                                            .checked(plugin.enabled)
                                            .accessibility_label(format!("启用 {}", plugin.name))
                                            .disabled(self.busy || !valid)
                                            .on_click(cx.listener(move |this, checked, _, cx| {
                                                if this.busy {
                                                    return;
                                                }
                                                if let Some(Value::Table(plugins)) =
                                                    at_mut(&mut this.draft.value, &[key("plugins")])
                                                {
                                                    let entry = plugins
                                                        .entry(toggle_key.clone())
                                                        .or_insert_with(|| {
                                                            Value::Table(Default::default())
                                                        });
                                                    if let Some(table) = entry.as_table_mut() {
                                                        table.insert(
                                                            "enabled".into(),
                                                            Value::Boolean(*checked),
                                                        );
                                                    }
                                                }
                                                if let Some(info) = this.catalog.get_mut(index) {
                                                    info.enabled = *checked;
                                                }
                                                this.queue_save(cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div().flex().flex_wrap().gap_1().children(
                                    plugin
                                        .keywords
                                        .iter()
                                        .map(|word| Tag::secondary().small().child(word.clone())),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .when(valid, |row| {
                                        row.child(
                                            self.plugin_priority_editor(&plugin.config_key, cx),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .ml_2()
                                            .child(
                                                Button::new(("configure", index))
                                                    .label("设置")
                                                    .small()
                                                    .disabled(self.busy || !valid)
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.navigate(
                                                                5,
                                                                Some(config_key.clone()),
                                                                window,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new(("folder", index))
                                                    .icon(Glyph::Folder)
                                                    .tooltip("打开插件目录")
                                                    .ghost()
                                                    .small()
                                                    .disabled(self.busy)
                                                    .on_click(move |_, _, _| {
                                                        let _ = std::process::Command::new(
                                                            "explorer.exe",
                                                        )
                                                        .arg(&open_dir)
                                                        .spawn();
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .ml_2()
                                            .pl_3()
                                            .border_l_1()
                                            .border_color(cx.theme().border)
                                            .child(
                                                Button::new(("uninstall", index))
                                                    .label("卸载")
                                                    .ghost()
                                                    .danger()
                                                    .small()
                                                    .disabled(self.busy || !valid)
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.ask(
                                                                Question::Uninstall(
                                                                    directory.clone(),
                                                                    uninstall_key.clone(),
                                                                ),
                                                                window,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    )
                    .when_some(plugin.error.clone(), |row, error| {
                        row.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
                    .into_any_element()
            })
            .collect()
    }
}

fn page_icon(page: usize) -> Glyph {
    match page {
        0 => Glyph::Settings,
        1 => Glyph::Palette,
        2 => Glyph::Search,
        3 => Glyph::FolderSearch,
        4 => Glyph::Globe,
        5 => Glyph::Blocks,
        6 => Glyph::ListOrdered,
        _ => Glyph::Info,
    }
}

impl Settings {
    fn empty_note(&self, text: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn plugin_section(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let install = |id: &'static str, label: &'static str, picker: Picker, cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .small()
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, window, cx| this.pick(picker, window, cx)))
        };
        let mut items = vec![div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child("已安装插件")
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("共 {} 个", self.catalog.len())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(install("install-zip", "安装 ZIP", Picker::InstallZip, cx).primary())
                    .child(install(
                        "install-folder",
                        "安装目录",
                        Picker::InstallDirectory,
                        cx,
                    ))
                    .child(install("register", "登记外部目录", Picker::Register, cx)),
            )
            .into_any_element()];
        if self.catalog.is_empty() {
            items.push(self.empty_note("尚未安装任何插件。", cx));
        } else {
            items.push(div().flex().flex_col().gap_2()
                .child(Input::new(&self.list_filter).disabled(self.busy))
                .child(div().flex().gap_2().children(["全部", "已启用", "已停用"].into_iter().enumerate().map(|(index, label)| {
                    Button::new(("plugin-filter", index)).label(label).small()
                        .when(self.plugin_filter == index, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.plugin_filter = index;
                            cx.notify();
                        }))
                })))
                .into_any_element());
            let cards = self.plugin_cards(cx);
            items.push(div().text_sm().text_color(muted)
                .child(format!("显示 {} / {} 个插件", cards.len(), self.catalog.len())).into_any_element());
            if cards.is_empty() { items.push(self.empty_note("没有匹配的插件，请调整筛选条件。", cx)); }
            items.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(self.section_off("plugins"), |el| el.opacity(0.5))
                    .children(cards)
                    .into_any_element(),
            );
        }
        items
    }

    fn alias_section(&self, empty: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let mut items = vec![div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("应用别名：为程序、文件或目录设置简短名称。"),
            )
            .child(
                Button::new("edit-list")
                    .icon(Glyph::Plus)
                    .label("添加 / 删除别名…")
                    .primary()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.edit_collection(window, cx))),
            )
            .into_any_element()];
        if empty {
            items.push(self.empty_note("还没有别名。", cx));
        }
        items
    }

    fn current_plugin(&self) -> Option<&plugin_catalog::PluginInfo> {
        let key = self.plugin.as_deref()?;
        self.catalog.iter().find(|p| p.config_key == key)
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.background;
        let fg = theme.foreground;
        let border = theme.border;
        let danger = theme.danger;
        let title = self
            .current_plugin()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| PAGES[self.page].into());
        let controls = self.render_fields(cx);
        let plugin_page = self.page == 5 && self.plugin.is_none();
        let mut content: Vec<AnyElement> = vec![];
        content.extend(controls);
        if self.page == 2 && self.plugin.is_none() {
            let aliases = self.render_alias_fields(cx);
            content.extend(self.alias_section(aliases.is_empty(), cx));
            content.extend(aliases);
            content.extend(self.application_sections(cx));
        }
        if self.page == 0 && self.plugin.is_none() {
            if let Some(directory) = self.draft.path.parent() {
                let directory_label = directory.display().to_string();
                let directory = directory.to_path_buf();
                content.push(
                    div()
                        .p_4()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_sm().child("当前数据目录"))
                                .child(
                                    Button::new("open-data-directory")
                                        .label("打开目录")
                                        .small()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Err(error) =
                                                std::process::Command::new("explorer.exe")
                                                    .arg(&directory)
                                                    .spawn()
                                            {
                                                this.error =
                                                    Some(format!("无法打开数据目录：{error}"));
                                                cx.notify();
                                            }
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(directory_label),
                        )
                        .into_any_element(),
                );
            }
        }
        if plugin_page {
            content.extend(self.plugin_section(cx));
        }
        if let Some(plugin) = self.current_plugin() {
            match &plugin.settings_extension {
                Some(plugin_catalog::SettingsExtension::TwoFactorAccounts { .. }) => {
                    content.push(self.render_tfa(cx));
                }
                Some(plugin_catalog::SettingsExtension::External {
                    command,
                    args,
                    label,
                }) => {
                    let cmd = command.clone();
                    let a = args.clone();
                    let lbl = if label.is_empty() {
                        "打开管理工具"
                    } else {
                        label.as_str()
                    };
                    content.push(
                        div()
                            .mt_4()
                            .p_4()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_sm().child("此插件提供独立管理入口"))
                            .child(
                                Button::new("launch-external")
                                    .primary()
                                    .label(lbl)
                                    .on_click(move |_, _, _| {
                                        let _ = std::process::Command::new(&cmd).args(&a).spawn();
                                    }),
                            )
                            .into_any_element(),
                    );
                }
                None => {}
            }
            if let Some(editor) = &self.plugin_credentials {
                content.push(editor.render(self.busy, cx));
            }
        }
        if self.page == 4 && self.plugin.is_none() {
            content.push(
                div()
                    .when(self.section_off("web_search"), |el| el.opacity(0.5))
                    .child(self.render_web(cx))
                    .into_any_element(),
            );
        }
        if self.page == 6 && self.plugin.is_none() {
            content.push(self.render_scope(cx));
        }
        if self.page == 7 && self.plugin.is_none() {
            content.extend(self.render_about(cx));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .font_family("Microsoft YaHei UI")
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Sidebar::<SidebarMenu>::new("nav")
                            .collapsible(false)
                            .w(px(200.))
                            .header(div().px_2().py_1().text_lg().child("MiniLauncher"))
                            .child(SidebarMenu::new().children(PAGES.iter().enumerate().map(
                                |(index, name)| {
                                    SidebarMenuItem::new(*name)
                                        .icon(page_icon(index))
                                        .active(index == self.page)
                                        .disable(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.navigate(index, None, window, cx)
                                        }))
                                },
                            ))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_6()
                                    .py_4()
                                    .border_b_1()
                                    .border_color(border)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().text_xl().child(title))
                                    .when(self.plugin.is_some(), |el| {
                                        el.child(
                                            Button::new("back")
                                                .icon(Glyph::ChevronLeft)
                                                .label("返回插件列表")
                                                .small()
                                                .disabled(self.busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.navigate(5, None, window, cx)
                                                })),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("settings-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle)
                                    .vertical_scrollbar(&self.scroll_handle)
                                    .p_6()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(780.))
                                            .flex()
                                            .flex_col()
                                            .gap_4()
                                            .children(content),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .when(self.error.is_some(), |el| el.text_color(danger))
                            .child(self.error.clone().unwrap_or_else(|| {
                                if self.busy {
                                    self.status.clone()
                                } else if self.edited
                                    || self.draft.dirty()
                                    || self.app_rules.dirty()
                                {
                                    "正在自动保存…".into()
                                } else {
                                    self.status.clone()
                                }
                            })),
                    )
                    .child(
                        Button::new("locate-error")
                            .label("定位错误").small()
                            .disabled(self.busy || !(self.fields.iter().any(|field| field.error.is_some()) || self.scope_error.is_some()))
                            .on_click(cx.listener(|this, _, window, cx| this.focus_first_error(window, cx))),
                    )
                    .child(
                        Button::new("close")
                            .label("关闭")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.confirm_close(window, cx) {
                                    window.remove_window();
                                }
                            })),
                    ),
            )
    }
}

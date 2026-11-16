use super::*;

impl State {
    pub(super) unsafe fn execution_finished(&mut self, id: u64, result: Result<(), String>) {
        let Some(execution) = self.execution.executions.remove(&id) else {
            return;
        };
        match result {
            Ok(()) => {
                if let Some(key) = execution.history_key {
                    self.record_launch(key);
                }
            }
            Err(error) => {
                let message = format!("无法执行「{}」：{error}", execution.title);
                // A late failure must not replace a newer query or steal its focus.
                if execution.paste_target.is_some() {
                    self.notify_execution_error(&message);
                } else if execution.generation == self.query.generation
                    && id == self.execution.next_execution
                {
                    if IsWindowVisible(self.hwnd) == 0 {
                        self.show();
                    }
                    self.execution.execution_error = Some(message.clone());
                    self.status = Some(message);
                    self.sync_height();
                    self.invalidate();
                } else {
                    self.notify_execution_error(&message);
                }
            }
        }
    }
    pub(super) unsafe fn notify_execution_error(&self, message: &str) {
        self.tray.notify_error(message);
    }
    pub(super) unsafe fn queue_launch(&mut self, id: u64, task: launch::Task) {
        if self.launch_sender.send((id, task)).is_err() {
            self.execution_finished(id, Err("启动线程已停止".into()));
        }
    }
    pub(super) unsafe fn begin_execution(
        &mut self,
        entry: &Entry,
        history_key: Option<String>,
    ) -> Option<u64> {
        if self
            .execution
            .executions
            .values()
            .any(|execution| execution.key == entry.key)
        {
            self.status = Some("此项目正在启动，请稍候".into());
            self.invalidate();
            return None;
        }
        let paste_target = if entry.paste_after_execute {
            let Some(target) = self.paste_target else {
                self.status = Some("未找到唤出前的输入焦点，请使用右键“仅复制”".into());
                self.sync_height();
                self.invalidate();
                return None;
            };
            Some(target)
        } else {
            None
        };
        self.execution.next_execution += 1;
        let id = self.execution.next_execution;
        self.execution.executions.insert(
            id,
            Execution {
                key: entry.key.clone(),
                title: entry.title.clone(),
                generation: self.query.generation,
                history_key,
                paste_target,
                paste_epoch: self.paste_epoch.load(Ordering::Relaxed),
            },
        );
        // Hide before dispatch, so even a very fast launch cannot race the foreground window.
        self.hide_immediately();
        Some(id)
    }
    pub(super) unsafe fn plugin_executed(
        &mut self,
        id: u64,
        result: Result<Option<Action>, String>,
    ) {
        if !self.execution.executions.contains_key(&id) {
            return;
        }
        let paste = self.execution.executions[&id].paste_target.is_some();
        if paste && matches!(result, Ok(Some(Action::Launch { .. } | Action::Open(_)))) {
            self.execution_finished(id, Err("自动粘贴动作只能返回复制内容或空响应".into()));
            return;
        }
        match result {
            Ok(Some(Action::Launch { executable, args })) => {
                self.queue_launch(id, launch::Task::Launch { executable, args })
            }
            Ok(Some(Action::Open(target))) => {
                if !target.contains("://") {
                    self.execution.executions.get_mut(&id).unwrap().history_key =
                        Some(target.to_lowercase());
                }
                self.queue_launch(
                    id,
                    launch::Task::Open {
                        target,
                        verb: "open".into(),
                        arguments: None,
                    },
                );
            }
            Ok(Some(Action::Copy(text))) => {
                let result = if copy_to_clipboard(self.hwnd, &text) {
                    Ok(())
                } else {
                    Err("无法复制到剪贴板".into())
                };
                if paste && result.is_ok() {
                    self.queue_paste(id);
                } else {
                    self.execution_finished(id, result);
                }
            }
            Ok(None) if paste => self.queue_paste(id),
            Ok(None) => self.execution_finished(id, Ok(())),
            Ok(Some(_)) => self.execution_finished(id, Err("插件返回了不支持的执行动作".into())),
            Err(error) => self.execution_finished(id, Err(error)),
        }
    }
    pub(super) unsafe fn queue_paste(&mut self, id: u64) {
        let Some(execution) = self.execution.executions.get(&id) else {
            return;
        };
        let Some(target) = execution.paste_target else {
            return;
        };
        if execution.paste_epoch != self.paste_epoch.load(Ordering::Relaxed)
            || IsWindowVisible(self.hwnd) != 0
        {
            self.execution_finished(
                id,
                Err("已开始新的操作，未执行自动粘贴；内容保留在剪贴板".into()),
            );
            return;
        }
        self.queue_launch(
            id,
            launch::Task::Paste {
                target,
                generation: execution.paste_epoch,
                latest: self.paste_epoch.clone(),
                launcher: self.hwnd as usize,
            },
        );
    }
}

#[derive(Default)]
pub(super) struct Model {
    pub(super) executions: HashMap<u64, Execution>,
    pub(super) next_execution: u64,
    pub(super) pending_execute: Option<(u64, Entry)>,
    pub(super) execution_error: Option<String>,
}

impl State {
    pub(super) unsafe fn execute(&mut self) {
        let Some(entry) = self.query.results.get(self.viewport.selected).cloned() else {
            return;
        };
        self.execute_entry(entry);
    }
    pub(super) unsafe fn execute_entry(&mut self, entry: Entry) {
        // Ignore queued Enter messages and clicks after the window has already closed.
        if IsWindowVisible(self.hwnd) == 0 {
            return;
        }
        if let Action::Plugin { plugin_id, action_id } = &entry.action {
            if !self.validate_plugin_callback(plugin_id, action_id, false) {
                return;
            }
        }
        if let Some(panel) = &entry.text_panel {
            if panel.state != mini_launcher::result::PanelState::Ready {
                if panel.retry {
                    self.focus_input();
                    self.request_search();
                }
                return;
            }
        }
        match &entry.action {
            Action::Copy(value) if entry.paste_after_execute => {
                let Some(id) = self.begin_execution(&entry, None) else {
                    return;
                };
                if copy_to_clipboard(self.hwnd, value) {
                    self.queue_paste(id);
                } else {
                    self.execution_finished(id, Err("无法复制到剪贴板".into()));
                }
            }
            Action::Copy(value) => {
                if copy_to_clipboard(self.hwnd, value) {
                    self.hide();
                } else {
                    self.status = Some("无法复制到剪贴板".into());
                    self.invalidate();
                }
            }
            Action::Settings => self.open_settings(),
            Action::PluginSettings => self.open_settings_page(true),
            _ => {
                let history_key =
                    matches!(&entry.action, Action::Open(target) if !target.contains("://"))
                        .then(|| entry.key.clone())
                        .filter(|_| entry.kind != mini_launcher::result::EntryKind::Web);
                let Some(id) = self.begin_execution(&entry, history_key) else {
                    return;
                };
                match entry.action {
                    Action::Open(target) => self.queue_launch(
                        id,
                        launch::Task::Open {
                            target,
                            verb: "open".into(),
                            arguments: None,
                        },
                    ),
                    Action::Launch { executable, args } => {
                        self.queue_launch(id, launch::Task::Launch { executable, args })
                    }
                    Action::Plugin {
                        plugin_id,
                        action_id,
                    } => {
                        if !self.plugins.send(crate::plugins::Request::Execute {
                            id,
                            plugin: plugin_id,
                            action: action_id,
                        }) {
                            self.execution_finished(id, Err("插件线程已停止".into()));
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    pub(super) fn record_launch(&mut self, key: String) {
        let history = Arc::make_mut(&mut self.history);
        let usage = history.entry(key).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last_used = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if history.len() > 1000 {
            if let Some(oldest) = history
                .iter()
                .filter(|(_, usage)| usage.pinned_queries.is_empty() && !usage.removed)
                .min_by_key(|(_, usage)| usage.last_used)
                .map(|(key, _)| key.clone())
            {
                history.remove(&oldest);
            }
        }
        let _ = self.history_sender.send(self.history.clone());
    }
    pub(super) unsafe fn execute_item_command(&mut self, entry: Entry, command: u32) {
        match command {
            1 => self.execute_entry(entry),
            2 => {
                let text = match &entry.action {
                    Action::Open(target) => target,
                    _ => &entry.title,
                };
                if !copy_to_clipboard(self.hwnd, text) {
                    self.status = Some("无法复制到剪贴板".into());
                }
            }
            3 | 4 => {
                if let Action::Open(target) = &entry.action {
                    let (verb, file, arguments) = if command == 3 {
                        (
                            "open",
                            "explorer.exe",
                            Some(format!("/select,\"{target}\"")),
                        )
                    } else {
                        ("runas", target.as_str(), None)
                    };
                    let Some(id) = self.begin_execution(&entry, None) else {
                        return;
                    };
                    self.queue_launch(
                        id,
                        launch::Task::Open {
                            target: file.into(),
                            verb: verb.into(),
                            arguments,
                        },
                    );
                }
            }
            5 | 6 if entry.is_app() => {
                let query = self.query.pin_query.clone();
                let usage = Arc::make_mut(&mut self.history)
                    .entry(entry.key)
                    .or_default();
                if command == 5 {
                    if !usage.pinned_queries.remove(&query) {
                        usage.pinned_queries.insert(query);
                    }
                } else {
                    usage.removed = !usage.removed;
                }
                let _ = self.history_sender.send(self.history.clone());
                self.request_search();
            }
            7 => self.open_image_preview(&entry),
            8 => self.search_entry_text(&entry),
            value if value >= 100 => {
                if let Some(action) = entry.context_actions.get(value as usize - 100) {
                    let target = Entry::new(
                        action.title.clone(),
                        entry.subtitle.clone(),
                        action.action.clone(),
                        "插件",
                    );
                    if action.confirm {
                        self.confirm_item_action(target);
                    } else {
                        self.execute_entry(target);
                    }
                }
            }
            _ => {}
        }
        self.invalidate();
    }
    pub(super) unsafe fn open_settings(&mut self) {
        self.open_settings_page(false);
    }
    pub(super) unsafe fn open_settings_page(&mut self, plugins: bool) {
        let entry = Entry::new(
            "MiniLauncher 设置".into(),
            String::new(),
            Action::Settings,
            "设置",
        );
        let Some(id) = self.begin_execution(&entry, None) else {
            return;
        };
        let executable = std::env::current_exe()
            .map(|path| path.with_file_name("mini-launcher-config.exe"))
            .unwrap_or_else(|_| "mini-launcher-config.exe".into());
        self.queue_launch(
            id,
            launch::Task::Open {
                target: executable.to_string_lossy().into_owned(),
                verb: "open".into(),
                arguments: Some(
                    if plugins {
                        "--from-launcher --plugins"
                    } else {
                        "--from-launcher"
                    }
                    .into(),
                ),
            },
        );
    }
}

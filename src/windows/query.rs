use super::*;
use std::time::Duration;

pub(super) const PROGRESS_TIMER: usize = 2;
const PROGRESS_DELAY: Duration = Duration::from_millis(300);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Route {
    Default,
    Files,
    Web(String),
    Plugins(Vec<String>),
}

impl State {
    pub(super) unsafe fn effective_query(&self) -> String {
        self.editor.effective_text()
    }
    pub(super) unsafe fn search_if_changed(&mut self) {
        // IME can notify several times for one edit, including after a menu opens.
        if self.effective_query() != self.query.query_text {
            self.request_search();
        }
    }
    pub(super) unsafe fn request_search(&mut self) {
        if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
        if self.closing.is_some() {
            return;
        }
        self.layout_input();
        let text = self.effective_query();
        let (plugin_text, attachments) = self.editor.query_input();
        let has_images = !attachments.is_empty();
        let old_input_extra = self.input_extra();
        let identity = (self.scope.clone(), text.clone());
        let route = if self.scope != scope::Scope::All {
            Route::Default
        } else if let Some((engine, _)) = self.settings.web_search.explicit(&text) {
            Route::Web(engine.id.clone())
        } else if !has_images
            && providers::files_query(&text, &self.settings.everything)
                .is_some_and(|(_, exclusive)| exclusive)
        {
            Route::Files
        } else {
            let plugins = self.plugin_routing.exclusive_context(&plugin_text);
            if plugins.is_empty() {
                Route::Default
            } else {
                Route::Plugins(plugins)
            }
        };
        let context = (self.scope.clone(), route, has_images);
        let retain_results = self.query.begin_query(identity, context);
        if !retain_results {
            self.panels.clear(self.hwnd);
        }
        self.close_item_menu();
        self.result_navigation = false;
        // A new query owns the selection until the pointer actually moves.
        // Result/height changes can synthesize mouse messages under a still cursor.
        let mut cursor: POINT = zeroed();
        if GetCursorPos(&mut cursor) != 0 {
            self.mouse_position = Some((cursor.x, cursor.y));
        }
        self.hovered = None;
        self.execution.execution_error = None;
        let was_direct = self.scope == scope::Scope::All
            && self.query.query_active
            && self.query.query_text.trim().is_empty()
            && self.direct_display;
        self.query.pin_query = search::pin_query(&text);
        self.query.query_text = text.clone();
        if self.input_extra() != old_input_extra {
            self.layout_input();
            self.sync_monitor_rows();
            self.selection = animation::Tween::fixed(self.selection_top());
            self.sync_height();
            self.animate();
            self.invalidate();
        }
        self.query.generation += 1;
        self.latest_generation
            .store(self.query.generation, Ordering::Relaxed);
        self.status = self.config_error.clone();
        let direct_display =
            self.scope == scope::Scope::All && text.trim().is_empty() && self.direct_display;
        self.query.query_active = !text.trim().is_empty()
            || matches!(self.scope, scope::Scope::Plugin(_))
            || direct_display;
        if !self.query.query_active || was_direct != direct_display || !retain_results {
            self.query.clear_results();
        }
        if !self.query.query_active
            || was_direct != direct_display
            || !retain_results
            || !self.query.preserve_selection
        {
            self.viewport
                .reset(self.query.results.len(), self.visible_rows);
            // reset drops measured heights even when the previous rows remain
            // visible. An identical provider response can skip apply_provider's
            // reflow, so restore their geometry before painting retained rows.
            self.reflow_rows();
            self.hovered = None;
            if self.scroll_drag.take().is_some() {
                ReleaseCapture();
            }
            self.selection = animation::Tween::fixed(self.row_top());
            self.selection_first = 0;
            self.sync_height();
            self.animate();
            self.invalidate();
        }
        if !self.query.query_active {
            self.query.groups = Groups::default();
            self.query.busy_since = None;
            self.sync_query_progress();
            self.sync_accessibility(true);
            return;
        }
        let web = match &self.scope {
            scope::Scope::All => self.settings.web_search.explicit(&text),
            scope::Scope::Web(id) => self
                .settings
                .web_search
                .engines
                .iter()
                .find(|engine| &engine.id == id)
                .map(|engine| (engine, text.trim())),
            _ => None,
        };
        let web_explicit = web.is_some();
        let web_entry =
            web.and_then(|(engine, query)| (!query.is_empty()).then(|| engine.entry(query)));
        let file_only = !has_images
            && (self.scope == scope::Scope::Files
                || (!web_explicit
                    && self.scope == scope::Scope::All
                    && providers::files_query(&text, &self.settings.everything)
                        .is_some_and(|(_, exclusive)| exclusive)));
        let plugin_only = matches!(self.scope, scope::Scope::Plugin(_));
        let plugin_exclusive =
            plugin_only || matches!(self.query.context.as_ref(), Some((_, Route::Plugins(_), _)));
        let query_files = !has_images
            && !web_explicit
            && !plugin_exclusive
            && (self.scope == scope::Scope::Files
                || providers::files_query(&text, &self.settings.everything).is_some());
        let query_plugins = has_images || (!file_only && !web_explicit);
        let query_apps =
            !has_images && !plugin_exclusive && ((!file_only && !direct_display) || web_explicit);
        let exclusive = if plugin_exclusive {
            Some(Source::Plugins)
        } else if file_only {
            Some(Source::Files)
        } else {
            None
        };
        self.query.groups.begin(
            [query_apps, query_files, query_plugins],
            retain_results,
            exclusive,
        );
        self.query.groups.set_priorities(
            self.settings.application_priority,
            self.settings.everything.priority,
        );
        self.query.start_busy(Instant::now(), retain_results);
        self.sync_query_progress();
        self.invalidate();
        if query_files {
            self.everything.query(crate::everything::Query {
                scoped: self.scope == scope::Scope::Files,
                generation: self.query.generation,
                text: text.clone(),
                settings: self.settings.everything.clone(),
            });
        }
        if query_plugins {
            self.plugins.send(crate::plugins::Request::Query {
                allow_empty: self.scope == scope::Scope::All,
                scope: if let scope::Scope::Plugin(id) = &self.scope {
                    Some(id.clone())
                } else {
                    None
                },
                generation: self.query.generation,
                text: plugin_text,
                attachments,
                settings: self.settings.plugins.clone(),
            });
        }
        self.sync_accessibility(true);
        if query_apps {
            let _ = self.search_sender.send(SearchRequest {
                generation: self.query.generation,
                text,
                index: self.index.clone(),
                history: self.history.clone(),
                limit: self.index.len().saturating_add(1),
                file_only: web_explicit,
                web_entry,
            });
        }
    }

    pub(super) unsafe fn sync_query_progress(&mut self) {
        KillTimer(self.hwnd, PROGRESS_TIMER);
        if !self.query.query_active || !self.query.groups.is_pending() {
            self.query.busy_since = None;
        }
        if let Some(started) = self.query.busy_since {
            let remaining = PROGRESS_DELAY.saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                SetTimer(
                    self.hwnd,
                    PROGRESS_TIMER,
                    remaining.as_millis().max(1) as u32,
                    None,
                );
            }
        }
        self.animate();
    }

    // Declarative actions carry their own payload. Plugin callbacks must still
    // be offered by this generation before reusing a retained row's action ID.
    pub(super) unsafe fn validate_plugin_callback(
        &mut self,
        plugin: &str,
        action: &str,
        drag: bool,
    ) -> bool {
        let valid = self.query.groups.current_entries(Source::Plugins).iter().any(|entry| {
            if drag {
                matches!(entry.drag_source(), Some(mini_launcher::drag_data::DragSource::Plugin { plugin_id, action_id })
                    if plugin_id == plugin && action_id == action)
            } else {
                std::iter::once(&entry.action).chain(entry.context_actions.iter().map(|a| &a.action))
                    .any(|candidate| matches!(candidate, Action::Plugin { plugin_id, action_id }
                        if plugin_id == plugin && action_id == action))
            }
        });
        if !valid {
            self.status = Some(
                if self.query.groups.pending[Source::Plugins as usize] {
                    "结果正在更新，请稍后重试"
                } else {
                    "结果已更新，请重新选择"
                }
                .into(),
            );
            self.invalidate();
        }
        valid
    }
    pub(super) unsafe fn retry_panel(&mut self) {
        let retry = self
            .query
            .results
            .get(self.viewport.selected)
            .and_then(|e| e.text_panel.as_ref())
            .is_some_and(|p| p.retry);
        if retry {
            self.focus_input();
            self.request_search();
        }
    }
    pub(super) unsafe fn start_index(&mut self) {
        if self.query.indexing {
            self.query.pending_index = true;
            return;
        }
        self.query.indexing = true;
        let settings = self.settings.clone();
        let sender = self.event_sender.clone();
        let hwnd = self.hwnd as usize;
        std::thread::spawn(move || {
            send_event(
                &sender,
                hwnd,
                UiEvent::Indexed(search::build_index(&settings)),
            );
        });
    }
    pub(super) unsafe fn drain_events(&mut self) {
        let mut changed = false;
        if !self.panels.focused() {
            for update in self.panels.take_deferred() {
                if update.generation == self.query.generation {
                    self.apply_provider(update);
                }
            }
        }
        while let Ok(event) = self.events.try_recv() {
            // Provider updates invalidate only when the displayed data changes.
            // Focus notifications with no queued events must not cause a paint loop.
            changed |= !matches!(
                &event,
                UiEvent::Provider(_)
                    | UiEvent::Results { .. }
                    | UiEvent::Icon { .. }
                    | UiEvent::Preview { .. }
            );
            match event {
                UiEvent::SearchText {
                    request,
                    generation,
                    result,
                } => {
                    if self.text_search_request == request
                        && self.query.generation == generation
                        && IsWindowVisible(self.hwnd) != 0
                    {
                        match result {
                            Ok(text) => self.fill_search_text(text),
                            Err(error) => self.status = Some(format!("无法读取搜索文本：{error}")),
                        }
                    }
                }
                UiEvent::DragPrepared { id, result } => {
                    if self.drag_pending != Some(id) {
                        continue;
                    }
                    if IsWindowVisible(self.hwnd) == 0 || GetAsyncKeyState(VK_LBUTTON as i32) >= 0 {
                        self.cancel_pending_drag();
                        continue;
                    }
                    match result {
                        Ok(prepared) => {
                            self.prepared_drag = Some(prepared);
                            PostMessageW(self.hwnd, START_DRAG, id as usize, 0);
                        }
                        Err(error) => {
                            self.cancel_pending_drag();
                            self.status = Some(format!("无法准备拖动：{error}"));
                        }
                    }
                }
                UiEvent::PluginSelection {
                    request,
                    plugin,
                    result,
                } if request == self.selection_request => {
                    if !self.plugin_scopes.iter().any(|(id, _)| id == &plugin) {
                        continue;
                    }
                    match result {
                        Ok(text) => {
                            self.show_plugin_scope(plugin, &text);
                            self.editor.select_all();
                        }
                        Err(error) => {
                            self.show();
                            self.status = Some(error);
                            self.invalidate();
                        }
                    }
                }
                UiEvent::Settings(Ok(settings)) => self.apply_settings(*settings),
                UiEvent::Settings(Err(error)) => self.reject_config(error, false),
                UiEvent::Provider(update) if update.generation == self.query.generation => {
                    self.apply_provider(update)
                }
                UiEvent::PluginExecuted { id, result } => self.plugin_executed(id, result),
                UiEvent::Launched { id, result } => self.execution_finished(id, result),
                UiEvent::Indexed(index) => {
                    self.query.indexing = false;
                    if self.query.pending_index {
                        self.query.pending_index = false;
                        self.start_index();
                        continue;
                    }
                    self.index = Arc::new(index);
                    self.query.indexed = true;
                    self.query.indexing = false;
                    if IsWindowVisible(self.hwnd) != 0 && !self.panels.focused() {
                        self.request_search();
                    }
                }
                UiEvent::Results {
                    generation,
                    entries,
                } if generation == self.query.generation => {
                    self.apply_provider(Update {
                        kind: crate::providers::UpdateKind::Results,
                        generation,
                        source: Source::Apps,
                        entries,
                        exclusive: false,
                        pending: false,
                        status: None,
                    });
                }
                UiEvent::Icon { path, size, handle } => {
                    self.pending_icons.remove(&path);
                    if size != self.px(32) {
                        if handle != 0 {
                            DestroyIcon(handle as HICON);
                        }
                        continue;
                    }
                    changed |= self.query.results[self.viewport.range()]
                        .iter()
                        .any(|entry| entry.icon_path.as_ref() == Some(&path));
                    if !self.icons.contains_key(&path) && self.icons.len() >= MAX_ICONS {
                        if let Some(key) = self
                            .icons
                            .keys()
                            .find(|key| {
                                !self.query.results[self.viewport.range()]
                                    .iter()
                                    .any(|r| r.icon_path.as_ref() == Some(key))
                            })
                            .cloned()
                        {
                            self.d2d_icons.remove(&key);
                            if let Some(old) = self.icons.remove(&key) {
                                if old != 0 {
                                    DestroyIcon(old as HICON);
                                }
                            }
                        }
                    }
                    if let Some(old) = self.icons.insert(path, handle) {
                        if old != 0 {
                            DestroyIcon(old as HICON);
                        }
                    }
                }
                UiEvent::Preview {
                    preview,
                    dpi,
                    image,
                } => {
                    self.pending_previews.remove(&(preview.clone(), dpi));
                    if dpi != self.dpi
                        || IsWindowVisible(self.hwnd) == 0
                        || !self.query.results[self.viewport.range()]
                            .iter()
                            .any(|entry| entry.preview.as_ref() == Some(&preview))
                    {
                        continue;
                    }
                    changed = true;
                    if self.previews.len() >= 16 {
                        if let Some(key) = self
                            .previews
                            .keys()
                            .find(|key| {
                                !self.query.results[self.viewport.range()]
                                    .iter()
                                    .any(|e| e.preview.as_ref() == Some(*key))
                            })
                            .cloned()
                        {
                            self.previews.remove(&key);
                        }
                    }
                    self.previews.insert(
                        preview,
                        image.and_then(|image| rows::Thumbnail::from_shared(image).ok()),
                    );
                }
                UiEvent::Error(error) => self.status = Some(error),
                _ => {}
            }
        }
        if IsWindowVisible(self.hwnd) != 0 {
            self.queue_visible_icons();
            self.queue_visible_previews();
            self.sync_height();
            self.animate();
            if changed {
                self.invalidate();
            }
        }
    }
    pub(super) unsafe fn apply_provider(&mut self, update: Update) {
        if self.panels.focused() {
            self.panels.defer(update);
            return;
        }
        if !self.query.query_active {
            return;
        }
        let _timing = mini_launcher::diagnostics::timing("ui_apply_provider", "");
        if update.kind == providers::UpdateKind::State {
            self.query.groups.update(update);
            self.sync_query_progress();
            return;
        }
        let preserve = self.query.preserve_selection;
        let previous_key = preserve
            .then(|| {
                self.query
                    .results
                    .get(self.viewport.selected)
                    .map(|entry| entry.key.clone())
            })
            .flatten();
        let mut results = self.query.groups.apply(update);
        self.sync_query_progress();
        if self.scope == scope::Scope::All && self.editor.query_input().1.is_empty() {
            self.query.groups.append_web(
                &mut results,
                &self.query.query_text,
                &self.settings.web_search,
            );
        }
        if results.is_empty()
            && matches!(self.scope, scope::Scope::Plugin(_))
            && !self.query.groups.pending[Source::Plugins as usize]
        {
            use mini_launcher::result::{PanelState, TextPanel};
            let error = self.query.groups.status();
            let panel = TextPanel {
                state: if error.is_some() {
                    PanelState::Error
                } else {
                    PanelState::Empty
                },
                message: error.unwrap_or_default(),
                retry: true,
                ..Default::default()
            };
            let title = if let scope::Scope::Plugin(id) = &self.scope {
                self.plugin_scopes
                    .iter()
                    .find(|(plugin_id, _)| plugin_id == id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| panel.status().to_string())
            } else {
                panel.status().to_string()
            };
            let mut entry = Entry::new(title, String::new(), Action::Copy(String::new()), "插件");
            entry.key = format!("panel-state:{:?}", self.scope);
            entry.text_panel = Some(Arc::new(panel));
            results.push(entry);
        }
        let previous_status = self.status.clone();
        self.status = self
            .config_error
            .clone()
            .or_else(|| self.execution.execution_error.clone())
            .or_else(|| self.query.groups.status());
        if !self.query.commit_results(results) {
            if self.status != previous_status {
                if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
                self.invalidate();
            }
            return;
        }
        if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
        if preserve {
            let selected = previous_key
                .and_then(|key| self.query.results.iter().position(|entry| entry.key == key))
                .unwrap_or(0);
            self.viewport
                .update_total(self.query.results.len(), selected);
        } else {
            self.viewport
                .reset(self.query.results.len(), self.visible_rows);
        }
        if self.minimum_panel_rows() != self.query.panel_rows {
            self.sync_monitor_rows();
        } else {
            self.reflow_rows();
        }
        self.sync_accessibility(true);
        self.hovered = None;
        // Results can replace/reorder rows; never animate across different entries.
        self.selection = animation::Tween::fixed(self.selection_top());
        self.selection_first = self.viewport.first;
        self.invalidate();
    }
}

#[derive(Default)]
pub(super) struct Model {
    pub(super) context: Option<(scope::Scope, Route, bool)>,
    pub(super) busy_since: Option<Instant>,
    pub(super) panel_rows: usize,
    pub(super) identity: Option<(scope::Scope, String)>,
    pub(super) preserve_selection: bool,
    pub(super) results: Vec<Entry>,
    pub(super) groups: Groups,
    // None means no committed snapshot in this context. Some includes an empty
    // result, so rows and the empty message share the same display lifecycle.
    displayed_generation: Option<u64>,
    pub(super) query_text: String,
    pub(super) pin_query: String,
    pub(super) generation: u64,
    pub(super) query_active: bool,
    pub(super) indexed: bool,
    pub(super) indexing: bool,
    pub(super) pending_index: bool,
}

impl Model {
    /// Retaining rows avoids flicker while providers work, but a different
    /// search must not inherit the previous search's selection or scroll.
    fn begin_query(
        &mut self,
        identity: (scope::Scope, String),
        context: (scope::Scope, Route, bool),
    ) -> bool {
        let retain_results = self.context.as_ref() == Some(&context);
        self.preserve_selection = retain_results && self.identity.as_ref() == Some(&identity);
        self.context = Some(context);
        self.identity = Some(identity);
        retain_results
    }

    pub(super) fn clear_results(&mut self) {
        self.results.clear();
        self.displayed_generation = None;
    }

    pub(super) fn empty_message_visible(&self) -> bool {
        self.query_active && self.results.is_empty() && self.displayed_generation.is_some()
    }

    /// Pending work is not an empty result. Keep the displayed snapshot until
    /// there is replacement content or the relevant providers have finished.
    /// Provider freshness/pending state is tracked separately by Groups.
    fn commit_results(&mut self, results: Vec<Entry>) -> bool {
        if results.is_empty() && self.groups.is_pending() {
            return false;
        }
        let changed = self.displayed_generation.is_none()
            || self.results.len() != results.len()
            || !self
                .results
                .iter()
                .zip(&results)
                .all(|(old, new)| old.same_presentation(new));
        // Always refresh actions, even when the rows look identical.
        self.results = results;
        self.displayed_generation = Some(self.generation);
        changed
    }

    fn start_busy(&mut self, now: Instant, retain: bool) {
        if !retain || self.busy_since.is_none() {
            self.busy_since = Some(now);
        }
    }

    pub(super) fn progress_phase(&self, now: Instant) -> Option<f32> {
        let elapsed = now.checked_duration_since(self.busy_since?)?;
        (self.query_active && elapsed >= PROGRESS_DELAY)
            .then(|| ((elapsed - PROGRESS_DELAY).as_secs_f32() / 1.2).fract())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_search_retains_rows_without_preserving_navigation() {
        let mut model = Model::default();
        let context = || (scope::Scope::All, Route::Default, false);
        assert!(!model.begin_query((scope::Scope::All, "old".into()), context()));
        model.commit_results(vec![result("row", "payload")]);

        assert!(model.begin_query(
            (scope::Scope::All, "https://example.com".into()),
            context(),
        ));
        assert!(!model.preserve_selection);
        assert_eq!(model.results.len(), 1);
        // Partial asynchronous responses must not start preserving an old row
        // when a later provider inserts a higher-priority result before it.
        model.commit_results(vec![result("partial response", "new payload")]);
        assert!(!model.preserve_selection);

        // An index/provider refresh of the same search keeps the user's place.
        assert!(model.begin_query(
            (scope::Scope::All, "https://example.com".into()),
            context(),
        ));
        assert!(model.preserve_selection);

        // Routing changes invalidate navigation even with identical input.
        assert!(!model.begin_query(
            (scope::Scope::All, "https://example.com".into()),
            (scope::Scope::All, Route::Files, false),
        ));
        assert!(!model.preserve_selection);
    }

    fn result(title: &str, payload: &str) -> Entry {
        let mut entry = Entry::new(
            title.into(),
            String::new(),
            Action::Copy(payload.into()),
            "插件",
        );
        entry.key = "stable-row".into();
        entry
    }

    #[test]
    fn confirmed_empty_message_survives_pending_queries_until_context_reset() {
        let mut model = Model {
            query_active: true,
            ..Default::default()
        };
        model.groups.begin([true, false, true], false, None);
        assert!(!model.empty_message_visible());
        assert!(!model.commit_results(Vec::new()));
        model.groups.pending = [false; 3];
        // The first completed empty response must trigger a paint, even though
        // both the old and new row vectors are empty.
        assert!(model.commit_results(Vec::new()));
        assert!(model.empty_message_visible());
        for _ in 0..3 {
            model.groups.begin([true, false, true], true, None);
            assert!(model.empty_message_visible());
            assert!(!model.commit_results(Vec::new()));
            assert!(model.empty_message_visible());
            model.groups.pending = [false; 3];
            assert!(!model.commit_results(Vec::new()));
            assert!(model.empty_message_visible());
        }
        model.clear_results();
        model.groups.begin([true, false, true], false, None);
        assert!(!model.empty_message_visible());
        model.groups.pending = [false; 3];
        assert!(model.commit_results(Vec::new()));
        assert!(model.commit_results(vec![result("match", "payload")]));
        assert!(!model.empty_message_visible());
        model.commit_results(Vec::new());
        model.query_active = false;
        assert!(!model.empty_message_visible());
    }

    #[test]
    fn unchanged_rows_refresh_actions_without_resetting_presentation() {
        let mut model = Model::default();
        assert!(model.commit_results(vec![result("row", "old")]));
        model.generation = 2;
        assert!(!model.commit_results(vec![result("row", "new")]));
        assert_eq!(model.displayed_generation, Some(2));
        assert!(matches!(&model.results[0].action, Action::Copy(value) if value == "new"));
        assert!(model.commit_results(vec![result("changed", "new")]));
        let mut panel = result("changed", "new");
        panel.text_panel = Some(Arc::new(search::TextPanel::default()));
        assert!(model.commit_results(vec![panel]));
    }

    #[test]
    fn partial_empty_updates_keep_the_composed_web_row_until_completion() {
        let web = mini_launcher::web_search::WebSearchSettings::default();
        let mut model = Model::default();
        let mut results = Vec::new();
        model.groups.append_web(&mut results, "first", &web);
        assert!(model.commit_results(results));
        assert!(!model.groups.has_results());
        // The displayed snapshot includes host-composed rows absent from Groups.
        assert!(!model.results.is_empty());
        for generation in 1..=3 {
            model.generation = generation;
            model.groups.begin([true, false, true], true, None);
            for source in [Source::Apps, Source::Plugins] {
                let mut results = model.groups.apply(Update {
                    kind: providers::UpdateKind::Results,
                    generation,
                    source,
                    entries: Vec::new(),
                    exclusive: false,
                    pending: false,
                    status: None,
                });
                model.groups.append_web(&mut results, "next", &web);
                model.commit_results(results);
                assert_eq!(model.results.len(), 1);
            }
            assert!(model.results[0].title.contains("next"));
        }
    }

    #[test]
    fn pending_plugin_empty_keeps_rows_but_completed_empty_removes_them() {
        let mut model = Model::default();
        model.commit_results(vec![result("row", "payload")]);
        model
            .groups
            .begin([false, false, true], true, Some(Source::Plugins));
        let update = |pending| Update {
            kind: providers::UpdateKind::Results,
            generation: 1,
            source: Source::Plugins,
            entries: Vec::new(),
            exclusive: true,
            pending,
            status: None,
        };
        let results = model.groups.apply(update(true));
        assert!(!model.commit_results(results));
        assert_eq!(model.results.len(), 1);
        assert!(model.groups.current_entries(Source::Plugins).is_empty());
        let results = model.groups.apply(update(false));
        assert!(model.commit_results(results));
        assert!(model.results.is_empty());
    }

    #[test]
    fn progress_waits_for_sustained_work_and_survives_continuous_input() {
        let now = Instant::now();
        let mut model = Model {
            query_active: true,
            ..Default::default()
        };
        model.start_busy(now, false);
        assert!(model
            .progress_phase(now + Duration::from_millis(299))
            .is_none());
        model.start_busy(now + Duration::from_millis(250), true);
        assert_eq!(model.busy_since, Some(now));
        assert!(model.progress_phase(now + PROGRESS_DELAY).is_some());
        let phase = model.progress_phase(now + Duration::from_millis(600));
        model.start_busy(now + Duration::from_millis(600), true);
        assert_eq!(
            model.progress_phase(now + Duration::from_millis(600)),
            phase
        );
        model.busy_since = None;
        assert!(model.progress_phase(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn progress_resets_on_context_change_and_stays_hidden_when_inactive() {
        let now = Instant::now();
        let mut model = Model {
            query_active: true,
            ..Default::default()
        };
        model.start_busy(now, false);
        let switched = now + Duration::from_secs(1);
        model.start_busy(switched, false);
        assert!(model.progress_phase(switched).is_none());
        assert!(model.progress_phase(switched + PROGRESS_DELAY).is_some());
        model.query_active = false;
        assert!(model.progress_phase(switched + PROGRESS_DELAY).is_none());
    }
}

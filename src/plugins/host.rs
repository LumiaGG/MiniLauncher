use super::{
    protocol::{parse_items, PluginAction},
    routing::route_plugins,
    session::{RpcError, Session},
    Executed, Request,
};
use crate::{
    config::PluginSettings,
    plugin_catalog::{discover, Plugin},
    providers::{Emit, Source, Update},
    result::{Action, Entry},
};
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::{Duration, Instant},
};
const GLOBAL_DEBOUNCE: Duration = Duration::from_millis(60);

pub(super) struct Host {
    attachments: Vec<crate::input_attachment::InputImage>,
    settings: PluginSettings,
    plugins: Vec<Plugin>,
    errors: Vec<String>,
    sessions: HashMap<String, Session>,
    emit: Emit,
    executed: Executed,
    latest: Arc<AtomicU64>,
}

fn query_plugin(
    plugin: &Plugin,
    session: Option<Session>,
    query: &str,
    attachments: &[crate::input_attachment::InputImage],
    generation: u64,
    global: bool,
    latest: &AtomicU64,
) -> (Option<Session>, Result<Vec<Entry>, RpcError>) {
    if !attachments.is_empty() && !plugin.manifest.accepts_images {
        return (
            session,
            Err(RpcError::Failed(
                "此插件未声明支持图片输入，请切换到支持图片的插件范围".into(),
            )),
        );
    }
    let _timing = crate::diagnostics::timing("plugin_query", &plugin.manifest.id);
    if generation != latest.load(Ordering::Relaxed) {
        return (session, Err(RpcError::Cancelled));
    }
    let mut session = match session
        .map(Ok)
        .unwrap_or_else(|| Session::start_cancellable(plugin, Some((latest, generation))))
    {
        Ok(session) => session,
        Err(error) => return (None, Err(RpcError::Failed(error))),
    };
    if generation != latest.load(Ordering::Relaxed) {
        return (Some(session), Err(RpcError::Cancelled));
    }
    let result = session
        .rpc(
            "query",
            json!({"query":query,"max_results":100,"generation":generation,"is_global":global,"attachments":attachments}),
            Some((latest, generation)),
            Duration::from_millis(plugin.manifest.timeout_ms),
        )
        .and_then(|value| parse_items(value, plugin).map_err(RpcError::Failed));
    if matches!(result, Err(RpcError::Failed(_))) {
        (None, result)
    } else {
        (Some(session), result)
    }
}

impl Host {
    pub(super) fn new(
        settings: PluginSettings,
        emit: Emit,
        executed: Executed,
        latest: Arc<AtomicU64>,
    ) -> Self {
        let (plugins, errors) = discover(&settings);
        Self {
            attachments: Vec::new(),
            settings,
            plugins,
            errors,
            sessions: HashMap::new(),
            emit,
            executed,
            latest,
        }
    }
    fn reload(&mut self, settings: PluginSettings) {
        self.sessions.clear();
        self.settings = settings;
        (self.plugins, self.errors) = discover(&self.settings);
        self.start_residents();
    }
    fn start_residents(&mut self) {
        for plugin in self.plugins.iter().filter(|p| p.manifest.resident) {
            match Session::start(plugin) {
                Ok(session) => {
                    self.sessions.insert(plugin.manifest.id.clone(), session);
                }
                Err(error) => self
                    .errors
                    .push(format!("{}：{error}", plugin.manifest.name)),
            }
        }
    }
    pub(super) fn run(mut self, receiver: Receiver<Request>) {
        self.start_residents();
        loop {
            self.sessions.retain(|_, session| {
                let alive = session.poll_abandoned();
                (session.resident
                    || session.last_used.elapsed() < Duration::from_secs(session.idle_secs))
                    && alive
            });
            for plugin in self.plugins.iter().filter(|p| p.manifest.resident) {
                if !self.sessions.contains_key(&plugin.manifest.id) {
                    if let Ok(session) = Session::start(plugin) {
                        self.sessions.insert(plugin.manifest.id.clone(), session);
                    }
                }
            }
            let delay = self
                .sessions
                .values()
                .filter_map(|session| {
                    let idle = (!session.resident).then(|| {
                        Duration::from_secs(session.idle_secs)
                            .saturating_sub(session.last_used.elapsed())
                    });
                    let abandoned = session
                        .abandoned_until
                        .map(|deadline| deadline.saturating_duration_since(Instant::now()));
                    match (idle, abandoned) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                })
                .min();
            let next = if let Some(delay) = delay {
                match receiver.recv_timeout(delay) {
                    Ok(request) => Some(request),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => None,
                }
            } else {
                receiver.recv().ok()
            };
            let Some(first) = next else {
                break;
            };
            let mut actions = VecDeque::new();
            let mut query = None;
            for request in std::iter::once(first).chain(receiver.try_iter()) {
                match request {
                    Request::Query { .. } => query = Some(request),
                    other => actions.push_back(other),
                }
            }
            for action in actions {
                match action {
                    Request::Reload(settings) => self.reload(settings),
                    Request::PrepareDrag {
                        plugin,
                        action,
                        reply,
                    } => {
                        let _ = reply.send(self.prepare_drag(&plugin, &action));
                    }
                    Request::Execute { id, plugin, action } => {
                        let result = self.execute(&plugin, &action);
                        (self.executed)(id, result);
                    }
                    _ => {}
                }
            }
            while let Some(Request::Query {
                attachments,
                generation,
                text,
                settings,
                scope,
                allow_empty,
            }) = query.take()
            {
                if self.settings != settings {
                    self.reload(settings);
                }
                let (matched, _, exclusive) = route_plugins(&self.plugins, &text);
                if scope.is_none() && !exclusive && !matched.is_empty() {
                    match receiver.recv_timeout(GLOBAL_DEBOUNCE) {
                        Ok(next @ Request::Query { .. }) => {
                            query = Some(next);
                            continue;
                        }
                        Ok(Request::Reload(settings)) => {
                            self.reload(settings);
                            break;
                        }
                        Ok(Request::Execute { id, plugin, action }) => {
                            let result = self.execute(&plugin, &action);
                            (self.executed)(id, result);
                        }
                        Ok(Request::PrepareDrag {
                            plugin,
                            action,
                            reply,
                        }) => {
                            let _ = reply.send(self.prepare_drag(&plugin, &action));
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                self.attachments = attachments;
                self.query_scoped(generation, &text, scope.as_deref(), allow_empty);
                self.attachments.clear();
            }
        }
    }
    fn session(&mut self, plugin: &Plugin) -> Result<&mut Session, String> {
        let id = &plugin.manifest.id;
        if !self.sessions.contains_key(id) {
            if !plugin.manifest.resident
                && self.sessions.values().filter(|s| !s.resident).count()
                    >= self.settings.max_processes
            {
                if let Some(oldest) = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| !s.resident)
                    .min_by_key(|(_, s)| s.last_used)
                    .map(|(id, _)| id.clone())
                {
                    self.sessions.remove(&oldest);
                }
            }
            self.sessions.insert(id.clone(), Session::start(plugin)?);
        }
        Ok(self.sessions.get_mut(id).unwrap())
    }
    #[cfg(test)]
    fn query(&mut self, generation: u64, text: &str) {
        self.query_scoped(generation, text, None, true);
    }
    fn query_scoped(
        &mut self,
        generation: u64,
        text: &str,
        scope: Option<&str>,
        allow_empty: bool,
    ) {
        if generation != self.latest.load(Ordering::Relaxed) {
            return;
        }
        if scope.is_none() && text.trim() == ">plugins" {
            let entries = vec![Entry::new(
                "插件管理".into(),
                "打开全部设置 TUI 的插件管理页".into(),
                Action::PluginSettings,
                "设置",
            )];
            let status = if self.errors.is_empty() {
                format!("已加载 {} 个插件", self.plugins.len())
            } else {
                self.errors.join("；")
            };
            (self.emit)(Update {
                kind: crate::providers::UpdateKind::Results,
                generation,
                source: Source::Plugins,
                entries,
                exclusive: true,
                pending: false,
                status: Some(status),
            });
            return;
        }
        let (plugins, query, exclusive) = if let Some(id) = scope {
            (
                self.plugins
                    .iter()
                    .filter(|p| p.manifest.id == id)
                    .cloned()
                    .collect::<Vec<_>>(),
                text,
                true,
            )
        } else if !allow_empty && text.trim().is_empty() {
            (Vec::new(), "", false)
        } else {
            route_plugins(&self.plugins, text)
        };
        if plugins.is_empty() {
            (self.emit)(Update {
                kind: crate::providers::UpdateKind::Results,
                generation,
                source: Source::Plugins,
                entries: Vec::new(),
                exclusive: false,
                pending: false,
                status: (!self.attachments.is_empty())
                    .then(|| "请选择支持图片输入的插件范围，或在输入中添加其关键词".into()),
            });
            return;
        }
        (self.emit)(Update {
            kind: crate::providers::UpdateKind::State,
            generation,
            source: Source::Plugins,
            entries: Vec::new(),
            exclusive,
            pending: true,
            status: None,
        });
        let count = plugins.len();
        let mut results = vec![Vec::<Entry>::new(); count];
        let mut errors = vec![None; count];
        let mut waiting = plugins.into_iter().enumerate().peekable();
        let mut running = 0;
        let (sender, receiver) = mpsc::channel();
        // Each job owns its session. Bound live processes, including idle sessions,
        // and join cancelled jobs before accepting the next query or callback.
        std::thread::scope(|scope| loop {
            while generation == self.latest.load(Ordering::Relaxed)
                && running < self.settings.max_processes
                && waiting.peek().is_some()
            {
                let (index, plugin) = waiting.next().unwrap();
                let session = self.sessions.remove(&plugin.manifest.id);
                if session.is_none() {
                    while !plugin.manifest.resident
                        && self.sessions.values().filter(|s| !s.resident).count() + running
                            >= self.settings.max_processes
                    {
                        let oldest = self
                            .sessions
                            .iter()
                            .filter(|(_, session)| !session.resident)
                            .min_by_key(|(_, session)| session.last_used)
                            .map(|(id, _)| id.clone())
                            .unwrap();
                        self.sessions.remove(&oldest);
                    }
                }
                running += 1;
                let sender = sender.clone();
                let latest = self.latest.clone();
                let attachments = &self.attachments;
                scope.spawn(move || {
                    let (session, result) = query_plugin(
                        &plugin,
                        session,
                        query,
                        attachments,
                        generation,
                        !exclusive,
                        &latest,
                    );
                    let _ = sender.send((index, plugin, session, result));
                });
            }
            if running == 0 {
                break;
            }
            let (index, plugin, session, result) = receiver.recv().unwrap();
            running -= 1;
            if let Some(session) = session {
                self.sessions.insert(plugin.manifest.id.clone(), session);
            }
            if generation != self.latest.load(Ordering::Relaxed) {
                continue;
            }
            match result {
                Ok(entries) => results[index] = entries,
                Err(RpcError::Failed(error)) => {
                    errors[index] = Some(format!("{}：{error}", plugin.manifest.name))
                }
                Err(RpcError::Cancelled) => continue,
            }
            (self.emit)(Update {
                kind: crate::providers::UpdateKind::Results,
                generation,
                source: Source::Plugins,
                entries: results.iter().flatten().cloned().collect(),
                exclusive,
                pending: running > 0 || waiting.peek().is_some(),
                status: {
                    let errors: Vec<_> = errors.iter().flatten().cloned().collect();
                    (!errors.is_empty()).then(|| errors.join("；"))
                },
            });
        });
    }
    fn execute(&mut self, id: &str, action: &str) -> Result<Option<Action>, String> {
        let plugin = self
            .plugins
            .iter()
            .find(|plugin| plugin.manifest.id == id)
            .cloned()
            .ok_or("插件已移除，请重新搜索")?;
        let result = self.session(&plugin)?.rpc(
            "execute",
            json!({"action_id":action}),
            None,
            Duration::from_millis(plugin.manifest.timeout_ms),
        );
        match result {
            Ok(mut value) => match value
                .as_object_mut()
                .and_then(|object| object.remove("action"))
            {
                Some(action) => {
                    let action: PluginAction =
                        serde_json::from_value(action).map_err(|e| e.to_string())?;
                    if matches!(action, PluginAction::Execute { .. }) {
                        return Err("execute 不能递归返回另一个 execute 动作".into());
                    }
                    Ok(Some(action.into_action(&plugin.manifest.id)?))
                }
                None => Ok(None),
            },
            Err(error) => {
                self.sessions.remove(id);
                Err(error.message())
            }
        }
    }
    fn prepare_drag(
        &mut self,
        id: &str,
        action: &str,
    ) -> Result<crate::drag_data::DragData, String> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.manifest.id == id)
            .cloned()
            .ok_or("插件已移除，请重新搜索")?;
        let response = self.session(&plugin)?.rpc(
            "prepare_drag",
            json!({"action_id":action}),
            None,
            Duration::from_millis(plugin.manifest.timeout_ms),
        );
        match response {
            Ok(mut value) => {
                let mut data: crate::drag_data::DragData = serde_json::from_value(
                    value
                        .as_object_mut()
                        .and_then(|object| object.remove("data"))
                        .ok_or("拖动响应缺少 data")?,
                )
                .map_err(|e| e.to_string())?;
                data.resolve(&plugin.directory)?;
                Ok(data)
            }
            Err(error) => {
                self.sessions.remove(id);
                Err(error.message())
            }
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;

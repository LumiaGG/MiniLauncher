//! On-demand executable plugins: bounded NDJSON JSON-RPC, process isolation,
//! cooperative query cancellation, deadlines and idle process eviction.
pub use crate::plugin_catalog::{
    inspect, plugin_hotkeys, search_catalog, search_scopes, validate_directory,
    validate_web_keywords, PluginInfo, SearchCatalog,
};
use crate::{config::PluginSettings, providers::Emit, result::Action};
use std::sync::{
    atomic::AtomicU64,
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc,
};
mod host;
mod protocol;
mod routing;
mod session;
use host::Host;

/// Immutable routing snapshot; querying it does no filesystem or process work.
#[derive(Default)]
pub struct QueryRouting(Vec<crate::plugin_catalog::Plugin>);

impl QueryRouting {
    pub(crate) fn new(plugins: Vec<crate::plugin_catalog::Plugin>) -> Self {
        Self(plugins)
    }

    pub fn exclusive_context(&self, text: &str) -> Vec<String> {
        let (plugins, _, exclusive) = routing::route_plugins(&self.0, text);
        if exclusive {
            plugins.into_iter().map(|p| p.manifest.id).collect()
        } else {
            Vec::new()
        }
    }
}
pub type Executed = Arc<dyn Fn(u64, Result<Option<Action>, String>) + Send + Sync>;
pub enum Request {
    PrepareDrag {
        plugin: String,
        action: String,
        reply: SyncSender<Result<crate::drag_data::DragData, String>>,
    },
    Query {
        attachments: Vec<crate::input_attachment::InputImage>,
        generation: u64,
        text: String,
        settings: PluginSettings,
        scope: Option<String>,
        allow_empty: bool,
    },
    Execute {
        id: u64,
        plugin: String,
        action: String,
    },
    Reload(PluginSettings),
}
pub struct Client {
    sender: Sender<Request>,
}
impl Client {
    pub fn prepare_drag(
        &self,
        plugin: String,
        action: String,
    ) -> Result<Receiver<Result<crate::drag_data::DragData, String>>, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(Request::PrepareDrag {
                plugin,
                action,
                reply,
            })
            .map_err(|_| "插件线程已停止".to_string())?;
        Ok(receiver)
    }
    pub fn start(
        settings: PluginSettings,
        emit: Emit,
        executed: Executed,
        latest: Arc<AtomicU64>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || Host::new(settings, emit, executed, latest).run(receiver));
        Self { sender }
    }
    pub fn send(&self, request: Request) -> bool {
        self.sender.send(request).is_ok()
    }
}

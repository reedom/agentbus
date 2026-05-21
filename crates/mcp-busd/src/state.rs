//! Process-wide shared state.

use std::sync::Arc;

use mcp_bus_core::eventlog::EventLog;
use mcp_bus_core::registry::Registry;
use mcp_bus_core::router::Router;

use crate::config::Config;
use crate::hookinbox::HookInbox;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub registry: Arc<Registry>,
    pub router: Arc<Router>,
    pub log: Arc<EventLog>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<mcp_bus_core::envelope::Envelope>,
    pub hookinbox: Arc<HookInbox>,
}

impl AppState {
    pub async fn new_async(cfg: Config, log: Arc<EventLog>) -> anyhow::Result<Self> {
        let registry = Arc::new(Registry::new());
        let router = Router::new(registry.clone());
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(256);
        let hookinbox = Arc::new(HookInbox::new(cfg.resolved_inbox_dir()).await?);
        Ok(Self {
            cfg: Arc::new(cfg),
            registry,
            router,
            log,
            broadcast_tx,
            hookinbox,
        })
    }
}

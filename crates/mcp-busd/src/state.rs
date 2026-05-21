//! Process-wide shared state.

use std::sync::Arc;

use mcp_bus_core::eventlog::EventLog;
use mcp_bus_core::registry::Registry;
use mcp_bus_core::router::Router;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub registry: Arc<Registry>,
    pub router: Arc<Router>,
    pub log: Arc<EventLog>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<mcp_bus_core::envelope::Envelope>,
}

impl AppState {
    pub fn new(cfg: Config, log: Arc<EventLog>) -> Self {
        let registry = Arc::new(Registry::new());
        let router = Router::new(registry.clone());
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            cfg: Arc::new(cfg),
            registry,
            router,
            log,
            broadcast_tx,
        }
    }
}

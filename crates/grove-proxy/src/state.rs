//! Shared, hot-reloadable state handed to every request handler.

use std::sync::Arc;

use tokio::sync::RwLock;

use grove_core::registry::SiteRegistry;
use grove_core::RequestLog;

/// The registry is swapped wholesale on `reload`, so requests in flight keep
/// using a consistent snapshot.
#[derive(Clone)]
pub struct SharedState {
    pub registry: Arc<RwLock<SiteRegistry>>,
    /// Ring buffer of recent proxied requests (the request timeline).
    pub log: Arc<RequestLog>,
    /// Captured inbound webhooks (requests to `/__grove/hooks/...`), reusing the
    /// same store so they get inspect + replay + copy-as-test for free.
    pub hooks: Arc<RequestLog>,
}

impl SharedState {
    pub fn new(registry: SiteRegistry) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
            log: Arc::new(RequestLog::new(500)),
            hooks: Arc::new(RequestLog::new(200)),
        }
    }

    /// The shared request log, so the daemon can answer timeline queries.
    pub fn log(&self) -> Arc<RequestLog> {
        self.log.clone()
    }

    /// The shared webhook store.
    pub fn hooks(&self) -> Arc<RequestLog> {
        self.hooks.clone()
    }

    pub async fn replace(&self, registry: SiteRegistry) {
        *self.registry.write().await = registry;
    }
}

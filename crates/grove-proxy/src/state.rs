//! Shared, hot-reloadable state handed to every request handler.

use std::sync::Arc;

use tokio::sync::RwLock;

use grove_core::registry::{KnownHosts, SiteRegistry};
use grove_core::RequestLog;

/// The registry is swapped wholesale on `reload`, so requests in flight keep
/// using a consistent snapshot.
#[derive(Clone)]
pub struct SharedState {
    pub registry: Arc<RwLock<SiteRegistry>>,
    /// The registry's hostnames, readable without awaiting.
    ///
    /// rustls resolves a certificate inside a synchronous callback, so the TLS
    /// layer cannot take the async lock above. This mirror is refreshed
    /// wherever the registry is, and is the only thing the SNI resolver reads.
    pub known_hosts: Arc<std::sync::RwLock<KnownHosts>>,
    /// Ring buffer of recent proxied requests (the request timeline).
    pub log: Arc<RequestLog>,
    /// Captured inbound webhooks (requests to `/__grove/hooks/...`), reusing the
    /// same store so they get inspect + replay + copy-as-test for free.
    pub hooks: Arc<RequestLog>,
}

impl SharedState {
    pub fn new(registry: SiteRegistry) -> Self {
        let known_hosts = Arc::new(std::sync::RwLock::new(registry.known_hosts()));
        Self {
            registry: Arc::new(RwLock::new(registry)),
            known_hosts,
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
        // Refresh the mirror first: a certificate refused for a site that now
        // exists is a worse failure than one briefly issued for a site that just
        // went away, and the window is microseconds either way.
        if let Ok(mut hosts) = self.known_hosts.write() {
            *hosts = registry.known_hosts();
        }
        *self.registry.write().await = registry;
    }
}

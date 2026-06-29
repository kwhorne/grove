//! Shared, hot-reloadable state handed to every request handler.

use std::sync::Arc;

use tokio::sync::RwLock;

use grove_core::registry::SiteRegistry;

/// The registry is swapped wholesale on `reload`, so requests in flight keep
/// using a consistent snapshot.
#[derive(Clone)]
pub struct SharedState {
    pub registry: Arc<RwLock<SiteRegistry>>,
}

impl SharedState {
    pub fn new(registry: SiteRegistry) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
        }
    }

    pub async fn replace(&self, registry: SiteRegistry) {
        *self.registry.write().await = registry;
    }
}

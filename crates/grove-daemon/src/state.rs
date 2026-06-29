//! Mutable daemon state: the live config + a handle to the proxy's hot-swap
//! registry. All config mutations funnel through here so they are persisted and
//! the registry rebuilt atomically.

use tokio::sync::Mutex;

use grove_core::{paths::GrovePaths, Config, SiteRegistry};
use grove_proxy::SharedState;

pub struct DaemonState {
    pub paths: GrovePaths,
    pub config: Mutex<Config>,
    pub shared: SharedState,
}

impl DaemonState {
    pub fn new(paths: GrovePaths, config: Config, shared: SharedState) -> Self {
        Self {
            paths,
            config: Mutex::new(config),
            shared,
        }
    }

    /// Persist the current config and rebuild + swap the live registry.
    pub async fn persist_and_reload(&self) -> anyhow::Result<usize> {
        let config = self.config.lock().await;
        config.save(&self.paths)?;
        let registry = SiteRegistry::build(&config);
        let count = registry.len();
        self.shared.replace(registry).await;
        Ok(count)
    }

    /// Rebuild the registry from current config without writing to disk.
    pub async fn reload(&self) -> anyhow::Result<usize> {
        let config = self.config.lock().await;
        let registry = SiteRegistry::build(&config);
        let count = registry.len();
        self.shared.replace(registry).await;
        Ok(count)
    }
}

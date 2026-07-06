//! Mutable daemon state: the live config + a handle to the proxy's hot-swap
//! registry. All config mutations funnel through here so they are persisted and
//! the registry rebuilt atomically.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use grove_core::{paths::GrovePaths, Config, SiteRegistry};
use grove_proxy::SharedState;
use grove_runtime::FpmManager;
use grove_services::{MailStore, ServiceManager};

use crate::dev::DevManager;
use crate::docker::DockerContainer;
use crate::tunnels::TunnelManager;

pub struct DaemonState {
    pub paths: GrovePaths,
    pub config: Mutex<Config>,
    pub shared: SharedState,
    /// Captured outgoing mail (mail-catcher).
    pub mail: MailStore,
    /// Bundled service supervisor (PostgreSQL, …).
    pub services: Arc<ServiceManager>,
    /// Lazy PHP-FPM pool supervisor (needed to reload pools on config changes
    /// such as toggling Xdebug).
    pub fpm: Arc<FpmManager>,
    /// Active public tunnels (`grove share`).
    pub tunnels: Arc<TunnelManager>,
    /// Per-site dev processes (Vite / queue worker).
    pub dev: Arc<DevManager>,
    /// Docker/OrbStack containers discovered as `<name>.test` sites.
    pub docker_sites: Mutex<Vec<DockerContainer>>,
    /// Notified when a graceful shutdown is requested (via IPC or signal).
    pub shutdown: Arc<Notify>,
}

impl DaemonState {
    pub fn new(
        paths: GrovePaths,
        config: Config,
        shared: SharedState,
        mail: MailStore,
        services: Arc<ServiceManager>,
        fpm: Arc<FpmManager>,
    ) -> Self {
        Self {
            paths,
            config: Mutex::new(config),
            shared,
            mail,
            services,
            fpm,
            tunnels: Arc::new(TunnelManager::new()),
            dev: Arc::new(DevManager::new()),
            docker_sites: Mutex::new(Vec::new()),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Trigger a graceful shutdown.
    pub fn request_shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Build the live registry from config + discovered Docker sites.
    async fn build_registry(&self, config: &Config) -> SiteRegistry {
        let mut registry = SiteRegistry::build(config);
        if config.general.docker {
            for d in self.docker_sites.lock().await.iter() {
                registry.insert_docker(&d.name, d.upstream.as_deref(), Some(&d.id), d.running);
            }
        }
        registry
    }

    /// Persist the current config and rebuild + swap the live registry.
    pub async fn persist_and_reload(&self) -> anyhow::Result<usize> {
        let config = self.config.lock().await;
        config.save(&self.paths)?;
        let registry = self.build_registry(&config).await;
        let count = registry.len();
        self.shared.replace(registry).await;
        Ok(count)
    }

    /// Rebuild the registry from current config without writing to disk.
    pub async fn reload(&self) -> anyhow::Result<usize> {
        let config = self.config.lock().await;
        let registry = self.build_registry(&config).await;
        let count = registry.len();
        self.shared.replace(registry).await;
        Ok(count)
    }
}

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

/// What became of one network listener when the daemon started.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListenerHealth {
    /// Not attempted yet.
    #[default]
    Pending,
    /// Bound and accepting.
    Up,
    /// The bind failed; this is the OS error.
    Failed(String),
}

/// Bind results for the three listeners, kept so `grove status` and
/// `grove doctor` report what actually happened rather than assuming.
///
/// Before this, a bind failure was one line in `daemon.log`. IPC came up
/// regardless, `grove start` said "started", `status` printed `● dns` from a
/// hardcoded `true`, and the GUI showed a green pill — over a daemon that
/// served nothing because Apache held port 80.
#[derive(Debug, Default)]
pub struct Listeners {
    inner: std::sync::Mutex<[ListenerHealth; 4]>,
}

impl Listeners {
    const DNS: usize = 0;
    const HTTP: usize = 1;
    const HTTPS: usize = 2;
    const MAIL: usize = 3;

    fn set(&self, which: usize, health: ListenerHealth) {
        self.inner.lock().unwrap()[which] = health;
    }
    fn get(&self, which: usize) -> ListenerHealth {
        self.inner.lock().unwrap()[which].clone()
    }

    pub fn set_dns(&self, h: ListenerHealth) {
        self.set(Self::DNS, h)
    }
    pub fn set_http(&self, h: ListenerHealth) {
        self.set(Self::HTTP, h)
    }
    pub fn set_https(&self, h: ListenerHealth) {
        self.set(Self::HTTPS, h)
    }
    pub fn dns(&self) -> ListenerHealth {
        self.get(Self::DNS)
    }
    pub fn http(&self) -> ListenerHealth {
        self.get(Self::HTTP)
    }
    pub fn https(&self) -> ListenerHealth {
        self.get(Self::HTTPS)
    }
    pub fn set_mail(&self, h: ListenerHealth) {
        self.set(Self::MAIL, h)
    }
    pub fn mail(&self) -> ListenerHealth {
        self.get(Self::MAIL)
    }
}

pub struct DaemonState {
    pub paths: GrovePaths,
    /// Bind results for the DNS/HTTP/HTTPS listeners.
    pub listeners: Arc<Listeners>,
    pub config: Mutex<Config>,
    pub shared: SharedState,
    /// Captured outgoing mail (mail-catcher).
    pub mail: MailStore,
    /// Whether SQL query capture (MySQL general log) is currently on.
    pub sql_capture: Mutex<bool>,
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
            listeners: Arc::new(Listeners::default()),
            config: Mutex::new(config),
            shared,
            mail,
            sql_capture: Mutex::new(false),
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

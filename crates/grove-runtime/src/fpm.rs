//! Lazy PHP-FPM pool supervisor.
//!
//! One pool per PHP version, each listening on a Unix socket under the run dir.
//! Pools are spawned on first request for that version (`pm = ondemand`) and the
//! FPM process itself reaps idle workers, keeping memory low.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_proxy::fastcgi::FpmAddr;
use grove_proxy::FpmLocator;

use crate::registry::PhpRegistry;

#[derive(Debug, thiserror::Error)]
pub enum FpmError {
    #[error("no PHP build registered for version {0}")]
    UnknownVersion(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// A running (or to-be-spawned) FPM pool for a single PHP version.
pub struct FpmPool {
    pub version: String,
    pub socket: PathBuf,
    child: Option<Child>,
}

impl Drop for FpmPool {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Supervises FPM pools and answers FastCGI socket lookups for the proxy.
pub struct FpmManager {
    paths: GrovePaths,
    registry: Mutex<PhpRegistry>,
    pools: Mutex<HashMap<String, FpmPool>>,
}

impl FpmManager {
    pub fn new(paths: GrovePaths, registry: PhpRegistry) -> Self {
        Self {
            paths,
            registry: Mutex::new(registry),
            pools: Mutex::new(HashMap::new()),
        }
    }

    /// Look up a build, reloading the on-disk registry once if it is missing
    /// (e.g. just installed via the GUI while the daemon is running).
    fn build_for(&self, version: &str) -> Option<crate::registry::PhpBuild> {
        {
            let reg = self.registry.lock().unwrap();
            if let Some(b) = reg.get(version) {
                return Some(b.clone());
            }
        }
        let fresh = PhpRegistry::load(&self.paths);
        let build = fresh.get(version).cloned();
        *self.registry.lock().unwrap() = fresh;
        build
    }

    fn socket_path(&self, version: &str) -> PathBuf {
        self.paths
            .run_dir()
            .join(format!("php-fpm-{}.sock", version.replace('.', "_")))
    }

    /// Ensure a pool for `version` is running; return its socket address.
    fn ensure_pool(&self, version: &str) -> Result<FpmAddr, FpmError> {
        let mut pools = self.pools.lock().unwrap();

        if let Some(pool) = pools.get_mut(version) {
            // If the child died, fall through and respawn.
            let alive = pool
                .child
                .as_mut()
                .map(|c| matches!(c.try_wait(), Ok(None)))
                .unwrap_or(false);
            if alive {
                return Ok(FpmAddr::Unix(pool.socket.clone()));
            }
        }

        let build = self
            .build_for(version)
            .ok_or_else(|| FpmError::UnknownVersion(version.to_string()))?;

        self.paths.ensure()?;
        let socket = self.socket_path(version);
        let _ = std::fs::remove_file(&socket);
        let log = self.paths.logs_dir().join(format!("php-fpm-{version}.log"));
        let conf = self.write_pool_config(version, &socket, &log)?;

        tracing::info!(version, binary = %build.fpm_binary.display(), "spawning PHP-FPM pool");
        let mut cmd = std::process::Command::new(&build.fpm_binary);
        cmd.arg("--nodaemonize").arg("--fpm-config").arg(&conf);
        // When the daemon runs as root (privileged ports), php-fpm refuses to
        // start unless explicitly allowed; workers then drop to the real user
        // via the pool's `user`/`group` directives (see write_pool_config).
        if running_as_root() {
            cmd.arg("--allow-to-run-as-root");
        }
        let child = cmd.spawn()?;

        // Give FPM a moment to create its listen socket.
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        pools.insert(
            version.to_string(),
            FpmPool {
                version: version.to_string(),
                socket: socket.clone(),
                child: Some(child),
            },
        );
        Ok(FpmAddr::Unix(socket))
    }

    fn write_pool_config(
        &self,
        version: &str,
        socket: &std::path::Path,
        log: &std::path::Path,
    ) -> Result<PathBuf, FpmError> {
        let conf_path = self
            .paths
            .runtimes_dir()
            .join(format!("fpm-{}.conf", version.replace('.', "_")));
        let pid = self
            .paths
            .run_dir()
            .join(format!("php-fpm-{}.pid", version.replace('.', "_")));
        // If we're root, run the workers as the real (non-root) user so the
        // app's PHP doesn't execute as root and files stay user-owned.
        let user_directives = match (running_as_root(), target_user()) {
            (true, Some(user)) => format!("user = {user}\nlisten.owner = {user}\n"),
            _ => String::new(),
        };
        let body = format!(
            r#"[global]
pid = {pid}
error_log = {log}
daemonize = no
log_limit = 8192

[grove]
listen = {socket}
listen.mode = 0660
{user_directives}pm = ondemand
pm.max_children = 16
pm.process_idle_timeout = 10s
pm.max_requests = 500
catch_workers_output = yes
clear_env = no
"#,
            pid = pid.display(),
            log = log.display(),
            socket = socket.display(),
            user_directives = user_directives,
        );
        std::fs::write(&conf_path, body)?;
        Ok(conf_path)
    }
}

/// Whether the current process runs with effective uid 0 (root).
fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            #[link_name = "geteuid"]
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid is always safe.
        unsafe { geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The real user to run PHP workers as when the daemon is root. Prefers an
/// explicit `GROVE_RUN_USER` (set by the service installer), else `SUDO_USER`.
fn target_user() -> Option<String> {
    for var in ["GROVE_RUN_USER", "SUDO_USER"] {
        if let Ok(u) = std::env::var(var) {
            if !u.is_empty() && u != "root" {
                return Some(u);
            }
        }
    }
    None
}

impl FpmLocator for FpmManager {
    fn locate(&self, php_version: &str) -> Option<FpmAddr> {
        match self.ensure_pool(php_version) {
            Ok(addr) => Some(addr),
            Err(e) => {
                tracing::error!(error = %e, version = php_version, "failed to start FPM pool");
                None
            }
        }
    }
}

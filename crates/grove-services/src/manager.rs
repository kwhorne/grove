//! Downloads, initialises and supervises bundled services.
//!
//! Mirrors the PHP runtime approach: a portable build is fetched into
//! `$GROVE_HOME/services/<key>/`, initialised once (e.g. `initdb`), and run as a
//! child process with its data directory under the same tree. Stopping the
//! daemon stops the services (the child handles are killed on drop).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use grove_core::paths::GrovePaths;

use crate::catalog::{self, ServiceKind, ServiceSpec};

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("unknown service {0:?}")]
    Unknown(String),
    #[error("no portable build of {0} for this platform")]
    Unsupported(String),
    #[error("service {0} is not installed")]
    NotInstalled(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("init failed: {0}")]
    Init(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ServiceError>;

/// Status projection surfaced to the CLI/GUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub key: String,
    pub name: String,
    pub category: String,
    pub installed: bool,
    pub running: bool,
    pub port: u16,
    pub version: String,
    /// Loopback host clients connect to.
    pub host: String,
    /// Default username for local dev (if any).
    pub username: Option<String>,
    /// Unix socket path (Postgres/MySQL), if applicable.
    pub socket: Option<String>,
    /// Ready-to-copy connection URI.
    pub uri: String,
}

/// Persisted, re-derivable service state: which services should auto-start.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ServicesState {
    /// service key -> auto-start on daemon boot.
    #[serde(default)]
    autostart: BTreeMap<String, bool>,
    /// service key -> port override (falls back to the catalog default).
    #[serde(default)]
    ports: BTreeMap<String, u16>,
}

/// Supervises bundled services. Child handles live for the daemon's lifetime.
pub struct ServiceManager {
    paths: GrovePaths,
    procs: Mutex<HashMap<String, Child>>,
    state: Mutex<ServicesState>,
}

impl ServiceManager {
    pub fn new(paths: GrovePaths) -> Self {
        let state = load_state(&paths);
        Self {
            paths,
            procs: Mutex::new(HashMap::new()),
            state: Mutex::new(state),
        }
    }

    fn set_autostart(&self, key: &str, enabled: bool) {
        self.state
            .lock()
            .unwrap()
            .autostart
            .insert(key.to_string(), enabled);
        save_state(&self.paths, &self.state.lock().unwrap());
    }

    /// Effective listen port: a user override, else the catalog default.
    fn effective_port(&self, spec: &ServiceSpec) -> u16 {
        self.state
            .lock()
            .unwrap()
            .ports
            .get(spec.key)
            .copied()
            .unwrap_or(spec.default_port)
    }

    /// Override a service's listen port (takes effect on next start/restart).
    pub fn set_port(&self, key: &str, port: u16) -> Result<()> {
        let _ = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.to_string()))?;
        self.state
            .lock()
            .unwrap()
            .ports
            .insert(key.to_string(), port);
        save_state(&self.paths, &self.state.lock().unwrap());
        Ok(())
    }

    fn wants_autostart(&self, key: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .autostart
            .get(key)
            .copied()
            .unwrap_or(false)
    }

    /// Start every service that is **installed** and flagged for auto-start.
    /// Called on daemon boot; never touches services that aren't installed.
    pub fn autostart_installed(&self) {
        for spec in catalog::CATALOG {
            if self.is_installed(spec) && self.wants_autostart(spec.key) {
                if let Err(e) = self.start(spec.key) {
                    tracing::warn!(service = spec.key, error = %e, "auto-start failed");
                }
            }
        }
    }

    fn service_root(&self, spec: &ServiceSpec) -> PathBuf {
        self.paths.services_dir().join(spec.key)
    }

    fn data_dir(&self, spec: &ServiceSpec) -> PathBuf {
        self.service_root(spec).join("data")
    }

    /// Directory containing the service's executables.
    fn bin_dir(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        let root = self.service_root(spec).join(catalog::archive_root(spec)?);
        Some(match spec.kind {
            ServiceKind::Postgres | ServiceKind::Mysql => root.join("bin"),
            // Redis builds in place; binaries land in `src/`.
            ServiceKind::Redis => root.join("src"),
        })
    }

    /// Service base directory (the extracted archive root) — needed by mysqld.
    fn base_dir(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        Some(self.service_root(spec).join(catalog::archive_root(spec)?))
    }

    fn primary_binary(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        let bin = self.bin_dir(spec)?;
        let exe = match spec.kind {
            ServiceKind::Postgres => "postgres",
            ServiceKind::Redis => "redis-server",
            ServiceKind::Mysql => "mysqld",
        };
        Some(bin.join(exe))
    }

    fn is_installed(&self, spec: &ServiceSpec) -> bool {
        self.primary_binary(spec)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    fn is_running(&self, key: &str) -> bool {
        let mut procs = self.procs.lock().unwrap();
        match procs.get_mut(key) {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Status for every catalog entry, including connection details.
    pub fn status_all(&self) -> Vec<ServiceStatus> {
        catalog::CATALOG
            .iter()
            .map(|spec| {
                let port = self.effective_port(spec);
                let (username, socket, uri) = self.connection_info(spec, port);
                ServiceStatus {
                    key: spec.key.to_string(),
                    name: spec.name.to_string(),
                    category: spec.category.to_string(),
                    installed: self.is_installed(spec),
                    running: self.is_running(spec.key),
                    port,
                    version: spec.version.to_string(),
                    host: "127.0.0.1".to_string(),
                    username,
                    socket,
                    uri,
                }
            })
            .collect()
    }

    /// Build (username, socket, connection-uri) for a service.
    fn connection_info(
        &self,
        spec: &ServiceSpec,
        port: u16,
    ) -> (Option<String>, Option<String>, String) {
        match spec.kind {
            ServiceKind::Postgres => (
                Some("grove".into()),
                Some(self.paths.run_dir().to_string_lossy().into_owned()),
                format!("postgresql://grove@127.0.0.1:{port}/postgres"),
            ),
            ServiceKind::Mysql => (
                Some("root".into()),
                Some(
                    self.paths
                        .run_dir()
                        .join("mysql.sock")
                        .to_string_lossy()
                        .into_owned(),
                ),
                format!("mysql://root@127.0.0.1:{port}"),
            ),
            ServiceKind::Redis => (None, None, format!("redis://127.0.0.1:{port}")),
        }
    }

    /// Download + extract + initialise a service. Idempotent.
    pub fn install(&self, key: &str, progress: impl Fn(&str)) -> Result<()> {
        let spec = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.to_string()))?;
        let url = catalog::download_url(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        self.paths.ensure()?;
        let root = self.service_root(spec);
        std::fs::create_dir_all(&root)?;

        if !self.is_installed(spec) {
            progress(&format!("downloading {} {}…", spec.name, spec.version));
            let bytes = http_get(&url)?;
            progress("extracting…");
            extract_tar_gz(&bytes, &root)?;
            // Redis ships source only; compile it in place (no external deps).
            if spec.kind == ServiceKind::Redis {
                self.build_redis(spec, &progress)?;
            }
            make_executables(&self.bin_dir(spec))?;
        }

        // One-time initialisation.
        match spec.kind {
            ServiceKind::Postgres => self.init_postgres(spec, &progress)?,
            ServiceKind::Mysql => self.init_mysql(spec, &progress)?,
            ServiceKind::Redis => {}
        }
        progress(&format!("{} ready", spec.name));
        self.set_autostart(spec.key, true);
        Ok(())
    }

    /// Stop then start a service.
    pub fn restart(&self, key: &str) -> Result<()> {
        self.stop(key)?;
        // Give the OS a moment to release the port/socket.
        std::thread::sleep(std::time::Duration::from_millis(300));
        self.start(key)
    }

    /// Initialise a MySQL data directory with a passwordless root (local dev).
    fn init_mysql(&self, spec: &ServiceSpec, progress: &impl Fn(&str)) -> Result<()> {
        let data = self.data_dir(spec);
        if data.join("auto.cnf").exists() {
            return Ok(()); // already initialised
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        let base = self
            .base_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        progress("initialising MySQL data directory…");
        std::fs::create_dir_all(&data)?;
        let out = std::process::Command::new(bin.join("mysqld"))
            .arg("--initialize-insecure")
            .arg(format!("--datadir={}", data.display()))
            .arg(format!("--basedir={}", base.display()))
            .output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(())
    }

    /// Compile Redis from source with `make` (libc malloc, no TLS) — yields a
    /// self-contained `redis-server` linking only system libraries.
    fn build_redis(&self, spec: &ServiceSpec, progress: &impl Fn(&str)) -> Result<()> {
        let src = self.service_root(spec).join(
            catalog::archive_root(spec)
                .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?,
        );
        progress("compiling Redis (make)…");
        let out = std::process::Command::new("make")
            .current_dir(&src)
            .args(["-j4", "MALLOC=libc", "BUILD_TLS=no"])
            .output()
            .map_err(|e| {
                ServiceError::Init(format!(
                    "make failed to start ({e}); a C toolchain is required"
                ))
            })?;
        if !out.status.success() {
            let tail: String = String::from_utf8_lossy(&out.stderr)
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ServiceError::Init(tail));
        }
        Ok(())
    }

    fn init_postgres(&self, spec: &ServiceSpec, progress: &impl Fn(&str)) -> Result<()> {
        let data = self.data_dir(spec);
        if data.join("PG_VERSION").exists() {
            return Ok(()); // already initialised
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        progress("initialising database cluster (initdb)…");
        std::fs::create_dir_all(&data)?;
        let out = std::process::Command::new(bin.join("initdb"))
            .arg("-D")
            .arg(&data)
            .args(["-U", "grove", "--auth=trust", "--encoding=UTF8"])
            .output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(())
    }

    /// Start a service if not already running.
    pub fn start(&self, key: &str) -> Result<()> {
        let spec = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.to_string()))?;
        if !self.is_installed(spec) {
            return Err(ServiceError::NotInstalled(spec.name.into()));
        }
        if self.is_running(key) {
            return Ok(());
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        let log = self.paths.logs_dir().join(format!("{key}.log"));
        let logf = std::fs::File::create(&log)?;
        let port = self.effective_port(spec);

        let child = match spec.kind {
            ServiceKind::Postgres => std::process::Command::new(bin.join("postgres"))
                .arg("-D")
                .arg(self.data_dir(spec))
                .args(["-p", &port.to_string()])
                .arg("-k")
                .arg(self.paths.run_dir())
                .stdout(logf.try_clone()?)
                .stderr(logf)
                .spawn()?,
            ServiceKind::Redis => {
                let data = self.data_dir(spec);
                std::fs::create_dir_all(&data)?;
                std::process::Command::new(bin.join("redis-server"))
                    .args(["--port", &port.to_string()])
                    .arg("--dir")
                    .arg(&data)
                    .args(["--daemonize", "no", "--save", ""])
                    .stdout(logf.try_clone()?)
                    .stderr(logf)
                    .spawn()?
            }
            ServiceKind::Mysql => {
                let base = self
                    .base_dir(spec)
                    .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
                std::process::Command::new(bin.join("mysqld"))
                    .arg(format!("--datadir={}", self.data_dir(spec).display()))
                    .arg(format!("--basedir={}", base.display()))
                    .args(["--port", &port.to_string()])
                    .arg(format!(
                        "--socket={}",
                        self.paths.run_dir().join("mysql.sock").display()
                    ))
                    .arg("--mysqlx=OFF")
                    .stdout(logf.try_clone()?)
                    .stderr(logf)
                    .spawn()?
            }
        };
        tracing::info!(service = key, port, "started service");
        self.procs.lock().unwrap().insert(key.to_string(), child);
        self.set_autostart(key, true);
        Ok(())
    }

    /// Stop a running service. Clears its auto-start flag so it stays stopped
    /// across daemon restarts until the user starts it again.
    pub fn stop(&self, key: &str) -> Result<()> {
        if let Some(mut child) = self.procs.lock().unwrap().remove(key) {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!(service = key, "stopped service");
        }
        self.set_autostart(key, false);
        Ok(())
    }
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        let mut procs = self.procs.lock().unwrap();
        for (_, child) in procs.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---- persisted autostart state ------------------------------------------

fn state_file(paths: &GrovePaths) -> PathBuf {
    paths.services_dir().join("state.json")
}

fn load_state(paths: &GrovePaths) -> ServicesState {
    match std::fs::read_to_string(state_file(paths)) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => ServicesState::default(),
    }
}

fn save_state(paths: &GrovePaths, state: &ServicesState) {
    let _ = paths.ensure();
    if let Ok(body) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(state_file(paths), body);
    }
}

// ---- download / extract helpers -----------------------------------------

fn http_get(url: &str) -> Result<Vec<u8>> {
    // A browser-like UA is required by some mirrors (e.g. Oracle's MySQL CDN
    // returns 403 without one).
    let resp = ureq::get(url)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Grove/0.1",
        )
        .call()
        .map_err(|e| ServiceError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(1024 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn extract_tar_gz(gz_bytes: &[u8], dest: &std::path::Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

#[cfg(unix)]
fn make_executables(bin_dir: &Option<PathBuf>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(dir) = bin_dir else { return Ok(()) };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata() {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(e.path(), perms);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_executables(_bin_dir: &Option<PathBuf>) -> Result<()> {
    Ok(())
}

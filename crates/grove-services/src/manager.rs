//! Downloads, initialises and supervises bundled services.
//!
//! Mirrors the PHP runtime approach: a portable build is fetched into
//! `$GROVE_HOME/services/<key>/`, initialised once (e.g. `initdb`), and run as a
//! child process with its data directory under the same tree. Stopping the
//! daemon stops the services (the child handles are killed on drop).

use std::collections::HashMap;
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
}

/// Supervises bundled services. Child handles live for the daemon's lifetime.
pub struct ServiceManager {
    paths: GrovePaths,
    procs: Mutex<HashMap<String, Child>>,
}

impl ServiceManager {
    pub fn new(paths: GrovePaths) -> Self {
        Self {
            paths,
            procs: Mutex::new(HashMap::new()),
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
            ServiceKind::Postgres => root.join("bin"),
            // Redis builds in place; binaries land in `src/`.
            ServiceKind::Redis => root.join("src"),
        })
    }

    fn primary_binary(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        let bin = self.bin_dir(spec)?;
        let exe = match spec.kind {
            ServiceKind::Postgres => "postgres",
            ServiceKind::Redis => "redis-server",
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

    /// Status for every catalog entry.
    pub fn status_all(&self) -> Vec<ServiceStatus> {
        catalog::CATALOG
            .iter()
            .map(|spec| ServiceStatus {
                key: spec.key.to_string(),
                name: spec.name.to_string(),
                category: spec.category.to_string(),
                installed: self.is_installed(spec),
                running: self.is_running(spec.key),
                port: spec.default_port,
                version: spec.version.to_string(),
            })
            .collect()
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
        if spec.kind == ServiceKind::Postgres {
            self.init_postgres(spec, &progress)?;
        }
        progress(&format!("{} ready", spec.name));
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

        let child = match spec.kind {
            ServiceKind::Postgres => std::process::Command::new(bin.join("postgres"))
                .arg("-D")
                .arg(self.data_dir(spec))
                .args(["-p", &spec.default_port.to_string()])
                .arg("-k")
                .arg(self.paths.run_dir())
                .stdout(logf.try_clone()?)
                .stderr(logf)
                .spawn()?,
            ServiceKind::Redis => {
                let data = self.data_dir(spec);
                std::fs::create_dir_all(&data)?;
                std::process::Command::new(bin.join("redis-server"))
                    .args(["--port", &spec.default_port.to_string()])
                    .arg("--dir")
                    .arg(&data)
                    .args(["--daemonize", "no", "--save", ""])
                    .stdout(logf.try_clone()?)
                    .stderr(logf)
                    .spawn()?
            }
        };
        tracing::info!(service = key, port = spec.default_port, "started service");
        self.procs.lock().unwrap().insert(key.to_string(), child);
        Ok(())
    }

    /// Stop a running service.
    pub fn stop(&self, key: &str) -> Result<()> {
        if let Some(mut child) = self.procs.lock().unwrap().remove(key) {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!(service = key, "stopped service");
        }
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

// ---- download / extract helpers -----------------------------------------

fn http_get(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
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

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
                Some(self.data_dir(spec).to_string_lossy().into_owned()),
                format!("postgresql://grove@127.0.0.1:{port}/postgres"),
            ),
            ServiceKind::Mysql => (
                Some("root".into()),
                Some(
                    self.data_dir(spec)
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

    /// Migrate all user databases from another MySQL server (e.g. Laravel Herd)
    /// into Grove's MySQL, via a logical dump + restore using Grove's own client
    /// tools. Returns a human-readable summary.
    pub fn migrate_mysql(
        &self,
        host: &str,
        port: u16,
        user: &str,
        password: &str,
        progress: impl Fn(&str),
    ) -> Result<String> {
        let spec = catalog::spec("mysql").ok_or_else(|| ServiceError::Unknown("mysql".into()))?;
        if !self.is_installed(spec) {
            return Err(ServiceError::NotInstalled(
                "Grove's MySQL — install it under Services first".into(),
            ));
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        let mysql = bin.join("mysql");
        let mysqldump = bin.join("mysqldump");

        let target_port = self.effective_port(spec);
        let is_local = matches!(host, "127.0.0.1" | "localhost" | "::1");
        if is_local && port == target_port {
            return Err(ServiceError::Init(format!(
                "source and Grove's MySQL both use port {port}. Change Grove's MySQL \
                 port under Services (e.g. 3307), start it, then migrate."
            )));
        }

        // Make sure Grove's MySQL is up to import into.
        if !self.is_running("mysql") {
            progress("starting Grove's MySQL…");
            self.start("mysql")?;
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }

        // Password is passed via MYSQL_PWD to keep it off the process args.
        let pwd_env = |cmd: &mut std::process::Command| {
            if !password.is_empty() {
                cmd.env("MYSQL_PWD", password);
            }
        };

        // 1. List the source's user databases (skip system schemas).
        progress(&format!("reading databases from {host}:{port}…"));
        let mut list_cmd = std::process::Command::new(&mysql);
        list_cmd
            .args(["-h", host, "-P", &port.to_string(), "-u", user, "-N", "-B"])
            .args(["-e", "SHOW DATABASES"]);
        pwd_env(&mut list_cmd);
        let out = list_cmd.output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(format!(
                "cannot connect to source MySQL at {host}:{port}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let system = ["information_schema", "performance_schema", "mysql", "sys"];
        let dbs: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|d| !d.is_empty() && !system.contains(&d.as_str()))
            .collect();
        if dbs.is_empty() {
            return Ok("No user databases found on the source — nothing to migrate.".into());
        }

        // 2. Dump them to a temp file.
        progress(&format!("dumping {} database(s)…", dbs.len()));
        let dump_path = std::env::temp_dir().join(format!("grove-mysql-migrate-{}.sql", port));
        let dump_file = std::fs::File::create(&dump_path)?;
        let mut dump_cmd = std::process::Command::new(&mysqldump);
        dump_cmd
            .args(["-h", host, "-P", &port.to_string(), "-u", user])
            .args([
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                "--no-tablespaces",
                "--column-statistics=0",
                "--databases",
            ])
            .args(&dbs)
            .stdout(dump_file);
        pwd_env(&mut dump_cmd);
        let dump_status = dump_cmd.status()?;
        if !dump_status.success() {
            let _ = std::fs::remove_file(&dump_path);
            return Err(ServiceError::Init("mysqldump failed on the source".into()));
        }

        // 3. Import into Grove's MySQL (root, no password, on the local port).
        progress("importing into Grove…");
        let infile = std::fs::File::open(&dump_path)?;
        let import = std::process::Command::new(&mysql)
            .args([
                "-h",
                "127.0.0.1",
                "-P",
                &target_port.to_string(),
                "-u",
                "root",
            ])
            .stdin(infile)
            .output()?;
        let _ = std::fs::remove_file(&dump_path);
        if !import.status.success() {
            return Err(ServiceError::Init(format!(
                "import into Grove's MySQL failed: {}",
                String::from_utf8_lossy(&import.stderr).trim()
            )));
        }

        Ok(format!(
            "Migrated {} database(s) into Grove's MySQL: {}",
            dbs.len(),
            dbs.join(", ")
        ))
    }

    /// Ensure a bundled DB service is installed + running, returning (bin, port).
    fn db_ready(&self, key: &str) -> Result<(PathBuf, u16)> {
        let spec = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.into()))?;
        if !self.is_installed(spec) {
            return Err(ServiceError::NotInstalled(format!(
                "{key} (add it under Services first)"
            )));
        }
        if !self.is_running(key) {
            self.start(key)?;
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        Ok((bin, self.effective_port(spec)))
    }

    /// Turn MySQL's general query log on or off. When on, statements are written
    /// to `file` (which Grove owns and reads back to correlate SQL with the
    /// request timeline). Requires the bundled MySQL to be running.
    pub fn set_mysql_general_log(&self, on: bool, file: &std::path::Path) -> Result<()> {
        let (bin, port) = self.db_ready("mysql")?;
        let sql = if on {
            format!(
                "SET GLOBAL log_output='FILE'; SET GLOBAL general_log_file='{}'; SET GLOBAL general_log=1;",
                file.display()
            )
        } else {
            "SET GLOBAL general_log=0;".to_string()
        };
        let out = std::process::Command::new(bin.join("mysql"))
            .args(["-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root", "-e", &sql])
            .output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(format!(
                "could not toggle MySQL general log: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Dump a database (or all user databases when `db` is None) from Grove's
    /// bundled MySQL to `out` as SQL.
    pub fn snapshot_mysql(&self, db: Option<&str>, out: &std::path::Path) -> Result<()> {
        let (bin, port) = self.db_ready("mysql")?;
        let file = std::fs::File::create(out)?;
        let mut cmd = std::process::Command::new(bin.join("mysqldump"));
        cmd.args(["-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root"])
            .args([
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                "--no-tablespaces",
                "--column-statistics=0",
            ]);
        match db {
            Some(name) => {
                cmd.arg("--databases").arg(name);
            }
            None => {
                cmd.arg("--all-databases");
            }
        }
        cmd.stdout(file);
        if !cmd.status()?.success() {
            let _ = std::fs::remove_file(out);
            return Err(ServiceError::Init("mysqldump failed".into()));
        }
        Ok(())
    }

    /// Restore an SQL dump into Grove's bundled MySQL.
    pub fn restore_mysql(&self, sql: &std::path::Path) -> Result<()> {
        let (bin, port) = self.db_ready("mysql")?;
        let infile = std::fs::File::open(sql)?;
        let out = std::process::Command::new(bin.join("mysql"))
            .args(["-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root"])
            .stdin(infile)
            .output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(format!(
                "restore into MySQL failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Dump a PostgreSQL database (self-contained, with CREATE/DROP) to `out`.
    pub fn snapshot_postgres(&self, db: &str, out: &std::path::Path) -> Result<()> {
        let (bin, port) = self.db_ready("postgres")?;
        let status = std::process::Command::new(bin.join("pg_dump"))
            .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "grove"])
            .args(["--clean", "--create", "-d", db, "-f"])
            .arg(out)
            .status()?;
        if !status.success() {
            let _ = std::fs::remove_file(out);
            return Err(ServiceError::Init("pg_dump failed".into()));
        }
        Ok(())
    }

    /// Restore a PostgreSQL dump (created with --create) via the `postgres` db.
    pub fn restore_postgres(&self, sql: &std::path::Path) -> Result<()> {
        let (bin, port) = self.db_ready("postgres")?;
        let out = std::process::Command::new(bin.join("psql"))
            .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "grove"])
            .args(["-d", "postgres", "-f"])
            .arg(sql)
            .output()?;
        if !out.status.success() {
            return Err(ServiceError::Init(format!(
                "restore into PostgreSQL failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
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
        // The daemon may be root, but mysqld refuses to run as root — initialise
        // (and later run) as the invoking user, owning the data dir to match.
        let ids = drop_ids();
        if let Some((uid, gid)) = ids {
            chown_recursive(&data, uid, gid);
        }
        let mut cmd = std::process::Command::new(bin.join("mysqld"));
        cmd.arg("--initialize-insecure")
            .arg(format!("--datadir={}", data.display()))
            .arg(format!("--basedir={}", base.display()));
        apply_drop(&mut cmd, ids);
        let out = cmd.output()?;
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
        // Postgres refuses to run as root; init (and run) as the invoking user.
        let ids = drop_ids();
        if let Some((uid, gid)) = ids {
            chown_recursive(&data, uid, gid);
        }
        let mut cmd = std::process::Command::new(bin.join("initdb"));
        cmd.arg("-D")
            .arg(&data)
            .args(["-U", "grove", "--auth=trust", "--encoding=UTF8"]);
        apply_drop(&mut cmd, ids);
        let out = cmd.output()?;
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

        // Databases refuse to run as root; when the daemon is root, drop to the
        // invoking user and make sure the data dir is owned by them. Redis is
        // happy as root, so it is left untouched.
        let ids = drop_ids();
        if ids.is_some() && matches!(spec.kind, ServiceKind::Postgres | ServiceKind::Mysql) {
            if let Some((uid, gid)) = ids {
                chown_recursive(&self.data_dir(spec), uid, gid);
            }
        }

        let child = match spec.kind {
            ServiceKind::Postgres => {
                let data = self.data_dir(spec);
                let mut cmd = std::process::Command::new(bin.join("postgres"));
                cmd.arg("-D")
                    .arg(&data)
                    .args(["-p", &port.to_string()])
                    // Put the unix socket in the user-owned data dir.
                    .arg("-k")
                    .arg(&data)
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
                apply_drop(&mut cmd, ids);
                cmd.spawn()?
            }
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
                let mut cmd = std::process::Command::new(bin.join("mysqld"));
                cmd.arg(format!("--datadir={}", self.data_dir(spec).display()))
                    .arg(format!("--basedir={}", base.display()))
                    .args(["--port", &port.to_string()])
                    .arg(format!(
                        "--socket={}",
                        self.data_dir(spec).join("mysql.sock").display()
                    ))
                    .arg("--mysqlx=OFF")
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
                apply_drop(&mut cmd, ids);
                cmd.spawn()?
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

// ---- privilege dropping ---------------------------------------------------
// The daemon may run as root (macOS LaunchDaemon) so it can bind 53/80/443, but
// MySQL and PostgreSQL refuse to run as root. When we're root, run them as the
// invoking user (like PHP-FPM) and own their data dirs accordingly.

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

/// The real user to run DB servers as. Prefers `GROVE_RUN_USER` (set by the
/// service installer), else `SUDO_USER`.
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

/// `(uid, gid)` of the run user when the daemon is root and a run user exists;
/// otherwise `None` (run in-process, no privilege change).
fn drop_ids() -> Option<(u32, u32)> {
    if !running_as_root() {
        return None;
    }
    let user = target_user()?;
    let uid = id_of(&["-u", &user])?;
    let gid = id_of(&["-g", &user])?;
    Some((uid, gid))
}

fn id_of(args: &[&str]) -> Option<u32> {
    let out = std::process::Command::new("id").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Recursively chown a path so a dropped process can use a dir the root daemon
/// created (best-effort).
fn chown_recursive(path: &std::path::Path, uid: u32, gid: u32) {
    let _ = std::process::Command::new("chown")
        .arg("-R")
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .status();
}

/// Configure a command to drop to `(uid, gid)` before exec, if provided.
fn apply_drop(cmd: &mut std::process::Command, ids: Option<(u32, u32)>) {
    #[cfg(unix)]
    if let Some((uid, gid)) = ids {
        use std::os::unix::process::CommandExt;
        // SAFETY: only libc setgid/setuid are called; no allocation in the child.
        unsafe {
            cmd.pre_exec(move || {
                extern "C" {
                    fn setgid(gid: u32) -> i32;
                    fn setuid(uid: u32) -> i32;
                    fn setgroups(size: usize, list: *const u32) -> i32;
                }
                setgroups(1, &gid as *const u32);
                if setgid(gid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if setuid(uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, ids);
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

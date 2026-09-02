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
use grove_core::privdrop;
use grove_core::securefs;

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
            verify_download(spec, &url, &bytes, &progress)?;
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

        // 2. Dump them to a staging file.
        //
        // Deliberately Grove's own run dir rather than `/tmp`: the old path was
        // `/tmp/grove-mysql-migrate-<port>.sql`, a name anyone could predict and
        // pre-create as a symlink so a root daemon would write every database in
        // the source server wherever they pointed. Not world-writable beats
        // trying to make a `/tmp` name unguessable.
        progress(&format!("dumping {} database(s)…", dbs.len()));
        let staging = self.paths.run_dir();
        std::fs::create_dir_all(&staging)?;
        let dump_path = staging.join(format!("mysql-migrate-{port}.sql"));
        let dump_file = securefs::create_private(&dump_path)?;
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
            .args([
                "-h",
                "127.0.0.1",
                "-P",
                &port.to_string(),
                "-u",
                "root",
                "-e",
                &sql,
            ])
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
        // A dump is the whole database in plaintext: owner-only, and never
        // written through a symlink.
        let file = securefs::create_private(out)?;
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
        // `pg_dump -f` opens the file itself, so its flags are not ours to set.
        // Creating it first does the two things that matter: the path is now a
        // regular file, so pg_dump's `O_CREAT` cannot be redirected through a
        // symlink, and `O_CREAT` on an existing file ignores its mode argument,
        // so the dump keeps the 0600 set here. A racing unlink between the two
        // opens is still possible in a directory an attacker can write; the
        // durable answer to that is the directory, not the flags.
        drop(securefs::create_private(out)?);
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
        let run_as = privdrop::target();
        privdrop::own_tree(&data, run_as);
        let mut cmd = std::process::Command::new(bin.join("mysqld"));
        cmd.arg("--initialize-insecure")
            .arg(format!("--datadir={}", data.display()))
            .arg(format!("--basedir={}", base.display()));
        privdrop::apply(&mut cmd, run_as);
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
        // `make` runs whatever the Makefile in this tree says, and the tree came
        // out of a download into `$GROVE_HOME`. Running it as root turned a
        // tampered archive into root code execution; drop first, and give the
        // tree to the user so the build can still write its objects.
        let run_as = privdrop::target();
        privdrop::own_tree(&src, run_as);
        let mut make = std::process::Command::new("make");
        make.current_dir(&src)
            .args(["-j4", "MALLOC=libc", "BUILD_TLS=no"]);
        privdrop::apply(&mut make, run_as);
        let out = make.output().map_err(|e| {
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
        let run_as = privdrop::target();
        privdrop::own_tree(&data, run_as);
        let mut cmd = std::process::Command::new(bin.join("initdb"));
        cmd.arg("-D")
            .arg(&data)
            .args(["-U", "grove", "--auth=trust", "--encoding=UTF8"]);
        privdrop::apply(&mut cmd, run_as);
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
        // Not secret, but a symlink here would let a root daemon append service
        // output into an arbitrary file.
        let logf = securefs::create_public(&log)?;
        let port = self.effective_port(spec);

        // Every service drops to the invoking user when the daemon is root, and
        // its data dir goes with it. Redis used to be exempted — "happy as
        // root" — but `redis-server` is fetched into `$GROVE_HOME`, which the
        // user owns, so a root Redis meant replacing that binary was a root
        // shell. Being happy as root is not a reason to be root.
        let run_as = privdrop::target();
        privdrop::own_tree(&self.data_dir(spec), run_as);

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
                privdrop::apply(&mut cmd, run_as);
                cmd.spawn()?
            }
            ServiceKind::Redis => {
                let data = self.data_dir(spec);
                std::fs::create_dir_all(&data)?;
                privdrop::own_tree(&data, run_as);
                let mut cmd = std::process::Command::new(bin.join("redis-server"));
                cmd.args(["--port", &port.to_string()])
                    .arg("--dir")
                    .arg(&data)
                    .args(["--daemonize", "no", "--save", ""])
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
                privdrop::apply(&mut cmd, run_as);
                cmd.spawn()?
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
                privdrop::apply(&mut cmd, run_as);
                cmd.spawn()?
            }
        };
        tracing::info!(service = key, port, "started service");
        // Recorded so a daemon that comes back after SIGKILL can find and stop
        // this process instead of spawning a second one on the same port.
        if let Some(spec) = catalog::spec(key) {
            let _ = securefs::write_public(&self.pid_file(spec), child.id().to_string());
        }
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
        if let Some(spec) = catalog::spec(key) {
            let _ = std::fs::remove_file(self.pid_file(spec));
        }
        self.set_autostart(key, false);
        Ok(())
    }

    /// Stop every running process without touching the auto-start flags, so
    /// they come back on the next boot. For daemon shutdown: before this the
    /// databases were left to the runtime's `Drop`s, which a SIGKILL skips.
    pub fn stop_all_processes(&self) {
        let mut procs = self.procs.lock().unwrap();
        for (key, child) in procs.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(spec) = catalog::spec(key) {
                let _ = std::fs::remove_file(self.pid_file(spec));
            }
            tracing::info!(service = %key, "stopped service for shutdown");
        }
        procs.clear();
    }

    /// Terminate database processes left over from a previous daemon that did
    /// not shut down. Each start records `services/<key>/service.pid`; a pid
    /// there that is alive and runs the expected binary is stopped. Run before
    /// [`autostart_installed`], which would otherwise spawn a second postgres
    /// that dies on the port or the data directory lock — and `is_running`
    /// would then say "not running" while the orphan kept serving.
    pub fn reap_orphans(&self) -> usize {
        let mut reaped = 0;
        for spec in catalog::CATALOG {
            let file = self.pid_file(spec);
            if let Some(pid) = grove_core::process::read_pid_file(&file) {
                let expected = process_name_for(spec.kind);
                if pid != std::process::id()
                    && grove_core::process::is_alive_and_named(pid, expected)
                {
                    tracing::warn!(
                        service = spec.key,
                        pid,
                        "terminating orphaned service from a previous run"
                    );
                    if grove_core::process::terminate(pid, std::time::Duration::from_secs(5)) {
                        reaped += 1;
                    }
                }
            }
            let _ = std::fs::remove_file(&file);
        }
        reaped
    }

    fn pid_file(&self, spec: &ServiceSpec) -> PathBuf {
        self.service_root(spec).join("service.pid")
    }
}

/// The executable name each service kind runs as — what a pid read from disk
/// has to match before it is signalled.
fn process_name_for(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres",
        ServiceKind::Mysql => "mysqld",
        ServiceKind::Redis => "redis-server",
    }
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        let mut procs = self.procs.lock().unwrap();
        for child in procs.values_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---- privilege dropping ---------------------------------------------------
// The daemon may run as root (macOS LaunchDaemon) so it can bind 53/80/443, but
// MySQL and PostgreSQL refuse to run as root. When we're root, run them as the
// invoking user (like PHP-FPM) and own their data dirs accordingly.

// ---- persisted autostart state ------------------------------------------

fn state_file(paths: &GrovePaths) -> PathBuf {
    paths.services_dir().join("state.json")
}

fn load_state(paths: &GrovePaths) -> ServicesState {
    // A corrupt file is set aside and logged, not read as "nothing installed,
    // nothing auto-starts, every port back to default".
    securefs::read_json_or_quarantine(&state_file(paths))
}

fn save_state(paths: &GrovePaths, state: &ServicesState) {
    let _ = paths.ensure();
    if let Ok(body) = serde_json::to_string_pretty(state) {
        if let Err(e) = securefs::write_public_atomic(&state_file(paths), body) {
            tracing::error!(error = %e, "could not save services state");
        }
    }
}

// ---- download / extract helpers -----------------------------------------

/// As [`http_get`], but for a small text document such as a `.sha256`.
fn http_get_string(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Grove/0.1",
        )
        .call()
        .map_err(|e| ServiceError::Http(e.to_string()))?;
    resp.into_string().map_err(ServiceError::from)
}

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

/// Redis's published hashes, one line per release ever made.
const REDIS_HASHES: &str = "https://raw.githubusercontent.com/redis/redis-hashes/master/README";

/// The SHA-256 Redis publishes for `filename`.
///
/// The document is `hash <file> <algo> <hex> <url>` per line, and it still
/// carries `sha1` lines for releases from 2013 — so the algorithm has to be
/// matched rather than assumed from its position. Reading a SHA-1 as a SHA-256
/// would compare a 40-character digest against a 64-character one and always
/// fail; treating it as authoritative would be worse.
fn redis_sha256(document: &str, filename: &str) -> Option<String> {
    document.lines().find_map(|line| {
        let mut f = line.split_whitespace();
        match (f.next(), f.next(), f.next(), f.next()) {
            (Some("hash"), Some(name), Some("sha256"), Some(hex))
                if name == filename && hex.len() == 64 =>
            {
                Some(hex.to_ascii_lowercase())
            }
            _ => None,
        }
    })
}

/// Check a downloaded service archive against the publisher's own hash.
///
/// Only PostgreSQL's publisher offers one usable here — a `.sha256` beside each
/// asset. The other two are recorded rather than silently skipped:
///
/// - **MySQL** publishes `.md5` and a GPG `.asc`, no SHA-256. MD5 catches a
///   corrupt transfer but not a chosen collision, and verifying the signature
///   needs an OpenPGP implementation and MySQL's key. Left unverified rather
///   than verified in a way that reads stronger than it is.
/// - **Redis** is fetched as a GitHub git-archive tarball, whose bytes GitHub
///   does not promise to keep stable — pinning a hash for that URL would break
///   on their next compression change. The fix is to move to
///   `download.redis.io` plus the hashes in `redis/redis-hashes`, which is a
///   source change rather than a verification one.
fn verify_download(
    spec: &ServiceSpec,
    url: &str,
    bytes: &[u8],
    progress: &impl Fn(&str),
) -> Result<()> {
    let filename = url.rsplit('/').next().unwrap_or_default().to_string();
    let expected = match spec.kind {
        // A `.sha256` beside each asset.
        ServiceKind::Postgres => {
            progress("verifying checksum…");
            let doc = http_get_string(&format!("{url}.sha256"))?;
            grove_core::checksum::expected_for(&doc, &filename)
        }
        // Redis publishes hashes in a repository of its own, in its own format.
        ServiceKind::Redis => {
            progress("verifying checksum…");
            let doc = http_get_string(REDIS_HASHES)?;
            redis_sha256(&doc, &filename)
        }
        // MySQL offers `.md5` and a GPG `.asc`, no SHA-256. MD5 catches a
        // corrupt transfer but not a chosen collision, and checking the
        // signature needs an OpenPGP implementation and MySQL's key. Left
        // unverified rather than verified in a way that reads stronger than it
        // is.
        ServiceKind::Mysql => None,
    };
    match expected {
        Some(expected) => grove_core::checksum::verify(&filename, bytes, &expected)
            .map_err(|e| ServiceError::Http(e.to_string())),
        None => {
            progress(&format!(
                "note: no sha256 published for {filename}; not verified"
            ));
            Ok(())
        }
    }
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

#[cfg(test)]
mod verify_tests {
    use super::*;

    /// A slice of the real document, including the sha1 lines it still carries
    /// from 2013 and a sha256 line for a version we do not want.
    const DOC: &str = "\
hash redis-2.8.0-rc5.tar.gz sha1 bd27589b71a0b406b982485051f32b7c40c9d2c1 http://download.redis.io/releases/redis-2.8.0-rc5.tar.gz
hash redis-7.4.1.tar.gz sha256 0e439cbc19f6db5d4c63d2bc0aa8d43fdaf9ab164717c67f30f2b8873c8dcb50 http://download.redis.io/releases/redis-7.4.1.tar.gz
hash redis-7.4.2.tar.gz sha256 4ddebbf09061cbb589011786febdb34f29767dd7f89dbe712d2b68e808af6a1f http://download.redis.io/releases/redis-7.4.2.tar.gz
";

    #[test]
    fn the_right_release_line_is_found() {
        assert_eq!(
            redis_sha256(DOC, "redis-7.4.2.tar.gz").as_deref(),
            Some("4ddebbf09061cbb589011786febdb34f29767dd7f89dbe712d2b68e808af6a1f")
        );
        // A near neighbour must not be picked up.
        assert_eq!(
            redis_sha256(DOC, "redis-7.4.1.tar.gz").as_deref(),
            Some("0e439cbc19f6db5d4c63d2bc0aa8d43fdaf9ab164717c67f30f2b8873c8dcb50")
        );
    }

    /// The one that matters: an old sha1 line must not be read as a sha256.
    #[test]
    fn sha1_lines_are_refused() {
        assert_eq!(
            redis_sha256(DOC, "redis-2.8.0-rc5.tar.gz"),
            None,
            "a sha1 digest must not be accepted as a sha256"
        );
    }

    #[test]
    fn unknown_and_malformed_lines_yield_nothing() {
        assert_eq!(redis_sha256(DOC, "redis-9.9.9.tar.gz"), None);
        assert_eq!(redis_sha256("", "redis-7.4.2.tar.gz"), None);
        assert_eq!(redis_sha256("404 Not Found", "redis-7.4.2.tar.gz"), None);
        // Right shape, truncated digest.
        assert_eq!(
            redis_sha256(
                "hash redis-7.4.2.tar.gz sha256 abc123 url",
                "redis-7.4.2.tar.gz"
            ),
            None
        );
    }

    /// The catalog must point at the verifiable source, not GitHub's
    /// git-archive — whose bytes are not promised to be stable.
    #[test]
    fn redis_is_fetched_from_the_official_release_host() {
        let spec = catalog::spec("redis").expect("redis is in the catalog");
        let url = catalog::download_url(spec).expect("a download url");
        assert!(
            url.starts_with("https://download.redis.io/releases/redis-"),
            "got {url}"
        );
        assert!(
            !url.contains("archive/refs/tags"),
            "the git-archive tarball has no stable hash: {url}"
        );
        // And the unpacked root still matches what the build step expects.
        assert_eq!(
            catalog::archive_root(spec).as_deref(),
            Some(format!("redis-{}", spec.version).as_str())
        );
    }
}

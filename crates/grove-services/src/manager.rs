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
use grove_core::securefs;

use crate::catalog::{self, ServiceKind, ServiceSpec};

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("unknown service {0:?}")]
    Unknown(String),
    /// Not "unknown service": restoring an id that is not in the index used to
    /// say `unknown service "snapshot 2026…"`, which sent people looking at the
    /// wrong thing.
    #[error("no snapshot with id {0:?} — `grove db list` shows the ones there are")]
    NoSnapshot(String),
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

/// Load a `mysqldump` file so every database in it comes back exactly as it was.
///
/// A dump made with `--databases` or `--all-databases` recreates each table it
/// holds (`DROP TABLE IF EXISTS`, then `CREATE`), but says nothing about tables
/// that did not exist when it was taken. Restoring left those behind: roll back
/// a migration that created `invoices` and `invoices` was still there, which is
/// precisely the case the sandboxed-migration tool snapshots for. So each
/// `CREATE DATABASE` in the dump is preceded, on the way in, by a `DROP
/// DATABASE` of the same name — never for MySQL's own system schemas, and never
/// for a database the dump does not contain. Snapshots taken before this change
/// get the same treatment, because it happens at restore time.
///
/// Streamed rather than read into memory: a dump is as large as the data.
pub fn restore_mysql_dump(bin: &std::path::Path, port: u16, sql: &std::path::Path) -> Result<()> {
    use std::io::{BufRead, Write};
    let mut child = std::process::Command::new(bin.join("mysql"))
        .args(["-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut reader = std::io::BufReader::new(std::fs::File::open(sql)?);
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let mut line = Vec::new();
    let fed: std::io::Result<()> = (|| loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if let Some(name) = created_database(&line) {
            if !is_system_schema(&name) {
                writeln!(
                    stdin,
                    "DROP DATABASE IF EXISTS `{}`;",
                    name.replace('`', "``")
                )?;
            }
        }
        stdin.write_all(&line)?;
    })();
    drop(stdin);
    let out = child.wait_with_output()?;
    // `mysql` stops at the first error and closes its input, which shows up
    // here as a broken pipe; its own message on stderr is the one worth showing.
    if !out.status.success() {
        return Err(ServiceError::Init(format!(
            "restore into MySQL failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if let Err(e) = fed {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e.into());
        }
    }
    Ok(())
}

/// The database a `mysqldump` `CREATE DATABASE` line creates, unquoted.
///
/// mysqldump writes `CREATE DATABASE /*!32312 IF NOT EXISTS*/ `name` …;` — the
/// first backquoted identifier on the line, with any backquote inside it
/// doubled.
fn created_database(line: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix("CREATE DATABASE ")?;
    let open = rest.find('`')?;
    let mut name = String::new();
    let mut chars = rest[open + 1..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            if chars.peek() == Some(&'`') {
                chars.next();
                name.push('`');
            } else {
                return Some(name);
            }
        } else {
            name.push(c);
        }
    }
    None
}

/// Schemas a restore must never drop, whatever a dump says.
fn is_system_schema(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "mysql" | "sys" | "performance_schema" | "information_schema"
    )
}

/// How long a bundled server gets to start accepting connections. MySQL after
/// an unclean shutdown runs InnoDB recovery first, which is the slow case.
const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A port nothing is listening on, for an on-demand server's internal side.
fn free_port() -> Result<u16> {
    let probe = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(probe.local_addr()?.port())
}

/// Does anything accept a TCP connection on `127.0.0.1:port`?
fn port_accepts(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

/// The command and pid listening on `port`, if `lsof` can say.
fn port_holder(port: u16) -> Option<String> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text.lines().find_map(|l| l.strip_prefix('p'))?;
    let cmd = text.lines().find_map(|l| l.strip_prefix('c'))?;
    Some(format!("{cmd} (pid {pid})"))
}

/// What a server wrote to its log during this start, reduced to the lines that
/// explain a failure: its own error lines if it wrote any, else the last few.
fn log_excerpt(log: &std::path::Path, from: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // At most the last 16 KiB of this run's output.
    let start = from.max(len.saturating_sub(16 * 1024));
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| {
            l.contains("[ERROR]")
                || l.contains("FATAL")
                || l.contains("# Fatal")
                || l.starts_with("ERROR")
        })
        .collect();
    let chosen: Vec<&str> = if errors.is_empty() {
        lines.iter().rev().take(5).rev().copied().collect()
    } else {
        errors.into_iter().take(5).collect()
    };
    if chosen.is_empty() {
        String::new()
    } else {
        format!(":\n  {}", chosen.join("\n  "))
    }
}

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
    /// Runs only while something is connected; `running` then says whether a
    /// server is up right now.
    #[serde(default)]
    pub on_demand: bool,
    /// How long an on-demand server stays up with nothing connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_secs: Option<u64>,
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
    /// service key -> idle seconds, for services that run only while something
    /// is connected. See `grove-daemon`'s `ondemand` for the front that holds
    /// the port and starts the server behind it.
    #[serde(default)]
    on_demand: BTreeMap<String, u64>,
}

/// Supervises bundled services. Child handles live for the daemon's lifetime.
pub struct ServiceManager {
    paths: GrovePaths,
    procs: Mutex<HashMap<String, Child>>,
    state: Mutex<ServicesState>,
    /// For a service started on demand: the internal port its server actually
    /// listens on. The public port belongs to the daemon's front.
    upstream: Mutex<HashMap<String, u16>>,
    /// One start or stop at a time. A connection arriving through the on-demand
    /// front and a snapshot calling `db_ready` can both find a server stopped
    /// in the same instant; two spawns on one data directory is one server
    /// that works and one that corrupts nothing only by luck.
    lifecycle: Mutex<()>,
}

impl ServiceManager {
    pub fn new(paths: GrovePaths) -> Self {
        let state = load_state(&paths);
        Self {
            paths,
            procs: Mutex::new(HashMap::new()),
            state: Mutex::new(state),
            upstream: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(()),
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
            if self.is_installed(spec)
                && self.wants_autostart(spec.key)
                && !self.is_on_demand(spec.key)
            {
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
            // One binary, at the top of the archive.
            ServiceKind::ElyraSql => root,
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
            ServiceKind::ElyraSql => "elyrasql",
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
                    on_demand: self.is_on_demand(spec.key),
                    idle_secs: self.on_demand_idle(spec.key).map(|d| d.as_secs()),
                }
            })
            .collect()
    }

    /// Does `key` run only while something is connected?
    pub fn is_on_demand(&self, key: &str) -> bool {
        self.state.lock().unwrap().on_demand.contains_key(key)
    }

    /// How long an on-demand server may sit with nothing connected.
    pub fn on_demand_idle(&self, key: &str) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap()
            .on_demand
            .get(key)
            .map(|s| std::time::Duration::from_secs(*s))
    }

    /// Switch `key` between always-on (`None`) and on demand with an idle
    /// timeout. Only the recorded mode changes here; the daemon moves the
    /// listener and the server to match.
    pub fn set_on_demand(&self, key: &str, idle: Option<std::time::Duration>) -> Result<()> {
        let _ = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.to_string()))?;
        let mut state = self.state.lock().unwrap();
        match idle {
            Some(d) => {
                state.on_demand.insert(key.to_string(), d.as_secs().max(1));
            }
            None => {
                state.on_demand.remove(key);
            }
        }
        save_state(&self.paths, &state);
        Ok(())
    }

    /// The internal port an on-demand server listens on, while it runs.
    pub fn upstream_port(&self, key: &str) -> Option<u16> {
        if !self.is_running(key) {
            return None;
        }
        self.upstream.lock().unwrap().get(key).copied()
    }

    /// Stop a server cleanly, without changing whether it should run.
    ///
    /// `SIGTERM` first and up to fifteen seconds to act on it: MySQL flushes
    /// InnoDB and exits clean, where `SIGKILL` leaves crash recovery for the
    /// next start. An on-demand server is stopped every time it goes idle, so
    /// the difference is paid over and over.
    pub fn suspend(&self, key: &str) -> Result<()> {
        let _guard = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let child = self.procs.lock().unwrap().remove(key);
        if let Some(mut child) = child {
            let clean = grove_core::process::terminate_child(
                &mut child,
                std::time::Duration::from_secs(15),
            );
            tracing::info!(service = key, clean, "stopped service");
        }
        if let Some(spec) = catalog::spec(key) {
            let _ = std::fs::remove_file(self.pid_file(spec));
        }
        self.upstream.lock().unwrap().remove(key);
        Ok(())
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
                // On demand the server's socket is named for its internal
                // port; clients connect over TCP to the front.
                (!self.is_on_demand(spec.key))
                    .then(|| self.data_dir(spec).to_string_lossy().into_owned()),
                format!("postgresql://grove@127.0.0.1:{port}/postgres"),
            ),
            ServiceKind::Mysql => (
                Some("root".into()),
                (!self.is_on_demand(spec.key)).then(|| {
                    self.data_dir(spec)
                        .join("mysql.sock")
                        .to_string_lossy()
                        .into_owned()
                }),
                format!("mysql://root@127.0.0.1:{port}"),
            ),
            ServiceKind::Redis => (None, None, format!("redis://127.0.0.1:{port}")),
            // Open auth on loopback: any username is accepted, so "root" keeps
            // Laravel's defaults working. TCP only. One logical database, `elyra`.
            ServiceKind::ElyraSql => (
                Some("root".into()),
                None,
                format!("mysql://root@127.0.0.1:{port}/{ELYRASQL_DATABASE}"),
            ),
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
            // Nothing to initialise: `serve` creates the `.edb` on first start.
            ServiceKind::Redis | ServiceKind::ElyraSql => {}
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
    /// The TCP port of a bundled database, started first if it is not running.
    ///
    /// For callers that speak the wire protocol themselves rather than through
    /// a client binary — `branches`, which needs a real connection to issue
    /// one atomic `RENAME TABLE` and read its result.
    pub fn ready_port(&self, key: &str) -> Result<u16> {
        self.db_ready(key).map(|(_, port)| port)
    }

    /// The port a bundled service listens on, whether or not it is running.
    pub fn port_of(&self, key: &str) -> Option<u16> {
        catalog::spec(key).map(|spec| self.effective_port(spec))
    }

    fn db_ready(&self, key: &str) -> Result<(PathBuf, u16)> {
        let spec = catalog::spec(key).ok_or_else(|| ServiceError::Unknown(key.into()))?;
        if !self.is_installed(spec) {
            return Err(ServiceError::NotInstalled(format!(
                "{key} (add it under Services first)"
            )));
        }
        if !self.is_running(key) {
            // `start` returns once the server accepts connections, so there is
            // no fixed wait to guess at here any more.
            self.start(key)?;
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
        restore_mysql_dump(&bin, port, sql)
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

    /// The single database file ElyraSQL serves.
    fn elyrasql_file(&self, spec: &ServiceSpec) -> PathBuf {
        self.data_dir(spec).join("grove.edb")
    }

    /// Snapshot ElyraSQL to `out` as a complete `.edb` file, while it serves.
    ///
    /// Not a SQL dump: ElyraSQL's own `BACKUP TO` copies the whole database from
    /// an MVCC snapshot without blocking writers, and the result is itself a
    /// normal database file. Two constraints shape the dance below. The server
    /// writes the file, and it runs as the dropped user — so the target has to
    /// be somewhere *it* can write, which is its own data directory, not
    /// `snapshots/`. And it refuses to overwrite, so the path must be fresh.
    /// Grove then moves the finished file into place with the modes it wants.
    pub fn snapshot_elyrasql(&self, out: &std::path::Path) -> Result<()> {
        let spec = catalog::spec("elyrasql").expect("elyrasql is in the catalog");
        let (_bin, port) = self.db_ready("elyrasql")?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let staging = self.data_dir(spec).join(format!(".snapshot-{nanos}.edb"));
        let _ = std::fs::remove_file(&staging);

        // `BACKUP TO` takes a string literal; the path is ours, but quote it anyway.
        let literal = staging.to_string_lossy().replace('\'', "''");
        // Explicit options rather than a URL. On connect, sqlx's MySQL driver
        // runs `SET sql_mode=(SELECT CONCAT(@@sql_mode, '…')), time_zone='+00:00'`.
        // ElyraSQL 1.11.1 rejected both halves (error 1235); 1.11.2 accepts
        // them. They stay off here regardless: copying a file needs neither, and
        // a 1.11.1 installed before the bump keeps working. `SET NAMES` stays.
        let options = sqlx::mysql::MySqlConnectOptions::new()
            .host("127.0.0.1")
            .port(port)
            .username("root")
            .database(ELYRASQL_DATABASE)
            .pipes_as_concat(false)
            .no_engine_substitution(false)
            .timezone(None);
        let result = block_on(async move {
            use sqlx::Connection;
            let mut conn = sqlx::MySqlConnection::connect_with(&options).await?;
            // The path is Grove's own (data dir + timestamp) and quote-escaped
            // above; the assertion says so to sqlx, which rightly refuses to
            // take a formatted string on trust.
            sqlx::query(sqlx::AssertSqlSafe(format!("BACKUP TO '{literal}'")))
                .execute(&mut conn)
                .await?;
            conn.close().await
        });
        if let Err(e) = result {
            let _ = std::fs::remove_file(&staging);
            return Err(ServiceError::Init(format!(
                "ElyraSQL BACKUP TO failed: {e}"
            )));
        }

        // Into snapshots/ as an owner-only regular file, never through a symlink.
        let mut src = std::fs::File::open(&staging)?;
        let mut dst = securefs::create_private(out)?;
        let copied = std::io::copy(&mut src, &mut dst);
        let _ = std::fs::remove_file(&staging);
        match copied {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(out);
                Err(e.into())
            }
        }
    }

    /// Restore a `.edb` snapshot into ElyraSQL.
    ///
    /// There is no hot restore — the engine holds an exclusive lock on the open
    /// file — so this stops the server, lets `elyrasql restore` validate the
    /// backup and copy it over the live file, and starts the server again.
    pub fn restore_elyrasql(&self, edb: &std::path::Path) -> Result<()> {
        let spec = catalog::spec("elyrasql").expect("elyrasql is in the catalog");
        if !self.is_installed(spec) {
            return Err(ServiceError::NotInstalled(
                "elyrasql (add it under Services first)".into(),
            ));
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        let was_running = self.is_running("elyrasql");
        if was_running {
            // Stop without clearing autostart: `stop()` treats a stop as the
            // user's decision to keep it down, and this one is not.
            if let Some(mut child) = self.procs.lock().unwrap().remove("elyrasql") {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = std::fs::remove_file(self.pid_file(spec));
        }
        let target = self.elyrasql_file(spec);
        let mut cmd = std::process::Command::new(bin.join("elyrasql"));
        cmd.arg("restore")
            .arg("--input")
            .arg(edb)
            .arg("--data")
            .arg(&target)
            .arg("--force");
        // The live file must stay the server's, and the server runs dropped.
        let out = cmd.output()?;
        let restored = out.status.success();
        let start_result = self.start("elyrasql");
        if !restored {
            return Err(ServiceError::Init(format!(
                "restore into ElyraSQL failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        start_result
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
        let mut cmd = std::process::Command::new(bin.join("mysqld"));
        cmd.arg("--initialize-insecure")
            .arg(format!("--datadir={}", data.display()))
            .arg(format!("--basedir={}", base.display()));
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
        // `make` runs whatever the Makefile in this tree says, and the tree
        // came out of a download into `$GROVE_HOME`. That was root code
        // execution from a tampered archive until the daemon stopped being
        // root; it is now the same user who could edit the tree anyway.
        let mut make = std::process::Command::new("make");
        make.current_dir(&src)
            .args(["-j4", "MALLOC=libc", "BUILD_TLS=no"]);
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
        let mut cmd = std::process::Command::new(bin.join("initdb"));
        cmd.arg("-D")
            .arg(&data)
            .args(["-U", "grove", "--auth=trust", "--encoding=UTF8"]);
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
        let _guard = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_running(key) {
            return Ok(());
        }
        let bin = self
            .bin_dir(spec)
            .ok_or_else(|| ServiceError::Unsupported(spec.name.into()))?;
        let log = self.paths.logs_dir().join(format!("{key}.log"));
        let on_demand = self.is_on_demand(key);
        // Not secret, but a symlink here would let a root daemon append service
        // output into an arbitrary file.
        let logf = securefs::create_public(&log)?;
        // On demand, the public port belongs to the daemon's front, and the
        // server listens on a free internal one that only the front dials.
        let port = if on_demand {
            free_port()?
        } else {
            self.effective_port(spec)
        };

        // Something already answering on the port means this server will fail
        // to bind — and worse, the readiness check below would connect to the
        // *other* server and report this one as up. Say so before starting.
        if port_accepts(port) {
            return Err(ServiceError::Init(format!(
                "port {port} is already in use{} — stop that server, or give {} another port with \
                 `grove service port {key} <port>`",
                port_holder(port)
                    .map(|h| format!(" by {h}"))
                    .unwrap_or_default(),
                spec.name
            )));
        }
        // Only this run's lines are worth quoting if it falls over; the log
        // accumulates every start there has ever been.
        let log_start = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);

        let mut child = match spec.kind {
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
                cmd.spawn()?
            }
            ServiceKind::Redis => {
                let data = self.data_dir(spec);
                std::fs::create_dir_all(&data)?;
                let mut cmd = std::process::Command::new(bin.join("redis-server"));
                cmd.args(["--port", &port.to_string()])
                    .arg("--dir")
                    .arg(&data)
                    .args(["--daemonize", "no", "--save", ""])
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
                cmd.spawn()?
            }
            ServiceKind::ElyraSql => {
                let data = self.data_dir(spec);
                std::fs::create_dir_all(&data)?;
                let mut cmd = std::process::Command::new(bin.join("elyrasql"));
                // No accounts: ElyraSQL's "open auth" makes every client Admin,
                // which it permits on a loopback bind and refuses elsewhere. That
                // is the same posture as Grove's MySQL (`--initialize-insecure`)
                // and Postgres (trust auth) — local development, on 127.0.0.1.
                cmd.arg("serve")
                    .arg("--data")
                    .arg(self.elyrasql_file(spec))
                    .arg("--listen")
                    .arg(format!("127.0.0.1:{port}"))
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
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
                        self.data_dir(spec)
                            .join(if on_demand {
                                // Not the advertised path: a client on the
                                // socket would bypass the front, go uncounted,
                                // and be cut off when the server went idle.
                                ".grove-on-demand.sock"
                            } else {
                                "mysql.sock"
                            })
                            .display()
                    ))
                    .arg("--mysqlx=OFF")
                    .stdout(logf.try_clone()?)
                    .stderr(logf);
                cmd.spawn()?
            }
        };
        // A spawn that succeeded is a process that exists, not a server that
        // runs. mysqld with no data directory, Postgres with a stale lock file,
        // Redis refusing its config — each exits within a second, and the old
        // code had already said "started". Wait until the port answers, or the
        // process dies and its own log says why.
        let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                return Err(ServiceError::Init(format!(
                    "{} exited while starting ({status}){}",
                    spec.name,
                    log_excerpt(&log, log_start)
                )));
            }
            if port_accepts(port) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                tracing::warn!(
                    service = key,
                    port,
                    "still not accepting connections after {}s; leaving it running",
                    STARTUP_TIMEOUT.as_secs()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        tracing::info!(service = key, port, "started service");
        // Recorded so a daemon that comes back after SIGKILL can find and stop
        // this process instead of spawning a second one on the same port.
        if let Some(spec) = catalog::spec(key) {
            let _ = securefs::write_public(&self.pid_file(spec), child.id().to_string());
        }
        self.procs.lock().unwrap().insert(key.to_string(), child);
        if on_demand {
            self.upstream.lock().unwrap().insert(key.to_string(), port);
        } else {
            self.set_autostart(key, true);
        }
        Ok(())
    }

    /// Stop a running service. Clears its auto-start flag so it stays stopped
    /// across daemon restarts until the user starts it again.
    pub fn stop(&self, key: &str) -> Result<()> {
        if self.is_on_demand(key) {
            // The next connection will start it again; turning that off is
            // `grove service on-demand <key> off`.
            return self.suspend(key);
        }
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
        self.upstream.lock().unwrap().clear();
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
/// ElyraSQL's one logical database. `CREATE DATABASE` is a no-op there only
/// with `IF NOT EXISTS`, so `.env` snippets and connection URIs name this.
pub const ELYRASQL_DATABASE: &str = "elyra";

/// Run a future to completion from a blocking context. The manager's methods
/// are synchronous and called from `spawn_blocking`, where a fresh
/// current-thread runtime is the safe way to drive an async client.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(fut)
}

fn process_name_for(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres",
        ServiceKind::Mysql => "mysqld",
        ServiceKind::Redis => "redis-server",
        ServiceKind::ElyraSql => "elyrasql",
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
        // A `.sha256` beside each asset (sha256sum format).
        ServiceKind::Postgres | ServiceKind::ElyraSql => {
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

#[cfg(test)]
mod honesty_tests {
    use super::*;

    #[test]
    fn a_dump_create_database_line_names_its_database() {
        assert_eq!(
            created_database(
                b"CREATE DATABASE /*!32312 IF NOT EXISTS*/ `shop` /*!40100 DEFAULT CHARACTER SET utf8mb4 */;\n"
            ),
            Some("shop".into())
        );
        assert_eq!(
            created_database(b"CREATE DATABASE /*!32312 IF NOT EXISTS*/ `we``ird`;\n"),
            Some("we`ird".into())
        );
        assert_eq!(created_database(b"USE `shop`;\n"), None);
        assert_eq!(created_database(b"-- CREATE DATABASE `shop`\n"), None);
        assert_eq!(
            created_database(b"INSERT INTO t VALUES ('CREATE DATABASE `x`');\n"),
            None
        );
    }

    /// An `--all-databases` dump contains `CREATE DATABASE `mysql``. Dropping
    /// that on restore would take the server's accounts with it.
    #[test]
    fn system_schemas_are_never_dropped() {
        for name in [
            "mysql",
            "sys",
            "performance_schema",
            "information_schema",
            "MySQL",
        ] {
            assert!(is_system_schema(name), "{name}");
        }
        assert!(!is_system_schema("shop"));
    }

    /// When a server falls over at start, its own error lines are the message
    /// — not the lines from every earlier start, and not the noise around them.
    #[test]
    fn the_log_excerpt_is_this_runs_errors() {
        let log = std::env::temp_dir().join(format!("grove-logx-{}.log", std::process::id()));
        std::fs::write(&log, "old run [ERROR] something from last week\n").unwrap();
        let from = std::fs::metadata(&log).unwrap().len();
        std::fs::write(
            &log,
            "old run [ERROR] something from last week\n\
             [System] starting\n\
             [ERROR] [MY-013276] Failed to set datadir (OS errno: 2 - No such file or directory)\n\
             [ERROR] [MY-010119] Aborting\n\
             [System] Shutdown complete\n",
        )
        .unwrap();
        let excerpt = log_excerpt(&log, from);
        assert!(excerpt.contains("Failed to set datadir"), "{excerpt}");
        assert!(excerpt.contains("Aborting"), "{excerpt}");
        assert!(!excerpt.contains("last week"), "{excerpt}");
        assert!(!excerpt.contains("starting"), "{excerpt}");

        // No error lines at all: the tail is the next best thing.
        std::fs::write(&log, "a\nb\nc\n").unwrap();
        assert_eq!(log_excerpt(&log, 0), ":\n  a\n  b\n  c");
        let _ = std::fs::remove_file(&log);
    }

    /// The negative half uses port 1, not the port just released: tests run in
    /// parallel, and another one binding `127.0.0.1:0` can be handed the freed
    /// port in the same instant — which failed this once in a full run. Port 1
    /// needs root to bind, so nothing unprivileged is ever listening there.
    #[test]
    fn a_listening_port_is_seen_as_taken() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_accepts(port));
        drop(listener);
        assert!(!port_accepts(1));
    }

    /// Against a real MySQL (`GROVE_TEST_MYSQL_PORT`, and the client binaries
    /// in `GROVE_TEST_MYSQL_BIN`): a table created after the snapshot is gone
    /// after the restore, changed rows are back, and a database the dump does
    /// not contain is left alone.
    #[test]
    fn a_restore_puts_the_database_back_exactly() {
        let (Some(port), Some(bin)) = (
            std::env::var("GROVE_TEST_MYSQL_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok()),
            std::env::var("GROVE_TEST_MYSQL_BIN")
                .ok()
                .map(PathBuf::from),
        ) else {
            eprintln!("skipped: set GROVE_TEST_MYSQL_PORT and GROVE_TEST_MYSQL_BIN");
            return;
        };
        let db = format!("grove_rt_{}", std::process::id());
        let other = format!("grove_rt_other_{}", std::process::id());
        let sql = |q: &str| {
            let out = std::process::Command::new(bin.join("mysql"))
                .args([
                    "-h",
                    "127.0.0.1",
                    "-P",
                    &port.to_string(),
                    "-u",
                    "root",
                    "-N",
                    "-e",
                    q,
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{q}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        sql(&format!(
            "DROP DATABASE IF EXISTS {db}; DROP DATABASE IF EXISTS {other};
             CREATE DATABASE {db}; CREATE TABLE {db}.users (id INT PRIMARY KEY, name VARCHAR(20));
             INSERT INTO {db}.users VALUES (1,'ada'),(2,'bob');
             CREATE DATABASE {other}; CREATE TABLE {other}.keep (id INT);"
        ));
        let dump = std::env::temp_dir().join(format!("grove-rt-{}.sql", std::process::id()));
        let status = std::process::Command::new(bin.join("mysqldump"))
            .args(["-h", "127.0.0.1", "-P", &port.to_string(), "-u", "root"])
            .args([
                "--single-transaction",
                "--no-tablespaces",
                "--column-statistics=0",
                "--databases",
                &db,
            ])
            .stdout(std::fs::File::create(&dump).unwrap())
            .status()
            .unwrap();
        assert!(status.success());

        // What a migration does after the snapshot.
        sql(&format!(
            "CREATE TABLE {db}.invoices (id INT); UPDATE {db}.users SET name='changed'; \
             DELETE FROM {db}.users WHERE id=2;"
        ));

        restore_mysql_dump(&bin, port, &dump).unwrap();
        assert_eq!(
            sql(&format!(
                "SELECT GROUP_CONCAT(table_name) FROM information_schema.tables WHERE table_schema='{db}'"
            )),
            "users",
            "the table created after the snapshot must be gone"
        );
        assert_eq!(
            sql(&format!(
                "SELECT GROUP_CONCAT(name ORDER BY id) FROM {db}.users"
            )),
            "ada,bob"
        );
        assert_eq!(
            sql(&format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='{other}'"
            )),
            "1",
            "a database the dump does not contain is not touched"
        );

        // A dump that fails part-way reports failure, not success.
        std::fs::write(
            &dump,
            format!("CREATE DATABASE `{db}`;\nTHIS IS NOT SQL;\n"),
        )
        .unwrap();
        assert!(restore_mysql_dump(&bin, port, &dump).is_err());

        sql(&format!(
            "DROP DATABASE IF EXISTS {db}; DROP DATABASE IF EXISTS {other};"
        ));
        let _ = std::fs::remove_file(&dump);
    }
}

#[cfg(test)]
mod on_demand_tests {
    use super::*;

    fn scratch(name: &str) -> ServiceManager {
        let base = std::env::temp_dir().join(format!("grove-od-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = GrovePaths::with_base(&base);
        paths.ensure().unwrap();
        ServiceManager::new(paths)
    }

    /// The mode is persisted, survives a new manager reading the same home,
    /// and switching it off forgets the idle period.
    #[test]
    fn on_demand_is_remembered_across_restarts() {
        let sm = scratch("persist");
        assert!(!sm.is_on_demand("mysql"));
        sm.set_on_demand("mysql", Some(std::time::Duration::from_secs(600)))
            .unwrap();
        let again = ServiceManager::new(sm.paths.clone());
        assert!(again.is_on_demand("mysql"));
        assert_eq!(
            again.on_demand_idle("mysql"),
            Some(std::time::Duration::from_secs(600))
        );
        again.set_on_demand("mysql", None).unwrap();
        assert!(!ServiceManager::new(sm.paths.clone()).is_on_demand("mysql"));
        assert!(sm.set_on_demand("nope", None).is_err());
    }

    /// On demand, the server's socket is not the advertised one — a client on
    /// it would bypass the front and go uncounted — so nothing advertises one.
    /// The TCP address stays exactly what it was.
    #[test]
    fn an_on_demand_service_advertises_tcp_only() {
        let sm = scratch("socket");
        let before = sm
            .status_all()
            .into_iter()
            .find(|s| s.key == "mysql")
            .unwrap();
        assert!(before.socket.is_some());
        assert!(!before.on_demand);
        sm.set_on_demand("mysql", Some(std::time::Duration::from_secs(60)))
            .unwrap();
        let after = sm
            .status_all()
            .into_iter()
            .find(|s| s.key == "mysql")
            .unwrap();
        assert_eq!(after.socket, None);
        assert!(after.on_demand);
        assert_eq!(after.idle_secs, Some(60));
        assert_eq!(after.port, before.port, "clients keep the port they had");
        assert_eq!(after.uri, before.uri);
    }

    /// A stopped on-demand server has no upstream to dial, which is how the
    /// front knows to start it.
    #[test]
    fn a_stopped_server_has_no_upstream() {
        let sm = scratch("upstream");
        sm.set_on_demand("redis", Some(std::time::Duration::from_secs(60)))
            .unwrap();
        assert_eq!(sm.upstream_port("redis"), None);
        // Suspending something that is not running is a no-op, not an error.
        sm.suspend("redis").unwrap();
    }
}

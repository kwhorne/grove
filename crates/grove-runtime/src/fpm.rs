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
use grove_core::privdrop;
use grove_proxy::fastcgi::FpmAddr;
use grove_proxy::FpmLocator;

use crate::registry::PhpRegistry;
use crate::xdebug::{self, DEFAULT_XDEBUG_PORT};

/// Runtime Xdebug configuration for freshly spawned FPM pools.
#[derive(Debug, Clone, Copy)]
struct XdebugState {
    enabled: bool,
    port: u16,
}

impl Default for XdebugState {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_XDEBUG_PORT,
        }
    }
}

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
    xdebug: Mutex<XdebugState>,
}

impl FpmManager {
    pub fn new(paths: GrovePaths, registry: PhpRegistry) -> Self {
        Self {
            paths,
            registry: Mutex::new(registry),
            pools: Mutex::new(HashMap::new()),
            xdebug: Mutex::new(XdebugState::default()),
        }
    }

    /// Enable/disable Xdebug for pools spawned from now on, and set the DBGp
    /// port the extension connects out to. Call [`FpmManager::reload_pools`]
    /// afterwards to apply it to already-running pools.
    pub fn set_xdebug(&self, enabled: bool, port: u16) {
        let mut x = self.xdebug.lock().unwrap();
        x.enabled = enabled;
        x.port = if port == 0 { DEFAULT_XDEBUG_PORT } else { port };
    }

    /// Whether Xdebug is currently enabled for new pools.
    pub fn xdebug_enabled(&self) -> bool {
        self.xdebug.lock().unwrap().enabled
    }

    /// Drop all live pools so the next request respawns them with the current
    /// Xdebug setting. Dropping a [`FpmPool`] kills its FPM process.
    pub fn reload_pools(&self) {
        self.pools.lock().unwrap().clear();
    }

    /// Stop every pool now — the daemon is shutting down. Same mechanism as
    /// [`reload_pools`], named for the intent: before this, pools were left to
    /// die with the runtime's `Drop`s during teardown, which a SIGKILL skips.
    pub fn stop_all(&self) {
        self.reload_pools();
    }

    /// Terminate php-fpm masters left over from a previous daemon that did not
    /// get to shut down. Each pool writes `run/fpm/php-fpm-<v>.pid`; a pid there
    /// that is alive and *is* a php-fpm is killed and its pid file removed. The
    /// socket path is left for `ensure_pool` to unlink as it always has.
    ///
    /// Without this, a daemon restarted after SIGKILL unlinked the orphan's
    /// socket and spawned a second master; orphans accumulated one per restart.
    pub fn reap_orphans(&self) -> usize {
        let reaped = grove_core::process::reap_pid_files(
            &self.fpm_run_dir(),
            "php-fpm-",
            "php-fpm",
            std::time::Duration::from_secs(3),
        );
        if !reaped.is_empty() {
            tracing::warn!(count = reaped.len(), pids = ?reaped, "reaped orphaned php-fpm masters");
        }
        reaped.len()
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

    /// Where FPM's own runtime files live.
    ///
    /// A subdirectory of `run/` rather than `run/` itself: when the pool master
    /// drops to the user it has to create its socket and pid file here, and
    /// handing over all of `run/` would also hand over `groved.pid` and the IPC
    /// socket. A user who can write here can point their own site's FastCGI
    /// socket somewhere else, which is their traffic to misdirect; the daemon's
    /// own state stays out of reach.
    fn fpm_run_dir(&self) -> PathBuf {
        self.paths.run_dir().join("fpm")
    }

    fn socket_path(&self, version: &str) -> PathBuf {
        self.fpm_run_dir()
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

        // A dead pool is still in the map, and it is about to be replaced. Take
        // it out *now*, before the new master binds. `FpmPool`'s Drop removes
        // its socket file, and the new pool binds the very same path — so if
        // the old pool were left for `pools.insert` to hand back and drop, it
        // would delete the socket the new master had just created. The new
        // child is alive, so nothing would ever respawn, and every request for
        // this PHP version would 502 until the daemon restarted. The first
        // crash was recovered from; the recovery itself was the outage.
        drop(pools.remove(version));

        let build = self
            .build_for(version)
            .ok_or_else(|| FpmError::UnknownVersion(version.to_string()))?;

        self.paths.ensure()?;
        // Who the pool will run as, decided once so the config, the paths and
        // the spawn cannot disagree.
        let run_as = privdrop::target();

        let socket = self.socket_path(version);
        std::fs::create_dir_all(self.fpm_run_dir())?;
        let _ = std::fs::remove_file(&socket);
        let log = self.paths.logs_dir().join(format!("php-fpm-{version}.log"));
        // A dropped master still has to write its socket, pid file and error
        // log, all of which live in directories the root daemon created. Hand it
        // the pieces it owns; without this the drop turns into a pool that
        // cannot start, which looks like a bug rather than a permission.
        privdrop::own_path(&self.fpm_run_dir(), run_as);
        if let Ok(f) = grove_core::securefs::create_public(&log) {
            drop(f);
            privdrop::own_path(&log, run_as);
        }
        let conf = self.write_pool_config(version, &socket, &log, run_as)?;

        tracing::info!(version, binary = %build.fpm_binary.display(), "spawning PHP-FPM pool");
        // When debug mode is on, load Xdebug via `-d` INI overrides (Zend
        // extensions must be set at startup, so pool-config `php_admin_value`
        // won't do). Trigger mode keeps Xdebug dormant until a request opts in,
        // so idle overhead stays negligible. This only works for a PHP that has
        // Xdebug available (built in, or a loadable xdebug.so) — Grove's fully
        // static builds can't, so those report as unavailable.
        let xdebug = *self.xdebug.lock().unwrap();
        let xdebug_entries = if xdebug.enabled {
            let plan = xdebug::resolve(&self.paths, &build);
            let entries = xdebug::debug_ini_entries(&plan, xdebug.port);
            if entries.is_empty() {
                tracing::warn!(
                    version,
                    "Xdebug enabled but unavailable for this build — register a PHP \
                     that has Xdebug (`grove php register`)"
                );
            } else {
                tracing::info!(
                    version,
                    plan = xdebug::describe(&plan),
                    "loading Xdebug into pool"
                );
            }
            entries
        } else {
            Vec::new()
        };

        let mut cmd = std::process::Command::new(&build.fpm_binary);
        cmd.arg("--nodaemonize").arg("--fpm-config").arg(&conf);
        xdebug::apply_dargs(&mut cmd, &xdebug_entries);
        if run_as.is_some() {
            // The master drops with the workers, so it never execs
            // `build.fpm_binary` as root — and that path comes from
            // `php-builds.json` in a user-writable tree, which is the whole
            // point. `--allow-to-run-as-root` is then unnecessary.
            privdrop::apply(&mut cmd, run_as);
        } else if privdrop::running_as_root() {
            // We are root but cannot tell who to become. Keep the old
            // behaviour rather than refusing to serve: php-fpm will not start as
            // root without this, and the pool config drops the workers.
            tracing::warn!(
                "running the PHP-FPM master as root: no run user is known \
                 (re-run `sudo grove install` to record one)"
            );
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
        run_as: Option<privdrop::RunAs>,
    ) -> Result<PathBuf, FpmError> {
        let conf_path = self
            .paths
            .runtimes_dir()
            .join(format!("fpm-{}.conf", version.replace('.', "_")));
        let pid = self
            .fpm_run_dir()
            .join(format!("php-fpm-{}.pid", version.replace('.', "_")));
        // `user`/`listen.owner` only mean anything to a master running as root.
        // When the master itself drops they are ignored, and php-fpm says so in
        // a NOTICE on every pool start — so only emit them for the fallback
        // path, where the master really is root and the workers still need to
        // come down.
        let user_directives = match (run_as, privdrop::running_as_root(), target_user()) {
            (None, true, Some(user)) => format!("user = {user}\nlisten.owner = {user}\n"),
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::registry::PhpBuild;
    use std::path::Path;

    /// A stand-in for php-fpm. It is handed `--nodaemonize --fpm-config <conf>`
    /// like the real thing; it reads the `listen` path out of the config,
    /// creates it so the manager sees the pool come up, then waits to be killed.
    fn fake_fpm(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-fpm.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nsock=$(sed -n 's/^listen = //p' \"$3\")\n: > \"$sock\"\nexec sleep 300\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn manager(name: &str) -> (FpmManager, PathBuf) {
        let base = std::env::temp_dir().join(format!("grove-fpm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = GrovePaths::with_base(&base);
        paths.ensure().unwrap();
        let mut registry = PhpRegistry::load(&paths);
        registry.register(PhpBuild {
            version: "8.9".into(),
            fpm_binary: fake_fpm(&base),
            cli_binary: None,
            variant: None,
            user_registered: false,
        });
        (FpmManager::new(paths, registry), base)
    }

    fn kill_master(mgr: &FpmManager, version: &str) {
        let mut pools = mgr.pools.lock().unwrap();
        let child = pools
            .get_mut(version)
            .and_then(|p| p.child.as_mut())
            .expect("a pool with a child");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// The bug: a master dies, the next request respawns it — and the *old*
    /// pool, handed back by `pools.insert`, is dropped and removes the socket
    /// file the new master just bound. The map then holds a live child, so
    /// nothing ever respawns again, and that PHP version 502s until restart.
    ///
    /// Two reviews called the respawn path correct. They checked that a dead
    /// child is detected, not what happens to the one being replaced.
    #[test]
    fn a_respawned_pool_keeps_the_socket_it_just_bound() {
        let (mgr, base) = manager("respawn");

        let FpmAddr::Unix(socket) = mgr.ensure_pool("8.9").expect("first spawn") else {
            panic!("pools listen on unix sockets");
        };
        assert!(socket.exists(), "the first master creates its socket");

        // OOM, a stray `killall php-fpm`, a crash: the master is gone.
        kill_master(&mgr, "8.9");

        let FpmAddr::Unix(again) = mgr.ensure_pool("8.9").expect("respawn") else {
            panic!("pools listen on unix sockets");
        };
        assert_eq!(again, socket, "the same version binds the same path");
        assert!(
            socket.exists(),
            "the respawned master's socket must survive the dead pool being dropped"
        );

        let pools = mgr.pools.lock().unwrap();
        let alive = pools["8.9"]
            .child
            .as_ref()
            .map(|c| c.id() > 0)
            .unwrap_or(false);
        assert!(alive, "the map holds the new master");
        drop(pools);

        drop(mgr);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The happy path the fix must not break: a live pool is reused, not
    /// respawned, and its socket stays.
    #[test]
    fn a_live_pool_is_reused() {
        let (mgr, base) = manager("reuse");
        let first = mgr.ensure_pool("8.9").expect("spawn");
        let pid = mgr.pools.lock().unwrap()["8.9"]
            .child
            .as_ref()
            .map(|c| c.id());
        let second = mgr.ensure_pool("8.9").expect("lookup");
        let pid_after = mgr.pools.lock().unwrap()["8.9"]
            .child
            .as_ref()
            .map(|c| c.id());
        assert_eq!(pid, pid_after, "no respawn while the master is alive");
        assert!(
            matches!((first, second), (FpmAddr::Unix(a), FpmAddr::Unix(b)) if a == b && a.exists())
        );
        drop(mgr);
        let _ = std::fs::remove_dir_all(&base);
    }
}

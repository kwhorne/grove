//! Per-site dev processes (Herd/`composer run dev` style, but Grove-managed).
//!
//! With Grove serving the app via FPM, you no longer need `artisan serve` — the
//! only long-running things a Laravel dev session needs are the **Vite dev
//! server** (`npm run dev`, for HMR) and a **queue worker**. Grove detects and
//! supervises those per site, runs them as the invoking user with the site's own
//! PHP/Node, and streams their output into a log the Logs panel already shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use tokio::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_core::site::ResolvedSite;
use grove_runtime::{NodeRegistry, PhpRegistry};

struct DevProc {
    name: &'static str,
    child: Child,
}

struct Session {
    procs: Vec<DevProc>,
}

/// Supervises per-site dev processes, keyed by site name.
#[derive(Default)]
pub struct DevManager {
    inner: Mutex<HashMap<String, Session>>,
}

impl DevManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start dev processes for `site`. Returns the names started (e.g. `vite`,
    /// `queue`), or an error if there was nothing to run.
    pub async fn start(
        &self,
        site: &ResolvedSite,
        paths: &GrovePaths,
    ) -> anyhow::Result<Vec<String>> {
        {
            let map = self.inner.lock().await;
            if map.contains_key(&site.name) {
                anyhow::bail!("dev already running for {}", site.name);
            }
        }
        if site.path.as_os_str().is_empty() || !site.path.is_dir() {
            anyhow::bail!("{} has no local project directory", site.name);
        }

        let ids = drop_ids();
        let mut procs: Vec<DevProc> = Vec::new();

        // --- Vite (npm run dev) ---
        if has_npm_dev_script(&site.path) {
            match resolve_node(paths, site.node.as_deref()) {
                Some((npm, bin_dir)) => {
                    let log = paths.logs_dir().join(format!("dev-{}-vite.log", site.name));
                    let mut cmd = Command::new(&npm);
                    cmd.args(["run", "dev"]).current_dir(&site.path);
                    prepend_path(&mut cmd, &bin_dir);
                    // For HTTPS sites, make a Grove-CA-signed cert available where
                    // Laravel/Herd/Valet vite configs look, so Vite serves HTTPS
                    // (no mixed-content) with a browser-trusted cert.
                    if site.secure {
                        if let Some((crt, key)) = ensure_vite_tls(paths, &site.hostname, &ids) {
                            cmd.env("VITE_DEV_SERVER_CERT", &crt);
                            cmd.env("VITE_DEV_SERVER_KEY", &key);
                        }
                    }
                    set_logs(&mut cmd, &log)?;
                    apply_env(&mut cmd, ids.clone());
                    procs.push(DevProc {
                        name: "vite",
                        child: cmd.spawn()?,
                    });
                }
                None => tracing::warn!(
                    site = %site.name,
                    "package.json has a dev script but no Node is installed (grove node install)"
                ),
            }
        }

        // --- Queue worker (php artisan queue:work) ---
        if site.path.join("artisan").is_file() && queue_enabled(&site.path) {
            if let Some(php) = resolve_php_cli(paths, &site.php) {
                let log = paths
                    .logs_dir()
                    .join(format!("dev-{}-queue.log", site.name));
                let mut cmd = Command::new(&php);
                cmd.args(["artisan", "queue:work", "--tries=1", "--sleep=1"])
                    .current_dir(&site.path);
                set_logs(&mut cmd, &log)?;
                apply_env(&mut cmd, ids.clone());
                procs.push(DevProc {
                    name: "queue",
                    child: cmd.spawn()?,
                });
            }
        }

        if procs.is_empty() {
            anyhow::bail!(
                "nothing to run for {} (needs a package.json `dev` script and/or a non-sync queue)",
                site.name
            );
        }

        let names = procs.iter().map(|p| p.name.to_string()).collect();
        self.inner
            .lock()
            .await
            .insert(site.name.clone(), Session { procs });
        Ok(names)
    }

    /// Stop the dev processes for `site`.
    pub async fn stop(&self, site: &str) -> anyhow::Result<()> {
        match self.inner.lock().await.remove(site) {
            Some(mut s) => {
                for p in &mut s.procs {
                    let _ = p.child.kill();
                    let _ = p.child.wait();
                }
                Ok(())
            }
            None => anyhow::bail!("dev not running for {site}"),
        }
    }

    /// Names of sites with dev processes running (reaps any that have exited).
    pub async fn list(&self) -> Vec<String> {
        let mut map = self.inner.lock().await;
        map.retain(|_, s| {
            s.procs
                .iter_mut()
                .any(|p| matches!(p.child.try_wait(), Ok(None)))
        });
        let mut out: Vec<String> = map.keys().cloned().collect();
        out.sort();
        out
    }
}

fn has_npm_dev_script(project: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(project.join("package.json")) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("scripts").and_then(|s| s.get("dev")).map(|_| true))
        .unwrap_or(false)
}

/// True when the project uses a real (non-`sync`) queue connection.
fn queue_enabled(project: &Path) -> bool {
    let Ok(env) = std::fs::read_to_string(project.join(".env")) else {
        return false;
    };
    for line in env.lines() {
        if let Some(v) = line.trim().strip_prefix("QUEUE_CONNECTION=") {
            let v = v.trim().trim_matches('"').to_lowercase();
            return !v.is_empty() && v != "sync";
        }
    }
    false
}

/// (npm binary, node bin dir) for the site's Node, or the newest installed.
fn resolve_node(paths: &GrovePaths, version: Option<&str>) -> Option<(PathBuf, PathBuf)> {
    let reg = NodeRegistry::load(paths);
    let build = version
        .and_then(|v| reg.get(v))
        .or_else(|| reg.iter().max_by(|a, b| a.major.cmp(&b.major)))?;
    let bin_dir = build.node_binary.parent()?.to_path_buf();
    Some((build.npm_binary.clone(), bin_dir))
}

/// The PHP CLI for the site's version (downloading it if necessary).
fn resolve_php_cli(paths: &GrovePaths, version: &str) -> Option<PathBuf> {
    let reg = PhpRegistry::load(paths);
    if let Some(cli) = reg.get(version).and_then(|b| b.cli_binary.clone()) {
        return Some(cli);
    }
    grove_runtime::install::install_cli(paths, version, |_| {}).ok()
}

/// Issue a Grove-CA leaf for `hostname` into a Grove-owned dir and return its
/// (cert, key) paths. Fed to Vite via the standard `VITE_DEV_SERVER_CERT` /
/// `VITE_DEV_SERVER_KEY` env vars that `laravel-vite-plugin` reads natively —
/// no Herd/Valet involvement.
fn ensure_vite_tls(
    paths: &GrovePaths,
    hostname: &str,
    ids: &Option<(u32, u32, String)>,
) -> Option<(String, String)> {
    let ca = grove_tls::CertificateAuthority::load_or_create(paths).ok()?;
    let (cert_pem, key_pem) = ca.issue_leaf(&[hostname.to_string()]).ok()?;
    let dir = paths.certs_dir().join("dev");
    std::fs::create_dir_all(&dir).ok()?;
    let crt = dir.join(format!("{hostname}.crt"));
    let key = dir.join(format!("{hostname}.key"));
    std::fs::write(&crt, &cert_pem).ok()?;
    std::fs::write(&key, &key_pem).ok()?;
    // The Vite process runs as the invoking user; let it read the files.
    chown_path(&crt, ids);
    chown_path(&key, ids);
    Some((
        crt.to_string_lossy().into_owned(),
        key.to_string_lossy().into_owned(),
    ))
}

fn chown_path(path: &Path, ids: &Option<(u32, u32, String)>) {
    if let Some((_, _, user)) = ids {
        let _ = std::process::Command::new("chown")
            .arg(user)
            .arg(path)
            .status();
    }
}

fn set_logs(cmd: &mut Command, log: &Path) -> std::io::Result<()> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::fs::File::create(log)?;
    cmd.stdout(f.try_clone()?).stderr(f);
    cmd.stdin(std::process::Stdio::null());
    Ok(())
}

fn prepend_path(cmd: &mut Command, dir: &Path) {
    let base = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    cmd.env("PATH", format!("{}:{base}", dir.display()));
}

// ---- run as the invoking user (the daemon may be root) --------------------

fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            #[link_name = "geteuid"]
            fn geteuid() -> u32;
        }
        unsafe { geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_user() -> Option<String> {
    for var in ["GROVE_RUN_USER", "SUDO_USER"] {
        if let Ok(u) = std::env::var(var) {
            if !u.is_empty() && u != "root" {
                return Some(u);
            }
        }
    }
    None
}

fn drop_ids() -> Option<(u32, u32, String)> {
    if !running_as_root() {
        return None;
    }
    let user = run_user()?;
    let uid = id_of(&["-u", &user])?;
    let gid = id_of(&["-g", &user])?;
    Some((uid, gid, user))
}

fn id_of(args: &[&str]) -> Option<u32> {
    let out = std::process::Command::new("id").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .flatten()
}

/// Drop to the run user (setuid/gid) and point HOME at their home, so npm/php
/// caches land in the right place. No-op when not root.
fn apply_env(cmd: &mut Command, ids: Option<(u32, u32, String)>) {
    let Some((uid, gid, user)) = ids else {
        return;
    };
    let home = if cfg!(target_os = "macos") {
        format!("/Users/{user}")
    } else {
        format!("/home/{user}")
    };
    cmd.env("HOME", home).env("USER", &user);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(move || {
                extern "C" {
                    fn setgid(gid: u32) -> i32;
                    fn setuid(uid: u32) -> i32;
                    fn setgroups(n: usize, list: *const u32) -> i32;
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
}

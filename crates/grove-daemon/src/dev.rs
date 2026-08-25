//! Per-site dev processes (Herd/`composer run dev` style, but Grove-managed).
//!
//! With Grove serving the app via FPM, you no longer need `artisan serve`.
//!
//! From Laravel 13.16 an application declares its own dev processes through
//! `Illuminate\Foundation\DevCommands`, exposed as JSON by `artisan dev:list`.
//! Grove treats that as the source of truth: it reads the list and supervises
//! everything on it — including userland processes like `reverb:start` or
//! `stripe listen` — minus the entries Grove already provides itself (see
//! [`is_redundant`]). Running `grove dev` therefore replaces `php artisan dev`;
//! running both would start two Vite servers and two competing queue workers.
//!
//! For non-Laravel sites, or Laravel without `dev:list`, Grove falls back to its
//! own heuristic: a Vite dev server plus a queue worker.
//!
//! Either way the processes run as the invoking user with the site's own
//! PHP/Node, and their output is streamed into a log the Logs panel already shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use tokio::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_core::site::ResolvedSite;
use grove_runtime::{NodeRegistry, PhpRegistry};

struct DevProc {
    name: String,
    child: Child,
}

/// One process declared by the application via `artisan dev:list --json`.
struct DevSpec {
    name: String,
    /// A full shell-style command line, e.g. `php artisan queue:listen --tries=1`
    /// or `bun run dev` — Laravel pre-prefixes the binary for us.
    command: String,
}

struct Session {
    procs: Vec<DevProc>,
}

/// What [`DevManager::start`] did, plus anything the user should know about.
pub struct StartReport {
    /// Names of the processes started, e.g. `queue`, `vite`, `reverb`.
    pub names: Vec<String>,
    /// Non-fatal warnings to surface to the caller.
    pub warnings: Vec<String>,
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
    ) -> anyhow::Result<StartReport> {
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
        let is_laravel = site.path.join("artisan").is_file();
        let node = resolve_node(paths, site.node.as_deref());
        // Only resolve (and possibly download) a PHP CLI for projects that can
        // actually use one — a static site must not trigger an install.
        let php = if is_laravel {
            resolve_php_cli(paths, &site.php)
        } else {
            None
        };

        // Ask the app what it declares (Laravel >= 13.16). Booting artisan is
        // blocking work, so keep it off the async executor.
        let declared = match (&php, is_laravel) {
            (Some(php_bin), true) => {
                let php_bin = php_bin.clone();
                let project = site.path.clone();
                let node_bin = node.as_ref().map(|(_, dir)| dir.clone());
                let ids = ids.clone();
                tokio::task::spawn_blocking(move || {
                    discover_dev_specs(&project, &php_bin, node_bin.as_deref(), ids)
                })
                .await
                .ok()
                .flatten()
            }
            _ => None,
        };

        let procs = match declared {
            Some(specs) if !specs.is_empty() => {
                spawn_declared(specs, site, paths, node.as_ref(), php.as_deref(), &ids)?
            }
            _ => spawn_builtin(site, paths, node.as_ref(), php.as_deref(), &ids)?,
        };

        if procs.is_empty() {
            anyhow::bail!(
                "nothing to run for {} (needs a package.json `dev` script, a non-sync queue, \
                 or dev processes registered via Laravel's `DevCommands`)",
                site.name
            );
        }

        let names = procs.iter().map(|p| p.name.to_string()).collect();
        // Grove and `php artisan dev` supervise the same processes, so running
        // both gives you two Vite servers and two queue workers.
        let warnings = foreign_artisan_dev()
            .map(|cmd| {
                vec![format!(
                    "`{cmd}` is already running — if it belongs to this site, stop it. \
                     Grove supervises the same processes, so they are now running twice \
                     (two Vite servers competing for one port, two workers competing for \
                     the same jobs)."
                )]
            })
            .unwrap_or_default();

        self.inner
            .lock()
            .await
            .insert(site.name.clone(), Session { procs });
        Ok(StartReport { names, warnings })
    }

    /// Stop the dev processes for `site`.
    pub async fn stop(&self, site: &str) -> anyhow::Result<()> {
        match self.inner.lock().await.remove(site) {
            Some(mut s) => {
                for p in &mut s.procs {
                    kill_tree(p);
                }
                Ok(())
            }
            None => anyhow::bail!("dev not running for {site}"),
        }
    }

    /// Kill every site's dev processes (called on daemon shutdown so children
    /// aren't orphaned when the daemon restarts).
    pub async fn stop_all(&self) {
        let mut map = self.inner.lock().await;
        for (_, mut session) in map.drain() {
            for p in &mut session.procs {
                kill_tree(p);
            }
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

/// True when `command` is a `php artisan dev` invocation — Laravel's own process
/// supervisor, which overlaps with `grove dev`.
///
/// Deliberately narrow: only the *first three* tokens are considered, so the
/// process must actually be `<php> artisan dev`. Matching `artisan dev` anywhere
/// in the command line would fire on any shell script, editor or CI job that
/// merely mentions the string — including Grove's own test harness.
///
/// `dev:list` (Grove's own discovery call) and `npm run dev` must not match.
fn is_artisan_dev(command: &str) -> bool {
    // `ps` reports command lines unquoted, so a path containing spaces — such as
    // Grove's own `~/Library/Application Support/Grove/…/php` — splits into
    // several tokens. Anchor on the PHP binary rather than on token positions.
    let mut tokens = command.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if !is_php_binary(token) {
            continue;
        }
        let Some(script) = tokens.peek().copied() else {
            return false;
        };
        if script == "artisan" || script.ends_with("/artisan") || script.ends_with("\\artisan") {
            tokens.next();
            return tokens.next() == Some("dev");
        }
    }
    false
}

/// True for `php`, `php8.5`, `/usr/bin/php`, `php.exe` — but not `php.ini` or
/// `"$PHP"` (an unexpanded variable inside some other process's command line).
fn is_php_binary(token: &str) -> bool {
    let base = token
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(token)
        .trim_end_matches(".exe");
    match base.strip_prefix("php") {
        // Only a version suffix may follow, e.g. `php8.5` or `php83`.
        Some(rest) => rest.chars().all(|c| c.is_ascii_digit() || c == '.'),
        None => false,
    }
}

/// The command line of a `php artisan dev` process running outside Grove, if any.
///
/// A cheap `ps` scan: it can't tell *which* project the process belongs to, so
/// the caller must phrase this as a warning rather than an error.
fn foreign_artisan_dev() -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|line| is_artisan_dev(line))
        .map(str::to_string)
}

/// Processes Grove already provides itself. Running these under `grove dev`
/// would duplicate or fight with the daemon: Grove *is* the web server (FPM on
/// the site's `.test` domain, so `artisan serve --host=localhost` is both
/// redundant and misleading), and the Logs panel already tails the app log.
fn is_redundant(name: &str) -> bool {
    matches!(name, "server" | "serve" | "logs")
}

/// Node package-manager front-ends. These are resolved through Grove's bundled
/// Node `bin` dir rather than by absolute path.
fn is_node_runner(bin: &str) -> bool {
    matches!(
        bin,
        "npm" | "npx" | "pnpm" | "pnpx" | "yarn" | "bun" | "bunx"
    )
}

/// Ask the Laravel app which dev processes it declares.
///
/// Returns `None` when the command doesn't exist (Laravel < 13.16), the app
/// fails to boot, or the output isn't parseable — callers then fall back to
/// Grove's own heuristic. `stdin` is null, so `dev:list` can never block on a
/// prompt, and it emits JSON for non-interactive input regardless of `--json`.
fn discover_dev_specs(
    project: &Path,
    php: &Path,
    node_bin: Option<&Path>,
    ids: Option<(u32, u32, String)>,
) -> Option<Vec<DevSpec>> {
    let mut cmd = Command::new(php);
    cmd.args([
        "artisan",
        "dev:list",
        "--json",
        // Don't let an arbitrary composer package register processes into the
        // daemon; only framework defaults and the user's own code count.
        "--except-vendor",
        "--no-interaction",
    ])
    .current_dir(project)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    if let Some(dir) = node_bin {
        prepend_path(&mut cmd, dir);
    }
    // Run as the invoking user: booting artisan as root would leave root-owned
    // files in bootstrap/cache and storage/logs.
    apply_env(&mut cmd, ids);

    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_dev_specs(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the JSON array emitted by `artisan dev:list --json`.
///
/// Entries carry `name`, `command`, `color`, `source` and `priority`; Grove only
/// needs the first two. Unknown keys are ignored so a future Laravel can add
/// fields without breaking us.
fn parse_dev_specs(stdout: &str) -> Option<Vec<DevSpec>> {
    // Take the last non-empty line, in case something warmed up on stdout first.
    let json = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let specs = parsed
        .as_array()?
        .iter()
        .filter_map(|entry| {
            Some(DevSpec {
                name: entry.get("name")?.as_str()?.to_string(),
                command: entry.get("command")?.as_str()?.to_string(),
            })
        })
        .collect();
    Some(specs)
}

/// Supervise the processes the application declared.
fn spawn_declared(
    specs: Vec<DevSpec>,
    site: &ResolvedSite,
    paths: &GrovePaths,
    node: Option<&(PathBuf, PathBuf)>,
    php: Option<&Path>,
    ids: &Option<(u32, u32, String)>,
) -> anyhow::Result<Vec<DevProc>> {
    let mut procs = Vec::new();

    for spec in specs {
        if is_redundant(&spec.name) {
            tracing::debug!(
                site = %site.name, process = %spec.name,
                "skipping dev process that Grove already provides"
            );
            continue;
        }
        let Some(mut argv) = shell_split(&spec.command) else {
            tracing::warn!(
                site = %site.name, process = %spec.name, command = %spec.command,
                "could not parse declared dev command"
            );
            continue;
        };
        if argv.is_empty() {
            continue;
        }

        // Point the command at Grove's pinned runtimes instead of whatever
        // `php` / `npm` happens to be on the daemon's PATH.
        let node_runner = is_node_runner(&argv[0]);
        if argv[0] == "php" {
            match php {
                Some(bin) => argv[0] = bin.to_string_lossy().into_owned(),
                None => {
                    tracing::warn!(site = %site.name, process = %spec.name, "no PHP available");
                    continue;
                }
            }
        } else if node_runner && node.is_none() {
            tracing::warn!(
                site = %site.name, process = %spec.name,
                "declared a Node process but no Node is installed (grove node install)"
            );
            continue;
        }

        let log = paths
            .logs_dir()
            .join(format!("dev-{}-{}.log", site.name, spec.name));
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).current_dir(&site.path);
        if let Some((_, bin_dir)) = node {
            prepend_path(&mut cmd, bin_dir);
        }
        if node_runner && site.secure {
            if let Some((crt, key)) = ensure_vite_tls(paths, &site.hostname, ids) {
                cmd.env("VITE_DEV_SERVER_CERT", &crt);
                cmd.env("VITE_DEV_SERVER_KEY", &key);
            }
        }
        set_logs(&mut cmd, &log)?;
        new_process_group(&mut cmd);
        apply_env(&mut cmd, ids.clone());
        procs.push(DevProc {
            name: spec.name,
            child: cmd.spawn()?,
        });
    }

    Ok(procs)
}

/// Grove's built-in heuristic: a Vite dev server and a queue worker. Used for
/// non-Laravel sites and Laravel versions without `DevCommands`.
fn spawn_builtin(
    site: &ResolvedSite,
    paths: &GrovePaths,
    node: Option<&(PathBuf, PathBuf)>,
    php: Option<&Path>,
    ids: &Option<(u32, u32, String)>,
) -> anyhow::Result<Vec<DevProc>> {
    let mut procs = Vec::new();

    // --- Vite (npm run dev) ---
    if has_npm_dev_script(&site.path) {
        match node {
            Some((npm, bin_dir)) => {
                let log = paths.logs_dir().join(format!("dev-{}-vite.log", site.name));
                let mut cmd = Command::new(npm);
                cmd.args(["run", "dev"]).current_dir(&site.path);
                prepend_path(&mut cmd, bin_dir);
                // For HTTPS sites, make a Grove-CA-signed cert available where
                // Laravel/Herd/Valet vite configs look, so Vite serves HTTPS
                // (no mixed-content) with a browser-trusted cert.
                if site.secure {
                    if let Some((crt, key)) = ensure_vite_tls(paths, &site.hostname, ids) {
                        cmd.env("VITE_DEV_SERVER_CERT", &crt);
                        cmd.env("VITE_DEV_SERVER_KEY", &key);
                    }
                }
                set_logs(&mut cmd, &log)?;
                new_process_group(&mut cmd);
                apply_env(&mut cmd, ids.clone());
                procs.push(DevProc {
                    name: "vite".into(),
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
        if let Some(php) = php {
            let log = paths
                .logs_dir()
                .join(format!("dev-{}-queue.log", site.name));
            let mut cmd = Command::new(php);
            cmd.args(["artisan", "queue:work", "--tries=1", "--sleep=1"])
                .current_dir(&site.path);
            set_logs(&mut cmd, &log)?;
            new_process_group(&mut cmd);
            apply_env(&mut cmd, ids.clone());
            procs.push(DevProc {
                name: "queue".into(),
                child: cmd.spawn()?,
            });
        }
    }

    Ok(procs)
}

/// Minimal POSIX-ish argv split: handles single quotes, double quotes and
/// backslash escapes. Returns `None` on an unterminated quote or escape.
fn shell_split(input: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return None,
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => cur.push(chars.next()?),
                        Some(c) => cur.push(c),
                        None => return None,
                    }
                }
            }
            '\\' => {
                started = true;
                cur.push(chars.next()?);
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    Some(out)
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
    let variant = grove_runtime::install::Variant::configured(paths);
    grove_runtime::install::install_cli(paths, version, variant, |_| {}).ok()
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
    // These went out at the process umask with no `chmod` at all, so a root
    // daemon left a TLS private key world-readable. `write_private` gives it
    // 0600 from creation and refuses to write through a symlink.
    grove_core::securefs::write_public(&crt, &cert_pem).ok()?;
    grove_core::securefs::write_private(&key, &key_pem).ok()?;
    // The Vite process runs as the invoking user; let it read the files. 0600
    // plus this chown means only that user can, which is the point.
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
    // Symlink-safe: a link here would let a root daemon append a dev process's
    // output into an arbitrary file.
    let f = grove_core::securefs::create_public(log)?;
    cmd.stdout(f.try_clone()?).stderr(f);
    cmd.stdin(std::process::Stdio::null());
    Ok(())
}

/// Put the child in its own process group so Grove can signal the whole tree.
///
/// `npm run dev` execs a shell that spawns Vite as a *grandchild*, so killing
/// only the direct child leaves an orphaned Vite holding port 5173 — which then
/// breaks the next `grove dev start`. A dedicated group lets [`kill_tree`] reach
/// everything the dev process started.
fn new_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setpgid(pid: i32, pgid: i32) -> i32;
                }
                // pid 0 / pgid 0 = "make me the leader of my own new group".
                if setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let _ = cmd;
}

/// Stop a dev process and every process it spawned.
///
/// Signals the child's process group: `SIGTERM` first, so Vite and queue workers
/// can shut down cleanly, then `SIGKILL` for whatever is left.
fn kill_tree(proc: &mut DevProc) {
    #[cfg(unix)]
    {
        extern "C" {
            fn killpg(pgrp: i32, sig: i32) -> i32;
            fn getpgid(pid: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        const SIGKILL: i32 = 9;

        let pid = proc.child.id() as i32;
        // Only signal the group when the child really is its own group leader.
        // If `setpgid` failed, its group is *Grove's* group, and `killpg` would
        // take down the daemon itself.
        if pid > 0 && unsafe { getpgid(pid) } == pid {
            unsafe { killpg(pid, SIGTERM) };
            // Bounded grace period; long enough for a clean exit, short enough
            // not to stall `grove dev stop`.
            for _ in 0..10 {
                if matches!(proc.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            unsafe { killpg(pid, SIGKILL) };
        }
    }
    let _ = proc.child.kill();
    let _ = proc.child.wait();
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

/// Drop to the run user and point HOME at their home, so npm/php caches land in
/// the right place. No-op when not root.
///
/// The drop itself is `grove_core::privdrop`, which is where the third copy of
/// this `setgroups`/`setgid`/`setuid` sequence used to live. All three ignored
/// whether `setgroups` succeeded, so a failure left the child holding root's
/// supplementary groups after its uid and gid had come down. The username is
/// still resolved here because only this caller needs it — for `HOME`, not for
/// the drop.
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
    grove_core::privdrop::apply(cmd, Some(grove_core::privdrop::RunAs { uid, gid }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_and_quoted_commands() {
        assert_eq!(
            shell_split("php artisan queue:listen --tries=1").unwrap(),
            ["php", "artisan", "queue:listen", "--tries=1"]
        );
        assert_eq!(shell_split("bun run dev").unwrap(), ["bun", "run", "dev"]);
        assert_eq!(
            shell_split("stripe listen --forward-to 'https://a.test/hook'").unwrap(),
            ["stripe", "listen", "--forward-to", "https://a.test/hook"]
        );
        assert_eq!(
            shell_split(r#"say "hello   world" x"#).unwrap(),
            ["say", "hello   world", "x"]
        );
        assert_eq!(shell_split("  spaced   out  ").unwrap(), ["spaced", "out"]);
        assert!(shell_split("oops 'unterminated").is_none());
        assert_eq!(shell_split("").unwrap(), Vec::<String>::new());
    }

    /// Verbatim output of `php artisan dev:list --json` on a default Laravel
    /// 13.16 app (captured from a real project).
    const REAL_DEV_LIST: &str = r##"[{"command":"php artisan serve --host=localhost","name":"server","color":"#93c5fd","source":"Illuminate\\Foundation\\Providers\\ArtisanServiceProvider@boot","priority":0},{"command":"php artisan queue:listen --tries=1 --timeout=0","name":"queue","color":"#c4b5fd","source":"Illuminate\\Foundation\\Providers\\ArtisanServiceProvider@boot","priority":0},{"command":"php artisan pail --timeout=0","name":"logs","color":"#fb7185","source":"Illuminate\\Foundation\\Providers\\ArtisanServiceProvider@boot","priority":0},{"command":"npm run dev","name":"vite","color":"#fdba74","source":"Illuminate\\Foundation\\Providers\\ArtisanServiceProvider@boot","priority":0}]"##;

    #[test]
    fn parses_a_real_dev_list_payload() {
        let specs = parse_dev_specs(REAL_DEV_LIST).expect("should parse");
        let pairs: Vec<(&str, &str)> = specs
            .iter()
            .map(|s| (s.name.as_str(), s.command.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("server", "php artisan serve --host=localhost"),
                ("queue", "php artisan queue:listen --tries=1 --timeout=0"),
                ("logs", "php artisan pail --timeout=0"),
                ("vite", "npm run dev"),
            ]
        );
    }

    #[test]
    fn keeps_only_the_processes_grove_should_supervise() {
        let kept: Vec<String> = parse_dev_specs(REAL_DEV_LIST)
            .unwrap()
            .into_iter()
            .filter(|s| !is_redundant(&s.name))
            .map(|s| s.name)
            .collect();
        // `server` is Grove itself (FPM on the .test domain) and `logs` is the
        // Logs panel, so a default app yields exactly these two.
        assert_eq!(kept, ["queue", "vite"]);
    }

    #[test]
    fn splits_every_command_in_a_real_payload() {
        for spec in parse_dev_specs(REAL_DEV_LIST).unwrap() {
            let argv = shell_split(&spec.command)
                .unwrap_or_else(|| panic!("failed to split {:?}", spec.command));
            assert!(!argv.is_empty());
            // Every default command starts with a runtime Grove must rewrite or
            // resolve through its own bin dir.
            assert!(
                argv[0] == "php" || is_node_runner(&argv[0]),
                "unexpected runtime {:?}",
                argv[0]
            );
        }
    }

    #[test]
    fn rejects_non_json_and_empty_output() {
        assert!(parse_dev_specs("").is_none());
        assert!(parse_dev_specs("Command \"dev:list\" is not defined.").is_none());
        assert!(parse_dev_specs("{}").is_none());
        assert_eq!(parse_dev_specs("[]").unwrap().len(), 0);
    }

    #[test]
    fn ignores_warmup_noise_before_the_json() {
        let out = format!("Some deprecation notice\n\n{REAL_DEV_LIST}\n");
        assert_eq!(parse_dev_specs(&out).unwrap().len(), 4);
    }

    #[test]
    fn skips_processes_grove_already_provides() {
        assert!(is_redundant("server"));
        assert!(is_redundant("logs"));
        assert!(!is_redundant("queue"));
        assert!(!is_redundant("vite"));
        assert!(!is_redundant("reverb"));
    }

    #[test]
    fn detects_artisan_dev_but_not_lookalikes() {
        assert!(is_artisan_dev("php artisan dev"));
        assert!(is_artisan_dev(
            "/Users/x/Library/Application Support/Grove/runtimes/cli/8.5/php artisan dev"
        ));
        assert!(is_artisan_dev("  php   artisan   dev  "));
        assert!(is_artisan_dev("php8.5 artisan dev --no-tty"));
        assert!(is_artisan_dev("/opt/php/php artisan dev"));
        // Grove's own discovery call must never trip the warning.
        assert!(!is_artisan_dev(
            "php artisan dev:list --json --except-vendor"
        ));
        assert!(!is_artisan_dev("php artisan develop"));
        assert!(!is_artisan_dev("php artisan queue:listen --tries=1"));
        assert!(!is_artisan_dev("npm run dev"));
        assert!(!is_artisan_dev("php"));
        assert!(!is_artisan_dev("php artisan"));
        assert!(!is_artisan_dev(""));
        // Regression: a shell/editor/CI process that merely *mentions* the
        // string must not fire the warning. This fired in real testing.
        assert!(!is_artisan_dev(
            r#"/bin/bash -c cd /app && nohup "$PHP" artisan dev > /tmp/f.log 2>&1"#
        ));
        assert!(!is_artisan_dev("vim notes-about-php-artisan-dev.md"));
        assert!(!is_artisan_dev("grep -r 'artisan dev' ."));
        // Grove's own PHP lives under a path with a space in it, so the binary
        // must be found by scanning, not by token position.
        assert!(is_artisan_dev(
            "/Users/x/Library/Application Support/Grove/runtimes/cli/8.5/php artisan dev"
        ));
    }

    #[test]
    fn recognises_php_binaries() {
        assert!(is_php_binary("php"));
        assert!(is_php_binary("php8.5"));
        assert!(is_php_binary("php83"));
        assert!(is_php_binary("/usr/local/bin/php"));
        assert!(is_php_binary("php.exe"));
        assert!(!is_php_binary("php.ini"));
        assert!(!is_php_binary("phpstan"));
        assert!(!is_php_binary("\"$PHP\""));
        assert!(!is_php_binary("node"));
    }

    /// `new_process_group` and `apply_env` both register a `pre_exec` closure.
    /// The root code path relies on std running *both*, in order, after fork —
    /// so verify the chaining itself, which is the part that could silently drop
    /// the `setpgid` call.
    #[test]
    #[cfg(unix)]
    fn process_group_survives_a_second_pre_exec_hook() {
        use std::os::unix::process::CommandExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let ran_second = Arc::new(AtomicBool::new(false));

        let mut cmd = Command::new("/bin/sh");
        // Report our own process group; the parent asserts it equals our pid.
        cmd.args(["-c", "ps -o pgid= -p $$"]);
        cmd.stdout(std::process::Stdio::piped());
        new_process_group(&mut cmd);
        // Stand in for `apply_env`'s hook, registered after ours.
        let flag = Arc::clone(&ran_second);
        unsafe {
            cmd.pre_exec(move || {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            });
        }

        let child = cmd.spawn().expect("spawn");
        let pid = child.id();
        let out = child.wait_with_output().expect("wait");
        let pgid: u32 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("pgid");

        // Leader of its own group => kill_tree's killpg reaches the whole tree.
        assert_eq!(pgid, pid, "child should lead its own process group");
        // The flag is set in the *child* after fork, so the parent's copy stays
        // false; this only proves registration order didn't panic. The real
        // assertion is the pgid above.
        assert!(!ran_second.load(Ordering::SeqCst));
    }

    #[test]
    fn recognises_node_runners() {
        assert!(is_node_runner("npm"));
        assert!(is_node_runner("bun"));
        assert!(is_node_runner("pnpm"));
        assert!(!is_node_runner("php"));
        assert!(!is_node_runner("stripe"));
    }
}

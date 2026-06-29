//! `grove` — the CLI frontend. A thin client over the daemon for stateful
//! actions, with a few local-only commands (CA trust, PHP discovery).

mod cli;
mod output;

use anyhow::Context;
use clap::Parser;

use grove_core::paths::GrovePaths;
use grove_ipc::client;
use grove_ipc::protocol::Request;

use cli::{CaAction, Cli, Command, PhpAction};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    init_tracing(matches!(args.command, Command::Daemon));

    let paths = GrovePaths::discover().context("locating Grove home")?;

    match args.command {
        Command::Daemon => {
            tracing::info!(home = %paths.base().display(), "starting groved");
            grove_daemon::run(paths).await?;
            Ok(())
        }

        Command::Ca { action } => local::ca(&paths, action, args.json),
        Command::Php { action } => local::php(&paths, action, args.json),

        Command::Start => lifecycle::start(&paths, args.json).await,
        Command::Stop => lifecycle::stop(&paths, args.json).await,
        Command::Restart => lifecycle::restart(&paths, args.json).await,
        Command::Install => lifecycle::install(&paths, args.json),
        Command::Uninstall => lifecycle::uninstall(&paths, args.json),
        Command::Import => lifecycle::import_valet(&paths, args.json),

        // Everything else is an IPC round-trip to the daemon.
        other => {
            let request = to_request(other, &paths)?;
            let socket = paths.ipc_socket();
            if !client::is_running(&socket).await {
                anyhow::bail!(
                    "Grove daemon is not running. Start it with `grove daemon` \
                     (or install the service)."
                );
            }
            let response = client::send(&socket, &request)
                .await
                .context("talking to daemon")?;
            output::print_response(&response, args.json);
            if !response.ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// Translate a CLI command into an IPC request.
fn to_request(cmd: Command, _paths: &GrovePaths) -> anyhow::Result<Request> {
    let cwd = || {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    };
    Ok(match cmd {
        Command::Park { path } => Request::Park {
            path: path.or_else(cwd).context("no path and cwd unavailable")?,
        },
        Command::Unpark { path } => Request::Unpark {
            path: path.or_else(cwd).context("no path and cwd unavailable")?,
        },
        Command::Link { name, path } => Request::Link {
            path: path.or_else(cwd).context("no path and cwd unavailable")?,
            name,
        },
        Command::Unlink { name } => Request::Unlink { name },
        Command::List => Request::ListSites,
        Command::Status => Request::Status,
        Command::Secure { name } => Request::Secure { name, enable: true },
        Command::Unsecure { name } => Request::Secure { name, enable: false },
        Command::Isolate { name, version } => Request::Isolate {
            name,
            version: Some(version),
        },
        Command::Unisolate { name } => Request::Isolate {
            name,
            version: None,
        },
        Command::Proxy { name, url } => Request::Proxy { name, url },
        Command::Doctor => Request::Doctor,
        Command::Daemon
        | Command::Ca { .. }
        | Command::Php { .. }
        | Command::Start
        | Command::Stop
        | Command::Restart
        | Command::Install
        | Command::Uninstall
        | Command::Import => {
            unreachable!("handled before to_request")
        }
    })
}

fn init_tracing(daemon: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let default = if daemon { "info" } else { "warn" };
    let filter = EnvFilter::try_from_env("GROVE_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
}

/// Daemon lifecycle + migration commands.
mod lifecycle {
    use super::*;
    use grove_core::Config;
    use grove_ipc::protocol::Request;
    use std::time::Duration;

    /// Spawn `grove daemon` detached, waiting until the IPC socket is live.
    pub async fn start(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        let socket = paths.ipc_socket();
        if client::is_running(&socket).await {
            output::print_message("daemon already running", json);
            return Ok(());
        }
        paths.ensure()?;
        let exe = std::env::current_exe().context("resolving grove binary path")?;
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.base().join("daemon.log"))?;
        let err = out.try_clone()?;

        let mut cmd = std::process::Command::new(exe);
        cmd.arg("daemon").stdout(out).stderr(err).stdin(std::process::Stdio::null());
        // Preserve a custom GROVE_HOME if one is set.
        if let Ok(home) = std::env::var("GROVE_HOME") {
            cmd.env("GROVE_HOME", home);
        }
        detach(&mut cmd);
        cmd.spawn().context("spawning daemon")?;

        for _ in 0..100 {
            if client::is_running(&socket).await {
                output::print_message("daemon started", json);
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("daemon did not come up in time; see {}", paths.base().join("daemon.log").display());
    }

    /// Ask the daemon to shut down (IPC), falling back to SIGTERM via pidfile.
    pub async fn stop(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        let socket = paths.ipc_socket();
        if client::is_running(&socket).await {
            let _ = client::send(&socket, &Request::Shutdown).await;
        } else if !signal_pidfile(paths) {
            output::print_message("daemon not running", json);
            return Ok(());
        }
        // Wait for it to actually exit.
        for _ in 0..100 {
            if !client::is_running(&socket).await {
                output::print_message("daemon stopped", json);
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        output::print_message("shutdown requested", json);
        Ok(())
    }

    pub async fn restart(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        stop(paths, false).await?;
        start(paths, json).await
    }

    /// Send SIGTERM to the PID in the pidfile. Returns false if no pidfile.
    fn signal_pidfile(paths: &GrovePaths) -> bool {
        let Ok(raw) = std::fs::read_to_string(paths.pid_file()) else {
            return false;
        };
        let Ok(pid) = raw.trim().parse::<i32>() else {
            return false;
        };
        #[cfg(unix)]
        unsafe {
            libc_kill(pid, 15); // SIGTERM
        }
        true
    }

    pub fn install(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("resolving grove binary path")?;
        let unit = grove_os::service::install(&exe).context("installing service")?;
        let _ = paths;
        output::print_message(&format!("service installed: {}", unit.display()), json);
        Ok(())
    }

    pub fn uninstall(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        use grove_os::PlatformIntegration;
        grove_os::service::uninstall().context("removing service")?;
        let platform = grove_os::current();
        let config = Config::load(paths).unwrap_or_default();
        let _ = platform.uninstall_resolver(&config.general.tld);
        let _ = platform.untrust_ca(&paths.ca_cert());
        output::print_message("service, resolver and CA trust removed", json);
        Ok(())
    }

    /// Import parked dirs + linked sites from an existing Laravel Valet config.
    pub fn import_valet(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let candidates: Vec<std::path::PathBuf> = home
            .into_iter()
            .flat_map(|h| {
                vec![
                    h.join(".config/valet/config.json"),
                    h.join(".valet/config.json"),
                ]
            })
            .collect();
        let Some(valet_cfg) = candidates.iter().find(|p| p.exists()) else {
            anyhow::bail!("no Valet config found (looked in ~/.config/valet and ~/.valet)");
        };

        let raw = std::fs::read_to_string(valet_cfg)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;

        let mut config = Config::load(paths).unwrap_or_default();
        let mut parked = 0;
        let mut linked = 0;

        if let Some(paths_arr) = parsed.get("paths").and_then(|v| v.as_array()) {
            for p in paths_arr.iter().filter_map(|v| v.as_str()) {
                config.add_parked(std::path::PathBuf::from(p));
                parked += 1;
            }
        }
        // Valet keeps symlinked sites under ~/.config/valet/Sites.
        if let Some(home) = std::env::var_os("HOME") {
            let sites_dir = std::path::Path::new(&home).join(".config/valet/Sites");
            if let Ok(entries) = std::fs::read_dir(&sites_dir) {
                for e in entries.flatten() {
                    if let Ok(target) = std::fs::read_link(e.path()) {
                        let name = e.file_name().to_string_lossy().to_string();
                        let _ = config.add_site(grove_core::config::SiteConfig {
                            name,
                            path: Some(target),
                            php: None,
                            secure: false,
                            driver: None,
                            proxy_to: None,
                        });
                        linked += 1;
                    }
                }
            }
        }
        if let Some(tld) = parsed.get("tld").and_then(|v| v.as_str()) {
            config.general.tld = tld.to_string();
        }
        config.save(paths)?;
        output::print_message(
            &format!("imported from Valet: {parked} parked dir(s), {linked} linked site(s)"),
            json,
        );
        Ok(())
    }

    #[cfg(unix)]
    fn detach(cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid in the child detaches it from the controlling terminal.
        unsafe {
            cmd.pre_exec(|| {
                libc_setsid();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    fn detach(_cmd: &mut std::process::Command) {}

    #[cfg(unix)]
    extern "C" {
        #[link_name = "kill"]
        fn libc_kill(pid: i32, sig: i32) -> i32;
        #[link_name = "setsid"]
        fn libc_setsid() -> i32;
    }
}

/// Local-only commands that don't require the daemon.
mod local {
    use super::*;
    use grove_os::PlatformIntegration;
    use grove_runtime::{PhpBuild, PhpRegistry};
    use grove_tls::CertificateAuthority;
    use std::path::PathBuf;

    pub fn ca(paths: &GrovePaths, action: CaAction, json: bool) -> anyhow::Result<()> {
        let platform = grove_os::current();
        match action {
            CaAction::Trust => {
                let ca = CertificateAuthority::load_or_create(paths)?;
                let _ = ca; // ensures it exists on disk
                platform
                    .trust_ca(&paths.ca_cert())
                    .context("trusting root CA (needs elevation)")?;
                output::print_message(
                    &format!("Grove root CA trusted ({} store)", platform.name()),
                    json,
                );
            }
            CaAction::Uninstall => {
                platform.untrust_ca(&paths.ca_cert())?;
                output::print_message("Grove root CA removed from trust store", json);
            }
        }
        Ok(())
    }

    pub fn php(paths: &GrovePaths, action: PhpAction, json: bool) -> anyhow::Result<()> {
        let mut registry = PhpRegistry::load(paths);
        match action {
            PhpAction::Discover => {
                let n = registry.discover();
                registry.save(paths)?;
                output::print_message(&format!("discovered {n} new PHP build(s)"), json);
            }
            PhpAction::List => {
                output::print_php_list(&registry, json);
            }
            PhpAction::Register {
                version,
                fpm_binary,
            } => {
                let path = PathBuf::from(&fpm_binary);
                if !path.exists() {
                    anyhow::bail!("php-fpm binary not found at {fpm_binary}");
                }
                let cli = path.parent().map(|d| d.join("php")).filter(|p| p.exists());
                registry.register(PhpBuild {
                    version: version.clone(),
                    fpm_binary: path,
                    cli_binary: cli,
                    user_registered: true,
                });
                registry.save(paths)?;
                output::print_message(&format!("registered php@{version}"), json);
            }
        }
        Ok(())
    }
}

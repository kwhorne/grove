//! grove-daemon — the single long-running process.
//!
//! Binds the privileged ports (DNS 53, HTTP 80, HTTPS 443), supervises FPM
//! pools, and exposes an IPC endpoint the CLI/GUI drive. The CLI and GUI are
//! thin clients; all stateful logic lives here.

pub mod commands;
pub mod dev;
pub mod docker;
pub mod doctor;
pub mod ipc;
pub mod license;
pub mod logs;
pub mod state;
pub mod tunnels;

use crate::state::ListenerHealth;

pub use state::DaemonState;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Context;

use grove_core::{paths::GrovePaths, Config};
use grove_proxy::{SharedState, SniResolver};
use grove_runtime::{FpmManager, PhpRegistry};
use grove_tls::CertificateAuthority;

/// Boot the daemon: load config, build the registry, bring up DNS + proxy, and
/// start the IPC listener. Runs until cancelled.
pub async fn run(paths: GrovePaths) -> anyhow::Result<()> {
    paths.ensure()?;
    guard_single_instance(&paths)?;
    let config = Config::load(&paths).context("loading config")?;
    let general = config.general.clone();
    let general_services = config.services.clone();

    // Resolve PHP runtimes (auto-discover on first boot).
    let mut php_registry = PhpRegistry::load(&paths);
    if php_registry.iter().next().is_none() {
        let n = php_registry.discover();
        tracing::info!(discovered = n, "auto-discovered PHP builds");
        let _ = php_registry.save(&paths);
    }
    let fpm = Arc::new(FpmManager::new(paths.clone(), php_registry));
    // A previous daemon that was SIGKILLed left its php-fpm masters running.
    // Find them by pid file and stop them before any request spawns a new one
    // on the same socket path.
    fpm.reap_orphans();
    // Restore the persisted Xdebug setting so pools spawn correctly after a
    // daemon restart.
    fpm.set_xdebug(general.xdebug, general.xdebug_port);

    // Build the site registry and shared proxy state.
    let registry = grove_core::SiteRegistry::build(&config);
    tracing::info!(sites = registry.len(), tld = %general.tld, "registry built");
    let shared = SharedState::new(registry).with_https_port(general.https_port);

    // Local CA + SNI resolver for HTTPS.
    let ca = Arc::new(CertificateAuthority::load_or_create(&paths)?);
    // The resolver reads the registry's hostnames, not the registry itself:
    // rustls resolves a certificate synchronously and cannot await the lock.
    let sni = Arc::new(SniResolver::new(
        ca.clone(),
        paths.clone(),
        shared.known_hosts.clone(),
    ));

    // Built-in mail-catcher store, shared with the SMTP listener + IPC queries.
    let mail = grove_services::MailStore::new();

    // Bundled service supervisor (downloads + runs PostgreSQL, …).
    let services = Arc::new(grove_services::ServiceManager::new(paths.clone()));
    // Same for databases: stop what the previous daemon left behind, then
    // auto-start what is installed and was left running.
    services.reap_orphans();
    services.autostart_installed();

    let daemon = Arc::new(DaemonState::new(
        paths.clone(),
        config,
        shared.clone(),
        mail.clone(),
        services,
        fpm.clone(),
    ));

    // Write the pidfile so `grove stop/restart` can find us, and arrange for it
    // (and the socket) to be cleaned up on graceful shutdown.
    write_pidfile(&paths)?;

    // Translate OS signals into a graceful shutdown notification.
    spawn_signal_handler(daemon.shutdown.clone());

    // Auto-discover Docker / OrbStack containers as `*.test` sites and keep the
    // registry in sync as containers come and go.
    let mut docker_task = None;
    if general.docker {
        let daemon = daemon.clone();
        docker_task = Some(tokio::spawn(async move {
            let mut current: Vec<docker::DockerContainer> = Vec::new();
            loop {
                let found = docker::discover().await;
                if found != current {
                    let n = found.len();
                    *daemon.docker_sites.lock().await = found.clone();
                    current = found;
                    match daemon.reload().await {
                        Ok(_) => tracing::info!(containers = n, "docker sites updated"),
                        Err(e) => tracing::warn!(error = %e, "docker registry reload"),
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            }
        }));
    }

    // Spawn network listeners. A failure to bind a privileged port does not
    // abort the others, so e.g. DNS can still work without root — but it is
    // *recorded*, not just logged. IPC comes up regardless of these binds, and
    // before this a daemon that had lost port 80 to Apache still answered
    // `grove status` with every light green.
    let mut tasks = Vec::new();

    {
        let tld = general.tld.clone();
        let dns_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, general.dns_port));
        let listeners = daemon.listeners.clone();
        tasks.push(tokio::spawn(async move {
            match grove_dns::serve(&tld, dns_addr).await {
                Ok(mut server) => {
                    listeners.set_dns(ListenerHealth::Up);
                    if let Err(e) = server.block_until_done().await {
                        tracing::error!(error = %e, "DNS server stopped");
                        listeners.set_dns(ListenerHealth::Failed(e.to_string()));
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, %dns_addr, "failed to start DNS");
                    listeners.set_dns(ListenerHealth::Failed(e.to_string()));
                }
            }
        }));
    }

    {
        let http_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, general.http_port));
        match grove_proxy::bind(http_addr).await {
            Ok(listener) => {
                daemon.listeners.set_http(ListenerHealth::Up);
                let shared = shared.clone();
                let fpm = fpm.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = grove_proxy::serve_http_on(listener, shared, fpm).await {
                        tracing::error!(error = %e, "HTTP server stopped");
                    }
                }));
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to bind HTTP");
                daemon
                    .listeners
                    .set_http(ListenerHealth::Failed(bind_reason(&e)));
            }
        }
    }

    {
        let https_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, general.https_port));
        match grove_proxy::bind(https_addr).await {
            Ok(listener) => {
                daemon.listeners.set_https(ListenerHealth::Up);
                let shared = shared.clone();
                let fpm = fpm.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = grove_proxy::serve_https_on(listener, shared, fpm, sni).await {
                        tracing::error!(error = %e, "HTTPS server stopped");
                    }
                }));
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to bind HTTPS");
                daemon
                    .listeners
                    .set_https(ListenerHealth::Failed(bind_reason(&e)));
            }
        }
    }

    if general_services.mail_enabled {
        let mail_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, general_services.mail_port));
        match grove_services::bind_smtp(mail_addr).await {
            Ok(listener) => {
                daemon.listeners.set_mail(ListenerHealth::Up);
                let mail = mail.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = grove_services::serve_smtp_on(listener, mail).await {
                        tracing::error!(error = %e, %mail_addr, "mail-catcher stopped");
                    }
                }));
            }
            Err(e) => {
                tracing::error!(error = %e, %mail_addr, "failed to bind mail-catcher");
                daemon
                    .listeners
                    .set_mail(ListenerHealth::Failed(e.to_string()));
            }
        }
    }

    // IPC listener (foreground task). Returns when shutdown is requested.
    let dev = daemon.dev.clone();
    let daemon_for_shutdown = daemon.clone();
    ipc::serve(paths.ipc_socket(), daemon).await?;

    // Shut down in an order that leaves nothing behind. Before this, only the
    // dev processes were stopped explicitly; php-fpm pools and databases were
    // left to the runtime's `Drop`s during teardown, the docker poller held its
    // `Arc<DaemonState>` forever, and the listeners were aborted mid-request.
    dev.stop_all().await;
    daemon_for_shutdown.fpm.stop_all();
    daemon_for_shutdown.services.stop_all_processes();
    if let Some(t) = docker_task {
        t.abort();
    }
    // Stop accepting. In-flight requests run on their own tasks, so aborting
    // the accept loops does not cut them off; give them a moment to finish
    // before the runtime is torn down. A real drain would track them — this
    // is the bounded, honest version.
    for t in tasks {
        t.abort();
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = std::fs::remove_file(paths.pid_file());
    tracing::info!("groved stopped");
    Ok(())
}

/// The OS's reason for a failed bind, without the address the caller already
/// knows: "Address already in use (os error 48)".
fn bind_reason(e: &grove_proxy::server::ServerError) -> String {
    match e {
        grove_proxy::server::ServerError::Bind { source, .. } => source.to_string(),
    }
}

/// Refuse to start over a daemon that is already running, and clear a pidfile
/// that no longer names one.
///
/// `grove start` only probed the socket. A second `grove daemon` — from a
/// launchd restart racing a manual start, say — unlinked the live socket,
/// overwrote the pidfile, and then merely logged its failed binds. The pid
/// is only trusted when the process it names is alive *and* is a grove binary:
/// pids are recycled, and the file may be from a machine that has rebooted.
fn guard_single_instance(paths: &GrovePaths) -> anyhow::Result<()> {
    use grove_core::process;
    let file = paths.pid_file();
    let Some(pid) = process::read_pid_file(&file) else {
        return Ok(());
    };
    if pid == std::process::id() {
        return Ok(());
    }
    if process::is_alive_and_named(pid, "grove") {
        anyhow::bail!(
            "another Grove daemon is already running (pid {pid}, per {}). \
             Run `grove stop` first, or `grove restart`.",
            file.display()
        );
    }
    tracing::info!(pid, file = %file.display(), "removing stale pidfile from a previous run");
    let _ = std::fs::remove_file(&file);
    Ok(())
}

fn write_pidfile(paths: &GrovePaths) -> anyhow::Result<()> {
    paths.ensure()?;
    grove_core::securefs::write_public(&paths.pid_file(), std::process::id().to_string())?;
    Ok(())
}

/// On Unix, listen for SIGTERM/SIGINT and convert them into a graceful
/// shutdown. On other platforms, fall back to Ctrl-C.
fn spawn_signal_handler(shutdown: std::sync::Arc<tokio::sync::Notify>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
            let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
            tokio::select! {
                _ = term.recv() => tracing::info!("received SIGTERM"),
                _ = int.recv() => tracing::info!("received SIGINT"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("received Ctrl-C");
        }
        shutdown.notify_waiters();
    });
}

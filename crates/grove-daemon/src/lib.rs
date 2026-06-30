//! grove-daemon — the single long-running process.
//!
//! Binds the privileged ports (DNS 53, HTTP 80, HTTPS 443), supervises FPM
//! pools, and exposes an IPC endpoint the CLI/GUI drive. The CLI and GUI are
//! thin clients; all stateful logic lives here.

pub mod commands;
pub mod ipc;
pub mod logs;
pub mod state;
pub mod tunnels;

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

    // Build the site registry and shared proxy state.
    let registry = grove_core::SiteRegistry::build(&config);
    tracing::info!(sites = registry.len(), tld = %general.tld, "registry built");
    let shared = SharedState::new(registry);

    // Local CA + SNI resolver for HTTPS.
    let ca = Arc::new(CertificateAuthority::load_or_create(&paths)?);
    let sni = Arc::new(SniResolver::new(ca.clone(), paths.clone()));

    // Built-in mail-catcher store, shared with the SMTP listener + IPC queries.
    let mail = grove_services::MailStore::new();

    // Bundled service supervisor (downloads + runs PostgreSQL, …).
    let services = Arc::new(grove_services::ServiceManager::new(paths.clone()));
    // Auto-start only services that are installed and were left running.
    services.autostart_installed();

    let daemon = Arc::new(DaemonState::new(
        paths.clone(),
        config,
        shared.clone(),
        mail.clone(),
        services,
    ));

    // Write the pidfile so `grove stop/restart` can find us, and arrange for it
    // (and the socket) to be cleaned up on graceful shutdown.
    write_pidfile(&paths)?;

    // Translate OS signals into a graceful shutdown notification.
    spawn_signal_handler(daemon.shutdown.clone());

    // Spawn network listeners. A failure to bind a privileged port is logged but
    // does not abort the others, so e.g. DNS can still work without root.
    let mut tasks = Vec::new();

    {
        let tld = general.tld.clone();
        let dns_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, general.dns_port));
        tasks.push(tokio::spawn(async move {
            match grove_dns::serve(&tld, dns_addr).await {
                Ok(mut server) => {
                    if let Err(e) = server.block_until_done().await {
                        tracing::error!(error = %e, "DNS server stopped");
                    }
                }
                Err(e) => tracing::error!(error = %e, %dns_addr, "failed to start DNS"),
            }
        }));
    }

    {
        let http_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, general.http_port));
        let shared = shared.clone();
        let fpm = fpm.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = grove_proxy::serve_http(http_addr, shared, fpm).await {
                tracing::error!(error = %e, "HTTP server stopped");
            }
        }));
    }

    {
        let https_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, general.https_port));
        let shared = shared.clone();
        let fpm = fpm.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = grove_proxy::serve_https(https_addr, shared, fpm, sni).await {
                tracing::error!(error = %e, "HTTPS server stopped");
            }
        }));
    }

    if general_services.mail_enabled {
        let mail_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, general_services.mail_port));
        let mail = mail.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = grove_services::serve_smtp(mail_addr, mail).await {
                tracing::error!(error = %e, %mail_addr, "mail-catcher stopped");
            }
        }));
    }

    // IPC listener (foreground task). Returns when shutdown is requested.
    ipc::serve(paths.ipc_socket(), daemon).await?;

    for t in tasks {
        t.abort();
    }
    let _ = std::fs::remove_file(paths.pid_file());
    tracing::info!("groved stopped");
    Ok(())
}

fn write_pidfile(paths: &GrovePaths) -> anyhow::Result<()> {
    paths.ensure()?;
    std::fs::write(paths.pid_file(), std::process::id().to_string())?;
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

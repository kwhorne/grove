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
        Command::Daemon | Command::Ca { .. } | Command::Php { .. } => {
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

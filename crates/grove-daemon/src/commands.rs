//! Maps IPC `Request`s to mutations on `DaemonState`.

use std::path::PathBuf;
use std::sync::Arc;

use grove_core::config::SiteConfig;
use grove_core::registry::name_from_path;
use grove_core::Config;
use grove_ipc::protocol::{
    DaemonStatus, DiagnosticEntry, DiagnosticStatus, Request, Response, ResponseData, ServiceState,
    SiteStatus,
};

use crate::state::DaemonState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Execute one request against daemon state, returning the response to send.
pub async fn dispatch(state: &Arc<DaemonState>, req: Request) -> Response {
    match handle(state, req).await {
        Ok(resp) => resp,
        Err(e) => Response::err(e.to_string()),
    }
}

async fn handle(state: &Arc<DaemonState>, req: Request) -> anyhow::Result<Response> {
    match req {
        Request::Ping => Ok(Response::ok(ResponseData::Pong {
            version: VERSION.to_string(),
        })),

        Request::Status => {
            let config = state.config.lock().await;
            let registry = state.shared.registry.read().await;
            let status = DaemonStatus {
                version: VERSION.to_string(),
                tld: config.general.tld.clone(),
                http_port: config.general.http_port,
                https_port: config.general.https_port,
                dns_port: config.general.dns_port,
                site_count: registry.len(),
                services: vec![ServiceState {
                    name: "dns".into(),
                    running: true,
                    port: Some(config.general.dns_port),
                }],
            };
            Ok(Response::ok(ResponseData::Status(status)))
        }

        Request::ListSites => {
            let registry = state.shared.registry.read().await;
            let sites: Vec<SiteStatus> = registry
                .iter()
                .map(|s| SiteStatus { site: s.clone() })
                .collect();
            Ok(Response::ok(ResponseData::Sites(sites)))
        }

        Request::Park { path } => {
            let expanded = Config::expand(&PathBuf::from(&path));
            if !expanded.is_dir() {
                return Ok(Response::err(format!("{path} is not a directory")));
            }
            {
                let mut config = state.config.lock().await;
                config.add_parked(PathBuf::from(path.clone()));
            }
            let n = state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "parked {path} — {n} sites now served"
            ))))
        }

        Request::Unpark { path } => {
            let target = Config::expand(&PathBuf::from(&path));
            {
                let mut config = state.config.lock().await;
                config
                    .parked
                    .retain(|p| Config::expand(&p.path) != target);
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!("unparked {path}"))))
        }

        Request::Link { path, name } => {
            let expanded = Config::expand(&PathBuf::from(&path));
            if !expanded.is_dir() {
                return Ok(Response::err(format!("{path} is not a directory")));
            }
            let site_name = name
                .or_else(|| name_from_path(&expanded))
                .ok_or_else(|| anyhow::anyhow!("could not derive a site name from {path}"))?;
            {
                let mut config = state.config.lock().await;
                config.remove_site(&site_name);
                config.add_site(SiteConfig {
                    name: site_name.clone(),
                    path: Some(PathBuf::from(path)),
                    php: None,
                    secure: false,
                    driver: None,
                    proxy_to: None,
                })?;
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "linked {site_name}"
            ))))
        }

        Request::Unlink { name } => {
            let removed = {
                let mut config = state.config.lock().await;
                config.remove_site(&name)
            };
            if !removed {
                return Ok(Response::err(format!("no linked site named {name}")));
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!("unlinked {name}"))))
        }

        Request::Secure { name, enable } => {
            mutate_site(state, &name, |sc| sc.secure = enable).await?;
            let verb = if enable { "secured" } else { "unsecured" };
            Ok(Response::ok(ResponseData::Message(format!("{verb} {name}"))))
        }

        Request::Isolate { name, version } => {
            mutate_site(state, &name, |sc| sc.php = version.clone()).await?;
            let msg = match version {
                Some(v) => format!("{name} isolated to php@{v}"),
                None => format!("{name} reverted to default PHP"),
            };
            Ok(Response::ok(ResponseData::Message(msg)))
        }

        Request::Proxy { name, url } => {
            {
                let mut config = state.config.lock().await;
                config.remove_site(&name);
                config.add_site(SiteConfig {
                    name: name.clone(),
                    path: None,
                    php: None,
                    secure: false,
                    driver: Some(grove_core::Driver::Proxy),
                    proxy_to: Some(url.clone()),
                })?;
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "{name} → {url}"
            ))))
        }

        Request::Reload => {
            let n = state.reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "reloaded — {n} sites"
            ))))
        }

        Request::Doctor => Ok(Response::ok(ResponseData::Doctor(doctor(state).await))),
    }
}

/// Mutate an explicit site. If the named site only exists via parking, promote
/// it to an explicit `[[sites]]` entry first so the override persists.
async fn mutate_site(
    state: &Arc<DaemonState>,
    name: &str,
    f: impl FnOnce(&mut SiteConfig),
) -> anyhow::Result<()> {
    {
        let mut config = state.config.lock().await;
        if config.find_site(name).is_none() {
            // Promote from parked discovery.
            let registry = state.shared.registry.read().await;
            let resolved = registry
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("no site named {name}"))?;
            let promoted = SiteConfig {
                name: name.to_string(),
                path: Some(resolved.path.clone()),
                php: None,
                secure: resolved.secure,
                driver: Some(resolved.driver),
                proxy_to: resolved.proxy_to.clone(),
            };
            drop(registry);
            config.add_site(promoted)?;
        }
        if let Some(sc) = config.find_site_mut(name) {
            f(sc);
        }
    }
    state.persist_and_reload().await?;
    Ok(())
}

async fn doctor(state: &Arc<DaemonState>) -> Vec<DiagnosticEntry> {
    let mut out = Vec::new();
    let config = state.config.lock().await;

    out.push(DiagnosticEntry {
        check: "config".into(),
        status: DiagnosticStatus::Pass,
        detail: format!("loaded from {}", state.paths.config_file().display()),
    });

    let ca = state.paths.ca_cert();
    out.push(DiagnosticEntry {
        check: "root-ca".into(),
        status: if ca.exists() {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Warn
        },
        detail: if ca.exists() {
            format!("present at {}", ca.display())
        } else {
            "no root CA generated yet".into()
        },
    });

    out.push(DiagnosticEntry {
        check: "privileges".into(),
        status: if grove_os::is_elevated() || config.general.http_port > 1024 {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Warn
        },
        detail: format!(
            "http_port={}, elevated={}",
            config.general.http_port,
            grove_os::is_elevated()
        ),
    });

    out
}

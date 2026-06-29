//! Maps IPC `Request`s to mutations on `DaemonState`.

use std::path::PathBuf;
use std::sync::Arc;

use grove_core::config::SiteConfig;
use grove_core::registry::name_from_path;
use grove_core::Config;
use grove_ipc::protocol::{
    DaemonStatus, DiagnosticEntry, DiagnosticStatus, Request, Response, ResponseData, ServiceState,
    SettingsView, SiteStatus,
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
                services: vec![
                    ServiceState {
                        name: "dns".into(),
                        running: true,
                        port: Some(config.general.dns_port),
                    },
                    ServiceState {
                        name: "mail".into(),
                        running: config.services.mail_enabled,
                        port: Some(config.services.mail_port),
                    },
                ],
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
                config.parked.retain(|p| Config::expand(&p.path) != target);
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "unparked {path}"
            ))))
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
            Ok(Response::ok(ResponseData::Message(format!(
                "unlinked {name}"
            ))))
        }

        Request::Secure { name, enable } => {
            mutate_site(state, &name, |sc| sc.secure = enable).await?;
            let verb = if enable { "secured" } else { "unsecured" };
            Ok(Response::ok(ResponseData::Message(format!(
                "{verb} {name}"
            ))))
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

        Request::SetDefaultPhp { version } => {
            {
                let mut config = state.config.lock().await;
                config.general.default_php = version.clone();
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "default PHP set to php@{version}"
            ))))
        }

        Request::GetSettings => {
            let config = state.config.lock().await;
            let registry = state.shared.registry.read().await;
            let php_versions: Vec<String> = {
                let reg = grove_runtime::PhpRegistry::load(&state.paths);
                reg.iter().map(|b| b.version.clone()).collect()
            };
            let _ = &registry;
            let view = SettingsView {
                tld: config.general.tld.clone(),
                default_php: config.general.default_php.clone(),
                auto_start: config.general.auto_start,
                http_port: config.general.http_port,
                https_port: config.general.https_port,
                dns_port: config.general.dns_port,
                mail_enabled: config.services.mail_enabled,
                mail_port: config.services.mail_port,
                parked: config
                    .parked
                    .iter()
                    .map(|p| p.path.to_string_lossy().into_owned())
                    .collect(),
                php_versions,
            };
            Ok(Response::ok(ResponseData::Settings(view)))
        }

        Request::UpdateSettings { patch } => {
            {
                let mut config = state.config.lock().await;
                if let Some(v) = patch.tld {
                    config.general.tld = v;
                }
                if let Some(v) = patch.default_php {
                    config.general.default_php = v;
                }
                if let Some(v) = patch.auto_start {
                    config.general.auto_start = v;
                }
                if let Some(v) = patch.http_port {
                    config.general.http_port = v;
                }
                if let Some(v) = patch.https_port {
                    config.general.https_port = v;
                }
                if let Some(v) = patch.dns_port {
                    config.general.dns_port = v;
                }
                if let Some(v) = patch.mail_enabled {
                    config.services.mail_enabled = v;
                }
                if let Some(v) = patch.mail_port {
                    config.services.mail_port = v;
                }
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message("settings saved".into())))
        }

        Request::PhpInstall { version } => {
            let paths = state.paths.clone();
            let build = tokio::task::spawn_blocking(move || {
                let mut reg = grove_runtime::PhpRegistry::load(&paths);
                grove_runtime::install_php(&paths, &mut reg, &version, |_| {})
            })
            .await
            .map_err(|e| anyhow::anyhow!("install task panicked: {e}"))?
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "installed php@{}",
                build.version
            ))))
        }

        Request::ServiceList => Ok(Response::ok(ResponseData::Services(
            state.services.status_all(),
        ))),

        Request::ServiceInstall { key } => {
            let services = state.services.clone();
            tokio::task::spawn_blocking(move || services.install(&key, |_| {}))
                .await
                .map_err(|e| anyhow::anyhow!("install task panicked: {e}"))?
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(
                "service installed".into(),
            )))
        }

        Request::ServiceStart { key } => {
            state
                .services
                .start(&key)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "started {key}"
            ))))
        }

        Request::ServiceStop { key } => {
            state
                .services
                .stop(&key)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "stopped {key}"
            ))))
        }

        Request::Reload => {
            let n = state.reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "reloaded — {n} sites"
            ))))
        }

        Request::MailList => Ok(Response::ok(ResponseData::Mail(state.mail.summaries()))),

        Request::MailGet { id } => Ok(Response::ok(ResponseData::MailMessage(state.mail.get(id)))),

        Request::MailClear => {
            let n = state.mail.clear();
            Ok(Response::ok(ResponseData::Message(format!(
                "cleared {n} email(s)"
            ))))
        }

        Request::Doctor => Ok(Response::ok(ResponseData::Doctor(doctor(state).await))),

        Request::Shutdown => {
            state.request_shutdown();
            Ok(Response::ok(ResponseData::Message("shutting down".into())))
        }
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

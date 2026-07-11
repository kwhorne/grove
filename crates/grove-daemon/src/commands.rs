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
use grove_tunnel::ShareConfig;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the current Xdebug state + per-build availability.
async fn xdebug_status(state: &Arc<DaemonState>) -> grove_ipc::protocol::XdebugStatus {
    use grove_ipc::protocol::{XdebugBuild, XdebugStatus};
    use grove_runtime::xdebug::{self, XdebugPlan};
    let (enabled, port) = {
        let c = state.config.lock().await;
        (c.general.xdebug, c.general.xdebug_port)
    };
    let reg = grove_runtime::PhpRegistry::load(&state.paths);
    let builds = reg
        .iter()
        .map(|b| {
            let plan = xdebug::resolve(&state.paths, b);
            let ready = !matches!(plan, XdebugPlan::Unavailable);
            XdebugBuild {
                version: b.version.clone(),
                availability: xdebug::availability_label(&state.paths, b),
                ready,
            }
        })
        .collect();
    XdebugStatus {
        enabled,
        port,
        builds,
    }
}

/// Open a public tunnel for a site using the configured tunnel server.
async fn tunnel_start(
    state: &Arc<DaemonState>,
    site: String,
    subdomain: Option<String>,
    basic_auth: Option<String>,
) -> anyhow::Result<Response> {
    use std::net::SocketAddr;
    let (server, token, local_host, local_addr) = {
        let config = state.config.lock().await;
        let tld = config.general.tld.clone();
        let Some(server) = config.tunnel.server.clone() else {
            return Ok(Response::err(
                "no tunnel server configured — set [tunnel].server in config.toml",
            ));
        };
        let token = config.tunnel.token.clone().unwrap_or_default();
        let name = site
            .trim()
            .trim_end_matches(&format!(".{tld}"))
            .to_lowercase();
        if name.is_empty() {
            return Ok(Response::err("missing site name"));
        }
        let local_host = format!("{name}.{tld}");
        let local_addr: SocketAddr = format!("127.0.0.1:{}", config.general.http_port).parse()?;
        (server, token, local_host, local_addr)
    };

    let cfg = ShareConfig {
        server,
        token,
        subdomain,
        local_host: local_host.clone(),
        local_addr,
        basic_auth,
    };
    match state.tunnels.start(local_host, cfg).await {
        Ok(status) => Ok(Response::ok(ResponseData::Tunnels(vec![status]))),
        Err(e) => Ok(Response::err(e.to_string())),
    }
}

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
                    node: None,
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

        Request::ForgetSite { name } => {
            {
                let mut config = state.config.lock().await;
                // Drop any explicit [[sites]] entry too, then hide by name.
                config.remove_site(&name);
                if !config.ignored.iter().any(|n| n == &name) {
                    config.ignored.push(name.clone());
                }
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "removed {name} from the list (files kept)"
            ))))
        }

        Request::UnforgetSite { name } => {
            {
                let mut config = state.config.lock().await;
                config.ignored.retain(|n| n != &name);
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "restored {name}"
            ))))
        }

        Request::CreateSite {
            name,
            parent,
            kind,
            php,
            init_git,
        } => {
            let parent_dir = Config::expand(&PathBuf::from(&parent));
            let target = parent_dir.join(&name);
            let php_version = match php {
                Some(v) => v,
                None => state.config.lock().await.general.default_php.clone(),
            };
            let paths = state.paths.clone();
            let target_for_task = target.clone();
            let name_for_task = name.clone();
            tokio::task::spawn_blocking(move || {
                // Laravel kinds map to `laravel new` starter kits; "static" is a
                // plain site; anything else is a community kit (`--using`).
                let kit: Option<Option<&str>> = match kind.as_str() {
                    "static" => None,
                    "laravel" => Some(None),
                    other => Some(Some(other)),
                };
                match kit {
                    Some(kit) => grove_runtime::scaffold::new_laravel(
                        &paths,
                        &php_version,
                        &target_for_task,
                        kit,
                        init_git,
                        |_| {},
                    )
                    .map_err(|e| e.to_string()),
                    None => grove_runtime::scaffold::new_static(&target_for_task, &name_for_task)
                        .map_err(|e| e.to_string()),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("scaffold task panicked: {e}"))?
            .map_err(|e| anyhow::anyhow!(e))?;

            {
                let mut config = state.config.lock().await;
                config.remove_site(&name);
                config.add_site(SiteConfig {
                    name: name.clone(),
                    path: Some(target.clone()),
                    php: None,
                    node: None,
                    secure: false,
                    driver: None,
                    proxy_to: None,
                })?;
            }
            state.persist_and_reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "created {name} at {}",
                target.display()
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

        Request::SiteNode { name, version } => {
            mutate_site(state, &name, |sc| sc.node = version.clone()).await?;
            let msg = match version {
                Some(v) => format!("{name} pinned to node@{v}"),
                None => format!("{name} node version cleared"),
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
                    node: None,
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

        Request::ServiceRestart { key } => {
            state
                .services
                .restart(&key)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "restarted {key}"
            ))))
        }

        Request::ServiceSetPort { key, port } => {
            state
                .services
                .set_port(&key, port)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "{key} port set to {port} (restart the service to apply)"
            ))))
        }

        Request::Reload => {
            let n = state.reload().await?;
            Ok(Response::ok(ResponseData::Message(format!(
                "reloaded — {n} sites"
            ))))
        }

        Request::Debug { enable } => {
            let port = { state.config.lock().await.general.xdebug_port };
            if let Some(on) = enable {
                {
                    let mut config = state.config.lock().await;
                    config.general.xdebug = on;
                }
                state.persist_and_reload().await?;
                state.fpm.set_xdebug(on, port);
                // Respawn pools so the change takes effect immediately.
                state.fpm.reload_pools();
            }
            Ok(Response::ok(ResponseData::Xdebug(
                xdebug_status(state).await,
            )))
        }

        Request::MailList => Ok(Response::ok(ResponseData::Mail(state.mail.summaries()))),

        Request::MailGet { id } => Ok(Response::ok(ResponseData::MailMessage(state.mail.get(id)))),

        Request::MailClear => {
            let n = state.mail.clear();
            Ok(Response::ok(ResponseData::Message(format!(
                "cleared {n} email(s)"
            ))))
        }

        Request::EnvSnippet { site } => {
            let mail = {
                let config = state.config.lock().await;
                (config.services.mail_enabled, config.services.mail_port)
            };
            let services = state.services.status_all();
            let snippet = build_env_snippet(&services, mail.0, mail.1, site.as_deref());
            Ok(Response::ok(ResponseData::Message(snippet)))
        }

        Request::LogSources => {
            let config = state.config.lock().await;
            let registry = state.shared.registry.read().await;
            let sources = crate::logs::discover(&config, &registry, &state.paths);
            Ok(Response::ok(ResponseData::LogSources(sources)))
        }

        Request::LogEntries { path, limit } => {
            // Only allow reading files Grove itself discovered.
            let source = {
                let config = state.config.lock().await;
                let registry = state.shared.registry.read().await;
                crate::logs::discover(&config, &registry, &state.paths)
                    .into_iter()
                    .find(|s| s.path == path)
            };
            let Some(source) = source else {
                return Ok(Response::err("unknown log source"));
            };
            let entries = crate::logs::read_entries(
                std::path::Path::new(&source.path),
                &source.kind,
                limit.clamp(1, 1000),
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::LogEntries(entries)))
        }

        Request::PhpVersionList => {
            let reg = grove_runtime::PhpRegistry::load(&state.paths);
            let mut majors: Vec<String> = grove_runtime::install::OFFERED_MAJORS
                .iter()
                .map(|s| s.to_string())
                .collect();
            for b in reg.iter() {
                if !majors.contains(&b.version) {
                    majors.push(b.version.clone());
                }
            }
            let versions = majors
                .into_iter()
                .map(|m| {
                    let installed = reg.get(&m);
                    grove_ipc::protocol::NodeVersion {
                        major: m.clone(),
                        installed: installed.is_some(),
                        version: installed.map(|b| b.version.clone()),
                    }
                })
                .collect();
            Ok(Response::ok(ResponseData::PhpVersions(versions)))
        }

        Request::NodeList => {
            let reg = grove_runtime::NodeRegistry::load(&state.paths);
            let mut majors: Vec<String> = grove_runtime::node::OFFERED_MAJORS
                .iter()
                .map(|s| s.to_string())
                .collect();
            for b in reg.iter() {
                if !majors.contains(&b.major) {
                    majors.push(b.major.clone());
                }
            }
            let nodes = majors
                .into_iter()
                .map(|m| {
                    let installed = reg.get(&m);
                    grove_ipc::protocol::NodeVersion {
                        major: m.clone(),
                        installed: installed.is_some(),
                        version: installed.map(|b| b.version.clone()),
                    }
                })
                .collect();
            Ok(Response::ok(ResponseData::Nodes(nodes)))
        }

        Request::NodeInstall { version } => {
            let paths = state.paths.clone();
            let build = tokio::task::spawn_blocking(move || {
                let mut reg = grove_runtime::NodeRegistry::load(&paths);
                grove_runtime::install_node(&paths, &mut reg, &version, |_| {})
            })
            .await
            .map_err(|e| anyhow::anyhow!("install task panicked: {e}"))?
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "installed Node v{}",
                build.version
            ))))
        }

        Request::ProvisionToolchain { php } => {
            let paths = state.paths.clone();
            let default_php = state.config.lock().await.general.default_php.clone();
            let version = php.unwrap_or(default_php);
            let summary = tokio::task::spawn_blocking(move || {
                let mut done: Vec<String> = Vec::new();
                match grove_runtime::install::install_cli(&paths, &version, |_| {}) {
                    Ok(_) => done.push(format!("PHP {version} CLI")),
                    Err(e) => done.push(format!("PHP CLI failed: {e}")),
                }
                match grove_runtime::scaffold::ensure_composer(&paths) {
                    Ok(_) => done.push("Composer".into()),
                    Err(e) => done.push(format!("Composer failed: {e}")),
                }
                let mut reg = grove_runtime::NodeRegistry::load(&paths);
                if reg.iter().next().is_none() {
                    match grove_runtime::install_node(&paths, &mut reg, "22", |_| {}) {
                        Ok(_) => {
                            let _ = reg.save(&paths);
                            done.push("Node 22".into());
                        }
                        Err(e) => done.push(format!("Node failed: {e}")),
                    }
                } else {
                    done.push("Node (already installed)".into());
                }
                done.join(", ")
            })
            .await
            .map_err(|e| anyhow::anyhow!("provision task panicked: {e}"))?;
            Ok(Response::ok(ResponseData::Message(format!(
                "provisioned toolchain: {summary}"
            ))))
        }

        Request::DbSnapshot {
            engine,
            database,
            note,
        } => {
            let paths = state.paths.clone();
            let services = state.services.clone();
            let snap = tokio::task::spawn_blocking(move || {
                let store = grove_services::SnapshotStore::new(&paths);
                store.create(
                    &services,
                    &engine,
                    database.as_deref(),
                    note.as_deref().unwrap_or(""),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("snapshot task panicked: {e}"))?;
            match snap {
                Ok(s) => Ok(Response::ok(ResponseData::Message(format!(
                    "snapshot {} created ({}, {}, {} bytes)",
                    s.id, s.engine, s.database, s.bytes
                )))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::DbSnapshotList => {
            let store = grove_services::SnapshotStore::new(&state.paths);
            Ok(Response::ok(ResponseData::Snapshots(store.list())))
        }
        Request::DbDumpFile {
            engine,
            database,
            path,
        } => {
            let services = state.services.clone();
            let out = std::path::PathBuf::from(path);
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                match engine.as_str() {
                    "mysql" => services
                        .snapshot_mysql(database.as_deref(), &out)
                        .map_err(|e| anyhow::anyhow!(e.to_string())),
                    "postgres" => {
                        let db = database
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("postgres needs a database name"))?;
                        services
                            .snapshot_postgres(db, &out)
                            .map_err(|e| anyhow::anyhow!(e.to_string()))
                    }
                    other => Err(anyhow::anyhow!("cannot dump engine {other}")),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("dump task panicked: {e}"))?;
            match r {
                Ok(()) => Ok(Response::ok(ResponseData::Message("dumped".into()))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::DbRestoreFile { engine, path } => {
            let services = state.services.clone();
            let p = std::path::PathBuf::from(path);
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                match engine.as_str() {
                    "mysql" => services
                        .restore_mysql(&p)
                        .map_err(|e| anyhow::anyhow!(e.to_string())),
                    "postgres" => services
                        .restore_postgres(&p)
                        .map_err(|e| anyhow::anyhow!(e.to_string())),
                    other => Err(anyhow::anyhow!("cannot restore engine {other}")),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("restore task panicked: {e}"))?;
            match r {
                Ok(()) => Ok(Response::ok(ResponseData::Message("restored".into()))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::RequestLog { site, limit } => {
            let limit = if limit == 0 { 100 } else { limit.min(500) };
            let entries = state.shared.log.snapshot(site.as_deref(), limit);
            Ok(Response::ok(ResponseData::Requests(entries)))
        }
        Request::RequestDetail { id } => Ok(Response::ok(ResponseData::RequestDetail(
            state.shared.log.detail(id),
        ))),
        Request::ReplayRequest { id } => match state.shared.log.captured(id) {
            None => Ok(Response::err(format!("no request with id {id}"))),
            Some(cap) => {
                let port = state.config.lock().await.general.http_port;
                match grove_proxy::replay(port, &cap).await {
                    Ok((status, duration_ms)) => Ok(Response::ok(ResponseData::Replayed {
                        status,
                        duration_ms,
                    })),
                    Err(e) => Ok(Response::err(format!("replay failed: {e}"))),
                }
            }
        },
        Request::LicenseActivate { key } => match crate::license::activate(&state.paths, &key) {
            Ok(claims) => Ok(Response::ok(ResponseData::License(Some(claims)))),
            Err(e) => Ok(Response::err(format!("could not activate license: {e}"))),
        },
        Request::LicenseStatus => Ok(Response::ok(ResponseData::License(
            crate::license::current(&state.paths),
        ))),
        Request::LicenseDeactivate => {
            crate::license::deactivate(&state.paths)
                .map_err(|e| anyhow::anyhow!("removing license: {e}"))?;
            Ok(Response::ok(ResponseData::License(None)))
        }
        Request::DbSnapshotRestore { id } => {
            let paths = state.paths.clone();
            let services = state.services.clone();
            let res = tokio::task::spawn_blocking(move || {
                grove_services::SnapshotStore::new(&paths).restore(&services, &id)
            })
            .await
            .map_err(|e| anyhow::anyhow!("restore task panicked: {e}"))?;
            match res {
                Ok(s) => Ok(Response::ok(ResponseData::Message(format!(
                    "restored snapshot {} into {} ({})",
                    s.id, s.engine, s.database
                )))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::DbSnapshotRemove { id } => {
            let store = grove_services::SnapshotStore::new(&state.paths);
            match store.remove(&id) {
                Ok(s) => Ok(Response::ok(ResponseData::Message(format!(
                    "deleted snapshot {}",
                    s.id
                )))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }

        Request::TunnelStart {
            site,
            subdomain,
            basic_auth,
        } => tunnel_start(state, site, subdomain, basic_auth).await,
        Request::TunnelStop { site } => match state.tunnels.stop(&site).await {
            Ok(()) => Ok(Response::ok(ResponseData::Message(format!(
                "stopped sharing {site}"
            )))),
            Err(e) => Ok(Response::err(e.to_string())),
        },
        Request::TunnelList => Ok(Response::ok(ResponseData::Tunnels(
            state.tunnels.list().await,
        ))),
        Request::TunnelRequests { site } => Ok(Response::ok(ResponseData::TunnelRequests(
            state.tunnels.requests(site.as_deref()).await,
        ))),
        Request::MysqlMigrate {
            host,
            port,
            user,
            password,
        } => {
            let services = state.services.clone();
            let res = tokio::task::spawn_blocking(move || {
                services.migrate_mysql(&host, port, &user, &password, |_| {})
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
            match res {
                Ok(msg) => Ok(Response::ok(ResponseData::Message(msg))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::DevStart { site } => {
            let resolved = {
                let registry = state.shared.registry.read().await;
                registry.get(&site).cloned()
            };
            let Some(resolved) = resolved else {
                return Ok(Response::err(format!("no site named {site}")));
            };
            match state.dev.start(&resolved, &state.paths).await {
                Ok(names) => Ok(Response::ok(ResponseData::Message(format!(
                    "dev started for {site}: {}",
                    names.join(", ")
                )))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::DevStop { site } => match state.dev.stop(&site).await {
            Ok(()) => Ok(Response::ok(ResponseData::Message(format!(
                "dev stopped for {site}"
            )))),
            Err(e) => Ok(Response::err(e.to_string())),
        },
        Request::DevList => Ok(Response::ok(ResponseData::DevSites(state.dev.list().await))),

        Request::DockerControl { id, action } => {
            match crate::docker::control(&id, &action).await {
                Ok(()) => {
                    // Refresh discovery so the site's state updates promptly.
                    let found = crate::docker::discover().await;
                    *state.docker_sites.lock().await = found;
                    let _ = state.reload().await;
                    Ok(Response::ok(ResponseData::Message(format!(
                        "container {action}ed"
                    ))))
                }
                Err(e) => Ok(Response::err(e)),
            }
        }
        Request::DbConvert { source, target } => {
            match grove_services::convert_database(&source, &target, |_| {}).await {
                Ok(msg) => Ok(Response::ok(ResponseData::Message(msg))),
                Err(e) => Ok(Response::err(e.to_string())),
            }
        }
        Request::Doctor => Ok(Response::ok(ResponseData::Doctor(doctor(state).await))),

        Request::Shutdown => {
            state.request_shutdown();
            Ok(Response::ok(ResponseData::Message("shutting down".into())))
        }

        Request::RestartDaemon => {
            // When installed as a root LaunchDaemon, kickstart re-execs the
            // on-disk binary (picking up an app update) with no password prompt,
            // since we're already root. Delay slightly so this response flushes
            // to the client first. Fall back to a plain shutdown otherwise.
            #[cfg(target_os = "macos")]
            {
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    let ok = std::process::Command::new("launchctl")
                        .args(["kickstart", "-k", "system/com.elyra.grove"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if !ok {
                        // Not a system LaunchDaemon (e.g. dev) — just exit.
                        std::process::exit(0);
                    }
                });
                Ok(Response::ok(ResponseData::Message(
                    "restarting daemon…".into(),
                )))
            }
            #[cfg(not(target_os = "macos"))]
            {
                state.request_shutdown();
                Ok(Response::ok(ResponseData::Message(
                    "restarting daemon…".into(),
                )))
            }
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
                node: resolved.node.clone(),
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

/// Build a Laravel-style `.env` snippet pointing at Grove's bundled services.
fn build_env_snippet(
    services: &[grove_services::ServiceStatus],
    mail_enabled: bool,
    mail_port: u16,
    site: Option<&str>,
) -> String {
    let db_name = site
        .map(|s| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
                .to_lowercase()
        })
        .unwrap_or_else(|| "grove".to_string());

    let installed: Vec<_> = services.iter().filter(|s| s.installed).collect();
    let mut out = String::from("# Generated by `grove env` — Grove's bundled services\n");

    // Pick the primary DB: prefer a running one, else the first installed.
    let db = installed
        .iter()
        .find(|s| s.running && (s.key == "mysql" || s.key == "postgres"))
        .or_else(|| {
            installed
                .iter()
                .find(|s| s.key == "mysql" || s.key == "postgres")
        });

    if let Some(db) = db {
        let conn = if db.key == "postgres" {
            "pgsql"
        } else {
            "mysql"
        };
        out.push_str(&format!(
            "\nDB_CONNECTION={conn}\nDB_HOST={host}\nDB_PORT={port}\nDB_DATABASE={db_name}\nDB_USERNAME={user}\nDB_PASSWORD=\n",
            host = db.host,
            port = db.port,
            user = db.username.clone().unwrap_or_default(),
        ));
    }

    if let Some(redis) = installed.iter().find(|s| s.key == "redis") {
        out.push_str(&format!(
            "\nREDIS_HOST={host}\nREDIS_PORT={port}\nREDIS_PASSWORD=null\n",
            host = redis.host,
            port = redis.port,
        ));
    }

    if mail_enabled {
        out.push_str(&format!(
            "\nMAIL_MAILER=smtp\nMAIL_HOST=127.0.0.1\nMAIL_PORT={mail_port}\nMAIL_USERNAME=null\nMAIL_PASSWORD=null\nMAIL_ENCRYPTION=null\n",
        ));
    }

    if db.is_none() && !installed.iter().any(|s| s.key == "redis") && !mail_enabled {
        out.push_str(
            "\n# No bundled services installed yet. Try `grove service install postgres`.\n",
        );
    }
    out
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

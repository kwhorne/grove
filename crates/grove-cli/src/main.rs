//! `grove` — the CLI frontend. A thin client over the daemon for stateful
//! actions, with a few local-only commands (CA trust, PHP discovery).

mod cli;
mod output;

use anyhow::Context;
use clap::Parser;

use grove_core::paths::GrovePaths;
use grove_ipc::client;
use grove_ipc::protocol::{Request, ResponseData};

use cli::{
    CaAction, Cli, Command, DbAction, DebugAction, DevAction, MailAction, NodeAction, PathAction,
    PhpAction, ServiceAction,
};

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
        Command::Path { action } => {
            let action = action.unwrap_or(PathAction::Show);
            local::path(&paths, action.clone(), args.json)?;
            if matches!(action, PathAction::Install) {
                let socket = paths.ipc_socket();
                if client::is_running(&socket).await {
                    if !args.json {
                        eprintln!("\nProvisioning the bundled toolchain (php, composer, node)…");
                    }
                    let resp =
                        client::send(&socket, &Request::ProvisionToolchain { php: None }).await?;
                    output::print_response(&resp, args.json);
                } else if !args.json {
                    eprintln!(
                        "\nStart Grove to provision the toolchain: `grove start`, then re-run `grove path install`."
                    );
                }
            }
            Ok(())
        }
        Command::Resolve {
            tool,
            dir,
            args: rest,
        } => local::resolve(&paths, &tool, dir, rest),
        Command::Debug {
            action: DebugAction::Env,
        } => local::debug_env(&paths, args.json),

        Command::Env { site } => {
            let socket = paths.ipc_socket();
            if !client::is_running(&socket).await {
                anyhow::bail!("Grove daemon is not running. Start it with `grove start`.");
            }
            let resp = client::send(&socket, &Request::EnvSnippet { site })
                .await
                .context("talking to daemon")?;
            if args.json {
                output::print_response(&resp, true);
            } else if let Some(ResponseData::Message(s)) = resp.data {
                print!("{s}");
            }
            if !resp.ok {
                std::process::exit(1);
            }
            Ok(())
        }

        Command::Logs { target, lines } => {
            let socket = paths.ipc_socket();
            if !client::is_running(&socket).await {
                anyhow::bail!("Grove daemon is not running. Start it with `grove start`.");
            }
            let sources_resp = client::send(&socket, &Request::LogSources).await?;
            match target {
                None => output::print_response(&sources_resp, args.json),
                Some(q) => {
                    let path = match &sources_resp.data {
                        Some(ResponseData::LogSources(list)) => list
                            .iter()
                            .find(|s| s.name.to_lowercase().contains(&q.to_lowercase()))
                            .map(|s| s.path.clone()),
                        _ => None,
                    };
                    let Some(path) = path else {
                        anyhow::bail!("no log source matching {q:?}; run `grove logs` to list");
                    };
                    let resp =
                        client::send(&socket, &Request::LogEntries { path, limit: lines }).await?;
                    output::print_response(&resp, args.json);
                }
            }
            Ok(())
        }

        Command::Gui => lifecycle::gui(&paths).await,
        Command::Start => lifecycle::start(&paths, args.json).await,
        Command::Stop => lifecycle::stop(&paths, args.json).await,
        Command::Restart => lifecycle::restart(&paths, args.json).await,
        Command::Install => lifecycle::install(&paths, args.json),
        Command::Uninstall => lifecycle::uninstall(&paths, args.json),
        Command::Import => lifecycle::import_valet(&paths, args.json),
        Command::Init { php, no_php } => lifecycle::init(&paths, php, no_php, args.json),
        Command::Up {
            path,
            write,
            no_dev,
        } => lifecycle::up(&paths, path, write, no_dev, args.json).await,
        Command::Share {
            site,
            server,
            token,
            subdomain,
            basic_auth,
        } => {
            lifecycle::share(
                &paths, site, server, token, subdomain, basic_auth, args.json,
            )
            .await
        }

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
        Command::Forget { name } => Request::ForgetSite { name },
        Command::Restore { name } => Request::UnforgetSite { name },
        Command::New {
            name,
            kind,
            path,
            php,
            git,
        } => Request::CreateSite {
            name,
            parent: path,
            kind,
            php,
            init_git: git,
        },
        Command::List => Request::ListSites,
        Command::Status => Request::Status,
        Command::Secure { name } => Request::Secure { name, enable: true },
        Command::Unsecure { name } => Request::Secure {
            name,
            enable: false,
        },
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
        Command::Use { version } => Request::SetDefaultPhp { version },
        Command::Mail { action } => match action {
            Some(MailAction::Clear) => Request::MailClear,
            Some(MailAction::Show { id }) => Request::MailGet { id },
            Some(MailAction::List) | None => Request::MailList,
        },
        Command::Node { action } => match action {
            NodeAction::List => Request::NodeList,
            NodeAction::Install { version } => Request::NodeInstall { version },
            NodeAction::Use { site, version } => Request::SiteNode {
                name: site,
                version: Some(version),
            },
            NodeAction::Unuse { site } => Request::SiteNode {
                name: site,
                version: None,
            },
        },
        Command::Dev { action } => match action {
            DevAction::Start { site } => Request::DevStart { site },
            DevAction::Stop { site } => Request::DevStop { site },
            DevAction::List => Request::DevList,
        },
        Command::Debug { action } => match action {
            DebugAction::On => Request::Debug { enable: Some(true) },
            DebugAction::Off => Request::Debug {
                enable: Some(false),
            },
            DebugAction::Status => Request::Debug { enable: None },
            DebugAction::Env => unreachable!("handled before to_request"),
        },
        Command::Service { action } => match action {
            ServiceAction::List => Request::ServiceList,
            ServiceAction::Install { key } => Request::ServiceInstall { key },
            ServiceAction::Start { key } => Request::ServiceStart { key },
            ServiceAction::Stop { key } => Request::ServiceStop { key },
            ServiceAction::Restart { key } => Request::ServiceRestart { key },
            ServiceAction::Port { key, port } => Request::ServiceSetPort { key, port },
        },
        Command::Requests { site, limit } => Request::RequestLog { site, limit },
        Command::Db { action } => match action {
            DbAction::Snapshot { engine, db, note } => Request::DbSnapshot {
                engine,
                database: db,
                note,
            },
            DbAction::List => Request::DbSnapshotList,
            DbAction::Restore { id } => Request::DbSnapshotRestore { id },
            DbAction::Rm { id } => Request::DbSnapshotRemove { id },
        },
        Command::Daemon
        | Command::Ca { .. }
        | Command::Php { .. }
        | Command::Start
        | Command::Stop
        | Command::Restart
        | Command::Install
        | Command::Uninstall
        | Command::Import
        | Command::Init { .. }
        | Command::Up { .. }
        | Command::Share { .. }
        | Command::Env { .. }
        | Command::Logs { .. }
        | Command::Path { .. }
        | Command::Resolve { .. }
        | Command::Gui => {
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
        cmd.arg("daemon")
            .stdout(out)
            .stderr(err)
            .stdin(std::process::Stdio::null());
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
        anyhow::bail!(
            "daemon did not come up in time; see {}",
            paths.base().join("daemon.log").display()
        );
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

    /// Ensure the daemon is up, then launch the desktop GUI.
    pub async fn gui(paths: &GrovePaths) -> anyhow::Result<()> {
        if !client::is_running(&paths.ipc_socket()).await {
            start(paths, false).await?;
        }
        let exe = std::env::current_exe().context("resolving grove binary path")?;
        let dir = exe.parent().context("binary has no parent dir")?;
        let gui = dir.join("grove-gui");
        if !gui.exists() {
            anyhow::bail!(
                "grove-gui not found next to grove ({}). Build it with \
                 `cargo build --release -p grove-gui` (after `pnpm --dir crates/grove-gui/ui build`).",
                gui.display()
            );
        }
        let mut cmd = std::process::Command::new(&gui);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(home) = std::env::var("GROVE_HOME") {
            cmd.env("GROVE_HOME", home);
        }
        detach(&mut cmd);
        cmd.spawn().context("launching grove-gui")?;
        println!("✓ Grove GUI launched");
        Ok(())
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
        use std::path::PathBuf;
        let exe = std::env::current_exe().context("resolving grove binary path")?;
        // When run via sudo, run PHP workers as the real user, not root.
        let run_user = std::env::var("SUDO_USER")
            .ok()
            .or_else(|| std::env::var("USER").ok())
            .filter(|u| !u.is_empty() && u != "root");

        // The service should use the invoking user's Grove home (not root's),
        // so it shares config/sites with the GUI. Honor an explicit GROVE_HOME;
        // otherwise derive it from SUDO_USER when running under sudo.
        let service_home: PathBuf = if std::env::var_os("GROVE_HOME").is_some() {
            paths.base().to_path_buf()
        } else if let Some(user) = run_user.as_deref() {
            if cfg!(target_os = "macos") {
                PathBuf::from("/Users")
                    .join(user)
                    .join("Library/Application Support/Grove")
            } else {
                PathBuf::from("/home").join(user).join(".local/share/Grove")
            }
        } else {
            paths.base().to_path_buf()
        };

        let unit = grove_os::service::install(&exe, &service_home, run_user.as_deref())
            .context("installing service")?;

        // Self-heal the system resolver (other tools like Herd can remove
        // /etc/resolver/<tld>); ensure the root CA exists too.
        use grove_os::PlatformIntegration;
        let svc_paths = GrovePaths::with_base(&service_home);
        let cfg = Config::load(&svc_paths).unwrap_or_default();
        let platform = grove_os::current();
        let _ = grove_tls::CertificateAuthority::load_or_create(&svc_paths);
        match platform.install_resolver(&cfg.general.tld, cfg.general.dns_port) {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "resolver setup"),
        }

        output::print_message(
            &format!(
                "service installed: {} (runs at boot, binds the ports, resolver ensured)",
                unit.display()
            ),
            json,
        );
        Ok(())
    }

    /// Share a local site publicly through a Grove Tunnel server.
    /// Bring a project up from its `grove.toml`, or scaffold one with `--write`.
    pub async fn up(
        paths: &GrovePaths,
        path: Option<String>,
        write: bool,
        no_dev: bool,
        json: bool,
    ) -> anyhow::Result<()> {
        use grove_core::ProjectFile;
        use grove_ipc::protocol::Response;

        let dir = match path {
            Some(p) => std::path::PathBuf::from(p),
            None => std::env::current_dir().context("resolving current directory")?,
        };
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);

        if write {
            let target = ProjectFile::path_in(&dir);
            if target.exists() {
                anyhow::bail!("grove.toml already exists at {}", target.display());
            }
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("app")
                .to_string();
            let php = Config::load(paths).unwrap_or_default().general.default_php;
            std::fs::write(&target, ProjectFile::starter_template(&name, &php))?;
            output::print_message(
                &format!("wrote {} — edit it, then run `grove up`", target.display()),
                json,
            );
            return Ok(());
        }

        let Some(pf) = ProjectFile::load(&dir).map_err(|e| anyhow::anyhow!(e))? else {
            anyhow::bail!(
                "no grove.toml in {} — create one with `grove up --write`",
                dir.display()
            );
        };

        let socket = paths.ipc_socket();
        if !client::is_running(&socket).await {
            anyhow::bail!("Grove daemon is not running. Start it with `grove start`.");
        }

        let name = pf.site_name(&dir);
        if !json {
            println!("Bringing up {name}…");
        }

        fn print_step(label: &str, resp: &Response, json: bool) {
            if json {
                return;
            }
            if resp.ok {
                println!("  ✓ {label}");
            } else {
                println!("  ✗ {label}: {}", resp.error.as_deref().unwrap_or("failed"));
            }
        }

        // 1. Link the project (critical).
        let resp = client::send(
            &socket,
            &Request::Link {
                path: dir.to_string_lossy().into_owned(),
                name: Some(name.clone()),
            },
        )
        .await?;
        print_step("link", &resp, json);
        if !resp.ok {
            anyhow::bail!("could not link the project");
        }

        // 2. HTTPS.
        let resp = client::send(
            &socket,
            &Request::Secure {
                name: name.clone(),
                enable: pf.secure,
            },
        )
        .await?;
        print_step(
            if pf.secure { "https on" } else { "https off" },
            &resp,
            json,
        );

        // 3. PHP: ensure installed, then pin.
        if let Some(php) = &pf.php {
            if !json {
                println!("  … PHP {php} (may download)");
            }
            let _ = client::send(
                &socket,
                &Request::PhpInstall {
                    version: php.clone(),
                },
            )
            .await;
            let resp = client::send(
                &socket,
                &Request::Isolate {
                    name: name.clone(),
                    version: Some(php.clone()),
                },
            )
            .await?;
            print_step(&format!("php {php}"), &resp, json);
        }

        // 4. Node.
        if let Some(node) = &pf.node {
            if !json {
                println!("  … Node {node} (may download)");
            }
            let _ = client::send(
                &socket,
                &Request::NodeInstall {
                    version: node.clone(),
                },
            )
            .await;
            let resp = client::send(
                &socket,
                &Request::SiteNode {
                    name: name.clone(),
                    version: Some(node.clone()),
                },
            )
            .await?;
            print_step(&format!("node {node}"), &resp, json);
        }

        // 5. Services.
        for svc in &pf.services {
            if !json {
                println!("  … {svc} (may download)");
            }
            let _ = client::send(&socket, &Request::ServiceInstall { key: svc.clone() }).await;
            let resp = client::send(&socket, &Request::ServiceStart { key: svc.clone() }).await?;
            print_step(svc, &resp, json);
        }

        // 6. Dev processes.
        if pf.dev && !no_dev {
            let resp = client::send(&socket, &Request::DevStart { site: name.clone() }).await?;
            print_step("dev", &resp, json);
        }

        let scheme = if pf.secure { "https" } else { "http" };
        output::print_message(&format!("{name} is up → {scheme}://{name}.test"), json);
        Ok(())
    }

    pub async fn share(
        paths: &GrovePaths,
        site: String,
        server: Option<String>,
        token: Option<String>,
        subdomain: Option<String>,
        basic_auth: Option<String>,
        json: bool,
    ) -> anyhow::Result<()> {
        use std::net::SocketAddr;
        let config = Config::load(paths).unwrap_or_default();
        let tld = &config.general.tld;

        // Resolve the site to a `<name>.<tld>` host Grove already serves.
        let name = site
            .trim()
            .trim_end_matches(&format!(".{tld}"))
            .to_lowercase();
        if name.is_empty() {
            anyhow::bail!("missing site name");
        }
        let local_host = format!("{name}.{tld}");
        let local_addr: SocketAddr = format!("127.0.0.1:{}", config.general.http_port).parse()?;

        let server = server.or(config.tunnel.server.clone()).context(
            "no tunnel server set — pass --server host:port or set [tunnel].server in config.toml",
        )?;
        let token = token.or(config.tunnel.token.clone()).unwrap_or_default();

        // Make sure the local daemon is actually serving the site.
        if !client::is_running(&paths.ipc_socket()).await {
            anyhow::bail!("Grove daemon is not running — start it (or install the service) first");
        }

        let cfg = grove_tunnel::ShareConfig {
            server,
            token,
            subdomain,
            local_host: local_host.clone(),
            local_addr,
            basic_auth,
        };

        if !json {
            eprintln!("  Sharing {local_host} — connecting to tunnel…");
        }

        // Live request log (ngrok-style) on the terminal.
        let recorder: Option<grove_tunnel::Recorder> = if json {
            None
        } else {
            Some(std::sync::Arc::new(|r: grove_tunnel::RequestRecord| {
                println!(
                    "  {:<6} {:<40} {} ({}ms)",
                    r.method, r.path, r.status, r.duration_ms
                );
            }))
        };

        grove_tunnel::share(cfg, recorder, |public_host, public_url| {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "local": local_host,
                        "public_host": public_host,
                        "public_url": public_url,
                    })
                );
            } else {
                println!();
                println!("  🌿  Tunnel online");
                println!("     Public   {public_url}");
                println!("     Local    http://{local_host}");
                println!();
                println!("  Press Ctrl-C to stop sharing.");
            }
        })
        .await?;

        output::print_message("tunnel closed", json);
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

    /// First-run setup. Idempotent: safe to run repeatedly. Does everything that
    /// does not need the daemon, and clearly reports the privileged steps.
    pub fn init(paths: &GrovePaths, php: String, no_php: bool, json: bool) -> anyhow::Result<()> {
        use grove_os::PlatformIntegration;
        use grove_runtime::PhpRegistry;
        use grove_tls::CertificateAuthority;

        let mut steps: Vec<(bool, String)> = Vec::new();
        paths.ensure()?;

        // 1. Config (create default if absent, never clobber).
        let cfg_path = paths.config_file();
        let mut config = Config::load(paths).unwrap_or_default();
        if !cfg_path.exists() {
            // Park ~/Code by default so existing projects are picked up.
            let code = std::path::PathBuf::from("~/Code");
            let expanded = Config::expand(&code);
            if expanded.is_dir() {
                config.add_parked(code);
                steps.push((
                    true,
                    "parked ~/Code (existing projects auto-imported)".into(),
                ));
            }
            config.save(paths)?;
            steps.push((true, format!("created config at {}", cfg_path.display())));
        } else {
            steps.push((true, format!("config present at {}", cfg_path.display())));
        }

        // 2. Root CA (no elevation needed to generate).
        CertificateAuthority::load_or_create(paths)?;
        steps.push((true, format!("root CA at {}", paths.ca_cert().display())));

        // 3. Ensure a PHP build.
        let mut registry = PhpRegistry::load(paths);
        if !no_php {
            if registry.iter().next().is_none() {
                registry.discover();
            }
            if registry.get(&php).is_none() {
                if !json {
                    eprintln!("  installing php@{php} (static, self-contained)…");
                }
                match grove_runtime::install_php(paths, &mut registry, &php, |m| {
                    if !json {
                        eprintln!("    {m}");
                    }
                }) {
                    Ok(build) => {
                        config.general.default_php = build.version.clone();
                        steps.push((true, format!("installed php@{}", build.version)));
                    }
                    Err(e) => steps.push((false, format!("PHP install failed: {e}"))),
                }
            } else {
                config.general.default_php = php.clone();
                steps.push((true, format!("php@{php} already available")));
            }
            config.save(paths)?;
        }

        // 4. Privileged steps: resolver + CA trust (only if we can).
        let platform = grove_os::current();
        if grove_os::is_elevated() {
            match platform.install_resolver(&config.general.tld, config.general.dns_port) {
                Ok(()) => steps.push((
                    true,
                    format!("resolver installed for .{}", config.general.tld),
                )),
                Err(e) => steps.push((false, format!("resolver: {e}"))),
            }
            match platform.trust_ca(&paths.ca_cert()) {
                Ok(()) => steps.push((true, "root CA trusted in system store".into())),
                Err(e) => steps.push((false, format!("CA trust: {e}"))),
            }
        } else {
            steps.push((
                false,
                "resolver + CA trust need elevation — run `sudo grove init` or \
                 `sudo grove ca trust`"
                    .into(),
            ));
        }

        if json {
            let arr: Vec<_> = steps
                .iter()
                .map(|(ok, msg)| serde_json::json!({ "ok": ok, "step": msg }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&arr).unwrap_or_default());
        } else {
            println!("Grove setup:");
            for (ok, msg) in &steps {
                println!("  {} {msg}", if *ok { "✓" } else { "!" });
            }
            println!("\nNext: `grove start`, then `grove park ~/Code` and open a site.");
        }
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
                            node: None,
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
    use std::path::{Path, PathBuf};

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

    /// Print shell env exports that make a CLI PHP process connect to the
    /// debugger. Runs locally (no daemon needed): reads the port from config.
    pub fn debug_env(paths: &GrovePaths, json: bool) -> anyhow::Result<()> {
        let config = grove_core::Config::load(paths).context("loading config")?;
        let port = config.general.xdebug_port;
        let exports = grove_runtime::xdebug::cli_env_exports(port);
        if json {
            println!(
                "{}",
                serde_json::json!({ "ok": true, "port": port, "exports": exports })
            );
        } else {
            print!("{exports}");
        }
        Ok(())
    }

    pub fn php(paths: &GrovePaths, action: PhpAction, json: bool) -> anyhow::Result<()> {
        let mut registry = PhpRegistry::load(paths);
        match action {
            PhpAction::Install { version } => {
                let build = grove_runtime::install_php(paths, &mut registry, &version, |msg| {
                    if !json {
                        eprintln!("  {msg}");
                    }
                })
                .context("installing static PHP build")?;
                output::print_message(
                    &format!(
                        "php@{} ready at {}",
                        build.version,
                        build.fpm_binary.display()
                    ),
                    json,
                );
            }
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

    const SHIM_TOOLS: [&str; 6] = ["php", "composer", "node", "npm", "npx", "laravel"];

    /// Manage the PATH shims that expose Grove's bundled toolchain.
    pub fn path(paths: &GrovePaths, action: PathAction, json: bool) -> anyhow::Result<()> {
        let shims = paths.base().join("shims");
        match action {
            PathAction::Install => {
                let grove_bin = std::env::current_exe().context("locating the grove binary")?;
                std::fs::create_dir_all(&shims)?;
                for tool in SHIM_TOOLS {
                    let script = format!(
                        "#!/bin/sh\n# Managed by `grove path` — resolves the version Grove pinned for this dir.\nexec \"{}\" resolve {} --dir \"$PWD\" -- \"$@\"\n",
                        grove_bin.display(),
                        tool,
                    );
                    let dest = shims.join(tool);
                    std::fs::write(&dest, script)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                    }
                }
                print_path_instructions(&shims, true, json);
            }
            PathAction::Uninstall => {
                if shims.exists() {
                    std::fs::remove_dir_all(&shims)?;
                }
                output::print_message(
                    "Removed Grove shims. Delete the PATH line from your shell profile too.",
                    json,
                );
            }
            PathAction::Show => {
                let installed = shims.join("php").exists();
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "ok": true,
                            "installed": installed,
                            "shims_dir": shims.display().to_string(),
                            "on_path": path_contains(&shims),
                            "tools": SHIM_TOOLS,
                        })
                    );
                } else if installed {
                    print_path_instructions(&shims, false, json);
                } else {
                    println!("Grove shims are not installed. Run `grove path install`.");
                }
            }
        }
        Ok(())
    }

    fn path_contains(dir: &Path) -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|e| e == dir))
            .unwrap_or(false)
    }

    fn print_path_instructions(shims: &Path, just_installed: bool, json: bool) {
        if json {
            return;
        }
        let dir = shims.display();
        if just_installed {
            println!("✓ Installed shims for {}.\n", SHIM_TOOLS.join(", "));
        }
        if path_contains(shims) {
            println!("Grove's toolchain is on your PATH ({dir}).");
            println!("php, composer, node, npm, npx and laravel now resolve to the version each project pins.");
            return;
        }
        let shell = std::env::var("SHELL").unwrap_or_default();
        println!("Add Grove's toolchain to your PATH, then restart your shell:\n");
        if shell.ends_with("fish") {
            println!("    fish_add_path {dir}\n");
        } else {
            let profile = if shell.ends_with("zsh") {
                "~/.zshrc"
            } else {
                "~/.bashrc"
            };
            println!("    echo 'export PATH=\"{dir}:$PATH\"' >> {profile}\n");
        }
        println!("Then `php`, `composer`, `node`, `npm`, `npx` and `laravel` use Grove's bundled versions,");
        println!(
            "auto-switching to whatever each project pins with `grove isolate` / `grove node use`."
        );
    }

    /// Resolve a bundled tool for `dir` and exec it (replacing this process).
    pub fn resolve(
        paths: &GrovePaths,
        tool: &str,
        dir: Option<String>,
        args: Vec<String>,
    ) -> anyhow::Result<()> {
        let dir = dir
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let cfg = grove_core::Config::load(paths).unwrap_or_default();
        let (php_pin, node_pin) = pins_for_dir(&cfg, &dir);

        let (bin, lead): (PathBuf, Vec<PathBuf>) = match tool {
            "php" => (resolve_php(paths, &cfg, php_pin)?, vec![]),
            "composer" => {
                let php = resolve_php(paths, &cfg, php_pin)?;
                let phar = grove_runtime::scaffold::ensure_composer(paths)
                    .map_err(|e| anyhow::anyhow!("preparing composer: {e}"))?;
                (php, vec![phar])
            }
            "laravel" => {
                let php = resolve_php(paths, &cfg, php_pin)?;
                let installer = grove_runtime::scaffold::laravel_installer(paths);
                if !installer.exists() {
                    anyhow::bail!(
                        "the Laravel installer isn't set up yet — run `grove new <name>` once (it installs it), then retry"
                    );
                }
                (php, vec![installer])
            }
            "node" => (resolve_node(paths, node_pin)?.0, vec![]),
            "npm" => (resolve_node(paths, node_pin)?.1, vec![]),
            "npx" => (resolve_node(paths, node_pin)?.2, vec![]),
            other => anyhow::bail!(
                "unknown tool {other:?}; use php, composer, node, npm, npx or laravel"
            ),
        };

        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&lead).args(&args);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = cmd.exec();
            anyhow::bail!("failed to exec {}: {err}", bin.display());
        }
        #[cfg(not(unix))]
        {
            let status = cmd.status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    /// (php_version_override, node_major_override) from the site containing `dir`.
    fn pins_for_dir(cfg: &grove_core::Config, dir: &Path) -> (Option<String>, Option<String>) {
        cfg.sites
            .iter()
            .filter(|s| {
                s.path
                    .as_deref()
                    .map(|p| dir.starts_with(p))
                    .unwrap_or(false)
            })
            .max_by_key(|s| {
                s.path
                    .as_deref()
                    .map(|p| p.components().count())
                    .unwrap_or(0)
            })
            .map(|s| (s.php.clone(), s.node.clone()))
            .unwrap_or((None, None))
    }

    fn resolve_php(
        paths: &GrovePaths,
        cfg: &grove_core::Config,
        pin: Option<String>,
    ) -> anyhow::Result<PathBuf> {
        let version = pin.unwrap_or_else(|| cfg.general.default_php.clone());
        // Shims run as the user and can only read what the (root) daemon
        // provisioned, so never download here — resolve read-only.
        let reg = PhpRegistry::load(paths);
        if let Some(cli) = reg
            .iter()
            .filter(|b| b.version.starts_with(&version) || version.starts_with(&b.version))
            .find_map(|b| b.cli_binary.clone())
            .filter(|p| p.exists())
        {
            return Ok(cli);
        }
        // A CLI provisioned by `grove new` / `grove path install` lives under
        // runtimes/cli/<minor>/php. Prefer the pinned version, else the newest.
        let cli_root = paths.runtimes_dir().join("cli");
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(&cli_root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("php").exists())
            .collect();
        candidates.sort();
        if let Some(hit) = candidates.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&version))
                .unwrap_or(false)
        }) {
            return Ok(hit.join("php"));
        }
        if let Some(newest) = candidates.last() {
            return Ok(newest.join("php"));
        }
        anyhow::bail!(
            "no PHP {version} runtime found — run `grove path install` (or `grove php install {version}`) to provision it"
        )
    }

    fn resolve_node(
        paths: &GrovePaths,
        pin: Option<String>,
    ) -> anyhow::Result<(PathBuf, PathBuf, PathBuf)> {
        let reg = grove_runtime::NodeRegistry::load(paths);
        let build = pin
            .as_deref()
            .and_then(|major| reg.get(major).cloned())
            .or_else(|| {
                reg.iter()
                    .max_by_key(|b| b.major.parse::<u32>().unwrap_or(0))
                    .cloned()
            });
        let Some(b) = build else {
            anyhow::bail!("no Node installed — run `grove node install 22`");
        };
        let npx = b
            .node_binary
            .parent()
            .map(|d| d.join("npx"))
            .unwrap_or_else(|| PathBuf::from("npx"));
        Ok((b.node_binary, b.npm_binary, npx))
    }
}

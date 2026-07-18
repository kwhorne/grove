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
    BundleAction, CaAction, Cli, Command, DbAction, DebugAction, DevAction, HookAction,
    LicenseAction, MailAction, NodeAction, PathAction, PhpAction, SecretAction, ServiceAction,
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
            local::path(action.clone(), args.json)?;
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
        Command::Secret { action } => secret::run(&paths, action, args.json),
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

        Command::Mcp { allow_write } => mcp::serve(&paths, allow_write).await,
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
        Command::Bundle { action } => match action {
            BundleAction::Export { path, out, no_env } => {
                bundle::export(&paths, path, out, no_env, args.json).await
            }
            BundleAction::Import { file, into } => {
                bundle::import(&paths, file, into, args.json).await
            }
        },
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
        Command::Replay { id } => Request::ReplayRequest { id },
        Command::Request { id, format } => Request::RequestToTest { id, format },
        Command::Hooks { limit, action } => match action {
            None => Request::HookList { limit },
            Some(HookAction::Replay { id, to }) => Request::HookReplayTo { id, to },
            Some(HookAction::Clear) => Request::HookClear,
        },
        Command::License { action } => match action {
            LicenseAction::Activate { key } => Request::LicenseActivate { key },
            LicenseAction::Status => Request::LicenseStatus,
            LicenseAction::Deactivate => Request::LicenseDeactivate,
        },
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
        | Command::Bundle { .. }
        | Command::Mcp { .. }
        | Command::Share { .. }
        | Command::Env { .. }
        | Command::Logs { .. }
        | Command::Path { .. }
        | Command::Resolve { .. }
        | Command::Secret { .. }
        | Command::Gui => {
            unreachable!("handled before to_request")
        }
    })
}

fn init_tracing(daemon: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let default = if daemon { "info" } else { "warn" };
    let filter = EnvFilter::try_from_env("GROVE_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    // Always log to stderr so stdout stays clean for machine protocols (e.g. `grove mcp`).
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Model Context Protocol server (stdio). Exposes Grove's live local state —
/// sites, request timeline, webhooks, logs, and database schema/queries — as
/// read-only tools an AI client (Claude, Cursor, …) can call. Newline-delimited
/// JSON-RPC 2.0 over stdin/stdout; all logging goes to stderr.
mod mcp {
    use super::*;
    use grove_ipc::protocol::{Request, ResponseData};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    pub async fn serve(paths: &GrovePaths, allow_write: bool) -> anyhow::Result<()> {
        let socket = paths.ipc_socket();
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut out = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            // Notifications (no id) need no reply.
            let Some(id) = msg.get("id").cloned() else {
                continue;
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let reply = match dispatch(paths, &socket, method, &params, allow_write).await {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(e) => json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32000, "message": e.to_string()}
                }),
            };
            let mut s = serde_json::to_string(&reply)?;
            s.push('\n');
            out.write_all(s.as_bytes()).await?;
            out.flush().await?;
        }
        Ok(())
    }

    async fn dispatch(
        paths: &GrovePaths,
        socket: &Path,
        method: &str,
        params: &Value,
        allow_write: bool,
    ) -> anyhow::Result<Value> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "grove", "version": env!("CARGO_PKG_VERSION")}
            })),
            "tools/list" => Ok(json!({"tools": tool_defs(allow_write)})),
            "resources/list" => Ok(json!({"resources": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            "ping" => Ok(json!({})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match call_tool(paths, socket, name, &args, allow_write).await {
                    Ok(text) => Ok(json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    })),
                    Err(e) => Ok(json!({
                        "content": [{"type": "text", "text": e.to_string()}],
                        "isError": true
                    })),
                }
            }
            other => anyhow::bail!("method not found: {other}"),
        }
    }

    fn tool_defs(allow_write: bool) -> Value {
        let mut tools = match json!([
            {
                "name": "grove_sites",
                "description": "List all sites Grove serves on .test (name, host, driver, PHP/Node version, HTTPS, path).",
                "inputSchema": {"type": "object", "properties": {}}
            },
            {
                "name": "grove_requests",
                "description": "Recent HTTP requests Grove proxied (framework-agnostic), newest first. Optionally filter by site.",
                "inputSchema": {"type": "object", "properties": {
                    "site": {"type": "string", "description": "Site name to filter by"},
                    "limit": {"type": "integer", "description": "Max entries (default 40)"}
                }}
            },
            {
                "name": "grove_request",
                "description": "Full detail (headers + body) of one captured request, by id from grove_requests.",
                "inputSchema": {"type": "object", "properties": {
                    "id": {"type": "integer"}
                }, "required": ["id"]}
            },
            {
                "name": "grove_webhooks",
                "description": "Recently captured inbound webhooks (requests to /__grove/hooks/...).",
                "inputSchema": {"type": "object", "properties": {
                    "limit": {"type": "integer", "description": "Max entries (default 40)"}
                }}
            },
            {
                "name": "grove_logs",
                "description": "List available log sources, or read recent entries from one. Omit 'source' to list.",
                "inputSchema": {"type": "object", "properties": {
                    "source": {"type": "string", "description": "Log source name (substring match)"},
                    "lines": {"type": "integer", "description": "Max entries (default 50)"}
                }}
            },
            {
                "name": "grove_db_schema",
                "description": "Database schema (tables and their columns) for a site, read from its .env. Great for answering questions about data shape.",
                "inputSchema": {"type": "object", "properties": {
                    "site": {"type": "string"}
                }, "required": ["site"]}
            },
            {
                "name": "grove_db_query",
                "description": "Run a READ-ONLY SQL query (SELECT/SHOW/EXPLAIN/PRAGMA) against a site's database and return rows. Writes are refused.",
                "inputSchema": {"type": "object", "properties": {
                    "site": {"type": "string"},
                    "sql": {"type": "string"}
                }, "required": ["site", "sql"]}
            }
        ]) {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        // Agent-safe write tools are opt-in (`grove mcp --allow-write`). The
        // server stays read-only unless the operator turns this on.
        if allow_write {
            tools.push(json!({
                "name": "grove_migrate_sandboxed",
                "description": "Run database migrations inside an automatic snapshot sandbox. Grove takes a point-in-time snapshot first, runs `php artisan <command>` in the site, captures the schema diff, and AUTOMATICALLY ROLLS BACK if the command fails (or if roll_back=true). Returns the command output, the schema diff, the snapshot id, and whether a rollback happened. Works with MySQL, PostgreSQL and SQLite.",
                "inputSchema": {"type": "object", "properties": {
                    "site": {"type": "string", "description": "Site name or hostname"},
                    "command": {"type": "string", "description": "Artisan command to run (default: 'migrate --force'). Examples: 'migrate --force', 'migrate:fresh --force', 'migrate:rollback --force'."},
                    "roll_back": {"type": "boolean", "description": "Always roll back afterwards, even on success — a pure dry run (default false)."}
                }, "required": ["site"]}
            }));
            tools.push(json!({
                "name": "grove_sql_sandboxed",
                "description": "Run a WRITE SQL statement (INSERT/UPDATE/DELETE/DDL) inside an automatic snapshot sandbox. Grove snapshots the database first, runs the statement, reports rows_affected and the schema diff, and AUTOMATICALLY ROLLS BACK if it fails (or if roll_back=true for a dry run). Use grove_db_query for read-only SELECTs. Works with MySQL, PostgreSQL and SQLite.",
                "inputSchema": {"type": "object", "properties": {
                    "site": {"type": "string", "description": "Site name or hostname"},
                    "sql": {"type": "string", "description": "A single write statement, e.g. UPDATE users SET ..."},
                    "roll_back": {"type": "boolean", "description": "Always roll back afterwards, even on success — a pure dry run (default false)."}
                }, "required": ["site", "sql"]}
            }));
        }
        Value::Array(tools)
    }

    async fn call(socket: &Path, req: Request) -> anyhow::Result<ResponseData> {
        let resp = client::send(socket, &req)
            .await
            .context("talking to the Grove daemon (is it running? `grove start`)")?;
        if !resp.ok {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "request failed".into()));
        }
        resp.data
            .ok_or_else(|| anyhow::anyhow!("no data in response"))
    }

    async fn site_path(socket: &Path, name: &str) -> anyhow::Result<PathBuf> {
        match call(socket, Request::ListSites).await? {
            ResponseData::Sites(sites) => sites
                .into_iter()
                .find(|s| s.site.name == name || s.site.hostname == name)
                .map(|s| s.site.path)
                .ok_or_else(|| anyhow::anyhow!("no site named {name:?}")),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    fn is_readonly(sql: &str) -> bool {
        let s = sql.trim_start().to_ascii_lowercase();
        [
            "select", "with", "show", "explain", "pragma", "describe", "desc",
        ]
        .iter()
        .any(|kw| s.starts_with(kw))
    }

    async fn call_tool(
        paths: &GrovePaths,
        socket: &Path,
        name: &str,
        args: &Value,
        allow_write: bool,
    ) -> anyhow::Result<String> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| args.get(k).and_then(Value::as_u64);
        match name {
            "grove_migrate_sandboxed" => {
                if !allow_write {
                    anyhow::bail!(
                        "write tools are disabled; start the server with `grove mcp --allow-write` to enable snapshot-sandboxed migrations"
                    );
                }
                let site = s("site").ok_or_else(|| anyhow::anyhow!("site is required"))?;
                let command = s("command").unwrap_or_else(|| "migrate --force".into());
                let roll_back = args.get("roll_back").and_then(Value::as_bool).unwrap_or(false);
                migrate_sandboxed(paths, socket, &site, &command, roll_back).await
            }
            "grove_sql_sandboxed" => {
                if !allow_write {
                    anyhow::bail!(
                        "write tools are disabled; start the server with `grove mcp --allow-write` to enable snapshot-sandboxed SQL"
                    );
                }
                let site = s("site").ok_or_else(|| anyhow::anyhow!("site is required"))?;
                let sql = s("sql").ok_or_else(|| anyhow::anyhow!("sql is required"))?;
                let roll_back = args.get("roll_back").and_then(Value::as_bool).unwrap_or(false);
                sql_sandboxed(paths, socket, &site, &sql, roll_back).await
            }
            "grove_sites" => match call(socket, Request::ListSites).await? {
                ResponseData::Sites(sites) => {
                    let slim: Vec<Value> = sites
                        .iter()
                        .map(|ss| {
                            json!({
                                "name": ss.site.name,
                                "host": ss.site.hostname,
                                "driver": ss.site.driver.to_string(),
                                "php": ss.site.php,
                                "node": ss.site.node,
                                "https": ss.site.secure,
                                "docker": ss.site.docker,
                                "path": ss.site.path,
                            })
                        })
                        .collect();
                    Ok(serde_json::to_string_pretty(&slim)?)
                }
                _ => anyhow::bail!("unexpected response"),
            },
            "grove_requests" => {
                let limit = n("limit").unwrap_or(40) as usize;
                match call(
                    socket,
                    Request::RequestLog {
                        site: s("site"),
                        limit,
                    },
                )
                .await?
                {
                    ResponseData::Requests(r) => Ok(serde_json::to_string_pretty(&r)?),
                    _ => anyhow::bail!("unexpected response"),
                }
            }
            "grove_request" => {
                let id = n("id").ok_or_else(|| anyhow::anyhow!("id is required"))?;
                match call(socket, Request::RequestDetail { id }).await? {
                    ResponseData::RequestDetail(Some(d)) => Ok(serde_json::to_string_pretty(&d)?),
                    ResponseData::RequestDetail(None) => anyhow::bail!("no request with id {id}"),
                    _ => anyhow::bail!("unexpected response"),
                }
            }
            "grove_webhooks" => {
                let limit = n("limit").unwrap_or(40) as usize;
                match call(socket, Request::HookList { limit }).await? {
                    ResponseData::Hooks(h) => Ok(serde_json::to_string_pretty(&h)?),
                    _ => anyhow::bail!("unexpected response"),
                }
            }
            "grove_logs" => match s("source") {
                None => match call(socket, Request::LogSources).await? {
                    ResponseData::LogSources(list) => Ok(serde_json::to_string_pretty(&list)?),
                    _ => anyhow::bail!("unexpected response"),
                },
                Some(q) => {
                    let path = match call(socket, Request::LogSources).await? {
                        ResponseData::LogSources(list) => list
                            .into_iter()
                            .find(|l| l.name.to_lowercase().contains(&q.to_lowercase()))
                            .map(|l| l.path)
                            .ok_or_else(|| anyhow::anyhow!("no log source matching {q:?}"))?,
                        _ => anyhow::bail!("unexpected response"),
                    };
                    let limit = n("lines").unwrap_or(50) as usize;
                    match call(socket, Request::LogEntries { path, limit }).await? {
                        ResponseData::LogEntries(e) => Ok(serde_json::to_string_pretty(&e)?),
                        _ => anyhow::bail!("unexpected response"),
                    }
                }
            },
            "grove_db_schema" => {
                let site = s("site").ok_or_else(|| anyhow::anyhow!("site is required"))?;
                let path = site_path(socket, &site).await?;
                tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                    let cfg = e_db::from_env(&path)
                        .ok_or_else(|| anyhow::anyhow!("no database configured in {site}'s .env"))?;
                    let conn = e_db::connect(&cfg).map_err(|e| anyhow::anyhow!(e))?;
                    let tables = e_db::tables(&conn).map_err(|e| anyhow::anyhow!(e))?;
                    let mut schema = serde_json::Map::new();
                    for t in &tables {
                        let cols = e_db::columns(&conn, t).map_err(|e| anyhow::anyhow!(e))?;
                        let cols: Vec<Value> = cols
                            .iter()
                            .map(|c| {
                                json!({"name": c.name, "type": c.data_type, "nullable": c.nullable, "key": c.key})
                            })
                            .collect();
                        schema.insert(t.clone(), json!(cols));
                    }
                    Ok(serde_json::to_string_pretty(&schema)?)
                })
                .await?
            }
            "grove_db_query" => {
                let site = s("site").ok_or_else(|| anyhow::anyhow!("site is required"))?;
                let sql = s("sql").ok_or_else(|| anyhow::anyhow!("sql is required"))?;
                if !is_readonly(&sql) {
                    anyhow::bail!(
                        "only read-only queries are allowed via MCP (SELECT/SHOW/EXPLAIN/PRAGMA)"
                    );
                }
                let path = site_path(socket, &site).await?;
                tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                    let cfg = e_db::from_env(&path).ok_or_else(|| {
                        anyhow::anyhow!("no database configured in {site}'s .env")
                    })?;
                    let conn = e_db::connect(&cfg).map_err(|e| anyhow::anyhow!(e))?;
                    let result = e_db::query(&conn, &sql, 200).map_err(|e| anyhow::anyhow!(e))?;
                    Ok(serde_json::to_string_pretty(&result)?)
                })
                .await?
            }
            other => anyhow::bail!("unknown tool: {other}"),
        }
    }

    // ---- agent-safe write tools -------------------------------------------

    /// Table -> sorted column signatures (`name:type:null|notnull`).
    type Schema = std::collections::BTreeMap<String, Vec<String>>;

    /// Resolve a site's project path and its pinned PHP version.
    async fn site_info(socket: &Path, name: &str) -> anyhow::Result<(PathBuf, String)> {
        match call(socket, Request::ListSites).await? {
            ResponseData::Sites(sites) => sites
                .into_iter()
                .find(|s| s.site.name == name || s.site.hostname == name)
                .map(|s| (s.site.path, s.site.php))
                .ok_or_else(|| anyhow::anyhow!("no site named {name:?}")),
            _ => anyhow::bail!("unexpected response"),
        }
    }

    /// The PHP CLI for `version`, downloading it if necessary.
    fn resolve_php_cli(paths: &GrovePaths, version: &str) -> anyhow::Result<PathBuf> {
        use grove_runtime::PhpRegistry;
        let reg = PhpRegistry::load(paths);
        if let Some(cli) = reg.get(version).and_then(|b| b.cli_binary.clone()) {
            return Ok(cli);
        }
        grove_runtime::install::install_cli(paths, version, |_| {})
            .map_err(|e| anyhow::anyhow!("could not resolve the PHP {version} CLI: {e}"))
    }

    /// A stable, comparable snapshot of a database's shape: table -> sorted
    /// column signatures (`name:type:null|notnull`).
    fn read_schema(cfg: &e_db::DbConfig) -> anyhow::Result<Schema> {
        let conn = e_db::connect(cfg).map_err(|e| anyhow::anyhow!(e))?;
        let tables = e_db::tables(&conn).map_err(|e| anyhow::anyhow!(e))?;
        let mut map = Schema::new();
        for t in &tables {
            let cols = e_db::columns(&conn, t).map_err(|e| anyhow::anyhow!(e))?;
            let mut sigs: Vec<String> = cols
                .iter()
                .map(|c| {
                    format!(
                        "{}:{}:{}",
                        c.name,
                        c.data_type,
                        if c.nullable { "null" } else { "notnull" }
                    )
                })
                .collect();
            sigs.sort();
            map.insert(t.clone(), sigs);
        }
        Ok(map)
    }

    /// Structural diff between two schemas: added/removed tables and per-table
    /// column changes.
    fn schema_diff(before: &Schema, after: &Schema) -> Value {
        let added_tables: Vec<&String> = after.keys().filter(|t| !before.contains_key(*t)).collect();
        let removed_tables: Vec<&String> =
            before.keys().filter(|t| !after.contains_key(*t)).collect();
        let mut changed = serde_json::Map::new();
        for (t, acols) in after {
            if let Some(bcols) = before.get(t) {
                let added_columns: Vec<&String> =
                    acols.iter().filter(|c| !bcols.contains(*c)).collect();
                let removed_columns: Vec<&String> =
                    bcols.iter().filter(|c| !acols.contains(*c)).collect();
                if !added_columns.is_empty() || !removed_columns.is_empty() {
                    changed.insert(
                        t.clone(),
                        json!({"added_columns": added_columns, "removed_columns": removed_columns}),
                    );
                }
            }
        }
        json!({
            "added_tables": added_tables,
            "removed_tables": removed_tables,
            "changed_tables": changed,
        })
    }

    /// The daemon reports `snapshot <id> created (...)`; pull out the id.
    fn snapshot_id_from_msg(msg: &str) -> anyhow::Result<String> {
        msg.split_whitespace()
            .nth(1)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("could not parse snapshot id from response: {msg:?}"))
    }

    fn truncate(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            let t: String = s.chars().take(max).collect();
            format!("{t}\n... [truncated]")
        }
    }

    /// Append an audit record for every write operation, so operators can see
    /// exactly what an agent ran, when, and the outcome.
    fn audit_log(paths: &GrovePaths, entry: &Value) {
        let dir = paths.logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut record = entry.clone();
        if let Value::Object(ref mut m) = record {
            m.insert("ts".into(), json!(ts));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("mcp-writes.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "{record}");
        }
    }

    /// Inspect a site's database and read its current schema. Bundled
    /// MySQL/PostgreSQL are snapshotted via the daemon; SQLite is snapshotted by
    /// copying its file (see `open_sandbox`).
    fn inspect_db_sync(path: &Path) -> anyhow::Result<(e_db::DbConfig, Schema)> {
        let cfg = e_db::from_env(path)
            .ok_or_else(|| anyhow::anyhow!("no database configured in the site's .env"))?;
        match cfg.engine.as_str() {
            "mysql" | "postgres" => {
                if cfg.database.is_empty() {
                    anyhow::bail!("no DB_DATABASE set in the site's .env");
                }
            }
            "sqlite" => {
                if cfg.path.is_empty() {
                    anyhow::bail!("no sqlite database path resolved from the site's .env");
                }
            }
            other => anyhow::bail!(
                "snapshot-sandboxed writes support MySQL, PostgreSQL and SQLite (found {other})"
            ),
        }
        let before = read_schema(&cfg)?;
        Ok((cfg, before))
    }

    /// A human-facing label for the database being sandboxed.
    fn db_label(cfg: &e_db::DbConfig) -> String {
        if cfg.engine == "sqlite" {
            cfg.path.clone()
        } else {
            cfg.database.clone()
        }
    }

    /// A restorable point-in-time backup, engine-appropriate.
    enum Sandbox {
        /// Bundled MySQL/PostgreSQL snapshot handled by the daemon.
        Daemon { id: String },
        /// A copy of a SQLite file, restored by copying it back.
        Sqlite { original: PathBuf, backup: PathBuf },
    }

    impl Sandbox {
        /// A stable identifier surfaced to the caller for manual rollback.
        fn id(&self) -> String {
            match self {
                Sandbox::Daemon { id } => id.clone(),
                Sandbox::Sqlite { backup, .. } => backup
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            }
        }
    }

    /// Take an engine-appropriate snapshot before a write.
    async fn open_sandbox(
        paths: &GrovePaths,
        socket: &Path,
        cfg: &e_db::DbConfig,
        note: &str,
    ) -> anyhow::Result<Sandbox> {
        match cfg.engine.as_str() {
            "mysql" | "postgres" => {
                let msg = match call(
                    socket,
                    Request::DbSnapshot {
                        engine: cfg.engine.clone(),
                        database: Some(cfg.database.clone()),
                        note: Some(note.to_string()),
                    },
                )
                .await?
                {
                    ResponseData::Message(m) => m,
                    _ => anyhow::bail!("unexpected response taking snapshot"),
                };
                Ok(Sandbox::Daemon {
                    id: snapshot_id_from_msg(&msg)?,
                })
            }
            "sqlite" => {
                let original = PathBuf::from(&cfg.path);
                if !original.is_file() {
                    anyhow::bail!("sqlite file not found: {}", original.display());
                }
                let dir = paths.base().join("snapshots");
                std::fs::create_dir_all(&dir)?;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let stem = original
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "db".into());
                let backup = dir.join(format!("sqlite-{stem}-{ts}.sqlite"));
                std::fs::copy(&original, &backup)?;
                Ok(Sandbox::Sqlite { original, backup })
            }
            other => anyhow::bail!("cannot sandbox engine {other}"),
        }
    }

    /// Roll back the sandbox when `should` is set; returns any error text.
    async fn close_sandbox(socket: &Path, sandbox: Sandbox, should: bool) -> Option<String> {
        if !should {
            return None;
        }
        match sandbox {
            Sandbox::Daemon { id } => match call(
                socket,
                Request::DbSnapshotRestore { id: id.clone() },
            )
            .await
            {
                Ok(ResponseData::Message(_)) => None,
                Ok(_) => Some("unexpected response restoring snapshot".into()),
                Err(e) => Some(e.to_string()),
            },
            Sandbox::Sqlite { original, backup } => {
                std::fs::copy(&backup, &original).err().map(|e| e.to_string())
            }
        }
    }

    /// Run `php artisan <command>` inside an automatic snapshot sandbox: snapshot
    /// first, run, diff the schema, and roll back on failure (or on request).
    async fn migrate_sandboxed(
        paths: &GrovePaths,
        socket: &Path,
        site: &str,
        command: &str,
        roll_back: bool,
    ) -> anyhow::Result<String> {
        let (path, php_ver) = site_info(socket, site).await?;

        // 1. Inspect the site's database: engine, name, and pre-migration shape.
        let p = path.clone();
        let (cfg, before) = tokio::task::spawn_blocking(move || inspect_db_sync(&p)).await??;
        let engine = cfg.engine.clone();
        let database = db_label(&cfg);

        // 2. Snapshot before touching anything.
        let note = format!("agent-safe: before `artisan {command}` on {site}");
        let sandbox = open_sandbox(paths, socket, &cfg, &note).await?;
        let snapshot_id = sandbox.id();

        // 3. Run the migration and capture the resulting schema.
        let php = resolve_php_cli(paths, &php_ver)?;
        let run_path = path.clone();
        let run_cmd = command.to_string();
        let (code, stdout, stderr, after) = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(i32, String, String, Schema)> {
                let args: Vec<String> = std::iter::once("artisan".to_string())
                    .chain(run_cmd.split_whitespace().map(str::to_string))
                    .collect();
                let out = std::process::Command::new(&php)
                    .args(&args)
                    .current_dir(&run_path)
                    .output()
                    .map_err(|e| anyhow::anyhow!("failed to run php artisan: {e}"))?;
                let code = out.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let after = e_db::from_env(&run_path)
                    .and_then(|cfg| read_schema(&cfg).ok())
                    .unwrap_or_default();
                Ok((code, stdout, stderr, after))
            },
        )
        .await??;

        let success = code == 0;
        let should_rollback = !success || roll_back;

        // 4. Roll back on failure, or when a dry run was requested.
        let rollback_error = close_sandbox(socket, sandbox, should_rollback).await;
        let rolled_back = should_rollback && rollback_error.is_none();

        let note = if rolled_back && success {
            "dry run: the migration succeeded and was rolled back; schema_diff shows what it WOULD change"
        } else if rolled_back {
            "the migration failed and was rolled back to the pre-migration snapshot"
        } else if !success {
            "the migration failed and was NOT rolled back; use the snapshot_id to restore manually"
        } else {
            "the migration succeeded and was kept; use the snapshot_id to roll back if needed"
        };

        let result = json!({
            "site": site,
            "engine": engine,
            "database": database,
            "command": format!("php artisan {command}"),
            "snapshot_id": snapshot_id,
            "exit_code": code,
            "success": success,
            "rolled_back": rolled_back,
            "rollback_error": rollback_error,
            "schema_diff": schema_diff(&before, &after),
            "note": note,
            "stdout": truncate(&stdout, 8000),
            "stderr": truncate(&stderr, 8000),
        });

        audit_log(paths, &result);
        Ok(serde_json::to_string_pretty(&result)?)
    }

    /// Run a single WRITE SQL statement inside an automatic snapshot sandbox:
    /// snapshot first, run, diff the schema, and roll back on failure (or on
    /// request). Read-only statements are refused — use `grove_db_query`.
    async fn sql_sandboxed(
        paths: &GrovePaths,
        socket: &Path,
        site: &str,
        sql: &str,
        roll_back: bool,
    ) -> anyhow::Result<String> {
        if is_readonly(sql) {
            anyhow::bail!(
                "that statement looks read-only; use grove_db_query for SELECT/SHOW/EXPLAIN/PRAGMA"
            );
        }
        let (path, _php) = site_info(socket, site).await?;

        let p = path.clone();
        let (cfg, before) = tokio::task::spawn_blocking(move || inspect_db_sync(&p)).await??;
        let engine = cfg.engine.clone();
        let database = db_label(&cfg);

        let note = format!("agent-safe: before SQL on {site}");
        let sandbox = open_sandbox(paths, socket, &cfg, &note).await?;
        let snapshot_id = sandbox.id();

        let p2 = path.clone();
        let sql_owned = sql.to_string();
        let (success, rows_affected, run_error, after) = tokio::task::spawn_blocking(
            move || -> (bool, Option<u64>, Option<String>, Schema) {
                let cfg = match e_db::from_env(&p2) {
                    Some(c) => c,
                    None => {
                        return (false, None, Some("no database configured".into()), Schema::new())
                    }
                };
                let conn = match e_db::connect(&cfg) {
                    Ok(c) => c,
                    Err(e) => return (false, None, Some(e), Schema::new()),
                };
                let (success, rows_affected, run_error) = match e_db::query(&conn, &sql_owned, 1) {
                    Ok(r) => (true, r.rows_affected, None),
                    Err(e) => (false, None, Some(e)),
                };
                let after = read_schema(&cfg).unwrap_or_default();
                (success, rows_affected, run_error, after)
            },
        )
        .await?;

        let should_rollback = !success || roll_back;
        let rollback_error = close_sandbox(socket, sandbox, should_rollback).await;
        let rolled_back = should_rollback && rollback_error.is_none();

        let outcome_note = if rolled_back && success {
            "dry run: the statement succeeded and was rolled back; schema_diff shows what it WOULD change"
        } else if rolled_back {
            "the statement failed and was rolled back to the pre-write snapshot"
        } else if !success {
            "the statement failed and was NOT rolled back; use the snapshot_id to restore manually"
        } else {
            "the statement succeeded and was kept; use the snapshot_id to roll back if needed"
        };

        let result = json!({
            "site": site,
            "engine": engine,
            "database": database,
            "sql": truncate(sql, 2000),
            "snapshot_id": snapshot_id,
            "success": success,
            "rows_affected": rows_affected,
            "error": run_error,
            "rolled_back": rolled_back,
            "rollback_error": rollback_error,
            "schema_diff": schema_diff(&before, &after),
            "note": outcome_note,
        });

        audit_log(paths, &result);
        Ok(serde_json::to_string_pretty(&result)?)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn schema(pairs: &[(&str, &[&str])]) -> Schema {
            pairs
                .iter()
                .map(|(t, cols)| (t.to_string(), cols.iter().map(|c| c.to_string()).collect()))
                .collect()
        }

        #[test]
        fn snapshot_id_is_parsed_from_daemon_message() {
            let msg = "snapshot 20260718-120000 created (mysql, app, 4096 bytes)";
            assert_eq!(snapshot_id_from_msg(msg).unwrap(), "20260718-120000");
            assert!(snapshot_id_from_msg("").is_err());
        }

        #[test]
        fn schema_diff_reports_added_removed_and_changed() {
            let before = schema(&[("users", &["id:int:notnull"]), ("old", &["x:int:null"])]);
            let after = schema(&[
                ("users", &["id:int:notnull", "email:varchar:null"]),
                ("invoices", &["id:int:notnull"]),
            ]);
            let diff = schema_diff(&before, &after);
            assert_eq!(diff["added_tables"], serde_json::json!(["invoices"]));
            assert_eq!(diff["removed_tables"], serde_json::json!(["old"]));
            assert_eq!(
                diff["changed_tables"]["users"]["added_columns"],
                serde_json::json!(["email:varchar:null"])
            );
        }

        #[test]
        fn identical_schemas_have_no_diff() {
            let s = schema(&[("users", &["id:int:notnull"])]);
            let diff = schema_diff(&s, &s);
            assert_eq!(diff["added_tables"], serde_json::json!([]));
            assert_eq!(diff["removed_tables"], serde_json::json!([]));
            assert!(diff["changed_tables"].as_object().unwrap().is_empty());
        }

        #[test]
        fn truncate_caps_long_output() {
            assert_eq!(truncate("short", 10), "short");
            let out = truncate("abcdefghij", 4);
            assert!(out.starts_with("abcd"));
            assert!(out.contains("truncated"));
        }

        fn sqlite_cfg(path: &Path) -> e_db::DbConfig {
            e_db::DbConfig {
                engine: "sqlite".into(),
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            }
        }

        #[tokio::test]
        async fn sqlite_sandbox_restores_the_file_on_rollback() {
            let tmp = std::env::temp_dir().join(format!("grove-sbx-rb-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).unwrap();
            let db = tmp.join("app.sqlite");
            std::fs::write(&db, b"v1").unwrap();

            let paths = GrovePaths::with_base(&tmp);
            let socket = tmp.join("dummy.sock");
            let sandbox = open_sandbox(&paths, &socket, &sqlite_cfg(&db), "test")
                .await
                .unwrap();

            // Mutate after the snapshot, then roll back.
            std::fs::write(&db, b"v2-mutated").unwrap();
            assert!(close_sandbox(&socket, sandbox, true).await.is_none());
            assert_eq!(std::fs::read(&db).unwrap(), b"v1");

            let _ = std::fs::remove_dir_all(&tmp);
        }

        #[tokio::test]
        async fn sqlite_sandbox_keeps_changes_without_rollback() {
            let tmp = std::env::temp_dir().join(format!("grove-sbx-keep-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).unwrap();
            let db = tmp.join("app.sqlite");
            std::fs::write(&db, b"v1").unwrap();

            let paths = GrovePaths::with_base(&tmp);
            let socket = tmp.join("dummy.sock");
            let sandbox = open_sandbox(&paths, &socket, &sqlite_cfg(&db), "test")
                .await
                .unwrap();

            std::fs::write(&db, b"v2-kept").unwrap();
            assert!(close_sandbox(&socket, sandbox, false).await.is_none());
            assert_eq!(std::fs::read(&db).unwrap(), b"v2-kept");

            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
}

/// Reproducible environment bundles: package grove.toml + .env + database into
/// one shareable file, and restore it with a single command.
mod bundle {
    use super::*;
    use grove_core::ProjectFile;
    use grove_ipc::protocol::Request;
    use std::path::{Path, PathBuf};

    struct DbInfo {
        engine: String,
        database: String,
        sqlite_path: Option<PathBuf>,
    }

    fn unquote(s: &str) -> String {
        s.trim().trim_matches('"').trim_matches('\'').to_string()
    }

    fn read_env_db(dir: &Path) -> Option<DbInfo> {
        let env = std::fs::read_to_string(dir.join(".env")).ok()?;
        let (mut conn, mut database) = (String::new(), String::new());
        for line in env.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("DB_CONNECTION=") {
                conn = unquote(v);
            } else if let Some(v) = line.strip_prefix("DB_DATABASE=") {
                database = unquote(v);
            }
        }
        let engine = match conn.as_str() {
            "mysql" | "mariadb" => "mysql",
            "pgsql" | "postgres" | "postgresql" => "postgres",
            "sqlite" => "sqlite",
            _ => return None,
        }
        .to_string();
        let sqlite_path = if engine == "sqlite" {
            let p = if database.is_empty() {
                dir.join("database/database.sqlite")
            } else {
                let pb = PathBuf::from(&database);
                if pb.is_absolute() {
                    pb
                } else {
                    dir.join(&database)
                }
            };
            Some(p)
        } else {
            None
        };
        Some(DbInfo {
            engine,
            database,
            sqlite_path,
        })
    }

    pub async fn export(
        paths: &GrovePaths,
        path: Option<String>,
        out: Option<String>,
        no_env: bool,
        json: bool,
    ) -> anyhow::Result<()> {
        let dir = match path {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir()?,
        };
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);

        let Some(pf) = ProjectFile::load(&dir).map_err(|e| anyhow::anyhow!(e))? else {
            anyhow::bail!(
                "no grove.toml in {} — create one with `grove up --write` first",
                dir.display()
            );
        };
        let name = pf.site_name(&dir);
        let socket = paths.ipc_socket();

        let stage =
            std::env::temp_dir().join(format!("grove-bundle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)?;

        std::fs::copy(ProjectFile::path_in(&dir), stage.join("grove.toml"))?;

        let has_env = !no_env && dir.join(".env").exists();
        if has_env {
            std::fs::copy(dir.join(".env"), stage.join("env"))?;
        }

        let (mut db_engine, mut db_database) = (String::new(), String::new());
        if let Some(info) = read_env_db(&dir) {
            db_database = info.database.clone();
            match info.engine.as_str() {
                "sqlite" => {
                    if let Some(sp) = &info.sqlite_path {
                        if sp.exists() {
                            std::fs::copy(sp, stage.join("database.sqlite"))?;
                            db_engine = "sqlite".into();
                        }
                    }
                }
                eng => {
                    if !client::is_running(&socket).await {
                        anyhow::bail!("Grove daemon is not running (needed to dump the database). Start it with `grove start`.");
                    }
                    if !json {
                        println!("Dumping {eng} database {db_database}…");
                    }
                    let sqlpath = stage.join("database.sql");
                    let resp = client::send(
                        &socket,
                        &Request::DbDumpFile {
                            engine: eng.to_string(),
                            database: if db_database.is_empty() {
                                None
                            } else {
                                Some(db_database.clone())
                            },
                            path: sqlpath.to_string_lossy().into_owned(),
                        },
                    )
                    .await?;
                    if !resp.ok {
                        anyhow::bail!(
                            "database dump failed: {}",
                            resp.error.as_deref().unwrap_or("unknown")
                        );
                    }
                    db_engine = eng.to_string();
                }
            }
        }

        let meta = format!(
            "name = {name:?}\ndb_engine = {db_engine:?}\ndb_database = {db_database:?}\nhas_env = {has_env}\n"
        );
        std::fs::write(stage.join("bundle.toml"), meta)?;

        let out_file = out
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.join(format!("{name}.grovebundle")));
        write_targz(&stage, &out_file)?;
        let _ = std::fs::remove_dir_all(&stage);

        output::print_message(
            &format!(
                "wrote {} — share it; restore with `grove bundle import <file>`",
                out_file.display()
            ),
            json,
        );
        Ok(())
    }

    pub async fn import(
        paths: &GrovePaths,
        file: String,
        into: Option<String>,
        json: bool,
    ) -> anyhow::Result<()> {
        let file = PathBuf::from(file);
        if !file.exists() {
            anyhow::bail!("no such bundle: {}", file.display());
        }
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string();
        let target = match into {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir()?.join(&stem),
        };
        std::fs::create_dir_all(&target)?;
        extract_targz(&file, &target)?;

        let meta = std::fs::read_to_string(target.join("bundle.toml")).unwrap_or_default();
        let get = |k: &str| -> String {
            for line in meta.lines() {
                if let Some(v) = line.trim().strip_prefix(&format!("{k} = ")) {
                    return v.trim().trim_matches('"').to_string();
                }
            }
            String::new()
        };
        let db_engine = get("db_engine");
        let has_env = get("has_env") == "true";

        if has_env && target.join("env").exists() {
            let _ = std::fs::rename(target.join("env"), target.join(".env"));
        }

        if !json {
            println!("Restoring into {}", target.display());
        }
        lifecycle::up(
            paths,
            Some(target.to_string_lossy().into_owned()),
            false,
            false,
            json,
        )
        .await?;

        let socket = paths.ipc_socket();
        match db_engine.as_str() {
            "sqlite" => {
                let src = target.join("database.sqlite");
                let dest = read_env_db(&target).and_then(|i| i.sqlite_path);
                if let (true, Some(sp)) = (src.exists(), dest) {
                    if let Some(parent) = sp.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::copy(&src, &sp)?;
                }
            }
            "mysql" | "postgres" => {
                let sql = target.join("database.sql");
                if sql.exists() {
                    if !json {
                        println!("Loading {db_engine} database…");
                    }
                    let resp = client::send(
                        &socket,
                        &Request::DbRestoreFile {
                            engine: db_engine.clone(),
                            path: sql.to_string_lossy().into_owned(),
                        },
                    )
                    .await?;
                    if !resp.ok {
                        anyhow::bail!(
                            "database restore failed: {}",
                            resp.error.as_deref().unwrap_or("unknown")
                        );
                    }
                }
            }
            _ => {}
        }

        let _ = std::fs::remove_file(target.join("bundle.toml"));
        let _ = std::fs::remove_file(target.join("database.sql"));
        let _ = std::fs::remove_file(target.join("database.sqlite"));

        output::print_message(
            &format!(
                "restored to {} — live at https://{}.test",
                target.display(),
                get("name")
            ),
            json,
        );
        Ok(())
    }

    fn write_targz(src_dir: &Path, out: &Path) -> anyhow::Result<()> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let f = std::fs::File::create(out)?;
        let enc = GzEncoder::new(f, Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all(".", src_dir)?;
        tar.into_inner()?.finish()?;
        Ok(())
    }

    fn extract_targz(file: &Path, dest: &Path) -> anyhow::Result<()> {
        use flate2::read::GzDecoder;
        let f = std::fs::File::open(file)?;
        let mut ar = tar::Archive::new(GzDecoder::new(f));
        ar.unpack(dest)?;
        Ok(())
    }
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
    /// The shims live under the user's home (not `$GROVE_HOME`, which is
    /// root-owned under the LaunchDaemon and so not writable by the shims' user).
    fn shims_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".grove/bin")
    }

    pub fn path(action: PathAction, json: bool) -> anyhow::Result<()> {
        let shims = shims_dir();
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

/// Grove Teams: end-to-end encrypted secret sync (client-side crypto).
mod secret {
    use super::*;
    use grove_secrets::{HttpStore, Identity, PublicKey, SecretsClient};
    use std::path::PathBuf;

    const DEFAULT_SERVER: &str = "https://teams.elyracode.com";

    fn identity_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".grove/identity")
    }

    fn load_or_create_identity() -> anyhow::Result<Identity> {
        let path = identity_path();
        if let Ok(secret) = std::fs::read_to_string(&path) {
            return Identity::from_secret(secret.trim()).map_err(|e| anyhow::anyhow!("{e}"));
        }
        let id = Identity::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, id.secret_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(id)
    }

    fn license_token(paths: &GrovePaths) -> anyhow::Result<String> {
        let key = std::fs::read_to_string(paths.base().join("license.key")).map_err(|_| {
            anyhow::anyhow!("no license found — activate one with `grove license activate <key>`")
        })?;
        let key = key.trim().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let claims =
            grove_license::verify(&key, now).map_err(|e| anyhow::anyhow!("license: {e}"))?;
        if !claims.is_teams() {
            anyhow::bail!("secret sync is a Grove Teams feature — upgrade at elyracode.com/grove");
        }
        Ok(key)
    }

    fn server() -> String {
        std::env::var("GROVE_TEAMS_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into())
    }

    fn client(paths: &GrovePaths) -> anyhow::Result<SecretsClient<HttpStore>> {
        let token = license_token(paths)?;
        let id = load_or_create_identity()?;
        Ok(SecretsClient::new(HttpStore::new(server(), token), id))
    }

    pub fn run(paths: &GrovePaths, action: SecretAction, json: bool) -> anyhow::Result<()> {
        match action {
            SecretAction::Whoami => {
                let id = load_or_create_identity()?;
                println!("{}", id.public().as_str());
            }
            SecretAction::Set {
                project,
                assignment,
            } => {
                let (k, v) = assignment
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("use KEY=VALUE"))?;
                let c = client(paths)?;
                let me = load_or_create_identity()?.public();
                if c.members(&project)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .is_empty()
                {
                    c.init_project(&project, &[me])
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                c.set(&project, k, v).map_err(|e| anyhow::anyhow!("{e}"))?;
                output::print_message(&format!("set {k} in {project}"), json);
            }
            SecretAction::Pull { project, write } => {
                let c = client(paths)?;
                let env = c.pull(&project).map_err(|e| anyhow::anyhow!("{e}"))?;
                if write {
                    std::fs::write(".env", env.to_dotenv())?;
                    output::print_message("wrote .env", json);
                } else {
                    print!("{}", env.to_dotenv());
                }
            }
            SecretAction::Share {
                project,
                public_key,
            } => {
                let c = client(paths)?;
                c.add_member(&project, PublicKey(public_key.clone()))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                output::print_message(&format!("granted access to {project}"), json);
            }
            SecretAction::Revoke {
                project,
                public_key,
            } => {
                let c = client(paths)?;
                c.remove_member(&project, &PublicKey(public_key.clone()))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                output::print_message(&format!("revoked access to {project}"), json);
            }
            SecretAction::Members { project } => {
                let c = client(paths)?;
                for m in c.members(&project).map_err(|e| anyhow::anyhow!("{e}"))? {
                    println!("{}", m.as_str());
                }
            }
        }
        Ok(())
    }
}

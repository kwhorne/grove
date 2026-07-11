// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Grove desktop GUI. The Rust side is a thin bridge: every command
//! proxies to the running daemon over the same `grove-ipc` JSON-RPC the CLI
//! uses, so the GUI and CLI are always in parity.

use std::path::PathBuf;

use grove_core::paths::GrovePaths;
use grove_core::site::ResolvedSite;
use grove_core::RequestEntry;
use grove_ipc::client;
use grove_ipc::protocol::{
    DaemonStatus, DiagnosticEntry, LogEntry, LogSource, NodeVersion, Request, ResponseData,
    SettingsPatch, SettingsView, TunnelRequestEntry, TunnelStatus, XdebugStatus,
};
use grove_license::LicenseClaims;
use grove_runtime::PhpRegistry;
use grove_services::{CapturedEmail, DbConnSpec, EmailSummary, ServiceStatus};
use serde::Serialize;

type CmdResult<T> = Result<T, String>;

fn paths() -> CmdResult<GrovePaths> {
    GrovePaths::discover().map_err(|e| e.to_string())
}

/// Send a request to the daemon and return its data payload.
async fn call(req: Request) -> CmdResult<ResponseData> {
    let p = paths()?;
    let resp = client::send(&p.ipc_socket(), &req)
        .await
        .map_err(|e| format!("daemon unreachable: {e}"))?;
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "unknown daemon error".into()));
    }
    resp.data.ok_or_else(|| "empty daemon response".into())
}

/// Send a request that returns a human-readable status message.
async fn message(req: Request) -> CmdResult<String> {
    match call(req).await? {
        ResponseData::Message(m) => Ok(m),
        _ => Ok("ok".into()),
    }
}

// ---- queries -------------------------------------------------------------

#[tauri::command]
async fn daemon_running() -> bool {
    match paths() {
        Ok(p) => client::is_running(&p.ipc_socket()).await,
        Err(_) => false,
    }
}

#[tauri::command]
async fn get_status() -> CmdResult<DaemonStatus> {
    match call(Request::Status).await? {
        ResponseData::Status(s) => Ok(s),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn list_sites() -> CmdResult<Vec<ResolvedSite>> {
    match call(Request::ListSites).await? {
        ResponseData::Sites(s) => Ok(s.into_iter().map(|ss| ss.site).collect()),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn doctor() -> CmdResult<Vec<DiagnosticEntry>> {
    match call(Request::Doctor).await? {
        ResponseData::Doctor(d) => Ok(d),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn mail_list() -> CmdResult<Vec<EmailSummary>> {
    match call(Request::MailList).await? {
        ResponseData::Mail(m) => Ok(m),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn mail_get(id: u64) -> CmdResult<Option<CapturedEmail>> {
    match call(Request::MailGet { id }).await? {
        ResponseData::MailMessage(m) => Ok(m),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn mail_clear() -> CmdResult<String> {
    message(Request::MailClear).await
}

#[tauri::command]
async fn get_settings() -> CmdResult<SettingsView> {
    match call(Request::GetSettings).await? {
        ResponseData::Settings(s) => Ok(s),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn update_settings(patch: SettingsPatch) -> CmdResult<String> {
    message(Request::UpdateSettings { patch }).await
}

#[tauri::command]
async fn service_list() -> CmdResult<Vec<ServiceStatus>> {
    match call(Request::ServiceList).await? {
        ResponseData::Services(s) => Ok(s),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn service_install(key: String) -> CmdResult<String> {
    message(Request::ServiceInstall { key }).await
}

#[tauri::command]
async fn service_start(key: String) -> CmdResult<String> {
    message(Request::ServiceStart { key }).await
}

#[tauri::command]
async fn service_stop(key: String) -> CmdResult<String> {
    message(Request::ServiceStop { key }).await
}

#[tauri::command]
async fn service_restart(key: String) -> CmdResult<String> {
    message(Request::ServiceRestart { key }).await
}

#[tauri::command]
async fn service_set_port(key: String, port: u16) -> CmdResult<String> {
    message(Request::ServiceSetPort { key, port }).await
}

#[tauri::command]
async fn env_snippet(site: Option<String>) -> CmdResult<String> {
    message(Request::EnvSnippet { site }).await
}

#[tauri::command]
async fn log_sources() -> CmdResult<Vec<LogSource>> {
    match call(Request::LogSources).await? {
        ResponseData::LogSources(s) => Ok(s),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn log_entries(path: String, limit: usize) -> CmdResult<Vec<LogEntry>> {
    match call(Request::LogEntries { path, limit }).await? {
        ResponseData::LogEntries(e) => Ok(e),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn node_list() -> CmdResult<Vec<NodeVersion>> {
    match call(Request::NodeList).await? {
        ResponseData::Nodes(n) => Ok(n),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn node_install(version: String) -> CmdResult<String> {
    message(Request::NodeInstall { version }).await
}

#[tauri::command]
async fn tunnel_start(
    site: String,
    subdomain: Option<String>,
    basic_auth: Option<String>,
) -> CmdResult<Vec<TunnelStatus>> {
    match call(Request::TunnelStart {
        site,
        subdomain,
        basic_auth,
    })
    .await?
    {
        ResponseData::Tunnels(t) => Ok(t),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn tunnel_stop(site: String) -> CmdResult<String> {
    message(Request::TunnelStop { site }).await
}

#[tauri::command]
async fn forget_site(name: String) -> CmdResult<String> {
    message(Request::ForgetSite { name }).await
}

#[tauri::command]
async fn docker_control(id: String, action: String) -> CmdResult<String> {
    message(Request::DockerControl { id, action }).await
}

#[tauri::command]
async fn dev_start(site: String) -> CmdResult<String> {
    message(Request::DevStart { site }).await
}

#[tauri::command]
async fn dev_stop(site: String) -> CmdResult<String> {
    message(Request::DevStop { site }).await
}

#[tauri::command]
async fn dev_list() -> CmdResult<Vec<String>> {
    match call(Request::DevList).await? {
        ResponseData::DevSites(s) => Ok(s),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn db_convert(source: DbConnSpec, target: DbConnSpec) -> CmdResult<String> {
    message(Request::DbConvert { source, target }).await
}

#[tauri::command]
async fn debug_status() -> CmdResult<XdebugStatus> {
    match call(Request::Debug { enable: None }).await? {
        ResponseData::Xdebug(x) => Ok(x),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn debug_set(enable: bool) -> CmdResult<XdebugStatus> {
    match call(Request::Debug {
        enable: Some(enable),
    })
    .await?
    {
        ResponseData::Xdebug(x) => Ok(x),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn mysql_migrate(
    host: String,
    port: u16,
    user: String,
    password: String,
) -> CmdResult<String> {
    message(Request::MysqlMigrate {
        host,
        port,
        user,
        password,
    })
    .await
}

#[tauri::command]
async fn tunnel_list() -> CmdResult<Vec<TunnelStatus>> {
    match call(Request::TunnelList).await? {
        ResponseData::Tunnels(t) => Ok(t),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn tunnel_requests(site: Option<String>) -> CmdResult<Vec<TunnelRequestEntry>> {
    match call(Request::TunnelRequests { site }).await? {
        ResponseData::TunnelRequests(r) => Ok(r),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn request_log(site: Option<String>, limit: usize) -> CmdResult<Vec<RequestEntry>> {
    match call(Request::RequestLog { site, limit }).await? {
        ResponseData::Requests(r) => Ok(r),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn request_detail(id: u64) -> CmdResult<Option<grove_core::reqlog::RequestDetail>> {
    match call(Request::RequestDetail { id }).await? {
        ResponseData::RequestDetail(d) => Ok(d),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn replay_request(id: u64) -> CmdResult<(u16, u64)> {
    match call(Request::ReplayRequest { id }).await? {
        ResponseData::Replayed {
            status,
            duration_ms,
        } => Ok((status, duration_ms)),
        _ => Err("unexpected response".into()),
    }
}

// ---------------------------------------------------------------------------
// Database client (Pro) — reuses the `e-db` engine, auto-discovering each site's
// connection from its .env. Free tier is read-only; editing is Pro.
// ---------------------------------------------------------------------------

fn db_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, e_db::DbConfig>> {
    static C: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, e_db::DbConfig>>,
    > = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[derive(serde::Serialize)]
struct DbConnInfo {
    key: String,
    label: String,
    engine: String,
    database: String,
    environment: String,
    is_prod: bool,
}

#[tauri::command]
async fn db_connections() -> CmdResult<Vec<DbConnInfo>> {
    let sites = match call(Request::ListSites).await? {
        ResponseData::Sites(s) => s,
        _ => return Err("unexpected response".into()),
    };
    let mut infos = Vec::new();
    let mut cache = db_cache().lock().unwrap();
    cache.clear();
    for ss in sites {
        let site = ss.site;
        if let Some(cfg) = e_db::from_env(&site.path) {
            if cfg.engine.is_empty() {
                continue;
            }
            infos.push(DbConnInfo {
                key: site.name.clone(),
                label: if cfg.database.is_empty() {
                    site.name.clone()
                } else {
                    cfg.database.clone()
                },
                engine: cfg.engine.clone(),
                database: cfg.database.clone(),
                environment: format!("{:?}", cfg.environment()),
                is_prod: cfg.looks_like_prod(),
            });
            cache.insert(site.name, cfg);
        }
    }
    Ok(infos)
}

fn db_config_for(key: &str) -> Result<e_db::DbConfig, String> {
    db_cache()
        .lock()
        .unwrap()
        .get(key)
        .cloned()
        .ok_or_else(|| format!("unknown connection {key}"))
}

fn looks_readonly(sql: &str) -> bool {
    let s = sql.trim_start().to_ascii_lowercase();
    [
        "select", "with", "show", "explain", "pragma", "describe", "desc",
    ]
    .iter()
    .any(|kw| s.starts_with(kw))
}

async fn is_pro() -> bool {
    matches!(
        call(Request::LicenseStatus).await,
        Ok(ResponseData::License(Some(c))) if c.is_pro()
    )
}

#[tauri::command]
async fn db_tables(key: String) -> CmdResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        let conn = e_db::connect(&cfg)?;
        e_db::tables(&conn)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_query(key: String, sql: String) -> CmdResult<e_db::QueryResult> {
    // Free tier is read-only; anything that writes requires Grove Pro.
    if !looks_readonly(&sql) && !is_pro().await {
        return Err(
            "Editing data is a Grove Pro feature — activate a license to run writes.".into(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        let conn = e_db::connect(&cfg)?;
        e_db::query(&conn, &sql, 500)
    })
    .await
    .map_err(|e| e.to_string())?
}

// Pro data operations (schema inspector + row editing) are delegated to the
// `grove-pro` crate. In-tree it's an open-source stub that returns "not
// included"; official release builds swap in the proprietary implementation, so
// there's nothing in public source to patch. The free read-only browser
// (db_connections/db_tables/db_query) never touches it.
use grove_pro as prodb;

const DB_PRO_MSG: &str =
    "This is a Grove Pro feature — activate a license to unlock it (elyracode.com/grove).";

type PkPairs = Vec<(String, Option<String>)>;

#[tauri::command]
async fn db_columns(key: String, table: String) -> CmdResult<Vec<e_db::ColumnInfo>> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        prodb::columns(&cfg, &table)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
struct IndexRow {
    name: String,
    unique: bool,
    columns: Vec<String>,
}

#[derive(serde::Serialize)]
struct FkRow {
    table: String,
    column: String,
    ref_table: String,
    ref_column: String,
}

#[tauri::command]
async fn db_indexes(key: String, table: String) -> CmdResult<Vec<IndexRow>> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        Ok(prodb::indexes(&cfg, &table)?
            .into_iter()
            .map(|i| IndexRow {
                name: i.name,
                unique: i.unique,
                columns: i.columns,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_foreign_keys(key: String) -> CmdResult<Vec<FkRow>> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        Ok(prodb::foreign_keys(&cfg)?
            .into_iter()
            .map(|f| FkRow {
                table: f.table,
                column: f.column,
                ref_table: f.ref_table,
                ref_column: f.ref_column,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_table_ddl(key: String, table: String) -> CmdResult<String> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        prodb::table_ddl(&cfg, &table)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_update_cell(
    key: String,
    table: String,
    column: String,
    value: Option<String>,
    pk: PkPairs,
) -> CmdResult<u64> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        prodb::update_cell(&cfg, &table, &column, value.as_deref(), &pk)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_delete_row(key: String, table: String, pk: PkPairs) -> CmdResult<u64> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        prodb::delete_row(&cfg, &table, &pk)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn db_insert_row(key: String, table: String, values: PkPairs) -> CmdResult<u64> {
    if !is_pro().await {
        return Err(DB_PRO_MSG.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = db_config_for(&key)?;
        prodb::insert_row(&cfg, &table, &values)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn license_status() -> CmdResult<Option<LicenseClaims>> {
    match call(Request::LicenseStatus).await? {
        ResponseData::License(l) => Ok(l),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn license_activate(key: String) -> CmdResult<Option<LicenseClaims>> {
    match call(Request::LicenseActivate { key }).await? {
        ResponseData::License(l) => Ok(l),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn license_deactivate() -> CmdResult<Option<LicenseClaims>> {
    match call(Request::LicenseDeactivate).await? {
        ResponseData::License(l) => Ok(l),
        _ => Err("unexpected response".into()),
    }
}

#[derive(Serialize)]
struct PhpBuildView {
    version: String,
    fpm_binary: String,
    user_registered: bool,
}

#[tauri::command]
async fn php_versions() -> CmdResult<Vec<NodeVersion>> {
    match call(Request::PhpVersionList).await? {
        ResponseData::PhpVersions(v) => Ok(v),
        _ => Err("unexpected response".into()),
    }
}

#[tauri::command]
async fn php_install(version: String) -> CmdResult<String> {
    message(Request::PhpInstall { version }).await
}

/// PHP builds are local, re-derivable state — read the registry directly.
#[tauri::command]
fn php_list() -> CmdResult<Vec<PhpBuildView>> {
    let p = paths()?;
    let reg = PhpRegistry::load(&p);
    Ok(reg
        .iter()
        .map(|b| PhpBuildView {
            version: b.version.clone(),
            fpm_binary: b.fpm_binary.display().to_string(),
            user_registered: b.user_registered,
        })
        .collect())
}

// ---- mutations -----------------------------------------------------------

#[tauri::command]
async fn secure_site(name: String, enable: bool) -> CmdResult<String> {
    message(Request::Secure { name, enable }).await
}

#[tauri::command]
async fn isolate_site(name: String, version: Option<String>) -> CmdResult<String> {
    message(Request::Isolate { name, version }).await
}

#[tauri::command]
async fn site_node(name: String, version: Option<String>) -> CmdResult<String> {
    message(Request::SiteNode { name, version }).await
}

#[tauri::command]
async fn park_dir(path: String) -> CmdResult<String> {
    message(Request::Park { path }).await
}

#[tauri::command]
async fn unpark_dir(path: String) -> CmdResult<String> {
    message(Request::Unpark { path }).await
}

#[tauri::command]
async fn link_dir(path: String, name: Option<String>) -> CmdResult<String> {
    message(Request::Link { path, name }).await
}

#[tauri::command]
async fn unlink_site(name: String) -> CmdResult<String> {
    message(Request::Unlink { name }).await
}

#[tauri::command]
async fn create_site(
    name: String,
    parent: String,
    kind: String,
    php: Option<String>,
    init_git: bool,
) -> CmdResult<String> {
    message(Request::CreateSite {
        name,
        parent,
        kind,
        php,
        init_git,
    })
    .await
}

#[tauri::command]
async fn proxy_site(name: String, url: String) -> CmdResult<String> {
    message(Request::Proxy { name, url }).await
}

// ---- lifecycle + OS bridges ---------------------------------------------

#[tauri::command]
async fn stop_daemon() -> CmdResult<String> {
    message(Request::Shutdown).await
}

/// Restart the background daemon (picks up an updated app binary). No password
/// prompt: the root LaunchDaemon re-execs itself.
#[tauri::command]
async fn restart_daemon() -> CmdResult<String> {
    let p = paths()?;
    let socket = p.ipc_socket();
    let _ = client::send(&socket, &Request::RestartDaemon).await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    for _ in 0..60 {
        if client::is_running(&socket).await {
            return Ok("daemon restarted".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(
        "daemon did not come back — is the background service installed? (sudo grove install)"
            .into(),
    )
}

/// Locate the `grove` CLI binary (next to the GUI, then PATH) and spawn the
/// daemon detached.
#[tauri::command]
async fn start_daemon() -> CmdResult<String> {
    let p = paths()?;
    if client::is_running(&p.ipc_socket()).await {
        return Ok("already running".into());
    }
    let grove = locate_grove_binary();
    let mut cmd = std::process::Command::new(grove);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut cmd);
    cmd.spawn().map_err(|e| format!("spawning daemon: {e}"))?;

    for _ in 0..100 {
        if client::is_running(&p.ipc_socket()).await {
            return Ok("daemon started".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err("daemon did not start in time".into())
}

#[tauri::command]
fn open_url(url: String) -> CmdResult<()> {
    open_external(&url)
}

#[tauri::command]
fn open_path(path: String) -> CmdResult<()> {
    open_external(&path)
}

fn locate_grove_binary() -> PathBuf {
    // 1. Bundled sidecar lives next to the GUI binary (Contents/MacOS/grove).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("grove");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    // 2. Common install locations (GUI apps don't inherit the shell PATH).
    let mut candidates = vec![
        PathBuf::from("/usr/local/bin/grove"),
        PathBuf::from("/opt/homebrew/bin/grove"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".cargo/bin/grove"));
    }
    for c in candidates {
        if c.exists() {
            return c;
        }
    }
    // 3. Last resort: rely on PATH.
    PathBuf::from("grove")
}

fn open_external(target: &str) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    std::process::Command::new(program)
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
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
    #[link_name = "setsid"]
    fn libc_setsid() -> i32;
}

/// Build the menu-bar tray icon + menu, and wire its actions.
fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::Manager;

    fn show_main(app: &tauri::AppHandle) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
    }

    let open_i = MenuItem::with_id(app, "open", "Open Grove", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit Grove", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open_i, &PredefinedMenuItem::separator(app)?, &quit_i],
    )?;

    // Embed a transparent, menu-bar-optimized version of the app icon (the
    // node-graph mark). Kept in colour (not a template) so it shows the brand.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    TrayIconBuilder::with_id("grove-tray")
        .icon(icon)
        .icon_as_template(false)
        .tooltip("Grove")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // macOS menu-bar (system tray) icon with a small menu.
            install_tray(app.handle())?;

            // Open devtools automatically only in debug builds.
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                if let Some(win) = app.get_webview_window("main") {
                    win.open_devtools();
                }
            }
            Ok(())
        })
        // Closing the window hides it (Grove keeps running in the menu bar);
        // quit via the tray menu.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            daemon_running,
            get_status,
            list_sites,
            doctor,
            mail_list,
            mail_get,
            mail_clear,
            get_settings,
            update_settings,
            service_list,
            service_install,
            service_start,
            service_stop,
            service_restart,
            service_set_port,
            env_snippet,
            log_sources,
            log_entries,
            node_list,
            node_install,
            tunnel_start,
            tunnel_stop,
            forget_site,
            docker_control,
            dev_start,
            dev_stop,
            dev_list,
            mysql_migrate,
            db_convert,
            debug_status,
            debug_set,
            restart_daemon,
            tunnel_list,
            tunnel_requests,
            request_log,
            request_detail,
            replay_request,
            license_status,
            license_activate,
            license_deactivate,
            db_connections,
            db_tables,
            db_query,
            db_columns,
            db_indexes,
            db_foreign_keys,
            db_table_ddl,
            db_update_cell,
            db_delete_row,
            db_insert_row,
            php_versions,
            php_install,
            php_list,
            secure_site,
            isolate_site,
            site_node,
            park_dir,
            unpark_dir,
            link_dir,
            unlink_site,
            create_site,
            proxy_site,
            stop_daemon,
            start_daemon,
            open_url,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Grove GUI");
}

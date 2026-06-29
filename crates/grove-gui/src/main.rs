// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Grove desktop GUI (PRD §6.8). The Rust side is a thin bridge: every command
//! proxies to the running daemon over the same `grove-ipc` JSON-RPC the CLI
//! uses, so the GUI and CLI are always in parity.

use std::path::PathBuf;

use grove_core::paths::GrovePaths;
use grove_core::site::ResolvedSite;
use grove_ipc::client;
use grove_ipc::protocol::{DaemonStatus, DiagnosticEntry, Request, ResponseData};
use grove_runtime::PhpRegistry;
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

#[derive(Serialize)]
struct PhpBuildView {
    version: String,
    fpm_binary: String,
    user_registered: bool,
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
async fn proxy_site(name: String, url: String) -> CmdResult<String> {
    message(Request::Proxy { name, url }).await
}

// ---- lifecycle + OS bridges ---------------------------------------------

#[tauri::command]
async fn stop_daemon() -> CmdResult<String> {
    message(Request::Shutdown).await
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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("grove");
            if sibling.exists() {
                return sibling;
            }
        }
    }
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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            daemon_running,
            get_status,
            list_sites,
            doctor,
            php_list,
            secure_site,
            isolate_site,
            park_dir,
            unpark_dir,
            link_dir,
            unlink_site,
            proxy_site,
            stop_daemon,
            start_daemon,
            open_url,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Grove GUI");
}

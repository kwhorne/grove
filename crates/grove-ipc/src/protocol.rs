//! Request/Response message types exchanged over the IPC channel.

use serde::{Deserialize, Serialize};

use grove_core::site::ResolvedSite;

/// Commands the daemon understands. Mirrors the CLI/GUI action surface so both
/// frontends stay in parity (PRD §6.9).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Liveness probe + version handshake.
    Ping,
    /// Full daemon + sites status snapshot.
    Status,
    /// List every resolved site.
    ListSites,
    /// Park a directory (each subdir becomes a site).
    Park { path: String },
    /// Stop parking a directory.
    Unpark { path: String },
    /// Link the given directory as a single named site.
    Link { path: String, name: Option<String> },
    /// Remove a linked site.
    Unlink { name: String },
    /// Toggle HTTPS for a site.
    Secure { name: String, enable: bool },
    /// Pin a PHP version for a site (isolate / unisolate when `version` is None).
    Isolate { name: String, version: Option<String> },
    /// Route a `*.tld` host to a running upstream dev server.
    Proxy { name: String, url: String },
    /// Set the global default PHP version (`grove use`).
    SetDefaultPhp { version: String },
    /// Ask the daemon to re-read config + rebuild the registry.
    Reload,
    /// Diagnostics (PRD §7 — `grove doctor`).
    Doctor,
    /// Ask the daemon to shut down gracefully.
    Shutdown,
}

/// Uniform response envelope. `ok=false` carries a human-readable `error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseData>,
}

impl Response {
    pub fn ok(data: ResponseData) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }

    pub fn empty() -> Self {
        Self {
            ok: true,
            error: None,
            data: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseData {
    Pong { version: String },
    Status(DaemonStatus),
    Sites(Vec<SiteStatus>),
    Message(String),
    Doctor(Vec<DiagnosticEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub tld: String,
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
    pub site_count: usize,
    pub services: Vec<ServiceState>,
}

/// A site plus any live runtime info worth surfacing in the GUI/CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteStatus {
    #[serde(flatten)]
    pub site: ResolvedSite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    pub name: String,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEntry {
    pub check: String,
    pub status: DiagnosticStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

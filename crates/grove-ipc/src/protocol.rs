//! Request/Response message types exchanged over the IPC channel.

use serde::{Deserialize, Serialize};

use grove_core::site::ResolvedSite;
use grove_services::{CapturedEmail, EmailSummary, ServiceStatus};

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
    Isolate {
        name: String,
        version: Option<String>,
    },
    /// Pin a Node.js version for a site (clear when `version` is None).
    SiteNode {
        name: String,
        version: Option<String>,
    },
    /// Route a `*.tld` host to a running upstream dev server.
    Proxy { name: String, url: String },
    /// Set the global default PHP version (`grove use`).
    SetDefaultPhp { version: String },
    /// Fetch the editable settings (general + services + parked paths).
    GetSettings,
    /// Apply a partial settings update. Unset fields are left unchanged.
    UpdateSettings { patch: SettingsPatch },
    /// Download + install a static PHP-FPM build for the given version.
    PhpInstall { version: String },
    /// List offered + installed PHP versions (for the GUI PHP panel).
    PhpVersionList,
    /// List bundled services and their install/run state.
    ServiceList,
    /// Download + initialise a bundled service.
    ServiceInstall { key: String },
    /// Start a bundled service.
    ServiceStart { key: String },
    /// Stop a bundled service.
    ServiceStop { key: String },
    /// Restart a bundled service.
    ServiceRestart { key: String },
    /// Override a bundled service's listen port.
    ServiceSetPort { key: String, port: u16 },
    /// Generate a `.env` snippet wiring an app to Grove's bundled services.
    EnvSnippet { site: Option<String> },
    /// List discoverable log files (per-site Laravel logs + Grove service logs).
    LogSources,
    /// Read recent parsed entries from one log file (must be a known source).
    LogEntries { path: String, limit: usize },
    /// List installed + offered Node.js versions.
    NodeList,
    /// Download + install a Node.js version (major or exact).
    NodeInstall { version: String },
    /// Ask the daemon to re-read config + rebuild the registry.
    Reload,
    /// Diagnostics (PRD §7 — `grove doctor`).
    Doctor,
    /// List captured emails (newest first).
    MailList,
    /// Fetch one captured email in full.
    MailGet { id: u64 },
    /// Discard all captured emails.
    MailClear,
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
    Mail(Vec<EmailSummary>),
    MailMessage(Option<CapturedEmail>),
    Settings(SettingsView),
    Services(Vec<ServiceStatus>),
    LogSources(Vec<LogSource>),
    LogEntries(Vec<LogEntry>),
    Nodes(Vec<NodeVersion>),
    PhpVersions(Vec<NodeVersion>),
}

/// A Node.js major version: offered for install and, if present, its installed
/// full version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeVersion {
    pub major: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A discoverable log file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSource {
    /// Display label, e.g. "myapp · laravel.log".
    pub name: String,
    /// Absolute path on disk.
    pub path: String,
    /// "laravel" | "service".
    pub kind: String,
}

/// A single parsed log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub datetime: String,
    pub message: String,
    /// Stacktrace / JSON context belonging to this entry, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Snapshot of the editable settings shown in the GUI settings panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub tld: String,
    pub default_php: String,
    pub auto_start: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
    pub mail_enabled: bool,
    pub mail_port: u16,
    /// Parked directories (raw, unexpanded).
    pub parked: Vec<String>,
    /// PHP versions currently available to Grove.
    pub php_versions: Vec<String>,
}

/// Partial update; only `Some` fields are applied. Changing ports or the TLD
/// requires a daemon restart to take effect on the listeners.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub tld: Option<String>,
    pub default_php: Option<String>,
    pub auto_start: Option<bool>,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    pub dns_port: Option<u16>,
    pub mail_enabled: Option<bool>,
    pub mail_port: Option<u16>,
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

//! Platform integration: DNS resolver hookup, CA trust store, and
//! service installation. Each OS gets its own module behind a common API; the
//! rest of Grove never touches platform specifics directly.
//!
//! Privileged operations are concentrated here so the rest of the daemon can
//! run unprivileged.

use std::path::Path;

pub mod service;

#[derive(Debug, thiserror::Error)]
pub enum OsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("command {cmd} failed: {detail}")]
    Command { cmd: String, detail: String },
    #[error("operation not supported on this platform: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, OsError>;

/// Operations every platform backend must provide.
pub trait PlatformIntegration {
    /// Install a system resolver entry so `*.<tld>` is sent to Grove's DNS.
    /// `dns_port` is where Grove's resolver listens (usually 53).
    fn install_resolver(&self, tld: &str, dns_port: u16) -> Result<()>;

    /// Remove the resolver entry.
    fn uninstall_resolver(&self, tld: &str) -> Result<()>;

    /// Add the Grove root CA to the system trust store.
    fn trust_ca(&self, ca_cert: &Path) -> Result<()>;

    /// Remove the Grove root CA from the system trust store.
    fn untrust_ca(&self, ca_cert: &Path) -> Result<()>;

    /// Human-readable name of the active backend.
    fn name(&self) -> &'static str;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacOs as Platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Linux as Platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::Windows as Platform;

/// Construct the platform integration for the current OS.
pub fn current() -> Platform {
    Platform
}

/// Whether the current process is running with elevated privileges.
pub fn is_elevated() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: getuid is always safe.
        unsafe { libc_geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

//! Canonical filesystem locations for Grove state.
//!
//! Everything Grove persists lives under a single base directory so that an
//! uninstall is "delete one folder + remove the resolver/CA". The layout is
//! identical across platforms; only the base directory differs.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Error, Result};

/// Resolves and lazily creates the directories Grove uses.
#[derive(Debug, Clone)]
pub struct GrovePaths {
    base: PathBuf,
}

impl GrovePaths {
    /// Discover the platform-appropriate base directory.
    ///
    /// - macOS:   `~/Library/Application Support/Grove`
    /// - Linux:   `~/.config/grove`
    /// - Windows: `%APPDATA%\Grove\config`
    pub fn discover() -> Result<Self> {
        // Allow tests / power users to override the whole tree.
        if let Ok(custom) = std::env::var("GROVE_HOME") {
            return Ok(Self::with_base(custom));
        }
        let dirs = ProjectDirs::from("com", "elyra", "Grove").ok_or(Error::NoConfigDir)?;
        Ok(Self::with_base(dirs.config_dir()))
    }

    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Path to the single TOML source-of-truth config.
    pub fn config_file(&self) -> PathBuf {
        self.base.join("config.toml")
    }

    /// Directory holding the root CA + issued leaf certificates.
    pub fn certs_dir(&self) -> PathBuf {
        self.base.join("certs")
    }

    pub fn ca_cert(&self) -> PathBuf {
        self.certs_dir().join("grove-ca.pem")
    }

    pub fn ca_key(&self) -> PathBuf {
        self.certs_dir().join("grove-ca.key")
    }

    /// Per-site access/error logs.
    pub fn logs_dir(&self) -> PathBuf {
        self.base.join("logs")
    }

    /// Downloaded / registered PHP runtimes and FPM sockets.
    pub fn runtimes_dir(&self) -> PathBuf {
        self.base.join("runtimes")
    }

    /// Runtime sockets (daemon IPC, FPM pools).
    pub fn run_dir(&self) -> PathBuf {
        self.base.join("run")
    }

    /// IPC socket the CLI/GUI use to talk to the daemon.
    pub fn ipc_socket(&self) -> PathBuf {
        self.run_dir().join("groved.sock")
    }

    /// Create every directory in the tree if missing.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            self.base.clone(),
            self.certs_dir(),
            self.logs_dir(),
            self.runtimes_dir(),
            self.run_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }
}

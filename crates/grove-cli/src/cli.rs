//! clap command definitions. Command names mirror Valet where it makes sense
//! to keep the learning curve low (PRD §6.9).

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "grove",
    version,
    about = "Elyra Grove — native local dev environment in Rust",
    long_about = "Grove serves *.test domains with local HTTPS, multi-version PHP and \
                  zero external dependencies. A single Rust daemon; this CLI is a thin client."
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of human text (for scripts / elyra-conductor).
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the Grove daemon in the foreground (used by the service manager).
    Daemon,

    /// Start the daemon in the background (spawns `grove daemon`).
    Start,
    /// Stop the running daemon gracefully.
    Stop,
    /// Restart the daemon.
    Restart,
    /// Install Grove as an OS service (starts at login, restarts on crash).
    Install,
    /// Uninstall the Grove OS service and remove resolver + CA trust.
    Uninstall,
    /// Import sites/parked dirs from an existing Laravel Valet config.
    Import,

    /// Park a directory — every subdirectory becomes a `<name>.test` site.
    Park {
        /// Directory to park (defaults to the current directory).
        path: Option<String>,
    },
    /// Stop parking a directory.
    Unpark {
        path: Option<String>,
    },
    /// Link the current (or given) directory as a single named site.
    Link {
        /// Optional site name (defaults to the directory name).
        name: Option<String>,
        /// Directory to link (defaults to the current directory).
        #[arg(long)]
        path: Option<String>,
    },
    /// Remove a linked site.
    Unlink {
        name: String,
    },
    /// List every site Grove is serving.
    #[command(alias = "links")]
    List,
    /// Daemon + environment status.
    Status,
    /// Enable HTTPS for a site.
    Secure {
        name: String,
    },
    /// Disable HTTPS for a site.
    Unsecure {
        name: String,
    },
    /// Pin a PHP version for a site.
    Isolate {
        name: String,
        /// PHP version, e.g. 8.4
        version: String,
    },
    /// Revert a site to the default PHP version.
    Unisolate {
        name: String,
    },
    /// Route a `<name>.test` host to a running dev server.
    Proxy {
        name: String,
        /// Upstream URL, e.g. http://127.0.0.1:5173
        url: String,
    },
    /// Run diagnostics.
    Doctor,

    /// Root CA management.
    Ca {
        #[command(subcommand)]
        action: CaAction,
    },
    /// PHP runtime management.
    Php {
        #[command(subcommand)]
        action: PhpAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum CaAction {
    /// Generate (if needed) and trust the Grove root CA in the system store.
    Trust,
    /// Remove the Grove root CA from the system store.
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum PhpAction {
    /// Download + install a self-contained static PHP-FPM build into Grove.
    Install {
        /// PHP version, e.g. 8.4 (latest patch) or 8.4.22 (exact).
        version: String,
    },
    /// Auto-discover php-fpm binaries on this machine.
    Discover,
    /// List known PHP builds and their extensions.
    List,
    /// Register a custom php-fpm binary (bring-your-own — PRD §6.4).
    Register {
        /// Version label, e.g. 8.4
        version: String,
        /// Path to the php-fpm binary.
        fpm_binary: String,
    },
}

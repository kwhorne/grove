//! clap command definitions. Command names mirror Valet where it makes sense
//! to keep the learning curve low.

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

    /// Launch the desktop GUI (starts the daemon first if needed).
    Gui,

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
    /// First-run setup: config, root CA, a PHP build, resolver + trust (where possible).
    Init {
        /// PHP version to ensure is installed (default: 8.4). Use --no-php to skip.
        #[arg(long, default_value = "8.4")]
        php: String,
        /// Skip downloading/ensuring a PHP build.
        #[arg(long)]
        no_php: bool,
    },
    /// Set the global default PHP version.
    Use {
        /// PHP version, e.g. 8.4
        version: String,
    },

    /// Park a directory — every subdirectory becomes a `<name>.test` site.
    Park {
        /// Directory to park (defaults to the current directory).
        path: Option<String>,
    },
    /// Stop parking a directory.
    Unpark { path: Option<String> },
    /// Link the current (or given) directory as a single named site.
    Link {
        /// Optional site name (defaults to the directory name).
        name: Option<String>,
        /// Directory to link (defaults to the current directory).
        #[arg(long)]
        path: Option<String>,
    },
    /// Remove a linked site.
    Unlink { name: String },

    /// Remove a site from the list without deleting its files (hide it).
    Forget { name: String },

    /// Restore a previously removed (forgotten) site.
    Restore { name: String },
    /// Create a new site (a fresh Laravel or static project) and link it.
    New {
        /// Project name (becomes <name>.test).
        name: String,
        /// Kind: laravel | livewire | react | vue (Laravel starter kits via
        /// `laravel new`) | static.
        #[arg(long, default_value = "laravel")]
        kind: String,
        /// Parent directory (defaults to ~/Code).
        #[arg(long, default_value = "~/Code")]
        path: String,
        /// PHP version to scaffold with (defaults to the global default).
        #[arg(long)]
        php: Option<String>,
        /// Initialize a git repository.
        #[arg(long)]
        git: bool,
    },
    /// List every site Grove is serving.
    #[command(alias = "links")]
    List,
    /// Daemon + environment status.
    Status,
    /// Enable HTTPS for a site.
    Secure { name: String },
    /// Disable HTTPS for a site.
    Unsecure { name: String },
    /// Pin a PHP version for a site.
    Isolate {
        name: String,
        /// PHP version, e.g. 8.4
        version: String,
    },
    /// Revert a site to the default PHP version.
    Unisolate { name: String },
    /// Route a `<name>.test` host to a running dev server.
    Proxy {
        name: String,
        /// Upstream URL, e.g. http://127.0.0.1:5173
        url: String,
    },
    /// Share a local site publicly through a Grove Tunnel server (Expose/ngrok-style).
    Share {
        /// Site name (e.g. `elyra-web`) or `<name>.test` host.
        site: String,
        /// Tunnel server control address `host:port` (overrides config).
        #[arg(long)]
        server: Option<String>,
        /// Shared secret token (overrides config).
        #[arg(long)]
        token: Option<String>,
        /// Requested subdomain (the server may override if it's taken).
        #[arg(long)]
        subdomain: Option<String>,
        /// Protect the public URL with HTTP Basic auth, as `user:pass`.
        #[arg(long)]
        basic_auth: Option<String>,
    },

    /// Run diagnostics.
    Doctor,
    /// Print a .env snippet wiring an app to Grove's bundled services.
    Env {
        /// Optional site name; used as the database name.
        site: Option<String>,
    },
    /// View logs. With no argument, lists available log files.
    Logs {
        /// Log source to read (matches part of its name, e.g. a site name).
        target: Option<String>,
        /// Max entries to show.
        #[arg(long, default_value_t = 100)]
        lines: usize,
    },
    /// Inspect captured emails (built-in mail-catcher).
    Mail {
        #[command(subcommand)]
        action: Option<MailAction>,
    },
    /// Manage bundled services (databases, caches) Grove installs itself.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

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
    /// Node.js runtime management.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },

    /// Xdebug step-debugging control.
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// List bundled services and their state.
    List,
    /// Download + initialise a bundled service (e.g. postgres).
    Install { key: String },
    /// Start a bundled service.
    Start { key: String },
    /// Stop a bundled service.
    Stop { key: String },
    /// Restart a bundled service.
    Restart { key: String },
    /// Set a bundled service's listen port (applied on next start/restart).
    Port { key: String, port: u16 },
}

#[derive(Subcommand, Debug)]
pub enum MailAction {
    /// List captured emails (default).
    List,
    /// Show one captured email in full.
    Show { id: u64 },
    /// Discard all captured emails.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// List installed + installable Node versions.
    List,
    /// Download + install a Node version (major like 22, or exact 22.23.1).
    Install { version: String },
    /// Pin a Node version for a site.
    Use { site: String, version: String },
    /// Clear a site's pinned Node version.
    Unuse { site: String },
}

#[derive(Subcommand, Debug)]
pub enum DebugAction {
    /// Load Xdebug into FPM pools (step-debugging on).
    On,
    /// Unload Xdebug from FPM pools (step-debugging off).
    Off,
    /// Show whether Xdebug is enabled and available per PHP build.
    Status,
    /// Print shell env exports for debugging a CLI process (artisan, tests).
    /// Use with: eval "$(grove debug env)"
    Env,
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
    /// Register a custom php-fpm binary (bring-your-own).
    Register {
        /// Version label, e.g. 8.4
        version: String,
        /// Path to the php-fpm binary.
        fpm_binary: String,
    },
}

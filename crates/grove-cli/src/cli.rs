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
        /// Kind: laravel | livewire | react | vue (Laravel starter kits) |
        /// static | a community kit repo (vendor/package) via `laravel new
        /// --using`.
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

    /// Run a site's dev processes (Vite dev server + queue worker).
    Dev {
        #[command(subcommand)]
        action: DevAction,
    },

    /// Bring a project's environment up from its `grove.toml` (link, pin PHP/Node,
    /// start services, optional dev) — one command after `git clone`.
    Up {
        /// Project directory (defaults to the current directory).
        path: Option<String>,
        /// Write a starter `grove.toml` instead of bringing the project up.
        #[arg(long)]
        write: bool,
        /// Skip starting dev processes even if `grove.toml` enables them.
        #[arg(long = "no-dev")]
        no_dev: bool,
    },

    /// Package or restore a whole project environment (grove.toml + .env + database)
    /// as one shareable file — reproducible dev environments without Docker.
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },

    /// Inspect webhooks captured at `/__grove/hooks/...` (a local webhook.site),
    /// and re-deliver them to your app while you fix the handler.
    Hooks {
        /// Maximum entries to show when listing.
        #[arg(long, default_value_t = 40)]
        limit: usize,
        #[command(subcommand)]
        action: Option<HookAction>,
    },

    /// Show a live timeline of recent requests Grove proxied (any site, any framework).
    Requests {
        /// Only show requests for this site (host or name).
        site: Option<String>,
        /// Maximum entries to show.
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },

    /// Run a Model Context Protocol (MCP) server over stdio, exposing your local
    /// sites, requests, webhooks, logs, and database schema to AI tools like
    /// Claude and Cursor. Configure your client to launch `grove mcp`.
    Mcp,

    /// Re-issue a captured request through Grove (framework-agnostic replay).
    Replay {
        /// Request id from `grove requests`.
        id: u64,
    },

    /// Generate a curl command, .http file, or Pest test from a captured request.
    Request {
        /// Request id from `grove requests`.
        id: u64,
        /// Output format: curl (default), http, or pest.
        #[arg(long = "as", default_value = "curl")]
        format: String,
    },

    /// Activate and inspect a Grove Pro / Teams license.
    License {
        #[command(subcommand)]
        action: LicenseAction,
    },

    /// Grove Teams: end-to-end encrypted secret sync for a project.
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },

    /// Snapshot / restore Grove's bundled databases (time-travel before risky migrations).
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// Put Grove's bundled PHP, Composer, Node, npm and the Laravel installer on
    /// your PATH (per-directory version switching, like Herd — but zero-config).
    Path {
        #[command(subcommand)]
        action: Option<PathAction>,
    },

    /// (internal) Resolve and exec a bundled tool for a directory. Used by the
    /// shims that `grove path install` creates.
    #[command(hide = true)]
    Resolve {
        /// Tool to run: php, composer, node, npm, npx or laravel.
        tool: String,
        /// Directory whose site pins the version (defaults to the cwd).
        #[arg(long)]
        dir: Option<String>,
        /// Arguments passed through to the tool.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Print your member public key (creates your local identity if needed).
    Whoami,
    /// Set a secret: `grove secret set <project> KEY=VALUE`.
    Set { project: String, assignment: String },
    /// Fetch + decrypt a project's secrets (add --write to write .env).
    Pull {
        project: String,
        #[arg(long)]
        write: bool,
    },
    /// Grant a teammate access by their public key, and re-encrypt.
    Share { project: String, public_key: String },
    /// Revoke a teammate's access by their public key, and re-encrypt.
    Revoke { project: String, public_key: String },
    /// List the member public keys with access to a project.
    Members { project: String },
}

#[derive(Subcommand, Debug)]
pub enum LicenseAction {
    /// Activate a license key (verified offline).
    Activate { key: String },
    /// Show the current license entitlement.
    Status,
    /// Remove the stored license.
    Deactivate,
}

#[derive(Subcommand, Debug)]
pub enum DbAction {
    /// Take a snapshot of a bundled database.
    Snapshot {
        /// Engine: mysql (default) or postgres.
        #[arg(long, default_value = "mysql")]
        engine: String,
        /// Database name. Omit for all MySQL databases; required for postgres.
        #[arg(long)]
        db: Option<String>,
        /// Optional note to remember why you took it.
        #[arg(long)]
        note: Option<String>,
    },
    /// List stored snapshots.
    List,
    /// Restore a snapshot by id (see `grove db list`).
    Restore { id: String },
    /// Delete a snapshot by id.
    Rm { id: String },
}

#[derive(Subcommand, Debug)]
pub enum HookAction {
    /// Re-deliver a captured webhook to a local target URL.
    Replay {
        /// Webhook id (from `grove hooks`).
        id: u64,
        /// Target URL, e.g. https://myapp.test/stripe/webhook.
        #[arg(long)]
        to: String,
    },
    /// Clear all captured webhooks.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum BundleAction {
    /// Package a project (grove.toml + .env + database) into one shareable file.
    Export {
        /// Project directory (defaults to the current directory).
        path: Option<String>,
        /// Output file (defaults to <name>.grovebundle).
        #[arg(long)]
        out: Option<String>,
        /// Don't include the project's .env (secrets) in the bundle.
        #[arg(long = "no-env")]
        no_env: bool,
    },
    /// Restore a bundle: unpack, bring the environment up, and load the database.
    Import {
        /// The .grovebundle file to restore.
        file: String,
        /// Directory to restore into (defaults to ./<name>).
        #[arg(long)]
        into: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PathAction {
    /// Show whether the shims are installed and the line to add to your shell.
    Show,
    /// Create the shims and print the line to add to your shell profile.
    Install,
    /// Remove the shims.
    Uninstall,
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
pub enum DevAction {
    /// Start dev processes for a site.
    Start { site: String },
    /// Stop dev processes for a site.
    Stop { site: String },
    /// List sites with dev processes running.
    List,
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

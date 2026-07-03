//! Xdebug step-debugging support.
//!
//! Grove doesn't speak the debugger wire protocol itself — the editor (`e`) runs
//! a DAP client which spawns the `php-debug` adapter (over Grove's bundled Node);
//! that adapter listens on a TCP port and Xdebug *connects out* to it via DBGp.
//!
//! Grove's job is the runtime half: make sure `xdebug.so` is loaded into the PHP
//! that actually executes requests, and point it at the adapter's port. We do
//! this per FPM-pool at spawn time via `-d` INI overrides so the global php.ini
//! is never touched and the feature is entirely toggleable.
//!
//! Xdebug runs in `start_with_request=trigger` mode: the extension is resident
//! but dormant, so non-debugged requests pay only negligible overhead. A request
//! opts in with the `XDEBUG_TRIGGER` cookie / GET param (browser extension) or,
//! for the CLI, the env exported by `grove debug env`.

use std::path::{Path, PathBuf};

use grove_core::paths::GrovePaths;

use crate::registry::PhpBuild;

/// Default DBGp port the adapter listens on and Xdebug connects to.
pub const DEFAULT_XDEBUG_PORT: u16 = 9003;

/// How Xdebug will be made available to a given PHP build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XdebugPlan {
    /// Already compiled/loaded into this PHP — only mode directives are needed.
    AlreadyLoaded,
    /// Load the extension from this `.so`/`.dll` via `-d zend_extension=`.
    LoadFrom(PathBuf),
    /// No Xdebug found for this build; step-debugging is unavailable.
    Unavailable,
}

/// Platform file name for the compiled Xdebug extension.
fn extension_file() -> &'static str {
    if cfg!(windows) {
        "php_xdebug.dll"
    } else {
        "xdebug.so"
    }
}

/// Directory Grove looks in for a bundled Xdebug matched to `version`. A matching
/// build can be dropped here (or shipped by Grove) without touching php.ini:
/// `<runtimes>/xdebug/<version>/<xdebug.so>`.
pub fn bundled_path(paths: &GrovePaths, version: &str) -> PathBuf {
    paths
        .runtimes_dir()
        .join("xdebug")
        .join(version)
        .join(extension_file())
}

/// One-line availability label for `grove debug status` / the GUI.
pub fn availability_label(paths: &GrovePaths, build: &PhpBuild) -> String {
    describe(&resolve(paths, build)).to_string()
}

/// Decide how (or whether) Xdebug can be loaded for `build`.
pub fn resolve(paths: &GrovePaths, build: &PhpBuild) -> XdebugPlan {
    // 1. The build already ships Xdebug (user-registered PHP, custom build).
    if build
        .extensions()
        .iter()
        .any(|e| e.eq_ignore_ascii_case("xdebug"))
    {
        return XdebugPlan::AlreadyLoaded;
    }

    // 2. A Grove-managed build placed at the well-known path.
    let bundled = bundled_path(paths, &build.version);
    if bundled.exists() {
        return XdebugPlan::LoadFrom(bundled);
    }

    // 3. An `xdebug.so` sitting in the build's own extension_dir.
    if let Some(so) = extension_dir_so(build) {
        return XdebugPlan::LoadFrom(so);
    }

    XdebugPlan::Unavailable
}

/// Best-effort lookup of `<extension_dir>/xdebug.so` by asking `php -i`.
fn extension_dir_so(build: &PhpBuild) -> Option<PathBuf> {
    let cli = build.cli_binary.as_ref().or(Some(&build.fpm_binary))?;
    let output = std::process::Command::new(cli).arg("-i").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let dir = text.lines().find_map(|line| {
        let rest = line.strip_prefix("extension_dir")?.trim_start();
        // `extension_dir => /path => /path`
        let val = rest.trim_start_matches("=>").trim();
        let val = val.split("=>").next().unwrap_or(val).trim();
        if val.is_empty() || val == "(none)" {
            None
        } else {
            Some(PathBuf::from(val))
        }
    })?;
    let so = dir.join(extension_file());
    so.exists().then_some(so)
}

/// INI entries (`key=value`) to hand to php-fpm / php via `-d` to enable
/// step-debugging. Returns an empty vec when Xdebug is unavailable.
pub fn debug_ini_entries(plan: &XdebugPlan, port: u16) -> Vec<String> {
    if matches!(plan, XdebugPlan::Unavailable) {
        return Vec::new();
    }
    let mut entries = Vec::new();
    if let XdebugPlan::LoadFrom(so) = plan {
        entries.push(format!("zend_extension={}", so.display()));
    }
    entries.extend([
        "xdebug.mode=debug".to_string(),
        "xdebug.start_with_request=trigger".to_string(),
        "xdebug.client_host=127.0.0.1".to_string(),
        format!("xdebug.client_port={port}"),
        "xdebug.discover_client_host=false".to_string(),
    ]);
    entries
}

/// Append the `-d key=value` argument pairs for each INI entry to a command.
pub fn apply_dargs(cmd: &mut std::process::Command, entries: &[String]) {
    for entry in entries {
        cmd.arg("-d").arg(entry);
    }
}

/// Shell env exports that make a CLI PHP process (artisan, tests, `php script`)
/// connect to the debugger. Consumed via `eval "$(grove debug env)"`.
pub fn cli_env_exports(port: u16) -> String {
    format!(
        "export XDEBUG_MODE=debug\n\
         export XDEBUG_SESSION=1\n\
         export XDEBUG_CONFIG=\"client_host=127.0.0.1 client_port={port}\"\n"
    )
}

/// Human-readable summary of a plan for status output.
pub fn describe(plan: &XdebugPlan) -> &'static str {
    match plan {
        XdebugPlan::AlreadyLoaded => "ready (built into this PHP)",
        XdebugPlan::LoadFrom(_) => "ready (loadable xdebug.so)",
        XdebugPlan::Unavailable => "unavailable — needs a PHP with Xdebug (grove php register)",
    }
}

/// Whether `path` looks like a plausible Xdebug extension file (used by tests /
/// registration helpers).
pub fn looks_like_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case(extension_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_yields_no_entries() {
        assert!(debug_ini_entries(&XdebugPlan::Unavailable, 9003).is_empty());
    }

    #[test]
    fn already_loaded_sets_mode_but_not_zend_extension() {
        let entries = debug_ini_entries(&XdebugPlan::AlreadyLoaded, 9003);
        assert!(entries.iter().all(|e| !e.starts_with("zend_extension")));
        assert!(entries.iter().any(|e| e == "xdebug.mode=debug"));
        assert!(entries.iter().any(|e| e == "xdebug.client_port=9003"));
        assert!(entries
            .iter()
            .any(|e| e == "xdebug.start_with_request=trigger"));
    }

    #[test]
    fn load_from_prepends_zend_extension() {
        let so = PathBuf::from("/x/xdebug.so");
        let entries = debug_ini_entries(&XdebugPlan::LoadFrom(so), 9500);
        assert_eq!(entries[0], "zend_extension=/x/xdebug.so");
        assert!(entries.iter().any(|e| e == "xdebug.client_port=9500"));
    }

    #[test]
    fn apply_dargs_pairs_each_entry_with_d_flag() {
        let mut cmd = std::process::Command::new("php-fpm");
        apply_dargs(&mut cmd, &["xdebug.mode=debug".to_string()]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-d", "xdebug.mode=debug"]);
    }

    #[test]
    fn cli_env_mentions_port() {
        let env = cli_env_exports(9007);
        assert!(env.contains("XDEBUG_MODE=debug"));
        assert!(env.contains("client_port=9007"));
    }
}

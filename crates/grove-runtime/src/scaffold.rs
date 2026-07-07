//! Create new projects (Herd-style "Create a New Site").
//!
//! Grove scaffolds a fresh Laravel app with the **official `laravel new`
//! installer** (so you get the latest Laravel plus a real starter kit —
//! Livewire, React or Vue), or a minimal static site. Everything runs against
//! Grove's bundled PHP + Composer + Node, so the user installs nothing.

use std::io::Read;
use std::path::{Path, PathBuf};

use grove_core::paths::GrovePaths;

use crate::install;
use crate::node::{self, NodeRegistry};

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("a directory already exists at {0}")]
    Exists(PathBuf),
    #[error("php install: {0}")]
    Php(#[from] install::InstallError),
    #[error("composer/scaffold failed: {0}")]
    Command(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ScaffoldError>;

/// Path to Grove's bundled `composer.phar` (may not exist yet).
pub fn composer_phar(paths: &GrovePaths) -> PathBuf {
    paths.runtimes_dir().join("composer.phar")
}

/// Path to the Grove-managed `laravel` installer binary (may not exist yet).
pub fn laravel_installer(paths: &GrovePaths) -> PathBuf {
    composer_home(paths).join("vendor/bin/laravel")
}

/// Download composer.phar into Grove's runtimes dir if missing.
pub fn ensure_composer(paths: &GrovePaths) -> Result<PathBuf> {
    let dest = paths.runtimes_dir().join("composer.phar");
    if dest.exists() {
        return Ok(dest);
    }
    paths.ensure()?;
    let resp = ureq::get("https://getcomposer.org/composer-stable.phar")
        .call()
        .map_err(|e| ScaffoldError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    std::fs::write(&dest, &buf)?;
    Ok(dest)
}

/// Grove-managed Composer home (where the Laravel installer lives).
fn composer_home(paths: &GrovePaths) -> PathBuf {
    paths.runtimes_dir().join("composer-home")
}

/// Install (once) the official Laravel installer into Grove's Composer home and
/// return the `laravel` bin path.
fn ensure_laravel_installer(
    paths: &GrovePaths,
    php: &Path,
    composer: &Path,
    progress: &impl Fn(&str),
) -> Result<PathBuf> {
    let home = composer_home(paths);
    std::fs::create_dir_all(&home)?;
    let bin = home.join("vendor/bin/laravel");
    if bin.exists() {
        return Ok(bin);
    }
    progress("installing the Laravel installer…");
    let out = std::process::Command::new(php)
        .arg(composer)
        .args(["global", "require", "laravel/installer", "--no-interaction"])
        .env("COMPOSER_HOME", &home)
        .output()?;
    if !out.status.success() {
        return Err(ScaffoldError::Command(format!(
            "composer global require laravel/installer failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    if !bin.exists() {
        return Err(ScaffoldError::Command(
            "Laravel installer did not appear after install".into(),
        ));
    }
    Ok(bin)
}

/// A Node bin dir to put on PATH so the installer can `npm install && build`.
/// Prefers the newest installed Node; installs an LTS if none exist.
fn ensure_node_bin(paths: &GrovePaths, progress: &impl Fn(&str)) -> Option<PathBuf> {
    let mut reg = NodeRegistry::load(paths);
    if let Some(b) = reg.iter().max_by(|a, b| a.major.cmp(&b.major)) {
        return b.node_binary.parent().map(Path::to_path_buf);
    }
    progress("installing Node (for the asset build)…");
    match node::install(paths, &mut reg, "22", progress) {
        Ok(b) => {
            let _ = reg.save(paths);
            b.node_binary.parent().map(Path::to_path_buf)
        }
        Err(_) => None,
    }
}

/// Build a `bin` dir containing a `composer` shim that runs Grove's bundled
/// Composer, so the Laravel installer (which shells out to `composer`) works.
fn ensure_shim_bin(paths: &GrovePaths, php: &Path, composer: &Path) -> Result<PathBuf> {
    let dir = paths.runtimes_dir().join("scaffold-bin");
    std::fs::create_dir_all(&dir)?;
    let shim = dir.join("composer");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nexec \"{}\" \"{}\" \"$@\"\n",
            php.display(),
            composer.display()
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dir)
}

/// Create a fresh Laravel project at `target` using `laravel new`.
///
/// `kit` selects a starter kit: `Some("livewire" | "react" | "vue")`, or `None`
/// for a plain Laravel app.
pub fn new_laravel(
    paths: &GrovePaths,
    php_version: &str,
    target: &Path,
    kit: Option<&str>,
    init_git: bool,
    progress: impl Fn(&str),
) -> Result<()> {
    if target.exists() {
        return Err(ScaffoldError::Exists(target.to_path_buf()));
    }
    let parent = target
        .parent()
        .ok_or_else(|| ScaffoldError::Command("invalid target path".into()))?;
    std::fs::create_dir_all(parent)?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ScaffoldError::Command("invalid project name".into()))?;

    progress("preparing PHP CLI…");
    let php = install::install_cli(paths, php_version, &progress)?;
    let php_dir = php.parent().map(Path::to_path_buf).unwrap_or_default();
    progress("preparing Composer…");
    let composer = ensure_composer(paths)?;
    let home = composer_home(paths);
    let installer = ensure_laravel_installer(paths, &php, &composer, &progress)?;
    let shim = ensure_shim_bin(paths, &php, &composer)?;
    let node_bin = ensure_node_bin(paths, &progress);

    // Assemble a PATH the installer can find php / composer / node / git in.
    let mut path = format!("{}:{}", shim.display(), php_dir.display());
    if let Some(nb) = &node_bin {
        path.push(':');
        path.push_str(&nb.to_string_lossy());
    }
    let base_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    path.push(':');
    path.push_str(&base_path);

    let kit_label = match kit {
        None | Some("laravel") | Some("") => "none",
        Some(k) => k,
    };
    progress(&format!(
        "creating Laravel app ({kit_label}) via `laravel new`…"
    ));
    let mut cmd = std::process::Command::new(&php);
    cmd.arg(&installer)
        .arg("new")
        .arg(name)
        .arg("--no-interaction")
        .arg("--database=sqlite")
        .current_dir(parent)
        .env("PATH", &path)
        .env("COMPOSER_HOME", &home)
        // Composer/npm can warn loudly when run as root; keep them quiet.
        .env("COMPOSER_ALLOW_SUPERUSER", "1");
    match kit {
        Some("livewire") => {
            cmd.arg("--livewire");
        }
        Some("react") => {
            cmd.arg("--react");
        }
        Some("vue") => {
            cmd.arg("--vue");
        }
        // Any other value is treated as a community starter kit (a Composer
        // package or GitHub `org/repo`) via `laravel new --using=`.
        Some(other) if !other.is_empty() && other != "laravel" => {
            cmd.arg(format!("--using={other}"));
        }
        _ => {}
    }
    if init_git {
        cmd.arg("--git");
    }

    let out = cmd.output()?;
    if !out.status.success() {
        return Err(ScaffoldError::Command(format!(
            "laravel new failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    // The daemon may be root; hand the new project to the invoking user.
    chown_to_run_user(target);
    progress("done");
    Ok(())
}

/// When the daemon runs as root, `chown -R` the new project to the invoking
/// user so they own and can edit their files.
fn chown_to_run_user(target: &Path) {
    if !running_as_root() {
        return;
    }
    let Some(user) = run_user() else { return };
    let _ = std::process::Command::new("chown")
        .arg("-R")
        .arg(&user)
        .arg(target)
        .status();
}

fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            #[link_name = "geteuid"]
            fn geteuid() -> u32;
        }
        unsafe { geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_user() -> Option<String> {
    for var in ["GROVE_RUN_USER", "SUDO_USER"] {
        if let Ok(u) = std::env::var(var) {
            if !u.is_empty() && u != "root" {
                return Some(u);
            }
        }
    }
    None
}

/// Create a minimal static site at `target`.
pub fn new_static(target: &Path, name: &str) -> Result<()> {
    if target.exists() {
        return Err(ScaffoldError::Exists(target.to_path_buf()));
    }
    std::fs::create_dir_all(target)?;
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>{name}</title>\n  <style>body{{font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0;background:#16161e;color:#c0caf5}}h1{{font-weight:600}}</style>\n</head>\n<body>\n  <h1>🌳 {name}</h1>\n</body>\n</html>\n",
    );
    std::fs::write(target.join("index.html"), html)?;
    chown_to_run_user(target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real end-to-end: GROVE_TEST_SCAFFOLD=1 cargo test -p grove-runtime \
    //   laravel_new_scaffolds -- --ignored --nocapture
    #[test]
    #[ignore]
    fn laravel_new_scaffolds() {
        // GROVE_SCAFFOLD_KIT=vue selects a starter kit; default is plain Laravel.
        let kit = std::env::var("GROVE_SCAFFOLD_KIT").ok();
        let home = std::env::temp_dir().join(format!("grove-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let paths = GrovePaths::with_base(&home);

        let target = home.join("code").join("demo-app");
        new_laravel(&paths, "8.4", &target, kit.as_deref(), false, |m| {
            eprintln!(">> {m}")
        })
        .unwrap();

        assert!(target.join("artisan").is_file(), "artisan missing");
        assert!(
            target.join("composer.json").is_file(),
            "composer.json missing"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

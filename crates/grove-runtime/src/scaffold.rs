//! Create new projects (Herd-style "Create a New Site").
//!
//! Grove can scaffold a fresh Laravel app using a bundled PHP CLI + Composer
//! (downloaded on demand), or a minimal static site — all without the user
//! installing PHP/Composer separately.

use std::io::Read;
use std::path::{Path, PathBuf};

use grove_core::paths::GrovePaths;

use crate::install;

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

/// Download composer.phar into Grove's runtimes dir if missing.
fn ensure_composer(paths: &GrovePaths) -> Result<PathBuf> {
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

/// Create a fresh Laravel project at `target` (which must not yet exist).
/// `php_version` selects the CLI build used to run Composer.
pub fn new_laravel(
    paths: &GrovePaths,
    php_version: &str,
    target: &Path,
    init_git: bool,
    progress: impl Fn(&str),
) -> Result<()> {
    if target.exists() {
        return Err(ScaffoldError::Exists(target.to_path_buf()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    progress("preparing PHP CLI…");
    let php = install::install_cli(paths, php_version, &progress)?;
    progress("preparing Composer…");
    let composer = ensure_composer(paths)?;

    progress("creating Laravel project (composer)…");
    let out = std::process::Command::new(&php)
        .arg(&composer)
        .args([
            "create-project",
            "--prefer-dist",
            "--no-interaction",
            "laravel/laravel",
        ])
        .arg(target)
        .output()?;
    if !out.status.success() {
        return Err(ScaffoldError::Command(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }

    if init_git {
        let _ = std::process::Command::new("git")
            .arg("init")
            .current_dir(target)
            .output();
    }
    progress("done");
    Ok(())
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
    Ok(())
}

//! Download + install static PHP-FPM builds.
//!
//! This is what makes Grove genuinely zero-dependency: instead of requiring
//! Homebrew/Herd/Composer to supply PHP, Grove fetches a self-contained static
//! `php-fpm` binary (built by the static-php-cli project) straight into its own
//! `runtimes/` tree. The binary has no external shared-library dependencies.

use std::io::Read;
use std::path::PathBuf;

use grove_core::paths::GrovePaths;

use crate::registry::{PhpBuild, PhpRegistry};

/// Mirror that hosts prebuilt static PHP binaries.
const BASE_URL: &str = "https://dl.static-php.dev/static-php-cli/common/";

/// PHP major versions Grove offers in the GUI (latest first).
pub const OFFERED_MAJORS: &[&str] = &["8.5", "8.4", "8.3"];

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    #[error("no static PHP-FPM build found for version {req} ({plat})")]
    NoMatch { req: String, plat: String },
    #[error("http error: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, InstallError>;

/// A semantic version triple used for "latest patch" resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer(u64, u64, u64);

impl SemVer {
    fn parse(s: &str) -> Option<SemVer> {
        let mut it = s.split('.');
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(SemVer(a, b, c))
    }
    fn dotted(self) -> String {
        format!("{}.{}.{}", self.0, self.1, self.2)
    }
    /// Key used in config / registry (major.minor, e.g. "8.4").
    fn minor_key(self) -> String {
        format!("{}.{}", self.0, self.1)
    }
}

/// `(os, arch)` slugs used in the static-php filenames, e.g. ("macos","aarch64").
fn platform_slug() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        other => return Err(InstallError::UnsupportedPlatform(other.to_string())),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => return Err(InstallError::UnsupportedPlatform(other.to_string())),
    };
    Ok((os, arch))
}

/// Install a static PHP-FPM build matching `version_req` (e.g. "8.4" → latest
/// 8.4.x, or an exact "8.4.22"). Registers it in the runtime registry and
/// returns the resulting build descriptor.
pub fn install(
    paths: &GrovePaths,
    registry: &mut PhpRegistry,
    version_req: &str,
    progress: impl Fn(&str),
) -> Result<PhpBuild> {
    let (os, arch) = platform_slug()?;
    let plat = format!("{os}-{arch}");
    let suffix = format!("-fpm-{plat}.tar.gz");

    progress(&format!("resolving latest {version_req} for {plat}…"));
    let resolved = resolve_version(version_req, &suffix)?;
    let filename = format!("php-{}-fpm-{plat}.tar.gz", resolved.dotted());
    let url = format!("{BASE_URL}{filename}");
    let key = resolved.minor_key();

    let dest_dir = paths.runtimes_dir().join(&key);
    std::fs::create_dir_all(&dest_dir)?;
    let fpm_path = dest_dir.join("php-fpm");

    progress(&format!("downloading {filename}…"));
    let bytes = http_get(&url)?;

    progress("extracting…");
    extract_fpm(&bytes, &fpm_path)?;
    make_executable(&fpm_path)?;

    // Verify it actually runs.
    let actual = std::process::Command::new(&fpm_path)
        .arg("--version")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();
    progress(&format!("installed: {actual}"));

    let build = PhpBuild {
        version: key.clone(),
        fpm_binary: fpm_path,
        cli_binary: None,
        user_registered: false,
    };
    registry.register(build.clone());
    registry.save(paths).map_err(InstallError::Io)?;
    Ok(build)
}

/// Download a static PHP **CLI** build (for running composer/artisan during
/// project scaffolding) and return the path to the `php` binary.
pub fn install_cli(
    paths: &GrovePaths,
    version_req: &str,
    progress: impl Fn(&str),
) -> Result<PathBuf> {
    let (os, arch) = platform_slug()?;
    let plat = format!("{os}-{arch}");
    let suffix = format!("-cli-{plat}.tar.gz");
    let resolved = resolve_version(version_req, &suffix)?;
    let key = resolved.minor_key();
    let dest_dir = paths.runtimes_dir().join("cli").join(&key);
    let php_path = dest_dir.join("php");
    if php_path.exists() {
        return Ok(php_path);
    }
    std::fs::create_dir_all(&dest_dir)?;
    let filename = format!("php-{}-cli-{plat}.tar.gz", resolved.dotted());
    progress(&format!("downloading {filename}…"));
    let bytes = http_get(&format!("{BASE_URL}{filename}"))?;
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry
            .path()?
            .file_name()
            .map(|n| n == "php")
            .unwrap_or(false)
        {
            let mut out = std::fs::File::create(&php_path)?;
            std::io::copy(&mut entry, &mut out)?;
            break;
        }
    }
    make_executable(&php_path)?;
    Ok(php_path)
}

/// Scrape the listing and pick the best matching version.
fn resolve_version(version_req: &str, suffix: &str) -> Result<SemVer> {
    // Exact 3-part version: use as-is (still validate it exists in the listing).
    let listing = http_get_string(BASE_URL)?;
    let mut matches: Vec<SemVer> = Vec::new();
    for (idx, _) in listing.match_indices(suffix) {
        let prefix = &listing[..idx];
        if let Some(p) = prefix.rfind("php-") {
            if let Some(ver) = SemVer::parse(&listing[p + 4..idx]) {
                matches.push(ver);
            }
        }
    }
    matches.sort();
    matches.dedup();

    let want_parts: Vec<&str> = version_req.split('.').collect();
    let chosen = match want_parts.as_slice() {
        [maj, min, _patch] => {
            let exact = SemVer::parse(version_req);
            exact.filter(|v| matches.contains(v)).or_else(|| {
                // fall back to latest of that minor
                let _ = (maj, min);
                latest_minor(&matches, version_req)
            })
        }
        [maj, min] => {
            let prefix = format!("{maj}.{min}");
            latest_minor(&matches, &prefix)
        }
        _ => None,
    };

    chosen.ok_or_else(|| InstallError::NoMatch {
        req: version_req.to_string(),
        plat: suffix
            .trim_start_matches("-fpm-")
            .trim_end_matches(".tar.gz")
            .to_string(),
    })
}

fn latest_minor(matches: &[SemVer], minor_prefix: &str) -> Option<SemVer> {
    let parts: Vec<&str> = minor_prefix.split('.').collect();
    let (maj, min): (u64, u64) = match parts.as_slice() {
        [a, b] | [a, b, _] => (a.parse().ok()?, b.parse().ok()?),
        _ => return None,
    };
    matches
        .iter()
        .filter(|v| v.0 == maj && v.1 == min)
        .max()
        .copied()
}

/// Extract the single `php-fpm` entry from the gzipped tar into `dest`.
fn extract_fpm(gz_bytes: &[u8], dest: &PathBuf) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let is_fpm = path.file_name().map(|n| n == "php-fpm").unwrap_or(false);
        if is_fpm {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    Err(InstallError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "php-fpm not found inside archive",
    )))
}

#[cfg(unix)]
fn make_executable(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
#[cfg(not(unix))]
fn make_executable(_path: &PathBuf) -> Result<()> {
    Ok(())
}

/// Minimal blocking HTTP GET returning the body bytes (follows redirects).
fn http_get(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| InstallError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(512 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn http_get_string(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| InstallError::Http(e.to_string()))?;
    resp.into_string().map_err(InstallError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parse_and_order() {
        assert_eq!(SemVer::parse("8.4.22"), Some(SemVer(8, 4, 22)));
        assert_eq!(SemVer::parse("8.4"), None);
        assert!(SemVer(8, 4, 9) < SemVer(8, 4, 22));
        assert_eq!(SemVer(8, 4, 22).minor_key(), "8.4");
    }

    #[test]
    fn picks_latest_patch() {
        let v = vec![
            SemVer(8, 4, 9),
            SemVer(8, 4, 22),
            SemVer(8, 3, 99),
            SemVer(8, 4, 5),
        ];
        assert_eq!(latest_minor(&v, "8.4"), Some(SemVer(8, 4, 22)));
        assert_eq!(latest_minor(&v, "8.3"), Some(SemVer(8, 3, 99)));
        assert_eq!(latest_minor(&v, "8.9"), None);
    }
}

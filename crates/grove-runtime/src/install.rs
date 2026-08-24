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

/// Mirror that hosts the upstream prebuilt static PHP binaries.
const MIRROR: &str = "https://dl.static-php.dev/static-php-cli";

/// Rolling GitHub release holding Grove's own PHP archives, and the API endpoint
/// that lists its assets.
///
/// The listing is the release JSON rather than a directory index because GitHub
/// has no directory index — but the JSON carries every asset's file name, which
/// is all [`resolve_version`] reads.
const GROVE_DOWNLOAD_BASE: &str =
    "https://github.com/kwhorne/grove/releases/download/php-runtimes/";
const GROVE_LISTING: &str = "https://api.github.com/repos/kwhorne/grove/releases/tags/php-runtimes";

/// PHP major versions Grove offers in the GUI (latest first).
pub const OFFERED_MAJORS: &[&str] = &["8.5", "8.4", "8.3"];

/// Which extension set to fetch.
///
/// static-php-cli publishes several archives per PHP version, each compiled with
/// a different, *fixed* extension set — and they are not supersets of each
/// other. `common` ships the PDO SQLite and PostgreSQL drivers (Laravel's
/// default database, and Grove's bundled Postgres) but no `intl` or `mysqli`;
/// `bulk` ships `intl`, `mysqli`, `sodium`, `readline` and ~15 more, but drops
/// `pdo_sqlite` and `pdo_pgsql`. Neither is enough, which is why Grove builds
/// [`Variant::Grove`] — the union — itself.
///
/// Use `grove php ext` to see exactly what an installed build loads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Variant {
    /// Grove's own build: everything `common` has **plus** everything Grove
    /// wants from `bulk` — `pdo_sqlite`, `pdo_pgsql`, `intl`, `mysqli`,
    /// `sodium`, `readline`, `apcu`, `xsl` and the rest, in one binary. Built by
    /// `.github/workflows/php-build.yml` from [`crate::extensions::BUILD_SET`].
    #[default]
    Grove,
    /// Upstream `common`: ~14 MB, has `pdo_sqlite` + `pdo_pgsql`, lacks
    /// `intl`/`mysqli`. Also the fallback when no Grove build exists yet for a
    /// given version.
    Common,
    /// Upstream `bulk`: ~36 MB, has `intl`/`mysqli`/`sodium`/`readline`/`apcu`/
    /// `xsl`, lacks `pdo_sqlite` + `pdo_pgsql`.
    Bulk,
}

impl Variant {
    /// Path segment on the mirror / label recorded on a build.
    pub fn slug(self) -> &'static str {
        match self {
            Variant::Grove => "grove",
            Variant::Common => "common",
            Variant::Bulk => "bulk",
        }
    }

    /// Parse a user-supplied variant name.
    pub fn parse(s: &str) -> Option<Variant> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "grove" | "default" | "union" => Some(Variant::Grove),
            "common" => Some(Variant::Common),
            "bulk" | "full" | "max" => Some(Variant::Bulk),
            _ => None,
        }
    }

    /// What to try when this variant has no archive for the requested version.
    ///
    /// Only Grove's own builds fall back. They are published from this repo's
    /// CI, so a brand-new PHP patch — or one failed build job — can leave a gap
    /// that upstream has already filled, and refusing to install would be worse
    /// than installing a PHP with a documented hole. The upstream sets never
    /// fall back: asking for `bulk` and silently getting `common` would trade
    /// one extension hole for the other behind the user's back.
    fn fallback(self) -> Option<Variant> {
        match self {
            Variant::Grove => Some(Variant::Common),
            Variant::Common | Variant::Bulk => None,
        }
    }

    /// The variant the on-disk config selects.
    ///
    /// An unrecognised value falls back to the default rather than failing: a
    /// typo in `grove.toml` shouldn't be the reason PHP won't install.
    pub fn configured(paths: &GrovePaths) -> Variant {
        grove_core::Config::load(paths)
            .ok()
            .and_then(|c| Variant::parse(&c.general.php_variant))
            .unwrap_or_default()
    }

    /// A `GROVE_PHP_MIRROR` directory for this variant, when one is set.
    ///
    /// The override makes every variant — Grove's own included — behave like a
    /// plain static-php-cli directory tree, so a team can host all three from
    /// one bucket. It has to keep upstream's file naming
    /// (`php-<version>-<cli|fpm>-<os>-<arch>.tar.gz`).
    fn mirror_dir(self) -> Option<String> {
        std::env::var("GROVE_PHP_MIRROR")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .map(|m| format!("{}/{}/", m.trim_end_matches('/'), self.slug()))
    }

    /// URL of a document listing the available archive file names.
    pub fn listing_url(self) -> String {
        if let Some(dir) = self.mirror_dir() {
            return dir;
        }
        match self {
            Variant::Grove => GROVE_LISTING.to_string(),
            _ => format!("{MIRROR}/{}/", self.slug()),
        }
    }

    /// Prefix a resolved archive file name is appended to.
    pub fn download_base(self) -> String {
        if let Some(dir) = self.mirror_dir() {
            return dir;
        }
        match self {
            Variant::Grove => GROVE_DOWNLOAD_BASE.to_string(),
            _ => format!("{MIRROR}/{}/", self.slug()),
        }
    }
}

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
    variant: Variant,
    progress: impl Fn(&str),
) -> Result<PhpBuild> {
    let (os, arch) = platform_slug()?;
    let plat = format!("{os}-{arch}");
    let suffix = format!("-fpm-{plat}.tar.gz");

    progress(&format!(
        "resolving latest {version_req} for {plat} ({})…",
        variant.slug()
    ));
    // `variant` is what was asked for; `variant` below is what we can actually
    // get. Everything downstream — the CLI archive, the label recorded on the
    // build — has to follow the resolved one, or a build ends up labelled as
    // something it isn't.
    let (variant, resolved) = resolve_with_fallback(variant, version_req, &suffix, &progress)?;
    let base = variant.download_base();
    let filename = format!("php-{}-fpm-{plat}.tar.gz", resolved.dotted());
    let url = format!("{base}{filename}");
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

    // The CLI is what `grove php ext` / `php -m` and the PATH shims use, and it
    // must come from the same archive as the FPM binary — auditing a build whose
    // extensions were compiled from a different variant would be a lie. It is
    // cheap enough to fetch here rather than lazily from somewhere else.
    let cli_binary = match replace_cli(paths, version_req, variant, &progress) {
        Ok(path) => Some(path),
        Err(e) => {
            progress(&format!("note: CLI build unavailable ({e})"));
            None
        }
    };

    let build = PhpBuild {
        version: key.clone(),
        fpm_binary: fpm_path,
        cli_binary,
        variant: Some(variant.slug().to_string()),
        user_registered: false,
    };
    registry.register(build.clone());
    registry.save(paths).map_err(InstallError::Io)?;
    Ok(build)
}

/// Download a static PHP **CLI** build (for running composer/artisan during
/// project scaffolding) and return the path to the `php` binary.
///
/// An already-present CLI for that version is reused as-is — callers that only
/// need *a* PHP shouldn't pay for a download.
pub fn install_cli(
    paths: &GrovePaths,
    version_req: &str,
    variant: Variant,
    progress: impl Fn(&str),
) -> Result<PathBuf> {
    fetch_cli(paths, version_req, variant, false, progress)
}

/// As [`install_cli`], but overwrite any CLI already there.
///
/// Used when installing a full build: only one variant per minor version is on
/// disk at a time, and a CLI left over from the *previous* variant would make
/// `php -m` — and so every extension report — describe a different binary than
/// the one serving requests.
pub fn replace_cli(
    paths: &GrovePaths,
    version_req: &str,
    variant: Variant,
    progress: impl Fn(&str),
) -> Result<PathBuf> {
    fetch_cli(paths, version_req, variant, true, progress)
}

fn fetch_cli(
    paths: &GrovePaths,
    version_req: &str,
    variant: Variant,
    replace: bool,
    progress: impl Fn(&str),
) -> Result<PathBuf> {
    let (os, arch) = platform_slug()?;
    let plat = format!("{os}-{arch}");
    let suffix = format!("-cli-{plat}.tar.gz");
    let (variant, resolved) = resolve_with_fallback(variant, version_req, &suffix, &progress)?;
    let base = variant.download_base();
    let key = resolved.minor_key();
    let dest_dir = paths.runtimes_dir().join("cli").join(&key);
    let php_path = dest_dir.join("php");
    if php_path.exists() && !replace {
        return Ok(php_path);
    }
    std::fs::create_dir_all(&dest_dir)?;
    let filename = format!("php-{}-cli-{plat}.tar.gz", resolved.dotted());
    progress(&format!("downloading {filename}…"));
    let bytes = http_get(&format!("{base}{filename}"))?;
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
            // Extract beside the target and rename, so a failure part-way
            // through can't leave a truncated `php` behind for the shims to
            // exec — and so replacing one that is currently running is atomic.
            let tmp = dest_dir.join("php.part");
            let mut out = std::fs::File::create(&tmp)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            make_executable(&tmp)?;
            std::fs::rename(&tmp, &php_path)?;
            return Ok(php_path);
        }
    }
    Err(InstallError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "php not found inside archive",
    )))
}

/// Resolve `version_req` against `variant`, dropping to its fallback when that
/// variant has nothing for this version or platform.
///
/// Returns the variant that actually has the archive, so the caller labels the
/// build with what it got rather than what it asked for.
fn resolve_with_fallback(
    variant: Variant,
    version_req: &str,
    suffix: &str,
    progress: &impl Fn(&str),
) -> Result<(Variant, SemVer)> {
    let first = resolve_version(&variant.listing_url(), version_req, suffix);
    if first.is_ok() {
        return first.map(|v| (variant, v));
    }
    let Some(alt) = variant.fallback() else {
        return first.map(|v| (variant, v));
    };
    let resolved = resolve_version(&alt.listing_url(), version_req, suffix)?;
    progress(&format!(
        "no {} build for {version_req} yet — using upstream `{}` instead (`grove php ext` shows what it's missing)",
        variant.slug(),
        alt.slug(),
    ));
    Ok((alt, resolved))
}

/// Scrape the listing and pick the best matching version.
fn resolve_version(base_url: &str, version_req: &str, suffix: &str) -> Result<SemVer> {
    // Exact 3-part version: use as-is (still validate it exists in the listing).
    let listing = http_get_string(base_url)?;
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
    fn variant_parses_aliases_and_rejects_junk() {
        assert_eq!(Variant::parse("grove"), Some(Variant::Grove));
        assert_eq!(Variant::parse("common"), Some(Variant::Common));
        assert_eq!(Variant::parse(" BULK "), Some(Variant::Bulk));
        assert_eq!(Variant::parse("full"), Some(Variant::Bulk));
        assert_eq!(Variant::parse(""), Some(Variant::Grove));
        assert_eq!(Variant::parse("minimal"), None);
        assert_eq!(Variant::default(), Variant::Grove);
    }

    #[test]
    fn variant_urls_point_at_distinct_extension_sets() {
        // Guard against the variants collapsing onto one URL: they have
        // genuinely different extensions, and installing the wrong one silently
        // costs you either intl/mysqli or pdo_sqlite/pdo_pgsql.
        let urls: Vec<String> = [Variant::Grove, Variant::Common, Variant::Bulk]
            .iter()
            .map(|v| v.download_base())
            .collect();
        let unique: std::collections::BTreeSet<&String> = urls.iter().collect();
        assert_eq!(unique.len(), 3, "{urls:?}");
        assert!(urls[1].ends_with("/common/"), "{}", urls[1]);
        assert!(urls[2].ends_with("/bulk/"), "{}", urls[2]);
    }

    /// Grove's own archives come from a GitHub release, which has no directory
    /// index — the listing is the release API and the downloads are the asset
    /// URLs, so those two must not be assumed equal the way they are upstream.
    #[test]
    fn grove_variant_lists_and_downloads_from_different_hosts() {
        assert_ne!(Variant::Grove.listing_url(), Variant::Grove.download_base());
        assert!(Variant::Grove.listing_url().contains("api.github.com"));
        assert!(Variant::Grove.download_base().ends_with('/'));
        // Upstream keeps one directory for both.
        assert_eq!(
            Variant::Common.listing_url(),
            Variant::Common.download_base()
        );
    }

    /// Only Grove's own builds may quietly degrade — trading `bulk` for
    /// `common` behind the user's back would swap one extension hole for
    /// another.
    #[test]
    fn only_the_grove_variant_falls_back() {
        assert_eq!(Variant::Grove.fallback(), Some(Variant::Common));
        assert_eq!(Variant::Common.fallback(), None);
        assert_eq!(Variant::Bulk.fallback(), None);
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

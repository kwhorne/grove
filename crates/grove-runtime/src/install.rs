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
    /// The listing for the variant could not be read at all.
    ///
    /// Distinct from `NoMatch`, which means the listing *was* read and holds
    /// nothing for this version. The two used to be one error, and falling back
    /// on either is how a rate-limited API quietly installed a different PHP.
    #[error(
        "could not read the `{variant}` build listing: {reason}\n\n\
         Not falling back to an upstream build: that would install a PHP with a \
         different extension set — no mysqli, or no intl, or no PDO SQLite driver \
         — under the version you asked for, and the only sign would be a line you \
         had already scrolled past.\n\n\
         GitHub's unauthenticated API budget is 60 requests an hour per IP and \
         Grove reads this listing without a token, so this is usually that, and \
         usually over within the hour. `curl -s https://api.github.com/rate_limit` \
         says when it resets.\n\n\
         To install an upstream set deliberately: `grove php install {req} \
         --variant common` (or `bulk`), and `grove php ext` shows what each is \
         missing."
    )]
    ListingUnreadable {
        variant: String,
        req: String,
        reason: String,
    },
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
    let (variant, resolved, listing) =
        resolve_with_fallback(variant, version_req, &suffix, &progress)?;
    let base = variant.download_base();
    let filename = format!("php-{}-fpm-{plat}.tar.gz", resolved.dotted());
    let url = format!("{base}{filename}");
    let key = resolved.minor_key();

    let dest_dir = paths.runtimes_dir().join(&key);
    std::fs::create_dir_all(&dest_dir)?;
    let fpm_path = dest_dir.join("php-fpm");

    progress(&format!("downloading {filename}…"));
    let bytes = http_get(&url)?;
    verify_or_note(variant, &listing, &filename, &bytes, &progress)?;

    progress("extracting…");
    extract_fpm(&bytes, &fpm_path)?;
    make_executable(&fpm_path)?;

    // Verify it actually runs — as the user, not as root. This is the first
    // time anything execs a binary that arrived over the network a moment ago,
    // which is exactly where a compromised mirror would want a root exec.
    let actual = crate::probe::first_stdout_line(&fpm_path, &["--version"]).unwrap_or_default();
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
    let (variant, resolved, listing) =
        resolve_with_fallback(variant, version_req, &suffix, &progress)?;
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
    verify_or_note(variant, &listing, &filename, &bytes, &progress)?;
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

/// The digest a publisher offers for `filename`, if any.
///
/// Only Grove's own release has one: the GitHub API serves an asset's `digest`
/// in the same response as its URL, so a Grove-built PHP can be verified with no
/// hash pinned in this repository.
///
/// static-php-cli's upstream archives publish nothing — no `.sha256`, no
/// signature, nothing in the directory listing. So `common` and `bulk` cannot be
/// verified at all, and this returns `None` for them rather than pretending
/// otherwise. That is one more reason Grove's own build is the default.
fn published_digest(variant: Variant, listing: &str, filename: &str) -> Option<String> {
    if variant != Variant::Grove || variant.mirror_dir().is_some() {
        return None;
    }
    let release: serde_json::Value = serde_json::from_str(listing).ok()?;
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(filename))?
        .get("digest")?
        .as_str()
        .map(|d| d.to_string())
}

/// Verify `bytes` against whatever the publisher offers, or say it could not be.
fn verify_or_note(
    variant: Variant,
    listing: &str,
    filename: &str,
    bytes: &[u8],
    progress: &impl Fn(&str),
) -> Result<()> {
    match published_digest(variant, listing, filename) {
        Some(expected) => {
            progress("verifying checksum…");
            grove_core::checksum::verify(filename, bytes, &expected)
                .map_err(|e| InstallError::Http(e.to_string()))
        }
        None => {
            // Said out loud once per download rather than buried: this binary is
            // about to be installed and executed, and nothing proved it is the
            // one the publisher shipped. The two reasons are different and the
            // message should not conflate them — upstream genuinely publishes no
            // checksum, whereas a missing Grove digest means we could not read
            // one (a rate-limited API, a build published without it).
            if variant == Variant::Grove {
                progress(&format!(
                    "note: could not read a published digest; {filename} not verified"
                ));
            } else {
                progress(&format!(
                    "note: upstream `{}` publishes no checksum; {filename} not verified",
                    variant.slug()
                ));
            }
            Ok(())
        }
    }
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
) -> Result<(Variant, SemVer, String)> {
    // Fetch the listing once and carry it back: it answers both "which version"
    // and "what digest", and GitHub's unauthenticated budget is 60 requests an
    // hour for the whole IP. Asking twice per archive spent four of them on a
    // single `grove php install`.
    /*
     * Two different failures, and only one of them may fall back.
     *
     * This used to treat them as one: any error reaching or parsing the listing
     * dropped to the upstream variant. So an exhausted API budget -- sixty
     * requests an hour, shared by everything on the IP -- was indistinguishable
     * from "Grove does not build that version", and the installer answered it by
     * fetching a different PHP. The result was a build missing mysqli and intl,
     * installed under the version that was asked for, announced by one "no grove
     * build for 8.5 yet" line that was not even true: the build existed, the
     * listing just could not be read.
     *
     * So: a listing that cannot be *fetched* is now fatal and says why. A listing
     * that was read and holds nothing for this version is the case the fallback
     * was written for, and still falls back.
     */
    let listing = http_get_string(&variant.listing_url()).map_err(|e| {
        InstallError::ListingUnreadable {
            variant: variant.slug().to_string(),
            req: version_req.to_string(),
            reason: e.to_string(),
        }
    })?;

    if let Ok(resolved) = resolve_from_listing(&listing, version_req, suffix) {
        return Ok((variant, resolved, listing));
    }

    let Some(alt) = variant.fallback() else {
        return Err(InstallError::NoMatch {
            req: version_req.to_string(),
            plat: suffix.to_string(),
        });
    };
    let alt_listing =
        http_get_string(&alt.listing_url()).map_err(|e| InstallError::ListingUnreadable {
            variant: alt.slug().to_string(),
            req: version_req.to_string(),
            reason: e.to_string(),
        })?;
    let resolved = resolve_from_listing(&alt_listing, version_req, suffix)?;
    progress(&format!(
        "no {} build for {version_req} yet — using upstream `{}` instead (`grove php ext` shows what it's missing)",
        variant.slug(),
        alt.slug(),
    ));
    Ok((alt, resolved, alt_listing))
}

/// Scrape the listing and pick the best matching version.
fn resolve_from_listing(listing: &str, version_req: &str, suffix: &str) -> Result<SemVer> {
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
///
/// Written beside the target and renamed into place, for the reason the CLI path
/// beside this one already gives — and for one more that cost an afternoon.
///
/// `File::create(dest)` truncates and rewrites the *existing inode*. macOS caches a
/// binary's code signature against that inode, so a php-fpm replaced in place is a
/// file whose cached signature no longer describes its bytes, and the kernel answers
/// by killing it: `php-fpm -v` exits 137 with no output and every site on the daemon
/// answers 502. Nothing diagnoses it. `codesign -v` reports "valid on disk" and
/// "satisfies its Designated Requirement", because the signature *is* valid — it is
/// the kernel's memory of the old one that is not.
///
/// Proved rather than guessed: the same bytes copied to a path that did not exist
/// before run fine, and at the replaced path exit 137. `rename` gives the destination
/// a new inode, which is what makes that difference.
///
/// The CLI half of the installer has always done this. The FPM half did not, and only
/// the FPM half is the thing the daemon execs.
fn extract_fpm(gz_bytes: &[u8], dest: &PathBuf) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let is_fpm = path.file_name().map(|n| n == "php-fpm").unwrap_or(false);
        if is_fpm {
            let tmp = dest.with_extension("part");
            let mut out = std::fs::File::create(&tmp)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            make_executable(&tmp)?;
            std::fs::rename(&tmp, dest)?;
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

    /// An unreadable listing is fatal, and says so rather than installing something else.
    ///
    /// The regression this guards: a rate-limited GitHub API used to be handled by
    /// quietly fetching the upstream variant, so `grove php install 8.5` returned a PHP
    /// without mysqli or intl under the version that was asked for. The only clue was a
    /// line claiming "no grove build for 8.5 yet", which was false — the build was there,
    /// the listing was not readable.
    #[test]
    fn an_unreadable_listing_is_fatal_and_explains_itself() {
        let err = InstallError::ListingUnreadable {
            variant: "grove".into(),
            req: "8.5".into(),
            reason: "403 rate limit exceeded".into(),
        };

        let msg = err.to_string();

        // What failed, and what it refused to do about it.
        assert!(msg.contains("could not read the `grove` build listing"), "{msg}");
        assert!(msg.contains("403 rate limit exceeded"), "{msg}");
        assert!(msg.contains("Not falling back"), "{msg}");

        // The three extensions whose absence is the actual damage, so the message
        // explains the cost rather than only the symptom.
        assert!(msg.contains("mysqli"), "{msg}");
        assert!(msg.contains("intl"), "{msg}");

        // And a way out: the likely cause, how to check it, and the deliberate opt-in.
        assert!(msg.contains("60 requests an hour"), "{msg}");
        assert!(msg.contains("rate_limit"), "{msg}");
        assert!(msg.contains("--variant common"), "{msg}");
    }

    /// Replacing a php-fpm must not write into the inode the old one occupied.
    ///
    /// macOS caches a binary's code signature against its inode, so truncating and
    /// rewriting one leaves the kernel holding a signature that no longer describes the
    /// bytes — and it answers by killing the process. `php-fpm -v` exits 137 with no
    /// output, every site answers 502, and `codesign -v` says "valid on disk" the whole
    /// time, because the signature is valid; the kernel's memory of the old one is not.
    ///
    /// Asserted on the inode rather than on behaviour, because the behaviour is a kernel
    /// policy this test cannot provoke on another platform — and the inode is the thing
    /// the fix is actually about.
    #[test]
    fn replacing_an_fpm_binary_gives_it_a_new_inode() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("grove-fpm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let dest = dir.join("php-fpm");

        // Something already there, as there is on every upgrade.
        std::fs::File::create(&dest)
            .expect("seed")
            .write_all(b"the previous build")
            .expect("seed write");

        #[cfg(unix)]
        let before = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&dest).expect("stat").ino()
        };

        extract_fpm(&tar_gz_containing_fpm(b"the new build"), &dest).expect("extract");

        assert_eq!(
            std::fs::read(&dest).expect("read"),
            b"the new build",
            "the new bytes have to land at the destination"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let after = std::fs::metadata(&dest).expect("stat").ino();
            assert_ne!(
                before, after,
                "php-fpm was rewritten in place; macOS kills a binary whose inode kept \
                 the old signature"
            );
        }

        // And nothing left behind for the next run to trip over.
        assert!(
            !dest.with_extension("part").exists(),
            "the temporary file must be renamed, not left beside the binary"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The smallest gzipped tar holding one `php-fpm` entry.
    fn tar_gz_containing_fpm(contents: &[u8]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "php-fpm", contents)
            .expect("append");
        let tarred = tar.into_inner().expect("tar");

        let mut gz =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        use std::io::Write;
        gz.write_all(&tarred).expect("gz");
        gz.finish().expect("gz finish")
    }

    /// A one-shot HTTP server: `/grove/` refuses, `/common/` answers with a listing.
    ///
    /// Enough to tell the two failures apart, which is the whole point. Reaching for
    /// `GROVE_PHP_MIRROR` rather than a mocking crate because the seam already exists —
    /// it is how a team points Grove at its own bucket — so the test drives the same
    /// path a user would.
    fn mirror_that_refuses_grove() -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));

        let handle = std::thread::spawn(move || {
            // Two requests at most: the grove listing, then the fallback's.
            for _ in 0..2 {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 1024];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();

                let body = r#"{"assets":[{"name":"php-8.5.11-fpm-macos-aarch64.tar.gz"}]}"#;

                let resp = if req.contains("/grove/") {
                    // What a spent API budget looks like from here.
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                };

                let _ = sock.write_all(resp.as_bytes());
            }
        });

        (base, handle)
    }

    /// The regression, driven rather than asserted on a hand-built error.
    ///
    /// The first version of this test checked only the message text and passed with the
    /// bug put back — it never called the function that chooses. This one does: with the
    /// grove listing refusing, resolution must fail rather than return `Common`.
    #[test]
    fn a_refused_listing_does_not_become_a_different_variant() {
        let (base, _server) = mirror_that_refuses_grove();

        // Serialised: `GROVE_PHP_MIRROR` is process-wide.
        let _guard = env_lock();
        unsafe { std::env::set_var("GROVE_PHP_MIRROR", &base) };

        let got = resolve_with_fallback(
            Variant::Grove,
            "8.5",
            "-fpm-macos-aarch64.tar.gz",
            &|_: &str| {},
        );

        unsafe { std::env::remove_var("GROVE_PHP_MIRROR") };

        match got {
            Err(InstallError::ListingUnreadable { variant, .. }) => {
                assert_eq!(variant, "grove", "the error must name the listing that failed");
            }
            Ok((v, ..)) => panic!(
                "a refused grove listing silently resolved to `{}` — this is the bug",
                v.slug()
            ),
            Err(e) => panic!("expected ListingUnreadable, got {e:?}"),
        }
    }

    /// `GROVE_PHP_MIRROR` is process-wide, so the tests that set it take a turn each.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The fallback the code was written for is still there.
    ///
    /// Asserted so that tightening the error above does not quietly turn into "never
    /// fall back": a version Grove genuinely does not build should still resolve to an
    /// upstream set, with the line that says so.
    #[test]
    fn a_listing_without_the_version_is_not_an_unreadable_listing() {
        let listing = r#"{"assets":[{"name":"php-8.4.22-fpm-macos-aarch64.tar.gz"}]}"#;

        // Present: resolves.
        assert!(resolve_from_listing(listing, "8.4", "-fpm-macos-aarch64.tar.gz").is_ok());

        // Absent: a NoMatch, which is what the fallback keys on — not a transport error.
        let missing = resolve_from_listing(listing, "8.9", "-fpm-macos-aarch64.tar.gz");
        assert!(
            matches!(missing, Err(InstallError::NoMatch { .. })),
            "a readable listing without the version must be NoMatch, got {missing:?}"
        );
    }

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

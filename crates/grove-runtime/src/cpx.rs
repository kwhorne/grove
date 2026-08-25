//! `cpx` — the Composer Package Executor, bundled.
//!
//! cpx is to Composer what `npx` is to npm: run a CLI from any Composer package
//! without installing it in your project or globally (`cpx laravel/pint`,
//! `cpx friendsofphp/php-cs-fixer fix`). It also runs ad-hoc PHP with your
//! project booted (`cpx exec -r …`, `cpx tinker`).
//!
//! Grove ships it the same way it ships `php`, `composer` and `node`: as a PATH
//! shim over a Grove-managed binary, so `cpx` is simply there once
//! `grove path install` has run — nothing to `composer global require`.
//!
//! cpx is distributed as a self-contained PHAR with Composer bundled *inside*
//! it, so all Grove has to supply is the PHAR and a PHP ≥ 8.3 to run it on.

use std::io::Read;
use std::path::PathBuf;

/// The release API for cpx's newest version.
///
/// Used instead of the `releases/latest/download/cpx` redirect because this
/// response carries the asset's `digest` alongside its URL, so the download can
/// be verified without a hash pinned in this repository.
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/laravel/cpx/releases/latest";

/// The API-free download URL, used when the release metadata is unreachable.
const LATEST_REDIRECT: &str = "https://github.com/laravel/cpx/releases/latest/download/cpx";

/// Minimum PHP cpx supports (`"php": "^8.3"` in its composer.json).
pub const MIN_PHP: (u32, u32) = (8, 3);

#[derive(Debug, thiserror::Error)]
pub enum CpxError {
    #[error("http error: {0}")]
    Http(String),
    #[error(
        "the download from {0} was not a PHP archive — the mirror may be returning an error page"
    )]
    NotAPhar(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Checksum(String),
}

pub type Result<T> = std::result::Result<T, CpxError>;

/// Where Grove keeps the cpx PHAR.
///
/// Deliberately the user's `~/.grove` rather than `$GROVE_HOME`: the daemon
/// never needs cpx (it isn't part of serving a site or scaffolding one), only
/// the user does, through the shim. `$GROVE_HOME` is root-owned under the
/// LaunchDaemon, and a root-owned PHAR would break `cpx self-update`, which
/// replaces the file in place.
pub fn phar_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".grove/cpx.phar")
}

/// Path to the cpx PHAR, downloading it on first use.
pub fn ensure(progress: impl Fn(&str)) -> Result<PathBuf> {
    let dest = phar_path();
    if dest.exists() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    progress("resolving the latest cpx…");
    // GitHub's API is rate-limited to 60 requests an hour per IP when
    // unauthenticated, and that budget is shared by everything behind the same
    // NAT. Falling back to the plain redirect keeps `cpx` installable when the
    // budget is gone — unverified, and it says so — rather than turning a shared
    // office address into "cpx cannot be installed today".
    let (url, digest) = match latest_asset() {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the cpx release metadata");
            (LATEST_REDIRECT.to_string(), None)
        }
    };

    progress("downloading cpx…");
    let bytes = download(&url)?;
    if !looks_like_phar(&bytes) {
        return Err(CpxError::NotAPhar(url.clone()));
    }
    // This PHAR is executed by every `cpx` invocation, so verify before it lands
    // on disk. GitHub serves the digest in the same response as the URL, which
    // catches a corrupted or swapped asset — not a compromised publisher.
    match digest {
        Some(expected) => {
            grove_core::checksum::verify("cpx", &bytes, &expected)
                .map_err(|e| CpxError::Checksum(e.to_string()))?;
        }
        None => progress("note: no digest available; cpx not verified"),
    }
    // Write to a sibling then rename, so two shells racing on the first `cpx`
    // can't leave a half-written PHAR behind for the loser to execute.
    let tmp = dest.with_extension("phar.part");
    std::fs::write(&tmp, &bytes)?;
    make_executable(&tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// A PHAR starts with a PHP stub. Anything else — an HTML error page, an S3
/// XML fault — is a failed download dressed up as a success.
fn looks_like_phar(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(64)];
    let head = String::from_utf8_lossy(head);
    (head.starts_with("#!") && head.contains("php")) || head.starts_with("<?php")
}

/// The download URL and published digest of the newest `cpx` asset.
fn latest_asset() -> Result<(String, Option<String>)> {
    // GitHub's API refuses requests without a User-Agent.
    let body = ureq::get(LATEST_RELEASE_API)
        .set("User-Agent", "grove")
        .call()
        .map_err(|e| CpxError::Http(e.to_string()))?
        .into_string()
        .map_err(CpxError::Io)?;
    let release: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| CpxError::Http(e.to_string()))?;
    let asset = release
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(|n| n.as_str()) == Some("cpx"))
        })
        .ok_or_else(|| CpxError::Http("the cpx release has no `cpx` asset".into()))?;
    let url = asset
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| CpxError::Http("the cpx asset has no download URL".into()))?
        .to_string();
    let digest = asset
        .get("digest")
        .and_then(|d| d.as_str())
        .map(|d| d.to_string());
    Ok((url, digest))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| CpxError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}
#[cfg(not(unix))]
fn make_executable(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phar_stubs_are_accepted() {
        assert!(looks_like_phar(b"#!/usr/bin/env php\n<?php ..."));
        assert!(looks_like_phar(b"<?php // phar stub"));
    }

    #[test]
    fn error_pages_are_rejected() {
        assert!(!looks_like_phar(b"<!DOCTYPE html><html>404</html>"));
        assert!(!looks_like_phar(b"<?xml version=\"1.0\"?><Error/>"));
        assert!(!looks_like_phar(b""));
    }

    #[test]
    fn phar_lives_under_the_users_grove_dir() {
        let p = phar_path();
        assert!(p.ends_with(".grove/cpx.phar"), "{}", p.display());
    }
}

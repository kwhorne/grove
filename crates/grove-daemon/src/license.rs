//! License storage + entitlement checks for Grove Pro / Teams.
//!
//! The signed key is verified **offline** by `grove-license` against a baked-in
//! public key, then stored at `$GROVE_HOME/license.key`. Storage runs in the
//! (root) daemon so it lands in the shared home; verification is recomputed on
//! demand (cheap — read a file + one Ed25519 check).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use grove_core::paths::GrovePaths;
use grove_license::{LicenseClaims, LicenseError};

fn license_path(paths: &GrovePaths) -> PathBuf {
    paths.base().join("license.key")
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Verify a key and, if valid, persist it. Returns the claims on success.
pub fn activate(paths: &GrovePaths, key: &str) -> Result<LicenseClaims, LicenseError> {
    let key = key.trim();
    let claims = grove_license::verify(key, now_unix())?;
    if let Err(e) = std::fs::write(license_path(paths), key) {
        tracing::warn!(error = %e, "could not persist license");
        return Err(LicenseError::Misconfigured);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(license_path(paths), std::fs::Permissions::from_mode(0o644));
    }
    Ok(claims)
}

/// The current entitlement, if a valid (non-expired) license is stored.
pub fn current(paths: &GrovePaths) -> Option<LicenseClaims> {
    let key = std::fs::read_to_string(license_path(paths)).ok()?;
    grove_license::verify(key.trim(), now_unix()).ok()
}

/// Remove the stored license.
pub fn deactivate(paths: &GrovePaths) -> std::io::Result<()> {
    let path = license_path(paths);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// True when a valid Pro (or Teams) license is active.
pub fn is_pro(paths: &GrovePaths) -> bool {
    current(paths).map(|c| c.is_pro()).unwrap_or(false)
}

/// True when a valid Teams license is active.
pub fn is_teams(paths: &GrovePaths) -> bool {
    current(paths).map(|c| c.is_teams()).unwrap_or(false)
}

/// Gate a Pro feature: `Ok(())` when entitled, otherwise a friendly message.
pub fn require_pro(paths: &GrovePaths) -> Result<(), String> {
    if is_pro(paths) {
        Ok(())
    } else {
        Err(
            "This is a Grove Pro feature — activate a license with `grove license activate <key>`."
                .into(),
        )
    }
}

/// Gate a Teams feature.
pub fn require_teams(paths: &GrovePaths) -> Result<(), String> {
    if is_teams(paths) {
        Ok(())
    } else {
        Err("This is a Grove Teams feature — activate a Teams license with `grove license activate <key>`.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_license_is_not_entitled() {
        let dir = std::env::temp_dir().join(format!("grove-lic-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = GrovePaths::with_base(&dir);

        assert!(current(&paths).is_none());
        assert!(!is_pro(&paths) && !is_teams(&paths));
        assert!(require_pro(&paths).is_err());
        assert!(require_teams(&paths).is_err());

        // A bogus key is rejected (bad signature) and not stored.
        assert!(activate(&paths, "GROVE-not.valid").is_err());
        assert!(!license_path(&paths).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

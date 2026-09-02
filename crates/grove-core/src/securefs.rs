//! Creating files that a root process can safely write into.
//!
//! Grove's daemon usually runs as root, and several of the files it writes are
//! either secrets (a CA private key, a TLS leaf key) or dumps of an entire
//! database. Both were created with plain `fs::write` / `File::create`, which
//! has two problems when the destination sits anywhere an unprivileged user can
//! influence — and `$GROVE_HOME` is exactly such a place, since the service
//! installer points a root daemon at the user's own home:
//!
//! 1. **Symlinks are followed.** Replace `certs/grove-ca.key` with a link to
//!    `/etc/anything` and root writes — and `chmod`s — the target.
//! 2. **The mode arrives late.** `write` then `set_permissions` leaves the file
//!    readable at the process umask (usually `0644`) for the window in between,
//!    which is long enough to matter for a private key, and for the files that
//!    never got a `set_permissions` call at all it is simply the final mode.
//!
//! [`create_private`] and [`create_public`] close both: the mode is passed to
//! `open(2)` so it is right from the first byte, and `O_NOFOLLOW` makes the
//! kernel refuse a symlink at the destination rather than write through it.
//!
//! ## What this does not do
//!
//! `O_NOFOLLOW` only guards the **last** path component. If `certs/` is itself a
//! symlink, the write still lands wherever it points. [`parent_is_symlink`]
//! exists to check that one level up, because swapping a directory is the
//! realistic version of that attack; guarding the whole ancestry would need an
//! `openat` walk, and the real fix is for those directories not to be inside a
//! user-writable tree in the first place.

use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Mode for a file only its owner may read: private keys, database dumps.
pub const MODE_PRIVATE: u32 = 0o600;

/// Mode for a file that is meant to be readable — a certificate, a log.
pub const MODE_PUBLIC: u32 = 0o644;

/// Create (or truncate) `path` with mode `0600`, refusing to follow a symlink.
///
/// Truncates rather than failing on an existing file, so this is a drop-in for
/// `File::create`: refusing would change the behaviour of re-running a snapshot
/// or reissuing a certificate, and the symlink is what we are defending against,
/// not the overwrite.
pub fn create_private(path: &Path) -> std::io::Result<File> {
    create_with_mode(path, MODE_PRIVATE)
}

/// As [`create_private`], but for content that is not secret.
pub fn create_public(path: &Path) -> std::io::Result<File> {
    create_with_mode(path, MODE_PUBLIC)
}

/// Write `contents` to `path` with mode `0600`, refusing to follow a symlink.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    create_private(path)?.write_all(contents.as_ref())
}

/// Write `contents` to `path` with mode `0644`, refusing to follow a symlink.
pub fn write_public(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    create_public(path)?.write_all(contents.as_ref())
}

/// Write `contents` to `path` so that neither a concurrent reader nor a crash
/// can observe a half-written file: the bytes go to a sibling temp file with the
/// final mode, are fsynced, and are renamed over `path` in one step.
///
/// Every state file Grove owns — `config.toml`, `php-builds.json`, the service
/// and snapshot indexes — used to be written in place with truncate + write. A
/// crash between the two left a zero-length or truncated file, which the next
/// boot parsed as "corrupt", replaced with defaults, and saved: every site,
/// every registered PHP build, every port override, gone silently.
pub fn write_public_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    write_atomic(path, contents.as_ref(), MODE_PUBLIC)
}

/// As [`write_public_atomic`], with mode `0600`.
pub fn write_private_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    write_atomic(path, contents.as_ref(), MODE_PRIVATE)
}

fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = path.with_file_name(format!(".{name}.tmp-{}-{nanos}", std::process::id()));

    let result = (|| {
        let mut f = create_with_mode(&tmp, mode)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Move a file Grove could not parse aside as `<name>.corrupt-<unix-seconds>`,
/// so it is kept for inspection instead of being overwritten by the defaults
/// that replace it. Returns where it went.
pub fn quarantine(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let dest = path.with_file_name(format!("{name}.corrupt-{secs}"));
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// Read and parse a JSON state file, treating a file that exists but does not
/// parse as an *event*: it is quarantined and logged at error level, and the
/// caller gets `T::default()`. A missing file is simply the default.
///
/// The old shape everywhere was `serde_json::from_str(..).unwrap_or_default()`,
/// which turned a truncated `php-builds.json` into an empty registry and then
/// saved the empty registry over it.
pub fn read_json_or_quarantine<T>(path: &Path) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return T::default(),
    };
    match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            match quarantine(path) {
                Ok(dest) => tracing::error!(
                    file = %path.display(),
                    moved_to = %dest.display(),
                    error = %e,
                    "state file does not parse; moved aside and continuing with defaults"
                ),
                Err(mv) => tracing::error!(
                    file = %path.display(),
                    error = %e,
                    move_error = %mv,
                    "state file does not parse and could not be moved aside"
                ),
            }
            T::default()
        }
    }
}

/// Files a [`read_json_or_quarantine`] has set aside under `dir` (one level).
/// `grove doctor` reports them so the event is not only a log line.
pub fn quarantined_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.contains(".corrupt-") {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
fn create_with_mode(path: &Path, mode: u32) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .and_then(|f| {
            // `.mode()` applies only when the file is created. An existing file
            // keeps whatever it had, so bring it into line too — otherwise a key
            // reissued over a world-readable predecessor stays world-readable.
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            Ok(f)
        })
}

#[cfg(not(unix))]
fn create_with_mode(path: &Path, _mode: u32) -> std::io::Result<File> {
    std::fs::File::create(path)
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Whether `path`'s parent directory is a symlink.
///
/// `O_NOFOLLOW` cannot see this, so callers writing secrets into a directory
/// they do not own should check it and refuse. Returns `false` when the parent
/// cannot be inspected — an unreadable parent will fail the open anyway, and a
/// hard error here would turn a missing directory into a confusing refusal.
pub fn parent_is_symlink(path: &Path) -> bool {
    path.parent()
        .and_then(|p| std::fs::symlink_metadata(p).ok())
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("grove-securefs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn private_files_are_owner_only_from_creation() {
        let dir = scratch("private");
        let f = dir.join("key.pem");
        write_private(&f, b"secret").unwrap();
        assert_eq!(mode_of(&f), 0o600, "mode is {:o}", mode_of(&f));
        let mut s = String::new();
        File::open(&f).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn public_files_stay_readable() {
        let dir = scratch("public");
        let f = dir.join("cert.pem");
        write_public(&f, b"cert").unwrap();
        assert_eq!(mode_of(&f), 0o644);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The attack: a symlink where the secret is meant to go. Root must refuse
    /// rather than write through it.
    #[test]
    fn a_symlink_destination_is_refused_and_the_target_untouched() {
        let dir = scratch("symlink");
        let target = dir.join("victim");
        std::fs::write(&target, b"original").unwrap();
        let link = dir.join("key.pem");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = write_private(&link, b"attacker-controlled").unwrap_err();
        assert!(
            matches!(err.raw_os_error(), Some(libc::ELOOP) | Some(libc::EMLINK)),
            "expected ELOOP, got {err:?}"
        );

        let mut s = String::new();
        File::open(&target).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "original", "the symlink target must not be written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A dangling symlink is the sneakier variant: `File::create` would happily
    /// create the target.
    #[test]
    fn a_dangling_symlink_destination_is_refused() {
        let dir = scratch("dangling");
        let target = dir.join("does-not-exist-yet");
        let link = dir.join("key.pem");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(write_private(&link, b"nope").is_err());
        assert!(!target.exists(), "the target must not have been created");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Overwriting a real file must keep working — a snapshot or a reissued
    /// certificate writes to the same path again.
    #[test]
    fn an_existing_regular_file_is_truncated_not_refused() {
        let dir = scratch("overwrite");
        let f = dir.join("dump.sql");
        write_private(&f, b"a much longer first version").unwrap();
        write_private(&f, b"second").unwrap();
        let mut s = String::new();
        File::open(&f).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "second", "must truncate, not append");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that already exists with a loose mode must be tightened, not left
    /// as it was — the creation mode alone does not cover this.
    #[test]
    fn an_existing_world_readable_file_is_tightened() {
        let dir = scratch("tighten");
        let f = dir.join("key.pem");
        std::fs::write(&f, b"old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&f), 0o644);

        write_private(&f, b"new").unwrap();
        assert_eq!(mode_of(&f), 0o600, "mode is {:o}", mode_of(&f));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parent_symlink_is_detected() {
        let dir = scratch("parent");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("linked");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(parent_is_symlink(&link.join("key.pem")));
        assert!(!parent_is_symlink(&real.join("key.pem")));
        // A parent we cannot stat is not reported as a symlink.
        assert!(!parent_is_symlink(&dir.join("missing/key.pem")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_atomic_write_lands_whole_with_the_right_mode_and_no_leftovers() {
        let dir = std::env::temp_dir().join(format!("grove-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("state.json");
        write_private_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}");
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            MODE_PRIVATE
        );
        // Overwrite: the new content replaces the old in one step.
        write_public_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        // Nothing but the file itself remains in the directory.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["state.json".to_string()],
            "no temp files left: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[derive(Default, serde::Deserialize, PartialEq, Debug)]
    struct Toy {
        n: u32,
    }

    /// The shape that lost people their PHP builds: a truncated JSON file
    /// parsed as empty and then saved over. Now it is moved aside and shows up
    /// in `quarantined_files`.
    #[test]
    fn a_corrupt_state_file_is_quarantined_not_silently_defaulted() {
        let dir = std::env::temp_dir().join(format!("grove-quar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("builds.json");
        std::fs::write(&path, "{\"n\": 4").unwrap(); // truncated mid-write

        let got: Toy = read_json_or_quarantine(&path);
        assert_eq!(got, Toy::default(), "the caller proceeds with defaults");
        assert!(!path.exists(), "the corrupt file is no longer at its path");
        let set_aside = quarantined_files(&dir);
        assert_eq!(set_aside.len(), 1, "{set_aside:?}");
        assert_eq!(
            std::fs::read_to_string(&set_aside[0]).unwrap(),
            "{\"n\": 4",
            "its bytes are preserved for inspection"
        );

        // A valid file parses and is left alone; a missing one is the default.
        std::fs::write(&path, "{\"n\": 7}").unwrap();
        assert_eq!(read_json_or_quarantine::<Toy>(&path), Toy { n: 7 });
        assert!(path.exists());
        assert_eq!(
            read_json_or_quarantine::<Toy>(&dir.join("nope.json")),
            Toy::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

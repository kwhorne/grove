//! The CA key becoming root-owned — which can only be observed as root.
//!
//! The unit tests can show that claiming is a harmless no-op unprivileged. They
//! cannot show the half that matters: that a root process actually takes the key
//! away from the login user, including one it inherited from an unprivileged
//! `grove init`.
//!
//! Skips itself unless it finds itself root, so it is a no-op on a developer
//! machine and in the normal CI job:
//!
//! ```console
//! $ docker run --rm -v "$PWD:/w" -w /w rust:alpine \
//!     cargo test -p grove-tls --test ca_ownership_root
//! ```

use std::os::unix::fs::MetadataExt;

use grove_core::paths::GrovePaths;
use grove_tls::CertificateAuthority;

fn is_root() -> bool {
    grove_core::privdrop::running_as_root()
}

fn scratch(name: &str) -> GrovePaths {
    let base = std::env::temp_dir().join(format!("grove-ca-root-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    GrovePaths::with_base(&base)
}

#[test]
fn a_ca_created_by_root_is_owned_by_root() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let paths = scratch("create");
    CertificateAuthority::load_or_create(&paths).unwrap();
    let key = std::fs::metadata(paths.ca_key()).unwrap();
    assert_eq!(key.uid(), 0, "CA key should be root-owned");
    assert_eq!(key.mode() & 0o777, 0o600, "and owner-only");
    let _ = std::fs::remove_dir_all(paths.base());
}

/// The migration case: a key left behind by an unprivileged `grove init` must be
/// claimed the first time a root daemon loads it.
#[test]
fn a_user_owned_key_is_claimed_on_the_next_root_load() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let paths = scratch("claim");
    CertificateAuthority::load_or_create(&paths).unwrap();

    // Hand it to an unprivileged user, as a non-sudo first run would have.
    const NOBODY: u32 = 65534;
    std::os::unix::fs::chown(paths.ca_key(), Some(NOBODY), Some(NOBODY)).unwrap();
    assert_eq!(
        std::fs::metadata(paths.ca_key()).unwrap().uid(),
        NOBODY,
        "precondition: the key starts user-owned"
    );

    // Loading it as root should take it back.
    CertificateAuthority::load_or_create(&paths).unwrap();
    assert_eq!(
        std::fs::metadata(paths.ca_key()).unwrap().uid(),
        0,
        "a root load must claim a user-owned CA key"
    );
    let _ = std::fs::remove_dir_all(paths.base());
}

/// The certificate is public — it has to stay readable, or nothing can verify
/// the chain. Claiming the key must not tighten the cert by accident.
#[test]
fn the_certificate_stays_readable() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let paths = scratch("cert");
    CertificateAuthority::load_or_create(&paths).unwrap();
    let mode = std::fs::metadata(paths.ca_cert()).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o644, "CA certificate mode is {mode:o}");
    let _ = std::fs::remove_dir_all(paths.base());
}

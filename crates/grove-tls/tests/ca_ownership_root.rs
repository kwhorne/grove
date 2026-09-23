//! Who owns the CA private key — which can only be observed as root.
//!
//! The unit tests can show that claiming is a harmless no-op unprivileged. They
//! cannot show the half that matters: that a root process actually moves the
//! key to the identity the daemon will run as, and that it leaves the key with
//! root on a machine where no run user was ever recorded.
//!
//! This inverted with socket activation. The daemon used to be root, so the key
//! was claimed *for* root and unprivileged code could not read it. launchd and
//! systemd now bind the privileged ports and hand the descriptors over, so the
//! process that signs leaf certificates runs as the login user — and a key it
//! cannot read is a daemon that cannot serve HTTPS at all.
//!
//! Skips itself unless it finds itself root, so it is a no-op on a developer
//! machine and in the normal CI job:
//!
//! ```console
//! $ docker run --rm -v "$PWD:/w" -w /w rust:alpine \
//!     cargo test -p grove-tls --test ca_ownership_root
//! ```

use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_tls::CertificateAuthority;

/// `GROVE_RUN_USER_ID` is process-global, and these tests set it. Serialize
/// them rather than trusting the harness's thread count.
static ENV: Mutex<()> = Mutex::new(());

/// An account that exists on every Unix and owns nothing here.
const NOBODY: u32 = 65534;

fn is_root() -> bool {
    grove_core::ownership::running_as_root()
}

fn scratch(name: &str) -> GrovePaths {
    let base = std::env::temp_dir().join(format!("grove-ca-root-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    GrovePaths::with_base(&base)
}

fn with_run_user<T>(ids: Option<(u32, u32)>, body: impl FnOnce() -> T) -> T {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    match ids {
        Some((uid, gid)) => {
            std::env::set_var("GROVE_RUN_USER_ID", uid.to_string());
            std::env::set_var("GROVE_RUN_GROUP_ID", gid.to_string());
        }
        None => {
            std::env::remove_var("GROVE_RUN_USER_ID");
            std::env::remove_var("GROVE_RUN_GROUP_ID");
        }
    }
    // `ownership::named_from_env` falls back to these, which would defeat the
    // "nothing recorded" case on a machine where they happen to be set.
    std::env::remove_var("GROVE_RUN_USER");
    std::env::remove_var("SUDO_USER");
    let out = body();
    std::env::remove_var("GROVE_RUN_USER_ID");
    std::env::remove_var("GROVE_RUN_GROUP_ID");
    out
}

/// The daemon will run as the recorded user, so the key has to reach that user
/// — otherwise the first HTTPS request fails on a key the signer cannot open.
#[test]
fn a_root_install_hands_the_key_to_the_user_the_daemon_will_run_as() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    with_run_user(Some((NOBODY, NOBODY)), || {
        let paths = scratch("handover");
        CertificateAuthority::load_or_create(&paths).unwrap();
        let key = std::fs::metadata(paths.ca_key()).unwrap();
        assert_eq!(key.uid(), NOBODY, "the run user must own the CA key");
        assert_eq!(
            key.mode() & 0o777,
            0o600,
            "and still be the only one who can read it"
        );
        let _ = std::fs::remove_dir_all(paths.base());
    });
}

/// The migration case, in the direction it now runs: a key root created before
/// the handover existed must move to the run user on the next root load, not
/// stay where a non-root daemon cannot read it.
#[test]
fn a_root_owned_key_is_handed_over_on_the_next_root_load() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let paths = with_run_user(None, || {
        let paths = scratch("migrate");
        CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(
            std::fs::metadata(paths.ca_key()).unwrap().uid(),
            0,
            "precondition: the key starts root-owned"
        );
        paths
    });

    with_run_user(Some((NOBODY, NOBODY)), || {
        CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(
            std::fs::metadata(paths.ca_key()).unwrap().uid(),
            NOBODY,
            "a root load must hand a root-owned CA key to the run user"
        );
    });
    let _ = std::fs::remove_dir_all(paths.base());
}

/// No run user recorded means the daemon stays root as well, so root keeping
/// the key is still correct — and still readable by the process that needs it.
/// Handing it to a guessed identity would be worse than leaving it alone.
#[test]
fn with_no_run_user_recorded_the_key_stays_with_root() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    with_run_user(None, || {
        let paths = scratch("noone");
        CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(std::fs::metadata(paths.ca_key()).unwrap().uid(), 0);
        let _ = std::fs::remove_dir_all(paths.base());
    });
}

/// The certificate is public — it has to stay readable, or nothing can verify
/// the chain. Moving the key must not tighten the cert by accident.
#[test]
fn the_certificate_stays_readable() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    with_run_user(Some((NOBODY, NOBODY)), || {
        let paths = scratch("cert");
        CertificateAuthority::load_or_create(&paths).unwrap();
        let mode = std::fs::metadata(paths.ca_cert()).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o644, "CA certificate mode is {mode:o}");
        let _ = std::fs::remove_dir_all(paths.base());
    });
}

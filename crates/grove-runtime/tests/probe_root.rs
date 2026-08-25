//! Runtime probes dropping privileges — observable only as root.
//!
//! `probe::output` is how Grove asks a managed runtime about itself: `php-fpm
//! --version` after a download, `php -m` for the extension audit, `php -i` for
//! the Xdebug lookup. Those binaries live under `$GROVE_HOME`, which the service
//! installer points at the user's home directory, and the probes run inside the
//! root daemon — so they must not exec as root.
//!
//! Skips itself unless it finds itself root:
//!
//! ```console
//! $ docker run --rm -v "$PWD:/w" -w /w rust:alpine \
//!     cargo test -p grove-runtime --test probe_root
//! ```

use std::path::Path;

use grove_runtime::probe;

fn is_root() -> bool {
    grove_core::privdrop::running_as_root()
}

/// Ask a probe which uid it ran as.
fn probed_uid() -> String {
    probe::first_stdout_line(Path::new("/bin/sh"), &["-c", "id -u"]).expect("sh should run")
}

#[test]
fn a_probe_does_not_exec_as_root_when_a_run_user_is_known() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    // Tell the drop who to become, the way the service unit does.
    std::env::set_var("GROVE_RUN_USER_ID", "65534");
    std::env::set_var("GROVE_RUN_GROUP_ID", "65534");

    assert_eq!(
        probed_uid(),
        "65534",
        "the probe should have run as the run user, not as root"
    );

    std::env::remove_var("GROVE_RUN_USER_ID");
    std::env::remove_var("GROVE_RUN_GROUP_ID");
}

/// With no run user recorded there is nothing to drop to, and the probe must
/// still work — otherwise the extension audit would go blank on a daemon that
/// cannot identify its user, which is a worse outcome than the exec it avoids.
#[test]
fn a_probe_still_runs_when_no_run_user_is_recorded() {
    if !is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    for v in [
        "GROVE_RUN_USER_ID",
        "GROVE_RUN_GROUP_ID",
        "GROVE_RUN_USER",
        "SUDO_USER",
    ] {
        std::env::remove_var(v);
    }
    assert_eq!(
        probed_uid(),
        "0",
        "with nothing to drop to, the probe runs as-is rather than failing shut"
    );
}

//! The privilege drop, exercised for real — which needs root.
//!
//! `setgroups` is privileged even when it would change nothing, so the unit
//! tests in `privdrop` can only prove the *refusal* path from an unprivileged
//! run. That leaves the half that matters untested: that a dropped child really
//! comes out as the requested user, with root's supplementary groups gone.
//!
//! These tests skip themselves unless they happen to be running as root, so they
//! are a no-op on a developer machine and in the normal CI job, and real
//! evidence anywhere the suite runs privileged (a container, a root shell):
//!
//! ```console
//! $ docker run --rm -v "$PWD:/w" -w /w rust:alpine \
//!     cargo test -p grove-core --test privdrop_root -- --nocapture
//! ```

use std::process::Command;

use grove_core::privdrop::{apply, running_as_root, RunAs};

/// An unprivileged identity that exists on essentially every unix: `nobody` is
/// conventionally 65534, and the tests only need *an* id that is not 0.
const NOBODY: RunAs = RunAs {
    uid: 65534,
    gid: 65534,
};

fn stdout_of(mut cmd: Command) -> String {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_dropped_child_runs_as_the_requested_user() {
    if !running_as_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let mut cmd = Command::new("id");
    cmd.arg("-u");
    apply(&mut cmd, Some(NOBODY));
    assert_eq!(stdout_of(cmd), NOBODY.uid.to_string());
}

#[test]
fn a_dropped_child_runs_with_the_requested_group() {
    if !running_as_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let mut cmd = Command::new("id");
    cmd.arg("-g");
    apply(&mut cmd, Some(NOBODY));
    assert_eq!(stdout_of(cmd), NOBODY.gid.to_string());
}

/// The one the old implementation got wrong: it called `setgroups` and discarded
/// the result, so a failure left the child holding root's groups after its uid
/// and gid had been dropped.
#[test]
fn a_dropped_child_keeps_none_of_roots_supplementary_groups() {
    if !running_as_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let mut cmd = Command::new("id");
    cmd.arg("-G");
    apply(&mut cmd, Some(NOBODY));
    let out = stdout_of(cmd);
    let groups: Vec<&str> = out.split_whitespace().collect();
    assert!(
        !groups.contains(&"0"),
        "root's group 0 survived the drop: {groups:?}"
    );
    assert_eq!(
        groups,
        vec![NOBODY.gid.to_string()],
        "only the requested group should remain"
    );
}

/// Root really can spawn without dropping — confirms the tests above are
/// measuring the drop and not something incidental about the child.
#[test]
fn without_a_target_the_child_stays_root() {
    if !running_as_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let mut cmd = Command::new("id");
    cmd.arg("-u");
    apply(&mut cmd, None);
    assert_eq!(stdout_of(cmd), "0");
}

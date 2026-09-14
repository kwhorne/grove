//! Handing files to the user Grove serves.
//!
//! This module used to be called `privdrop`, and it did what the name says: the
//! daemon ran as root, because binding 80/443/53 needs root, and every child it
//! spawned — PHP-FPM pools, PostgreSQL, MySQL, Redis, Vite, Composer — had to
//! `setgroups`/`setgid`/`setuid` down to the login user before `exec`. Almost
//! all of those binaries live under `$GROVE_HOME`, a tree the user can write,
//! so a root process exec'ing out of it was a local privilege escalation
//! waiting for someone to rewrite one JSON file.
//!
//! Since 1.8.0 launchd and systemd bind the privileged ports and hand the
//! descriptors over, and the daemon runs as the user from its first
//! instruction. Children inherit that. There is nothing left to drop, so the
//! dropping is gone, and with it the `pre_exec` block that was the most
//! delicate `unsafe` in the workspace.
//!
//! What remains is the other half of the old module, which the privileged
//! *commands* still need. `sudo grove install` and `sudo grove init` create
//! files as root — the CA, the config, the whole Grove home — and those files
//! have to end up belonging to the user who will read and write them. That is
//! all this does now.

use std::process::Command;

/// The user Grove serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAs {
    pub uid: u32,
    pub gid: u32,
}

/// Whether this process has effective uid 0.
///
/// True for `sudo grove install` and friends. False for the daemon, which is
/// what [`crate::ownership`] exists to keep true — see `grove-daemon`, which
/// refuses to start as root rather than run your sites with privilege.
pub fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        // Safe: `geteuid` takes no arguments, touches no memory, cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The user a privileged command is acting on behalf of, or `None`.
///
/// `None` means there is nothing to hand over: either this process is not root
/// (so whatever it creates already belongs to the right person) or it cannot
/// tell who to hand files to. Never guesses — giving the Grove home to the
/// wrong account would lock the real user out of their own daemon.
#[cfg(unix)]
pub fn run_user() -> Option<RunAs> {
    if !running_as_root() {
        return None;
    }
    numeric_from_env().or_else(named_from_env)
}

#[cfg(not(unix))]
pub fn run_user() -> Option<RunAs> {
    None
}

/// `GROVE_RUN_USER_ID` / `GROVE_RUN_GROUP_ID`, written into the service unit by
/// `grove install`.
///
/// Preferred because it is already numeric: no `getpwnam`, no NSS, and no
/// subprocess just to learn a uid.
fn numeric_from_env() -> Option<RunAs> {
    let uid: u32 = std::env::var("GROVE_RUN_USER_ID")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if uid == 0 {
        return None;
    }
    let gid: u32 = std::env::var("GROVE_RUN_GROUP_ID")
        .ok()
        .and_then(|g| g.trim().parse().ok())
        .unwrap_or(uid);
    Some(RunAs { uid, gid })
}

fn named_from_env() -> Option<RunAs> {
    let user = ["GROVE_RUN_USER", "SUDO_USER"].iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|u| !u.is_empty() && u != "root")
    })?;
    let uid = id_of("-u", &user)?;
    let gid = id_of("-g", &user)?;
    if uid == 0 {
        return None;
    }
    Some(RunAs { uid, gid })
}

fn id_of(flag: &str, user: &str) -> Option<u32> {
    let out = Command::new("id").args([flag, user]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Give `path` to `run_as`, so the daemon can write inside it later.
///
/// Best-effort: a failure here surfaces as the daemon's own error, which is
/// more specific than anything this could report.
pub fn own_path(path: &std::path::Path, run_as: Option<RunAs>) {
    let Some(RunAs { uid, gid }) = run_as else {
        return;
    };
    if let Err(e) = std::os::unix::fs::chown(path, Some(uid), Some(gid)) {
        tracing::debug!(error = %e, path = %path.display(), "could not hand path to the run user");
    }
}

/// As [`own_path`], but for a whole tree.
pub fn own_tree(path: &std::path::Path, run_as: Option<RunAs>) {
    let Some(RunAs { uid, gid }) = run_as else {
        return;
    };
    // `chown -R` rather than a hand-rolled walk: it is one exec instead of one
    // syscall per entry on a Grove home that can hold ten thousand files, and
    // it does not follow symlinks out of the tree by default.
    let status = Command::new("chown")
        .arg("-R")
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .status();
    if let Ok(s) = status {
        if !s.success() {
            tracing::debug!(path = %path.display(), "chown -R reported a failure");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `set_var`/`remove_var` are process-global and tests run in parallel
    /// threads of one process.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear() {
        for v in [
            "GROVE_RUN_USER_ID",
            "GROVE_RUN_GROUP_ID",
            "GROVE_RUN_USER",
            "SUDO_USER",
        ] {
            std::env::remove_var(v);
        }
    }

    /// Numbers beat names: they need no `getpwnam`, no NSS, and no subprocess,
    /// none of which behave the same inside a sandbox.
    #[test]
    fn numeric_ids_win_over_a_username() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        std::env::set_var("GROVE_RUN_GROUP_ID", "20");
        std::env::set_var("GROVE_RUN_USER", "someone-else");
        assert_eq!(numeric_from_env(), Some(RunAs { uid: 501, gid: 20 }));
        clear();
    }

    /// A missing group id is not a reason to fall back to root's.
    #[test]
    fn a_missing_group_defaults_to_the_user_not_to_zero() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        assert_eq!(numeric_from_env(), Some(RunAs { uid: 501, gid: 501 }));
        clear();
    }

    /// Handing the tree to root would be handing it to nobody: the daemon that
    /// has to write there is not root any more.
    #[test]
    fn root_is_never_the_answer() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "0");
        assert_eq!(numeric_from_env(), None);
        clear();
        std::env::set_var("GROVE_RUN_USER", "root");
        std::env::set_var("SUDO_USER", "root");
        assert_eq!(named_from_env(), None);
        clear();
    }

    /// Unprivileged, whatever this process creates already belongs to the
    /// right person, so there is nothing to hand over.
    #[test]
    fn an_unprivileged_process_has_nobody_to_hand_files_to() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        if running_as_root() {
            eprintln!("skipped: runs as root, where there is someone to hand to");
            clear();
            return;
        }
        assert_eq!(run_user(), None);
        clear();
    }

    /// A `None` target must be a quiet no-op, not a chown of something.
    #[test]
    fn handing_a_path_to_nobody_does_nothing() {
        let dir = std::env::temp_dir().join(format!("grove-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        own_path(&dir, None);
        own_tree(&dir, None);
        assert!(dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

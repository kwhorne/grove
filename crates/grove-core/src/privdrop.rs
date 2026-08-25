//! Running child processes as the user instead of as root.
//!
//! The daemon runs as root because it binds 80/443/53, and it spawns a lot of
//! things: PHP-FPM pools, PostgreSQL, MySQL, Redis, Vite, queue workers. Almost
//! all of those binaries live under `$GROVE_HOME`, which the service installer
//! points at the *user's* home directory — so their paths, and the
//! `php-builds.json` that names them, are writable by an unprivileged user.
//!
//! That combination is a local privilege escalation: rewrite one JSON file and
//! root execs whatever you name. The durable answer is not to guard the
//! directory but to stop being root when we exec out of it. A child that runs as
//! the user gains nothing from a tampered binary — that user could already run
//! code as themselves.
//!
//! This module is the one implementation of that. It replaces three near-copies
//! (`grove-runtime`'s `target_user`, `grove-services`' `drop_ids`, and
//! `grove-daemon`'s `run_user`/`drop_ids`), each of which resolved a *username*
//! and then shelled out to `/usr/bin/id` to turn it into numbers.
//!
//! ## Two things the old copies got wrong
//!
//! - **`setgroups` was called and its result thrown away.** If it fails the
//!   child keeps root's supplementary groups — `wheel` among them — after
//!   dropping uid and gid, which is most of what dropping was for.
//! - **Nothing checked that the drop took.** [`apply`] now verifies the
//!   effective ids after the calls and fails the exec if they are not what was
//!   asked for. A child that was supposed to drop and silently did not is worse
//!   than one that refuses to start.

use std::process::Command;

/// The unprivileged identity to run children as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAs {
    pub uid: u32,
    pub gid: u32,
}

/// Whether this process has effective uid 0.
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

/// The identity children should drop to, or `None` when there is nothing to do.
///
/// `None` means "spawn as-is": either we are not root (a rootless dev daemon
/// already runs as the user) or we cannot tell who to become. Never guesses —
/// dropping to the wrong user would break every service instead of securing it.
#[cfg(unix)]
pub fn target() -> Option<RunAs> {
    if !running_as_root() {
        return None;
    }
    numeric_from_env().or_else(named_from_env)
}

#[cfg(not(unix))]
pub fn target() -> Option<RunAs> {
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
    // Dropping "to root" is not dropping. Treat it as no answer rather than
    // silently spawning with full privileges.
    if uid == 0 {
        return None;
    }
    let gid: u32 = std::env::var("GROVE_RUN_GROUP_ID")
        .ok()
        .and_then(|g| g.trim().parse().ok())
        .unwrap_or(uid);
    Some(RunAs { uid, gid })
}

/// `GROVE_RUN_USER` / `SUDO_USER` resolved through `id`, for service units
/// written before the numeric ids existed.
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

/// Make `cmd` drop to `run_as` before `exec`. A `None` target leaves `cmd` alone.
///
/// Order matters and is not interchangeable: supplementary groups first, then
/// gid, then uid. Once `setuid` has dropped root there is no privilege left to
/// change groups with, so a `setgroups` after it would silently do nothing.
#[cfg(unix)]
pub fn apply(cmd: &mut Command, run_as: Option<RunAs>) {
    use std::os::unix::process::CommandExt;
    let Some(RunAs { uid, gid }) = run_as else {
        return;
    };
    // SAFETY: the closure runs between fork and exec, where only
    // async-signal-safe work is allowed. It calls four libc id functions and
    // allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            // Shed root's supplementary groups. Unchecked before, which left a
            // child holding `wheel` after its uid and gid had been dropped.
            if libc::setgroups(1, &gid as *const libc::gid_t) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Belt and braces: refuse to exec if we are somehow still
            // privileged. Failing to start is recoverable; running the wrong
            // binary as root is the thing this exists to prevent.
            if libc::geteuid() != uid || libc::getegid() != gid {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "privilege drop did not take effect",
                ));
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn apply(_cmd: &mut Command, _run_as: Option<RunAs>) {}

/// Give `path` to `run_as` so a dropped child can write inside it.
///
/// Needed because the root daemon creates these directories: a socket dir, a
/// data dir, a build tree. Without this the child drops privileges and then
/// cannot write where it was told to, which looks like a service that will not
/// start rather than a permissions problem.
///
/// Best-effort: a failure here surfaces as the child's own error, which is more
/// specific than anything this could report.
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
    // syscall per entry on data directories that can hold thousands of files,
    // and it does not follow symlinks out of the tree by default.
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

    #[test]
    fn numeric_ids_are_preferred_and_need_no_subprocess() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        std::env::set_var("GROVE_RUN_GROUP_ID", "20");
        assert_eq!(numeric_from_env(), Some(RunAs { uid: 501, gid: 20 }));
        clear();
    }

    #[test]
    fn a_missing_group_falls_back_to_the_uid_not_to_zero() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        // gid 0 is `wheel`; defaulting there would hand the child a privileged
        // group while pretending to drop.
        assert_eq!(numeric_from_env(), Some(RunAs { uid: 501, gid: 501 }));
        clear();
    }

    #[test]
    fn dropping_to_root_is_not_an_answer() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "0");
        assert_eq!(numeric_from_env(), None);
        clear();
    }

    #[test]
    fn junk_is_refused_rather_than_guessed() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "not-a-number");
        assert_eq!(numeric_from_env(), None);
        clear();
    }

    #[test]
    fn a_named_run_user_still_resolves() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        // Whatever user is running the tests: `id -u` must agree with `geteuid`.
        let me = String::from_utf8_lossy(&Command::new("id").arg("-un").output().unwrap().stdout)
            .trim()
            .to_string();
        if me == "root" {
            clear();
            return; // the "not root" filter is covered by the next test
        }
        std::env::set_var("GROVE_RUN_USER", &me);
        let resolved = named_from_env().expect("named fallback should resolve");
        assert_eq!(resolved.uid, unsafe { libc::geteuid() });
        clear();
    }

    #[test]
    fn a_run_user_of_root_is_ignored() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER", "root");
        assert_eq!(named_from_env(), None);
        clear();
    }

    /// A non-root daemon has nothing to drop, whatever the environment says.
    #[test]
    fn a_non_root_process_has_no_target() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        std::env::set_var("GROVE_RUN_USER_ID", "501");
        if !running_as_root() {
            assert_eq!(target(), None, "must not try to drop when not root");
        }
        clear();
    }

    /// `apply` with no target must leave the command able to run. Verified by
    /// actually running it, since a stray `pre_exec` would break the spawn.
    #[test]
    fn apply_without_a_target_still_spawns() {
        let mut cmd = Command::new("true");
        apply(&mut cmd, None);
        assert!(cmd.status().unwrap().success());
    }

    /// A drop that cannot be performed must fail the spawn rather than exec the
    /// child anyway.
    ///
    /// From a non-root process *every* target fails, including our own ids:
    /// `setgroups` is privileged even when it would change nothing. That is why
    /// [`target`] returns `None` unless we are root — and why failing closed
    /// here is the only safe direction. The successful path is exercised as root
    /// in `tests/privdrop_root.rs`, which is ignored unless it finds itself
    /// privileged.
    #[test]
    fn a_drop_we_cannot_perform_fails_the_spawn() {
        if running_as_root() {
            return; // root can do all of this; the failure path needs a non-root run
        }
        for target in [
            RunAs { uid: 1, gid: 1 },
            RunAs {
                uid: unsafe { libc::geteuid() },
                gid: unsafe { libc::getegid() },
            },
        ] {
            let mut cmd = Command::new("true");
            apply(&mut cmd, Some(target));
            assert!(
                cmd.status().is_err(),
                "{target:?}: an unperformable drop must fail the spawn, not run anyway"
            );
        }
    }
}

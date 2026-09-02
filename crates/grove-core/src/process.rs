//! Asking the OS about processes Grove did not spawn in this lifetime.
//!
//! The daemon writes pid files for itself and for php-fpm, and after a SIGKILL
//! those files outlive the processes — or worse, the processes outlive the
//! daemon. Two questions come up on the next boot: is this pid still a live
//! process, and is it still *ours*? A pid alone answers neither: pids are
//! recycled, and a stale `groved.pid` can name whatever the kernel handed that
//! number to next. Sending SIGTERM to it is how a daemon kills someone's editor.
//!
//! So every decision here needs both the pid and the command name.

use std::path::Path;

/// Whether `pid` names a live process. `kill(pid, 0)` delivers nothing and
/// fails with ESRCH when there is no such process; EPERM means it exists but
/// is not ours to signal, which for this question still counts as alive.
///
/// A zombie counts too: an exited child nobody has `wait()`ed for is still in
/// the process table. That is the right answer for a pid read from a file — it
/// belongs to something the daemon did *not* spawn in this lifetime, so init
/// reaps it — but for the daemon's own children use `Child::try_wait`.
#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill with signal 0 has no effect beyond the permission/existence
    // check; any pid value is acceptable input.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub fn is_alive(_pid: u32) -> bool {
    false
}

/// The executable name of `pid` (`php-fpm`, `postgres`, `groved`…), without
/// path or arguments. `None` if the process is gone or the OS will not say.
///
/// One caveat: for a *freshly spawned* child the name is the parent's until
/// the child has `exec`ed, and on Linux `spawn()` can return before that. The
/// daemon only asks about pids read from files of long-running processes, so
/// it never sees that window; anything spawning and then asking must wait.
#[cfg(target_os = "linux")]
pub fn command_name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn command_name(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if comm.is_empty() {
        return None;
    }
    // `ps` prints the full path on macOS; callers compare on the basename.
    Some(
        Path::new(&comm)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or(comm),
    )
}

#[cfg(not(unix))]
pub fn command_name(_pid: u32) -> Option<String> {
    None
}

/// Whether `pid` is alive *and* running something whose name contains
/// `expected` — the test every reaper and `grove stop` must pass before
/// signalling a pid it read from a file.
pub fn is_alive_and_named(pid: u32, expected: &str) -> bool {
    is_alive(pid)
        && command_name(pid)
            .map(|name| name.contains(expected))
            .unwrap_or(false)
}

/// Read a pid file. `None` for a missing, empty or unparsable file — all of
/// which mean "nothing to act on", not an error.
pub fn read_pid_file(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|&p| p > 0)
}

/// Send SIGTERM, wait up to `grace` for the process to go, then SIGKILL.
/// Returns whether the process is gone afterwards.
#[cfg(unix)]
pub fn terminate(pid: u32, grace: std::time::Duration) -> bool {
    // SAFETY: signalling an arbitrary pid is what this function is for; the
    // caller has established that it is ours via `is_alive_and_named`.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    !is_alive(pid)
}

#[cfg(not(unix))]
pub fn terminate(_pid: u32, _grace: std::time::Duration) -> bool {
    false
}

/// Reap orphans recorded in pid files: for every `<prefix>*.pid` under `dir`,
/// terminate the process if it is alive **and** its command name contains
/// `expected` — then remove the pid file either way. Returns the pids that were
/// actually terminated.
///
/// This is what a daemon boot runs before spawning anything. After a SIGKILL
/// the previous daemon's php-fpm masters and databases are still running,
/// still holding their sockets and ports; the new daemon used to unlink the
/// sockets and spawn duplicates on top, or fail the bind and report the
/// service as "not running" while the orphan served. The name check is what
/// makes it safe to act on a pid read from disk at all.
pub fn reap_pid_files(
    dir: &Path,
    prefix: &str,
    expected: &str,
    grace: std::time::Duration,
) -> Vec<u32> {
    let mut reaped = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return reaped;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) || !name.ends_with(".pid") {
            continue;
        }
        if let Some(pid) = read_pid_file(&path) {
            if pid != std::process::id() && is_alive_and_named(pid, expected) {
                tracing::warn!(pid, file = %path.display(), "terminating orphaned process from a previous run");
                if terminate(pid, grace) {
                    reaped.push(pid);
                }
            }
        }
        let _ = std::fs::remove_file(&path);
    }
    reaped
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sleeper() -> std::process::Child {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep is everywhere");
        wait_for_name(child.id(), "sleep");
        child
    }

    /// Block until `pid` reports `expected` as its command name.
    ///
    /// Right after `spawn()` the child may not have `exec`ed yet, so
    /// `/proc/<pid>/comm` still shows the parent — this test thread's name,
    /// truncated to 15 bytes: `process::tests:`. That is exactly what CI saw
    /// on Ubuntu while Alpine and macOS, with different spawn paths, passed.
    fn wait_for_name(pid: u32, expected: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if command_name(pid).as_deref() == Some(expected) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "pid {pid} never became {expected:?} (last seen {:?})",
            command_name(pid)
        );
    }

    /// A sleeper nobody in this process tree will wait for, as an orphaned
    /// php-fpm after a daemon SIGKILL would be. Waits for it to exec.
    fn orphan_sleeper() -> u32 {
        let out = std::process::Command::new("sh")
            .args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!"])
            .output()
            .expect("sh");
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        wait_for_name(pid, "sleep");
        pid
    }

    #[test]
    fn a_live_process_is_alive_and_a_reaped_one_is_not() {
        let mut child = sleeper();
        let pid = child.id();
        assert!(is_alive(pid));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!is_alive(pid), "a reaped pid is gone");
    }

    #[test]
    fn the_command_name_is_the_basename() {
        let mut child = sleeper();
        assert_eq!(command_name(child.id()).as_deref(), Some("sleep"));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// The point of the module: a pid that is alive but not ours must not pass.
    /// Our own test process is alive and is not called "php-fpm".
    #[test]
    fn a_recycled_pid_running_something_else_is_not_ours() {
        let me = std::process::id();
        assert!(is_alive(me));
        assert!(!is_alive_and_named(me, "php-fpm"));
        let mut child = sleeper();
        assert!(is_alive_and_named(child.id(), "sleep"));
        assert!(!is_alive_and_named(child.id(), "php-fpm"));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn pid_files_that_mean_nothing_read_as_none() {
        let dir = std::env::temp_dir().join(format!("grove-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_pid_file(&dir.join("missing.pid")), None);
        std::fs::write(dir.join("empty.pid"), "").unwrap();
        assert_eq!(read_pid_file(&dir.join("empty.pid")), None);
        std::fs::write(dir.join("junk.pid"), "not a pid\n").unwrap();
        assert_eq!(read_pid_file(&dir.join("junk.pid")), None);
        std::fs::write(dir.join("zero.pid"), "0\n").unwrap();
        assert_eq!(
            read_pid_file(&dir.join("zero.pid")),
            None,
            "pid 0 is never a target"
        );
        std::fs::write(dir.join("ok.pid"), " 4242 \n").unwrap();
        assert_eq!(read_pid_file(&dir.join("ok.pid")), Some(4242));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The situation the reaper is for: a process nobody in this process tree
    /// will wait for. A direct child would stay a zombie after SIGTERM until
    /// we reaped it, and `is_alive` would rightly keep saying yes — so the
    /// sleeper is started through a shell that exits at once, leaving it to
    /// init, exactly like an orphaned php-fpm after a daemon SIGKILL.
    #[test]
    fn terminate_takes_an_orphan_down() {
        let pid = orphan_sleeper();
        assert!(is_alive(pid), "the orphan is running");
        assert!(terminate(pid, Duration::from_millis(500)));
        assert!(!is_alive(pid), "init reaped it");
    }

    /// Two pid files: one names a real orphan with the expected name, one names
    /// a live process that is *not* ours (this test process). Only the first
    /// may be killed; both files are cleaned up.
    #[test]
    fn reaping_kills_only_processes_with_the_expected_name() {
        let dir = std::env::temp_dir().join(format!("grove-reap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let orphan = orphan_sleeper();
        std::fs::write(dir.join("svc-a.pid"), orphan.to_string()).unwrap();
        // A recycled pid: alive, but running something else entirely.
        std::fs::write(dir.join("svc-b.pid"), std::process::id().to_string()).unwrap();
        // Not ours to touch: wrong prefix.
        std::fs::write(dir.join("other.pid"), orphan.to_string()).unwrap();

        let reaped = reap_pid_files(&dir, "svc-", "sleep", Duration::from_millis(500));
        assert_eq!(reaped, vec![orphan]);
        assert!(!is_alive(orphan), "the orphan is gone");
        assert!(
            is_alive(std::process::id()),
            "we are, unsurprisingly, still here"
        );
        assert!(!dir.join("svc-a.pid").exists() && !dir.join("svc-b.pid").exists());
        assert!(
            dir.join("other.pid").exists(),
            "files outside the prefix are untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

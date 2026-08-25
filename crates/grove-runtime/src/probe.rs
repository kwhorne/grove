//! Asking a Grove-managed runtime binary about itself, without being root.
//!
//! Grove interrogates the runtimes it manages in several places: `php-fpm
//! --version` to check a fresh download actually runs, again to identify a
//! discovered build, `php -m` for the extension audit, `php -i` to find an
//! `extension_dir`. All of them exec a binary out of `$GROVE_HOME` — which the
//! service installer points at the *user's* home directory — and all of them can
//! run inside the root daemon.
//!
//! That is the same shape as the spawns that [`grove_core::privdrop`] was written
//! for, and it is the corner it did not reach. The service and pool spawns drop
//! privileges; these one-shot probes did not, so a tampered tree or a compromised
//! download mirror still had a root exec to aim at — during `grove php install`,
//! at daemon startup via `discover()`, and on every `grove php ext` or
//! `grove doctor`.
//!
//! There is nothing a probe needs root for. It reads a version string.

use std::path::Path;
use std::process::Command;

use grove_core::privdrop;

/// Run `bin` with `args` as the unprivileged run user and capture its output.
///
/// `None` when the binary could not be run at all — missing, not executable, or
/// a privilege drop that could not be performed. Callers treat that as "this
/// build cannot tell us anything", which is the same conclusion they already
/// drew from a failed spawn, so failing closed here costs nothing.
pub fn output(bin: &Path, args: &[&str]) -> Option<std::process::Output> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    privdrop::apply(&mut cmd, privdrop::target());
    cmd.output().ok()
}

/// [`output`], decoded as the first line of stdout — the shape every
/// `--version` caller wants.
pub fn first_stdout_line(bin: &Path, args: &[&str]) -> Option<String> {
    let out = output(bin, args)?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_binary_is_none_not_a_panic() {
        assert!(output(Path::new("/definitely/not/here"), &["--version"]).is_none());
        assert!(first_stdout_line(Path::new("/definitely/not/here"), &["-m"]).is_none());
    }

    /// Unprivileged, `privdrop::target()` is `None`, so a probe still runs
    /// normally — the audit and `grove php install` must keep working for a
    /// rootless daemon and for a developer running the CLI directly.
    #[test]
    fn a_probe_still_runs_when_there_is_nothing_to_drop() {
        if privdrop::running_as_root() {
            return; // covered by tests/probe_root.rs
        }
        let out = output(Path::new("/bin/echo"), &["hello"]).expect("echo should run");
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hello",
            "stdout must still be captured"
        );
    }

    #[test]
    fn first_line_takes_only_the_first_line() {
        if privdrop::running_as_root() {
            return;
        }
        let line = first_stdout_line(Path::new("/bin/sh"), &["-c", "printf 'one\\ntwo\\n'"])
            .expect("sh should run");
        assert_eq!(line, "one");
    }
}

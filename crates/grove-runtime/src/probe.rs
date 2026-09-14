//! Asking a Grove-managed runtime binary about itself.
//!
//! Grove interrogates the runtimes it manages in several places: `php-fpm
//! --version` to check a fresh download actually runs, again to identify a
//! discovered build, `php -m` for the extension audit, `php -i` to find an
//! `extension_dir`. All of them exec a binary out of `$GROVE_HOME`.
//!
//! That used to be a root exec, and this module existed to drop privileges
//! before it. Since 1.8.0 the daemon runs as the login user and refuses to
//! start otherwise, so a probe is one user process exec'ing another user's
//! binary — nothing to drop, and nothing to aim at.

use std::path::Path;
use std::process::Command;

/// Run `bin` with `args` and capture its output.
///
/// `None` when the binary could not be run at all — missing, or not
/// executable. Callers treat that as "this build cannot tell us anything",
/// which is the same conclusion they already drew from a failed spawn.
pub fn output(bin: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new(bin).args(args).output().ok()
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

    /// The audit and `grove php install` must keep working, for the daemon and
    /// for a developer running the CLI directly.
    #[test]
    fn a_probe_captures_what_the_binary_printed() {
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
        let line = first_stdout_line(Path::new("/bin/sh"), &["-c", "printf 'one\\ntwo\\n'"])
            .expect("sh should run");
        assert_eq!(line, "one");
    }
}

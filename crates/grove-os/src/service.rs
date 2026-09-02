//! OS service installation. Installs Grove's daemon so it starts at
//! login and restarts on crash. Each platform writes the appropriate unit and
//! (un)loads it.

use std::path::PathBuf;
use std::process::Command;

use crate::{OsError, Result};

/// Service label / identifier shared across platforms.
pub const SERVICE_LABEL: &str = "com.elyra.grove";

/// Where the launchd/systemd unit lives, per platform.
pub fn unit_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // A system LaunchDaemon (runs as root) so it can bind 53/80/443.
        Some(PathBuf::from(format!(
            "/Library/LaunchDaemons/{SERVICE_LABEL}.plist"
        )))
    }
    #[cfg(target_os = "linux")]
    {
        // A *system* unit. The first version wrote a `--user` unit, which
        // cannot bind 53/80/443 (no capabilities in the user manager) and
        // stops with the session. Root here mirrors the macOS LaunchDaemon:
        // the daemon binds the ports and drops every child to the run user.
        Some(PathBuf::from("/etc/systemd/system/grove.service"))
    }
    #[cfg(target_os = "windows")]
    {
        None
    }
}

#[cfg(target_os = "linux")]
// Only the macOS plist needs the home directory; the Linux unit path is static.
#[cfg(target_os = "macos")]
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Escape a value for inclusion in a plist `<string>`.
///
/// The template interpolates `$GROVE_HOME` and the run user's name straight into
/// XML. Both come from the environment — and `GROVE_HOME` is one a user controls,
/// including under `sudo -E`. A path containing `&` or `<` produced a plist that
/// launchd rejects; one containing `</string>` could close the element early and
/// inject keys of the attacker's choosing into a **root** LaunchDaemon.
#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Install (and load) the Grove daemon as an OS service.
///
/// `exe` is the path to the `grove` binary; the service runs `grove daemon`.
///
/// `run_uid` is the `(uid, gid)` Grove is being installed on behalf of, when the
/// caller can tell — under `sudo` that is `SUDO_UID`/`SUDO_GID`. It is recorded
/// in the unit so the daemon can authorize that user on its IPC socket without
/// having to resolve a username at runtime. See `grove-daemon`'s `ipc` module:
/// inferring it from `$GROVE_HOME`'s owner is not enough, because root creates
/// that directory on a fresh install.
pub fn install(
    exe: &std::path::Path,
    grove_home: &std::path::Path,
    run_user: Option<&str>,
    run_uid: Option<(u32, u32)>,
    tld: &str,
    dns_port: u16,
) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // The resolver on macOS is a file under /etc/resolver, written by
        // `install_resolver`; the unit does not need it.
        let _ = (tld, dns_port);
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "installing the system service needs root — run `sudo grove install`".into(),
            ));
        }
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no unit path".into()))?;
        let run_user_xml = run_user
            .map(|u| {
                format!(
                    "        <key>GROVE_RUN_USER</key><string>{}</string>\n",
                    xml_escape(u)
                )
            })
            .unwrap_or_default();
        // Numeric ids so the daemon can authorize its IPC socket without
        // resolving a username. Rendered from `u32`, so no XML escaping is
        // needed here even though the surrounding template does not escape.
        let run_id_xml = run_uid
            .map(|(uid, gid)| {
                format!(
                    "        <key>GROVE_RUN_USER_ID</key><string>{uid}</string>\n\
                             <key>GROVE_RUN_GROUP_ID</key><string>{gid}</string>\n"
                )
            })
            .unwrap_or_default();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>GROVE_HOME</key><string>{home}</string>
{run_user_xml}{run_id_xml}    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>{home}/daemon.out.log</string>
    <key>StandardErrorPath</key><string>{home}/daemon.err.log</string>
</dict>
</plist>
"#,
            label = SERVICE_LABEL,
            exe = xml_escape(&exe.display().to_string()),
            home = xml_escape(&grove_home.display().to_string()),
            run_user_xml = run_user_xml,
            run_id_xml = run_id_xml,
        );
        std::fs::write(&path, plist)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        }
        // Reload cleanly (bootout may fail if not loaded — ignore).
        let _ = run("launchctl", &["bootout", "system", &path.to_string_lossy()]);
        run(
            "launchctl",
            &["bootstrap", "system", &path.to_string_lossy()],
        )?;
        Ok(path)
    }
    #[cfg(target_os = "linux")]
    {
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "installing the system service needs root — run `sudo grove install`".into(),
            ));
        }
        let path = unit_path().expect("linux unit path is static");
        let unit = linux_unit(exe, grove_home, run_user, run_uid, tld, dns_port);
        std::fs::write(&path, unit)?;
        run("systemctl", &["daemon-reload"])?;
        run("systemctl", &["enable", "--now", "grove.service"])?;
        Ok(path)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (exe, grove_home, run_user, run_uid, tld, dns_port);
        Err(OsError::Unsupported(
            "Windows service install not yet implemented".into(),
        ))
    }
}

/// The systemd unit for Grove's daemon.
///
/// Root, so it can bind 53/80/443 — every child is dropped to the run user,
/// recorded here numerically for the daemon's IPC authorization and privilege
/// drop. `ExecStartPre=+` (the `+` runs it as root even if `User=` were set)
/// recreates the systemd-resolved dummy link and its routing on every start,
/// because neither survives a reboot on its own.
pub fn linux_unit(
    exe: &std::path::Path,
    grove_home: &std::path::Path,
    run_user: Option<&str>,
    run_uid: Option<(u32, u32)>,
    tld: &str,
    dns_port: u16,
) -> String {
    let run_env = match (run_user, run_uid) {
        (Some(user), Some((uid, gid))) => format!(
            "Environment=GROVE_RUN_USER={user}\nEnvironment=GROVE_RUN_USER_ID={uid}\nEnvironment=GROVE_RUN_GROUP_ID={gid}\n"
        ),
        (Some(user), None) => format!("Environment=GROVE_RUN_USER={user}\n"),
        (None, Some((uid, gid))) => {
            format!("Environment=GROVE_RUN_USER_ID={uid}\nEnvironment=GROVE_RUN_GROUP_ID={gid}\n")
        }
        (None, None) => String::new(),
    };
    format!(
        "[Unit]\nDescription=Elyra Grove daemon\nAfter=network-online.target systemd-resolved.service\nWants=network-online.target\n\n\
         [Service]\nExecStartPre=+{pre}\nExecStart={exe} daemon\nEnvironment=GROVE_HOME={home}\n{run_env}Restart=on-failure\nRestartSec=2\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        pre = crate::linux::resolver_exec_start_pre(tld, dns_port),
        exe = exe.display(),
        home = grove_home.display(),
    )
}

/// Uninstall (and unload) the service.
/// Remove the OS service. Returns whether a unit was there to remove.
///
/// Every step here used to be `let _ =` and the function always returned
/// `Ok(())`, so `grove uninstall` without sudo printed "removed" having
/// removed nothing — `launchctl bootout system` and deleting from
/// `/Library/LaunchDaemons` both need root. Now it refuses without elevation
/// where that is required, tolerates only the failures that mean "already
/// gone", and propagates the rest.
pub fn uninstall() -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "removing the system service needs root — run `sudo grove uninstall`".into(),
            ));
        }
        let Some(path) = unit_path() else {
            return Ok(false);
        };
        if !path.exists() {
            return Ok(false);
        }
        // `bootout` fails when the job is not loaded, which is fine: the unit
        // file is what we are here to remove. Anything else is reported.
        if let Err(e) = run("launchctl", &["bootout", "system", &path.to_string_lossy()]) {
            tracing::warn!(error = %e, "launchctl bootout (service may not have been loaded)");
        }
        std::fs::remove_file(&path)?;
        Ok(true)
    }
    #[cfg(target_os = "linux")]
    {
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "removing the system service needs root — run `sudo grove uninstall`".into(),
            ));
        }
        let Some(path) = unit_path() else {
            return Ok(false);
        };
        let existed = path.exists();
        if existed {
            if let Err(e) = run("systemctl", &["disable", "--now", "grove.service"]) {
                tracing::warn!(error = %e, "systemctl disable (service may not have been enabled)");
            }
            std::fs::remove_file(&path)?;
            let _ = run("systemctl", &["daemon-reload"]);
        }
        Ok(existed)
    }
    #[cfg(target_os = "windows")]
    {
        Err(OsError::Unsupported(
            "Windows service uninstall not yet implemented".into(),
        ))
    }
}

#[allow(dead_code)]
fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd).args(args).status()?;
    if !status.success() {
        return Err(OsError::Command {
            cmd: format!("{cmd} {}", args.join(" ")),
            detail: format!("exit status {status}"),
        });
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn ordinary_paths_pass_through_unchanged() {
        for value in [
            "/Users/kh/Library/Application Support/Grove",
            "/home/kh/.local/share/Grove",
            "kh",
        ] {
            assert_eq!(xml_escape(value), value);
        }
    }

    /// The one that matters: a value cannot close the element it sits in and
    /// start writing keys of its own into a root LaunchDaemon.
    #[test]
    fn a_value_cannot_break_out_of_its_element() {
        let hostile = "/tmp/x</string><key>ProgramArguments</key><array><string>/bin/sh";
        let escaped = xml_escape(hostile);
        assert!(!escaped.contains("</string>"), "{escaped}");
        assert!(!escaped.contains('<'), "{escaped}");
        assert!(!escaped.contains('>'), "{escaped}");
    }

    #[test]
    fn the_xml_metacharacters_are_all_covered() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("a<b"), "a&lt;b");
        assert_eq!(xml_escape("a>b"), "a&gt;b");
        assert_eq!(xml_escape("a\"b"), "a&quot;b");
        assert_eq!(xml_escape("a'b"), "a&apos;b");
        // `&` must be escaped once, not twice.
        assert_eq!(xml_escape("&amp;"), "&amp;amp;");
    }
}

#[cfg(test)]
mod linux_unit_tests {
    use super::*;

    #[test]
    fn the_unit_binds_as_root_records_the_run_user_and_recreates_the_resolver_link() {
        let unit = linux_unit(
            std::path::Path::new("/usr/local/bin/grove"),
            std::path::Path::new("/home/u/.local/share/grove"),
            Some("u"),
            Some((1000, 1000)),
            "test",
            53,
        );
        assert!(
            unit.contains("WantedBy=multi-user.target"),
            "a system unit, not a user one: {unit}"
        );
        assert!(
            !unit.contains("User="),
            "root, so it can bind 53/80/443; children are dropped"
        );
        assert!(unit.contains("ExecStartPre=+/bin/sh -c 'ip link add grove0 type dummy 2>/dev/null || true; ip link set grove0 up; resolvectl dns grove0 127.0.0.1:53; resolvectl domain grove0 ~test'"), "{unit}");
        assert!(unit.contains("ExecStart=/usr/local/bin/grove daemon"));
        assert!(unit.contains("Environment=GROVE_HOME=/home/u/.local/share/grove"));
        assert!(unit.contains("Environment=GROVE_RUN_USER=u\n"));
        assert!(unit.contains("Environment=GROVE_RUN_USER_ID=1000\n"));
        assert!(unit.contains("Environment=GROVE_RUN_GROUP_ID=1000\n"));
        assert!(unit.contains("After=network-online.target systemd-resolved.service"));
    }
}

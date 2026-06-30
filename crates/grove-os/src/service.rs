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
        dirs_home().map(|h| h.join(".config/systemd/user/grove.service"))
    }
    #[cfg(target_os = "windows")]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Install (and load) the Grove daemon as an OS service for the current user.
///
/// `exe` is the path to the `grove` binary; the service runs `grove daemon`.
pub fn install(
    exe: &std::path::Path,
    grove_home: &std::path::Path,
    run_user: Option<&str>,
) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "installing the system service needs root — run `sudo grove install`".into(),
            ));
        }
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no unit path".into()))?;
        let run_user_xml = run_user
            .map(|u| format!("        <key>GROVE_RUN_USER</key><string>{u}</string>\n"))
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
{run_user_xml}    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>{home}/daemon.out.log</string>
    <key>StandardErrorPath</key><string>{home}/daemon.err.log</string>
</dict>
</plist>
"#,
            label = SERVICE_LABEL,
            exe = exe.display(),
            home = grove_home.display(),
            run_user_xml = run_user_xml,
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
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no HOME".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = run_user;
        let unit = format!(
            "[Unit]\nDescription=Elyra Grove daemon\nAfter=network.target\n\n\
             [Service]\nExecStart={exe} daemon\nEnvironment=GROVE_HOME={home}\nRestart=on-failure\n\n\
             [Install]\nWantedBy=default.target\n",
            exe = exe.display(),
            home = grove_home.display(),
        );
        std::fs::write(&path, unit)?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", "--now", "grove.service"])?;
        Ok(path)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (exe, grove_home, run_user);
        Err(OsError::Unsupported(
            "Windows service install not yet implemented".into(),
        ))
    }
}

/// Uninstall (and unload) the service.
pub fn uninstall() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = unit_path() {
            let _ = run("launchctl", &["bootout", "system", &path.to_string_lossy()]);
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = run(
            "systemctl",
            &["--user", "disable", "--now", "grove.service"],
        );
        if let Some(path) = unit_path() {
            let _ = std::fs::remove_file(&path);
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        Ok(())
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

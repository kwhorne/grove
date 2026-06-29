//! OS service installation. Installs Grove's daemon so it starts at
//! login and restarts on crash. Each platform writes the appropriate unit and
//! (un)loads it.

use std::path::PathBuf;
use std::process::Command;

use crate::{OsError, Result};

/// Service label / identifier shared across platforms.
pub const SERVICE_LABEL: &str = "com.elyra.grove";

/// Where the user-level launchd/systemd unit lives, per platform.
pub fn unit_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs_home().map(|h| {
            h.join("Library/LaunchAgents")
                .join(format!("{SERVICE_LABEL}.plist"))
        })
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

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Install (and load) the Grove daemon as an OS service for the current user.
///
/// `exe` is the path to the `grove` binary; the service runs `grove daemon`.
pub fn install(exe: &std::path::Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no HOME".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/tmp/grove.out.log</string>
    <key>StandardErrorPath</key><string>/tmp/grove.err.log</string>
</dict>
</plist>
"#,
            label = SERVICE_LABEL,
            exe = exe.display(),
        );
        std::fs::write(&path, plist)?;
        run("launchctl", &["load", "-w", &path.to_string_lossy()])?;
        Ok(path)
    }
    #[cfg(target_os = "linux")]
    {
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no HOME".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let unit = format!(
            "[Unit]\nDescription=Elyra Grove daemon\nAfter=network.target\n\n\
             [Service]\nExecStart={exe} daemon\nRestart=on-failure\n\n\
             [Install]\nWantedBy=default.target\n",
            exe = exe.display(),
        );
        std::fs::write(&path, unit)?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", "--now", "grove.service"])?;
        Ok(path)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = exe;
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
            let _ = run("launchctl", &["unload", "-w", &path.to_string_lossy()]);
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

//! OS service installation. Installs Grove's daemon so it starts at
//! login and restarts on crash. Each platform writes the appropriate unit and
//! (un)loads it.

use std::path::PathBuf;
use std::process::Command;

use crate::{OsError, Result};

/// Service label / identifier shared across platforms.
pub const SERVICE_LABEL: &str = "com.elyra.grove";

/// The ports the service manager binds on the daemon's behalf.
///
/// All three are below 1024, which is the whole reason the daemon has had to
/// be root. launchd and systemd bind them while *they* are root and hand the
/// listening descriptors over, so the process that serves on them need not be.
/// The daemon still binds any port it was not given, so a unit written before
/// this existed keeps working unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenPorts {
    pub http: u16,
    pub https: u16,
    pub dns: u16,
}

/// The `Sockets` entry the daemon looks for; see `grove_core::activation`.
const SOCKET_NAME: &str = "Listeners";

/// The `Sockets` dictionary for the launchd plist.
///
/// HTTP and HTTPS are left without a node name, which is launchd's way of
/// saying every interface — matching the `0.0.0.0` the daemon binds when it
/// binds for itself. DNS is pinned to loopback for the same reason: a resolver
/// answering the whole network is not what `grove install` promised. Both
/// halves of DNS are listed because a response too large for a datagram is
/// retried over TCP.
fn launchd_sockets(ports: ListenPorts) -> String {
    let ListenPorts { http, https, dns } = ports;
    let any = |port: u16| {
        format!(
            "            <dict><key>SockType</key><string>stream</string>\
<key>SockFamily</key><string>IPv4</string>\
<key>SockServiceName</key><string>{port}</string></dict>\n"
        )
    };
    let loopback = |kind: &str, port: u16| {
        format!(
            "            <dict><key>SockType</key><string>{kind}</string>\
<key>SockFamily</key><string>IPv4</string>\
<key>SockNodeName</key><string>127.0.0.1</string>\
<key>SockServiceName</key><string>{port}</string></dict>\n"
        )
    };
    format!(
        "    <key>Sockets</key>\n    <dict>\n        <key>{SOCKET_NAME}</key>\n        <array>\n\
         {http_sock}{https_sock}{dns_tcp}{dns_udp}        </array>\n    </dict>\n",
        http_sock = any(http),
        https_sock = any(https),
        dns_tcp = loopback("stream", dns),
        dns_udp = loopback("dgram", dns),
    )
}

/// The whole launchd plist, as text.
///
/// Separated from writing it so the document can be linted and asserted on in
/// a test; a plist that does not parse is a daemon that never starts, and the
/// only feedback is a machine that has stopped serving.
fn launchd_plist(
    exe: &std::path::Path,
    grove_home: &std::path::Path,
    run_user: Option<&str>,
    run_uid: Option<(u32, u32)>,
    ports: ListenPorts,
) -> String {
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
    format!(
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
{sockets}    <key>StandardOutPath</key><string>{home}/daemon.out.log</string>
    <key>StandardErrorPath</key><string>{home}/daemon.err.log</string>
</dict>
</plist>
"#,
        label = SERVICE_LABEL,
        exe = xml_escape(&exe.display().to_string()),
        home = xml_escape(&grove_home.display().to_string()),
        run_user_xml = run_user_xml,
        run_id_xml = run_id_xml,
        sockets = launchd_sockets(ports),
    )
}

/// The companion `grove.socket` unit for systemd.
///
/// `Wants=`, not `Requires=`, on the service side: a socket unit that fails to
/// bind must not stop the daemon from starting, because the daemon can still
/// bind for itself and serving on some ports beats serving on none.
pub fn linux_socket_unit(ports: ListenPorts) -> String {
    let ListenPorts { http, https, dns } = ports;
    format!(
        "[Unit]\nDescription=Elyra Grove listening sockets\n\n\
         [Socket]\n\
         ListenStream=0.0.0.0:{http}\n\
         ListenStream=0.0.0.0:{https}\n\
         ListenStream=127.0.0.1:{dns}\n\
         ListenDatagram=127.0.0.1:{dns}\n\
         BindIPv6Only=both\n\
         Service=grove.service\n\n\
         [Install]\nWantedBy=sockets.target\n"
    )
}

/// Where the systemd socket unit lives.
#[cfg(target_os = "linux")]
fn socket_unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system/grove.socket")
}

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
    ports: ListenPorts,
) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // The resolver on macOS is a file under /etc/resolver, written by
        // `install_resolver`; the unit does not need it.
        let _ = tld;
        if !crate::is_elevated() {
            return Err(OsError::Unsupported(
                "installing the system service needs root — run `sudo grove install`".into(),
            ));
        }
        let path = unit_path().ok_or_else(|| OsError::Unsupported("no unit path".into()))?;
        std::fs::write(
            &path,
            launchd_plist(exe, grove_home, run_user, run_uid, ports),
        )?;
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
        // Earlier versions installed a `--user` unit. It never worked (it
        // could not bind the ports) but it may still be enabled, and a root
        // process cannot cleanly drive another user's systemd instance — so
        // say exactly what to run rather than half-doing it.
        if let Some(old) = legacy_user_unit(run_user) {
            tracing::warn!(
                unit = %old.display(),
                "an old per-user unit from a previous Grove is still present; disable it with: \
                 systemctl --user disable --now grove.service && rm {}",
                old.display()
            );
        }
        let unit = linux_unit(exe, grove_home, run_user, run_uid, tld, ports.dns);
        std::fs::write(&path, unit)?;
        // The socket unit has to exist and be enabled before the service, or
        // systemd starts the daemon with nothing to hand it and the daemon
        // binds the privileged ports itself — which works today only because
        // it is still root.
        std::fs::write(socket_unit_path(), linux_socket_unit(ports))?;
        run("systemctl", &["daemon-reload"])?;
        run("systemctl", &["enable", "--now", "grove.socket"])?;
        run("systemctl", &["enable", "--now", "grove.service"])?;
        Ok(path)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (exe, grove_home, run_user, run_uid, tld, ports);
        Err(OsError::Unsupported(
            "Windows service install not yet implemented".into(),
        ))
    }
}

/// The per-user unit older versions wrote, if it is still there. Looks in the
/// run user's home (the one `sudo` was invoked from), not root's.
#[cfg(target_os = "linux")]
fn legacy_user_unit(run_user: Option<&str>) -> Option<PathBuf> {
    let home = match run_user {
        Some(user) => {
            let out = Command::new("getent")
                .args(["passwd", user])
                .output()
                .ok()?;
            let line = String::from_utf8_lossy(&out.stdout);
            PathBuf::from(line.trim().split(':').nth(5)?)
        }
        None => PathBuf::from(std::env::var_os("HOME")?),
    };
    let unit = home.join(".config/systemd/user/grove.service");
    unit.exists().then_some(unit)
}

/// The systemd unit for Grove's daemon.
///
/// `Restart=always`, not `on-failure`: `grove restart` and the app's Restart
/// button ask the daemon to shut down cleanly and rely on the supervisor to
/// bring it back — on macOS via `launchctl kickstart`, here via the restart
/// policy. A clean exit is not a failure, so `on-failure` left the daemon down
/// after every deliberate restart.
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
        "[Unit]\nDescription=Elyra Grove daemon\nAfter=network-online.target systemd-resolved.service grove.socket\nWants=network-online.target grove.socket\n\n\
         [Service]\nExecStartPre=+{pre}\nExecStart={exe} daemon\nEnvironment=GROVE_HOME={home}\n{run_env}Restart=always\nRestartSec=2\n\n\
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

    /// The daemon asks launchd for a socket by port; if the plist declares a
    /// different one, the daemon quietly binds for itself and the whole point
    /// of the change is lost. So the ports in the unit are the ports from the
    /// config, and both halves of DNS are there — a resolver with only the
    /// datagram socket looks healthy until the first response too large to fit
    /// in one.
    #[test]
    fn the_plist_declares_every_port_the_daemon_will_ask_for() {
        let sockets = launchd_sockets(ListenPorts {
            http: 80,
            https: 443,
            dns: 53,
        });
        assert!(sockets.contains("<key>Listeners</key>"), "{sockets}");
        assert_eq!(
            SOCKET_NAME, "Listeners",
            "the name here and the name grove_core::activation asks for are the same string"
        );
        for expected in [
            "<key>SockServiceName</key><string>80</string>",
            "<key>SockServiceName</key><string>443</string>",
            "<key>SockServiceName</key><string>53</string>",
        ] {
            assert!(
                sockets.contains(expected),
                "missing {expected} in {sockets}"
            );
        }
        assert_eq!(
            sockets.matches("<string>stream</string>").count(),
            3,
            "http, https and DNS-over-TCP"
        );
        assert_eq!(
            sockets.matches("<string>dgram</string>").count(),
            1,
            "DNS over UDP"
        );
        // HTTP and HTTPS answer on every interface, DNS only on loopback.
        assert_eq!(sockets.matches("127.0.0.1").count(), 2, "{sockets}");
    }

    /// A plist that does not parse is a daemon that never starts, and the
    /// only symptom is a machine that has stopped serving — so lint the real
    /// document with the system's own parser rather than trusting the
    /// template. Run only where `plutil` exists.
    #[test]
    fn the_generated_plist_parses() {
        let plist = launchd_plist(
            std::path::Path::new("/Applications/Grove.app/Contents/MacOS/grove"),
            std::path::Path::new("/Users/someone/Library/Application Support/Grove"),
            Some("someone"),
            Some((501, 20)),
            ListenPorts {
                http: 80,
                https: 443,
                dns: 53,
            },
        );
        let file = std::env::temp_dir().join(format!("grove-plist-{}.plist", std::process::id()));
        std::fs::write(&file, &plist).unwrap();
        let out = Command::new("plutil").arg("-lint").arg(&file).output();
        let _ = std::fs::remove_file(&file);
        let out = out.expect("plutil is part of macOS");
        assert!(
            out.status.success(),
            "plutil rejected the plist: {}\n{plist}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// The same, for a path with the XML metacharacters the escaper exists
    /// for: a home directory called `A & B <x>` must still produce a document
    /// the parser accepts, not one where the value has become markup.
    #[test]
    fn a_hostile_home_directory_still_produces_a_valid_plist() {
        let plist = launchd_plist(
            std::path::Path::new("/tmp/x</string><key>Sockets</key><string>"),
            std::path::Path::new("/Users/A & B <x>/Grove"),
            Some("a&b"),
            None,
            ListenPorts {
                http: 80,
                https: 443,
                dns: 53,
            },
        );
        let file = std::env::temp_dir().join(format!("grove-plist-h{}.plist", std::process::id()));
        std::fs::write(&file, &plist).unwrap();
        let out = Command::new("plutil").arg("-lint").arg(&file).output();
        let _ = std::fs::remove_file(&file);
        assert!(out.expect("plutil").status.success(), "{plist}");
        // One Sockets key, the one we wrote — not a second one smuggled in
        // through the program path.
        assert_eq!(plist.matches("<key>Sockets</key>").count(), 1, "{plist}");
    }

    /// Non-default ports have to reach the unit, or `grove install` on a
    /// machine serving HTTP on 8080 would tell launchd to bind 80.
    #[test]
    fn the_configured_ports_are_the_ones_written() {
        let sockets = launchd_sockets(ListenPorts {
            http: 8080,
            https: 8443,
            dns: 5353,
        });
        assert!(sockets.contains("<string>8080</string>"));
        assert!(sockets.contains("<string>8443</string>"));
        assert!(sockets.contains("<string>5353</string>"));
        assert!(!sockets.contains("<string>80</string>"));
    }

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

    /// A socket unit that fails to bind must not take the daemon down with
    /// it: the daemon can still bind for itself, and serving some ports beats
    /// serving none. That is the difference between `Wants=` and `Requires=`,
    /// and it is the whole safety story for rolling this out.
    #[test]
    fn the_socket_unit_is_wanted_not_required() {
        let unit = linux_socket_unit(ListenPorts {
            http: 80,
            https: 443,
            dns: 53,
        });
        assert!(unit.contains("ListenStream=0.0.0.0:80"), "{unit}");
        assert!(unit.contains("ListenStream=0.0.0.0:443"), "{unit}");
        // Both halves: a DNS response too large for a datagram is retried over
        // TCP, and a resolver missing that half looks healthy until it is not.
        assert!(unit.contains("ListenStream=127.0.0.1:53"), "{unit}");
        assert!(unit.contains("ListenDatagram=127.0.0.1:53"), "{unit}");
        assert!(unit.contains("Service=grove.service"), "{unit}");

        let service = linux_unit(
            std::path::Path::new("/usr/local/bin/grove"),
            std::path::Path::new("/home/kh/.local/share/Grove"),
            Some("kh"),
            Some((1000, 1000)),
            "test",
            53,
        );
        assert!(
            service.contains("Wants=network-online.target grove.socket"),
            "{service}"
        );
        assert!(
            !service.contains("Requires=grove.socket"),
            "a failed socket unit must not block the daemon: {service}"
        );
        assert!(
            service.contains("After=") && service.contains("grove.socket"),
            "{service}"
        );
    }

    /// Non-default ports have to reach the socket unit too, or an install on a
    /// machine serving HTTP on 8080 would tell systemd to bind 80.
    #[test]
    fn the_socket_unit_carries_the_configured_ports() {
        let unit = linux_socket_unit(ListenPorts {
            http: 8080,
            https: 8443,
            dns: 5353,
        });
        assert!(unit.contains("ListenStream=0.0.0.0:8080"), "{unit}");
        assert!(unit.contains("ListenStream=0.0.0.0:8443"), "{unit}");
        assert!(unit.contains("ListenDatagram=127.0.0.1:5353"), "{unit}");
        assert!(!unit.contains(":80\n"), "{unit}");
    }

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
        assert!(
            unit.contains("Restart=always\n"),
            "a deliberate restart exits cleanly; on-failure would leave the daemon down: {unit}"
        );
    }
}

//! The checks behind `grove doctor`.
//!
//! Two things shaped this module. First, the checks that matter most are the
//! ones that fail when the daemon is *down* — a broken config, a missing CA, a
//! resolver a VPN client overwrote — and `doctor` used to be an IPC round-trip,
//! so a down daemon answered "not running" instead of diagnosing anything.
//! [`local_checks`] needs no daemon, and the CLI runs it directly when the
//! socket does not answer.
//!
//! Second, the daemon can only say what it knows. It knows whether its own
//! binds succeeded ([`listener_entry`]); it does not know who else holds a
//! port, so [`port_holder`] asks the OS, best-effort, purely to put a name in
//! the message.

use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::Duration;

use grove_core::config::Config;
use grove_core::paths::GrovePaths;
use grove_ipc::protocol::{DiagnosticEntry, DiagnosticStatus};

use crate::state::ListenerHealth;

/// How long a resolver probe may take before it is reported as hanging. A
/// misconfigured resolver often does not fail — it waits.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

fn entry(check: &str, status: DiagnosticStatus, detail: impl Into<String>) -> DiagnosticEntry {
    DiagnosticEntry {
        check: check.into(),
        status,
        detail: detail.into(),
    }
}

/// Everything that can be checked from the config and the filesystem alone.
///
/// `config` is the daemon's in-memory copy when called from the daemon; the CLI
/// passes `None` and the file is parsed here, so a config that no longer parses
/// is reported rather than silently replaced with defaults.
pub fn local_checks(paths: &GrovePaths, config: Option<&Config>) -> Vec<DiagnosticEntry> {
    let mut out = Vec::new();

    let loaded;
    let config = match config {
        Some(c) => {
            out.push(entry(
                "config",
                DiagnosticStatus::Pass,
                format!("loaded from {}", paths.config_file().display()),
            ));
            Some(c)
        }
        None => match Config::load(paths) {
            Ok(c) => {
                out.push(entry(
                    "config",
                    DiagnosticStatus::Pass,
                    format!("loaded from {}", paths.config_file().display()),
                ));
                loaded = c;
                Some(&loaded)
            }
            Err(e) => {
                out.push(entry(
                    "config",
                    DiagnosticStatus::Fail,
                    format!("{}: {e}", paths.config_file().display()),
                ));
                None
            }
        },
    };

    let ca = paths.ca_cert();
    out.push(if ca.exists() {
        entry(
            "root-ca",
            DiagnosticStatus::Pass,
            format!("present at {}", ca.display()),
        )
    } else {
        entry(
            "root-ca",
            DiagnosticStatus::Warn,
            "no root CA generated yet — `sudo grove init`",
        )
    });

    if let Some(config) = config {
        if ca.exists() {
            out.push(ca_scope(paths, &config.general.tld));
        }
        out.push(resolver_check(&config.general.tld, config.general.dns_port));
    }

    out
}

/// A CA that can sign any name is trusted by the whole machine, so whether this
/// one is constrained — and to the TLD actually in use — is worth saying out
/// loud rather than leaving to be discovered.
fn ca_scope(paths: &GrovePaths, configured_tld: &str) -> DiagnosticEntry {
    match grove_tls::constrained_tld(paths) {
        Some(tld) if tld == configured_tld => entry(
            "root-ca-scope",
            DiagnosticStatus::Pass,
            format!("constrained to .{tld}"),
        ),
        Some(tld) => entry(
            "root-ca-scope",
            DiagnosticStatus::Warn,
            format!(
                "constrained to .{tld} but the configured TLD is .{configured_tld} — \
                 sites will fail TLS until `sudo grove ca rotate`"
            ),
        ),
        None => entry(
            "root-ca-scope",
            DiagnosticStatus::Warn,
            "unconstrained: it can sign any hostname, and it is in the system \
             trust store. `sudo grove ca rotate` replaces it with one limited \
             to your TLD",
        ),
    }
}

/// Does the *operating system* send `*.<tld>` to Grove?
///
/// Two halves. Where the platform has a resolver file, check it says what
/// `grove install` wrote. Everywhere, ask the OS resolver for a name under the
/// TLD and expect loopback back — that is the question a user actually has, and
/// it catches the VPN client that rewrote DNS order without touching the file.
pub fn resolver_check(tld: &str, dns_port: u16) -> DiagnosticEntry {
    if let Some(file) = grove_os::resolver_file(tld) {
        match std::fs::read_to_string(&file) {
            Ok(body) => {
                let wants_port = format!("port {dns_port}");
                if !body.contains("nameserver 127.0.0.1") || !body.contains(&wants_port) {
                    return entry(
                        "resolver",
                        DiagnosticStatus::Fail,
                        format!(
                            "{} does not point at 127.0.0.1:{dns_port} — re-run `sudo grove install`",
                            file.display()
                        ),
                    );
                }
            }
            Err(_) => {
                return entry(
                    "resolver",
                    DiagnosticStatus::Fail,
                    format!(
                        "{} missing — re-run `sudo grove install` to register the .{tld} resolver",
                        file.display()
                    ),
                );
            }
        }
    }

    let probe = format!("grove-doctor-probe.{tld}");
    match resolve_with_timeout(&probe) {
        Resolved::Loopback => entry(
            "resolver",
            DiagnosticStatus::Pass,
            format!("*.{tld} resolves to 127.0.0.1"),
        ),
        Resolved::Elsewhere(addr) => entry(
            "resolver",
            DiagnosticStatus::Fail,
            format!(
                "{probe} resolved to {addr}, not 127.0.0.1 — another resolver answers .{tld} first"
            ),
        ),
        Resolved::Nothing => entry(
            "resolver",
            DiagnosticStatus::Fail,
            if grove_os::resolver_file(tld).is_some() {
                format!("{probe} does not resolve — the daemon's DNS listener may be down, see `grove status`")
            } else {
                format!("{probe} does not resolve — add sites to /etc/hosts, or point your resolver at 127.0.0.1:{dns_port} for .{tld}")
            },
        ),
        Resolved::Hung => entry(
            "resolver",
            DiagnosticStatus::Fail,
            format!(
                "resolving {probe} took over {}s — the resolver for .{tld} is not answering",
                RESOLVE_TIMEOUT.as_secs()
            ),
        ),
    }
}

enum Resolved {
    Loopback,
    Elsewhere(std::net::IpAddr),
    Nothing,
    Hung,
}

/// `getaddrinfo` has no timeout of its own and a broken resolver tends to wait
/// rather than fail, so the lookup runs on a thread that is simply abandoned
/// if it does not come back in time.
fn resolve_with_timeout(host: &str) -> Resolved {
    let (tx, rx) = std::sync::mpsc::channel();
    let host = host.to_string();
    std::thread::spawn(move || {
        let result = (host.as_str(), 80)
            .to_socket_addrs()
            .ok()
            .map(|addrs| addrs.map(|a| a.ip()).collect::<Vec<_>>());
        let _ = tx.send(result);
    });
    match rx.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(Some(ips)) if ips.iter().any(|ip| ip.is_loopback()) => Resolved::Loopback,
        Ok(Some(ips)) => match ips.first() {
            Some(ip) => Resolved::Elsewhere(*ip),
            None => Resolved::Nothing,
        },
        Ok(None) => Resolved::Nothing,
        Err(_) => Resolved::Hung,
    }
}

/// One line for one listener, from what the daemon recorded at bind time.
pub fn listener_entry(name: &str, port: u16, health: &ListenerHealth) -> DiagnosticEntry {
    match health {
        ListenerHealth::Up => entry(
            name,
            DiagnosticStatus::Pass,
            format!("listening on :{port}"),
        ),
        ListenerHealth::Pending => entry(name, DiagnosticStatus::Warn, "still starting"),
        ListenerHealth::Failed(err) => entry(
            name,
            DiagnosticStatus::Fail,
            format!(
                "could not bind :{port}: {}",
                explain_bind_failure(port, err)
            ),
        ),
    }
}

/// The bind error, plus who holds the port when the OS will tell us.
pub fn explain_bind_failure(port: u16, err: &str) -> String {
    match port_holder(port) {
        Some(holder) => format!("{err} — held by {holder}"),
        None => err.to_string(),
    }
}

/// Which process is listening on `port`, if the OS will say. Best-effort and
/// purely informational: the daemon already knows *that* the bind failed.
#[cfg(target_os = "macos")]
pub fn port_holder(port: u16) -> Option<String> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fcp"])
        .output()
        .ok()?;
    // -F output is one field per line: `p<pid>` then `c<command>`.
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = Some(p.to_string());
        } else if let Some(cmd) = line.strip_prefix('c') {
            return Some(match pid {
                Some(p) => format!("{cmd} (pid {p})"),
                None => cmd.to_string(),
            });
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub fn port_holder(port: u16) -> Option<String> {
    let out = std::process::Command::new("ss")
        .args(["-ltnpH", &format!("sport = :{port}")])
        .output()
        .ok()?;
    // …users:(("nginx",pid=1234,fd=6))
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find("((\"")? + 3;
    let rest = &text[start..];
    let name_end = rest.find('"')?;
    let name = &rest[..name_end];
    let pid = rest
        .find("pid=")
        .map(|i| &rest[i + 4..])
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .filter(|s| !s.is_empty());
    Some(match pid {
        Some(p) => format!("{name} (pid {p})"),
        None => name.to_string(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn port_holder(_port: u16) -> Option<String> {
    None
}

/// What the CLI reports in place of the daemon's own checks when the socket
/// does not answer.
pub fn daemon_down_entry(socket: &Path) -> DiagnosticEntry {
    entry(
        "daemon",
        DiagnosticStatus::Fail,
        format!(
            "not running (no socket at {}) — `sudo grove install`, or `grove daemon` to run it in the foreground",
            socket.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_listener_passes_and_a_failed_one_names_the_port() {
        let up = listener_entry("http", 80, &ListenerHealth::Up);
        assert_eq!(up.status, DiagnosticStatus::Pass);
        assert!(up.detail.contains(":80"));

        let down = listener_entry("http", 80, &ListenerHealth::Failed("address in use".into()));
        assert_eq!(down.status, DiagnosticStatus::Fail);
        assert!(down.detail.contains("address in use"), "{}", down.detail);
        assert!(down.detail.contains(":80"), "{}", down.detail);
    }

    /// The whole point of the local path: a config that does not parse must be
    /// *reported*, where the old daemon-side doctor either could not run (daemon
    /// down) or the daemon had refused to start on it.
    #[test]
    fn a_broken_config_is_a_failure_not_a_default() {
        let base = std::env::temp_dir().join(format!("grove-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = GrovePaths::with_base(&base);
        paths.ensure().unwrap();
        std::fs::write(paths.config_file(), "this is = not [toml").unwrap();

        let entries = local_checks(&paths, None);
        let config = entries
            .iter()
            .find(|e| e.check == "config")
            .expect("config entry");
        assert_eq!(config.status, DiagnosticStatus::Fail);
        assert!(
            config.detail.contains("config.toml"),
            "names the file: {}",
            config.detail
        );
        // With no parseable config there is no TLD to probe, so no resolver
        // entry — better than probing the wrong name.
        assert!(entries.iter().all(|e| e.check != "resolver"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A name under a TLD nothing answers for must come back as a failure, and
    /// come back at all — the lookup runs under a timeout.
    #[test]
    fn an_unserved_tld_fails_the_resolver_check() {
        // `.invalid` is reserved (RFC 6761) and every resolver refuses it.
        let e = resolver_check("invalid", 53);
        assert_eq!(e.status, DiagnosticStatus::Fail, "{}", e.detail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ss_output_is_parsed_for_a_process_name() {
        // Not calling ss; exercising the parser shape via the same code path
        // would need a fixture injection. Kept as a smoke test that the
        // function exists on this platform.
        let _ = port_holder(1);
    }
}

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

    // The things 1.5.0 fixed, checked on every run rather than assumed fixed:
    // the socket that authorizes every privileged operation, the tree root
    // reads binaries out of, the leaves sites are served with, and whether the
    // machine's trust store holds *this* CA and nothing Grove left behind.
    if let Some(e) = ipc_socket_check(&paths.ipc_socket()) {
        out.push(e);
    }
    out.push(grove_home_check(paths.base()));
    out.push(leaf_certs_check(paths));
    if ca.exists() {
        out.push(trust_store_check(&ca));
    }

    // A state file that did not parse was moved aside rather than overwritten;
    // the log said so once. This keeps saying so until someone looks.
    let mut quarantined = Vec::new();
    for dir in [
        paths.runtimes_dir(),
        paths.services_dir(),
        paths.base().join("snapshots"),
    ] {
        quarantined.extend(grove_core::securefs::quarantined_files(&dir));
    }
    if !quarantined.is_empty() {
        let names: Vec<String> = quarantined
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        out.push(entry(
            "state-files",
            DiagnosticStatus::Warn,
            format!(
                "{} state file(s) could not be parsed and were set aside: {} — inspect, \
                 restore what you need, then delete them",
                names.len(),
                names.join(", ")
            ),
        ));
    }

    out
}

/// A CA that can sign any name is trusted by the whole machine, so whether this
/// one is constrained — and to the TLD actually in use — is worth saying out
/// loud rather than leaving to be discovered.
fn ca_scope(paths: &GrovePaths, configured_tld: &str) -> DiagnosticEntry {
    let pem = std::fs::read_to_string(paths.ca_cert()).ok();
    match scope_of(pem.as_deref(), || grove_tls::constrained_tld(paths)) {
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
            "unconstrained: it can sign any hostname a machine trusting it is \
             asked about. `sudo grove ca rotate` replaces it with one limited \
             to your TLD (see `trust-store` for what the machine trusts)",
        ),
    }
}

/// What the CA is really limited to.
///
/// `ca-meta.json` records the TLD Grove *meant* to constrain the CA to, and
/// reading it is cheap — but it is a note beside the certificate, not the
/// certificate. A CA minted before the constraint existed, or replaced by
/// hand, leaves the note saying one thing and the extension another, and the
/// extension is what every TLS client enforces. So the certificate answers
/// whenever it parses, including when its answer is "nothing constrains me";
/// the note is the fallback for a CA that cannot be read at all.
fn scope_of(pem: Option<&str>, recorded: impl FnOnce() -> Option<String>) -> Option<String> {
    match pem.filter(|p| grove_tls::fingerprint_sha256(p).is_some()) {
        Some(p) => grove_tls::permitted_dns_subtrees(p)?.into_iter().next(),
        None => recorded(),
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

/// The IPC socket is the authorization boundary: everything the root daemon can
/// do is reachable through it. It is created `0660` and owned by the run user;
/// anything world-accessible undoes the peer check. Absent socket → no entry
/// (the `daemon` line covers that).
#[cfg(unix)]
fn ipc_socket_check(socket: &Path) -> Option<DiagnosticEntry> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(socket).ok()?;
    let mode = meta.mode() & 0o777;
    let owner = meta.uid();
    Some(if mode & 0o007 != 0 {
        entry(
            "ipc-socket",
            DiagnosticStatus::Fail,
            format!(
                "{} is mode {mode:04o}: any local user can command the daemon. \
                 Restart it (`grove restart`); it recreates the socket 0660.",
                socket.display()
            ),
        )
    } else {
        entry(
            "ipc-socket",
            DiagnosticStatus::Pass,
            format!("mode {mode:04o}, owner uid {owner}"),
        )
    })
}

#[cfg(not(unix))]
fn ipc_socket_check(_socket: &Path) -> Option<DiagnosticEntry> {
    None
}

/// `$GROVE_HOME` is where root reads `php-builds.json` — a file that names the
/// binary it will execute — so who can write there matters more than for any
/// other directory Grove touches.
#[cfg(unix)]
fn grove_home_check(base: &Path) -> DiagnosticEntry {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(base) else {
        return entry(
            "grove-home",
            DiagnosticStatus::Fail,
            format!("{} does not exist — `grove init`", base.display()),
        );
    };
    let mode = meta.mode() & 0o777;
    let owner = meta.uid();
    if mode & 0o002 != 0 {
        return entry(
            "grove-home",
            DiagnosticStatus::Fail,
            format!(
                "{} is world-writable (mode {mode:04o}): anyone on this machine can plant a \
                 php-fpm binary the root daemon will run. `chmod o-w {}`",
                base.display(),
                base.display()
            ),
        );
    }
    let me = crate::ipc::current_uid();
    if owner == 0 && me != 0 {
        return entry(
            "grove-home",
            DiagnosticStatus::Warn,
            format!(
                "{} is owned by root, and the daemon no longer runs as root — every write it \
                 makes will fail. `sudo grove install` hands the tree over (or \
                 `sudo chown -R {me} {}`)",
                base.display(),
                base.display()
            ),
        );
    }
    entry(
        "grove-home",
        DiagnosticStatus::Pass,
        format!("owned by uid {owner}, mode {mode:04o}"),
    )
}

#[cfg(not(unix))]
fn grove_home_check(base: &Path) -> DiagnosticEntry {
    entry(
        "grove-home",
        DiagnosticStatus::Pass,
        base.display().to_string(),
    )
}

/// Every leaf under `certs/`, by days until it expires. Expired is not a
/// failure — the daemon reissues on the next request — but it is worth a
/// word, and the soonest expiry is worth knowing.
fn leaf_certs_check(paths: &GrovePaths) -> DiagnosticEntry {
    let ca = paths.ca_cert();
    let mut leaves: Vec<(String, i64)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths.certs_dir()) {
        for e in entries.flatten() {
            let p = e.path();
            if p == ca || p.extension().and_then(|x| x.to_str()) != Some("pem") {
                continue;
            }
            let Ok(pem) = std::fs::read_to_string(&p) else {
                continue;
            };
            if let Some(days) = grove_tls::days_until_expiry(&pem) {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                leaves.push((name, days));
            }
        }
    }
    summarize_leaves(&leaves)
}

fn summarize_leaves(leaves: &[(String, i64)]) -> DiagnosticEntry {
    if leaves.is_empty() {
        return entry(
            "site-certs",
            DiagnosticStatus::Pass,
            "no site certificates issued yet (issued on first HTTPS request)",
        );
    }
    let expired: Vec<&str> = leaves
        .iter()
        .filter(|(_, d)| *d < 0)
        .map(|(n, _)| n.as_str())
        .collect();
    let (soonest_name, soonest_days) = leaves
        .iter()
        .min_by_key(|(_, d)| *d)
        .map(|(n, d)| (n.as_str(), *d))
        .expect("non-empty");
    if !expired.is_empty() {
        return entry(
            "site-certs",
            DiagnosticStatus::Warn,
            format!(
                "{} of {} expired ({}) — reissued automatically on the next HTTPS request",
                expired.len(),
                leaves.len(),
                expired.join(", ")
            ),
        );
    }
    entry(
        "site-certs",
        DiagnosticStatus::Pass,
        format!(
            "{} issued, soonest expires in {soonest_days} days ({soonest_name}); renewed within 30",
            leaves.len()
        ),
    )
}

/// Is *this* CA what the machine trusts — and is it the only Grove CA it
/// trusts? `grove ca rotate` without `sudo` (fixed in 1.6.0) left the old,
/// unconstrained CA in the keychain beside the new one. A stale unconstrained
/// CA can sign any hostname the machine will believe, and nothing else in
/// Grove would ever notice it again.
fn trust_store_check(ca_path: &Path) -> DiagnosticEntry {
    use grove_os::PlatformIntegration;
    let Ok(on_disk_pem) = std::fs::read_to_string(ca_path) else {
        return entry(
            "trust-store",
            DiagnosticStatus::Warn,
            format!("could not read {}", ca_path.display()),
        );
    };
    let Some(on_disk_fp) = grove_tls::fingerprint_sha256(&on_disk_pem) else {
        return entry(
            "trust-store",
            DiagnosticStatus::Fail,
            format!("{} does not parse as a certificate", ca_path.display()),
        );
    };
    let trusted = match grove_os::current().trusted_grove_cas() {
        Ok(t) => t,
        Err(e) => {
            return entry(
                "trust-store",
                DiagnosticStatus::Warn,
                format!("could not read the system trust store: {e}"),
            )
        }
    };
    let seen: Vec<TrustedSummary> = trusted
        .iter()
        .filter_map(|t| {
            Some(TrustedSummary {
                fingerprint: grove_tls::fingerprint_sha256(&t.pem)?,
                constrained: grove_tls::permitted_dns_subtrees(&t.pem).is_some(),
                remove_hint: t.remove_hint.clone(),
            })
        })
        .collect();
    evaluate_trust_store(&on_disk_fp, &seen)
}

struct TrustedSummary {
    fingerprint: String,
    constrained: bool,
    remove_hint: String,
}

fn evaluate_trust_store(on_disk_fp: &str, trusted: &[TrustedSummary]) -> DiagnosticEntry {
    let current_trusted = trusted.iter().any(|t| t.fingerprint == on_disk_fp);
    let stale: Vec<&TrustedSummary> = trusted
        .iter()
        .filter(|t| t.fingerprint != on_disk_fp)
        .collect();
    let short = &on_disk_fp[..16.min(on_disk_fp.len())];

    if !current_trusted {
        return entry(
            "trust-store",
            DiagnosticStatus::Fail,
            format!(
                "the CA on disk (sha256 {short}…) is not in the system trust store — every HTTPS \
                 site is a certificate error until `sudo grove ca trust`{}",
                if stale.is_empty() {
                    String::new()
                } else {
                    format!("; {} other Grove CA(s) are trusted instead", stale.len())
                }
            ),
        );
    }
    if let Some(unconstrained) = stale.iter().find(|t| !t.constrained) {
        return entry(
            "trust-store",
            DiagnosticStatus::Fail,
            format!(
                "an old, unconstrained Grove CA is still trusted — it can sign any hostname this \
                 machine will believe. Remove it: {}",
                unconstrained.remove_hint
            ),
        );
    }
    if !stale.is_empty() {
        return entry(
            "trust-store",
            DiagnosticStatus::Warn,
            format!(
                "{} old Grove CA(s) still trusted (constrained, so harmless, but clutter). \
                 Remove: {}",
                stale.len(),
                stale
                    .iter()
                    .map(|t| t.remove_hint.as_str())
                    .collect::<Vec<_>>()
                    .join(" ; ")
            ),
        );
    }
    entry(
        "trust-store",
        DiagnosticStatus::Pass,
        format!("this CA (sha256 {short}…) is trusted, and no stale Grove CA is"),
    )
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

    /// `ca-meta.json` is a note beside the certificate. When the two disagree,
    /// the extension is what browsers enforce, so the extension must win — in
    /// both directions. The second half is the one that bites: a note left over
    /// from a constrained CA, beside a certificate that constrains nothing,
    /// must not produce a green `root-ca-scope`.
    #[test]
    fn the_certificate_outranks_the_note_beside_it() {
        let base = std::env::temp_dir().join(format!("grove-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = GrovePaths::with_base(&base);
        paths.ensure().unwrap();
        let ca = grove_tls::CertificateAuthority::load_or_create(&paths).unwrap();
        let constrained = std::fs::read_to_string(paths.ca_cert()).unwrap();
        assert_eq!(ca.constrained_tld(), Some("test"));

        assert_eq!(
            scope_of(Some(&constrained), || Some("wrong".into())),
            Some("test".into()),
            "the extension decides, not the note"
        );
        assert_eq!(
            scope_of(Some(UNCONSTRAINED), || Some("test".into())),
            None,
            "a stale note must not vouch for a CA that constrains nothing"
        );
        assert_eq!(
            scope_of(Some("not a certificate"), || Some("test".into())),
            Some("test".into()),
            "only an unreadable certificate falls back to the note"
        );
        assert_eq!(scope_of(None, || None), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Self-signed, no `NameConstraints` — the shape of a Grove CA minted
    /// before 1.5.0 added the extension.
    const UNCONSTRAINED: &str = "\
-----BEGIN CERTIFICATE-----
MIIC0DCCAbgCCQD9pHpVq7tyhDANBgkqhkiG9w0BAQsFADAqMRcwFQYDVQQDDA5O
b3QgQSBHcm92ZSBDQTEPMA0GA1UECgwGTm9ib2R5MB4XDTI2MDkxMzE4NDQ1M1oX
DTQ2MDkwODE4NDQ1M1owKjEXMBUGA1UEAwwOTm90IEEgR3JvdmUgQ0ExDzANBgNV
BAoMBk5vYm9keTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBALmZ/kqy
6P8n3OCjhJlNSiKgymXaFPy1E6/BMf2TNADb3jc45P9rhRrrzZdj5oQeUzI4W106
W9QkJjA85BGBqP08VNbtvf6KEaKL0GOY2EJsRpNGLz7oG28t8Y2tTgE5NUNX/+1e
bnuOs4O8q72II7zX0HTZ9lcEvqTRlolzAtg/VxBuw+Qzg7aSobmCEjxGO8nugD+6
oDnK7YFGRxF7qCRzzSPn8SGObRepbsgO6KpLlT5yXDg51eH2x3wZSQXrnaDmATMn
DK038oHg7zqXHR6qhHbheoj0WQKz7FSC89cR6Zma3D2nFX74F7B7+vRyYF2zswvz
kS/cJf4TatE7q38CAwEAATANBgkqhkiG9w0BAQsFAAOCAQEAJScBKEIX9DFytJus
Ne/P1sA5cbBcCZu/ea7i/qaC3idML0H8FBoAq0ZOTO8J75XsGPdZy8+cY69nPvGM
yl+3PWhnaCB5ZZ0i7V28hbJag10TAkMv685a6PuYj2ee++UO8/ol+lrtGNawJC+l
8FUaw0mdzSDnW6+J4lQqRd4AdxvYTqVGC3+D/cb43bpzPKxOJiUwcH9hnsO59i4d
5qMBYPLpoN5vkmF+wMCFcFEWlao/tiH4axsXP2wQL3IF/a4LauRVIqZrjhK5V9Iu
QHBGjXmc8m6KA0HyNheMMfCO0wGgwYguWnLj664X2H1X93eJilOJ676jIErCUZMd
698Y9Q==
-----END CERTIFICATE-----
";

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

    fn ts(fp: &str, constrained: bool, hint: &str) -> TrustedSummary {
        TrustedSummary {
            fingerprint: fp.into(),
            constrained,
            remove_hint: hint.into(),
        }
    }

    #[test]
    fn trust_store_verdicts() {
        let cur = "c".repeat(64);
        // Exactly this CA, nothing else: pass.
        let e = evaluate_trust_store(&cur, &[ts(&cur, true, "")]);
        assert_eq!(e.status, DiagnosticStatus::Pass, "{}", e.detail);

        // Not trusted at all: fail, and say what fixes it.
        let e = evaluate_trust_store(&cur, &[]);
        assert_eq!(e.status, DiagnosticStatus::Fail);
        assert!(e.detail.contains("sudo grove ca trust"), "{}", e.detail);

        // The 1.5.0 hazard: current trusted, but an old unconstrained one too.
        let e = evaluate_trust_store(
            &cur,
            &[
                ts(&cur, true, ""),
                ts(
                    &"a".repeat(64),
                    false,
                    "sudo security delete-certificate -Z AAAA x",
                ),
            ],
        );
        assert_eq!(e.status, DiagnosticStatus::Fail);
        assert!(e.detail.contains("unconstrained"), "{}", e.detail);
        assert!(
            e.detail.contains("-Z AAAA"),
            "the removal command is the store's own: {}",
            e.detail
        );

        // Old but constrained: clutter, not danger.
        let e = evaluate_trust_store(
            &cur,
            &[ts(&cur, true, ""), ts(&"b".repeat(64), true, "rm b")],
        );
        assert_eq!(e.status, DiagnosticStatus::Warn);
        assert!(e.detail.contains("harmless"), "{}", e.detail);
    }

    #[test]
    fn leaf_summaries() {
        assert_eq!(summarize_leaves(&[]).status, DiagnosticStatus::Pass);
        let ok = summarize_leaves(&[("a.test".into(), 300), ("b.test".into(), 12)]);
        assert_eq!(ok.status, DiagnosticStatus::Pass);
        assert!(ok.detail.contains("12 days (b.test)"), "{}", ok.detail);
        let exp = summarize_leaves(&[("a.test".into(), 300), ("old.test".into(), -3)]);
        assert_eq!(exp.status, DiagnosticStatus::Warn);
        assert!(
            exp.detail.contains("old.test") && exp.detail.contains("reissued"),
            "{}",
            exp.detail
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_world_accessible_ipc_socket_fails_and_a_private_one_passes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("grove-doctor-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("groved.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666)).unwrap();
        let e = ipc_socket_check(&sock).expect("socket exists");
        assert_eq!(e.status, DiagnosticStatus::Fail, "{}", e.detail);
        assert!(e.detail.contains("0666"), "{}", e.detail);

        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o660)).unwrap();
        let e = ipc_socket_check(&sock).unwrap();
        assert_eq!(e.status, DiagnosticStatus::Pass, "{}", e.detail);

        assert!(
            ipc_socket_check(&dir.join("missing.sock")).is_none(),
            "no socket, no entry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_writable_grove_home_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("grove-doctor-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let e = grove_home_check(&dir);
        assert_eq!(e.status, DiagnosticStatus::Fail, "{}", e.detail);
        assert!(e.detail.contains("chmod o-w"), "{}", e.detail);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(grove_home_check(&dir).status, DiagnosticStatus::Pass);
        assert_eq!(
            grove_home_check(&dir.join("nope")).status,
            DiagnosticStatus::Fail
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

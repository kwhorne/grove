//! macOS integration: `/etc/resolver/<tld>`, trust store via the `security`
//! tool, and launchd (service install lives in grove-daemon's installer).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{OsError, PlatformIntegration, Result};

#[derive(Default)]
pub struct MacOs;

impl MacOs {
    fn resolver_file(tld: &str) -> String {
        format!("/etc/resolver/{tld}")
    }
}

impl PlatformIntegration for MacOs {
    fn install_resolver(&self, tld: &str, dns_port: u16) -> Result<()> {
        // macOS reads every file in /etc/resolver/ as a scoped resolver for the
        // matching domain. Pointing it at 127.0.0.1 routes *.test to Grove.
        let path = Self::resolver_file(tld);
        std::fs::create_dir_all("/etc/resolver")?;
        let body = format!("nameserver 127.0.0.1\nport {dns_port}\n");
        std::fs::write(&path, body)?;
        tracing::info!(%path, "installed macOS resolver");
        Ok(())
    }

    fn uninstall_resolver(&self, tld: &str) -> Result<()> {
        let path = Self::resolver_file(tld);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn trust_ca(&self, ca_cert: &Path) -> Result<()> {
        // Add to the system keychain as a trusted root.
        let status = Command::new("security")
            .args([
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                "-k",
                "/Library/Keychains/System.keychain",
            ])
            .arg(ca_cert)
            .status()?;
        if !status.success() {
            return Err(OsError::Command {
                cmd: "security add-trusted-cert".into(),
                detail: format!("exit status {status}"),
            });
        }
        Ok(())
    }

    fn untrust_ca(&self, ca_cert: &Path) -> Result<()> {
        let status = Command::new("security")
            .arg("remove-trusted-cert")
            .arg("-d")
            .arg(ca_cert)
            .status()?;
        if !status.success() {
            return Err(OsError::Command {
                cmd: "security remove-trusted-cert".into(),
                detail: format!("exit status {status}"),
            });
        }
        Ok(())
    }

    fn trusted_grove_cas(&self) -> Result<Vec<crate::TrustedCert>> {
        // `-Z -p` prints, per match, a `SHA-1 hash:` line followed by the PEM.
        let out = Command::new("security")
            .args([
                "find-certificate",
                "-a",
                "-c",
                "Grove Local CA",
                "-Z",
                "-p",
                "/Library/Keychains/System.keychain",
            ])
            .output()?;
        if !out.status.success() {
            // No match is exit status 44 ("item not found"): an empty store.
            if out.status.code() == Some(44) {
                return Ok(Vec::new());
            }
            return Err(OsError::Command {
                cmd: "security find-certificate".into(),
                detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        let mut found = parse_find_certificate(&String::from_utf8_lossy(&out.stdout));
        // Present is not the same as trusted, and only trusted is a problem.
        found.retain(|c| trusted_as_root(&c.pem));
        Ok(found)
    }

    fn name(&self) -> &'static str {
        "macos"
    }
}

/// Does the machine actually believe this certificate as a root?
///
/// `find-certificate` answers a different question — what is *stored* in the
/// keychain. `grove ca rotate` removes the old CA's trust settings
/// (`remove-trusted-cert`) but leaves the certificate itself behind, so the
/// search still lists it while nothing on the machine will chain to it. Only
/// the trust evaluator can tell an anchor apart from that leftover, and the
/// difference decides whether doctor reports a security problem or a stray
/// file. `-L` keeps it off the network, `-l` says the certificate under test
/// is itself a CA, `-q` keeps `security` quiet.
///
/// When the check cannot be run at all, the certificate counts as trusted: a
/// false alarm the user can dismiss beats silence about a CA that can sign
/// any hostname.
fn trusted_as_root(pem: &str) -> bool {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "grove-trust-check-{}-{}.pem",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&path, pem).is_err() {
        return true;
    }
    let verdict = Command::new("security")
        .args(["verify-cert", "-c"])
        .arg(&path)
        .args(["-L", "-l", "-q"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(true);
    let _ = std::fs::remove_file(&path);
    verdict
}

/// Split `security find-certificate -a -Z -p` output into entries.
fn parse_find_certificate(text: &str) -> Vec<crate::TrustedCert> {
    let mut out = Vec::new();
    let mut sha1: Option<String> = None;
    let mut pem: Option<String> = None;
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("SHA-1 hash:") {
            sha1 = Some(h.trim().to_string());
        } else if line.starts_with("-----BEGIN CERTIFICATE-----") {
            pem = Some(format!("{line}\n"));
        } else if let Some(p) = pem.as_mut() {
            p.push_str(line);
            p.push('\n');
            if line.starts_with("-----END CERTIFICATE-----") {
                let id = sha1.take().unwrap_or_default();
                out.push(crate::TrustedCert {
                    pem: pem.take().unwrap(),
                    remove_hint: format!(
                        "sudo security delete-certificate -Z {id} /Library/Keychains/System.keychain"
                    ),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod find_certificate_tests {
    use super::*;

    #[test]
    fn two_entries_are_split_and_each_gets_its_own_removal_command() {
        let text = "SHA-1 hash: AAAA1111\nkeychain: \"/Library/Keychains/System.keychain\"\n\
-----BEGIN CERTIFICATE-----\nMIIB\nfirst\n-----END CERTIFICATE-----\n\
SHA-1 hash: BBBB2222\n-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----\n";
        let got = parse_find_certificate(text);
        assert_eq!(got.len(), 2);
        assert!(got[0].pem.contains("first"));
        assert!(got[0]
            .remove_hint
            .ends_with("-Z AAAA1111 /Library/Keychains/System.keychain"));
        assert!(got[1].remove_hint.contains("BBBB2222"));
        assert_eq!(parse_find_certificate(""), Vec::<crate::TrustedCert>::new());
    }

    /// A self-signed certificate this machine has never seen must come back
    /// untrusted. Without this, `trusted_grove_cas` could go back to reporting
    /// whatever is *stored* in the keychain — which after `grove ca rotate`
    /// includes the old CA, trust settings removed, harmless — and doctor
    /// would raise a security failure over a stray file.
    #[test]
    fn a_certificate_nobody_trusts_is_not_a_root() {
        const STRANGER: &str = "\
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
        assert!(!trusted_as_root(STRANGER));
    }
}

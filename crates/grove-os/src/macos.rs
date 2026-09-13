//! macOS integration: `/etc/resolver/<tld>`, trust store via the `security`
//! tool, and launchd (service install lives in grove-daemon's installer).

use std::path::Path;
use std::process::Command;

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
        Ok(parse_find_certificate(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    fn name(&self) -> &'static str {
        "macos"
    }
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
}

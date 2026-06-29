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

    fn name(&self) -> &'static str {
        "macos"
    }
}

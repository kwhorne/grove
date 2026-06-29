//! Windows integration (PRD §8.4, §12). Wildcard DNS on Windows is hard, so v1
//! leans on a local DNS proxy plus a `hosts` fallback for known sites. Trust
//! store work uses `certutil`.

use std::path::Path;
use std::process::Command;

use crate::{OsError, PlatformIntegration, Result};

#[derive(Default)]
pub struct Windows;

impl PlatformIntegration for Windows {
    fn install_resolver(&self, tld: &str, _dns_port: u16) -> Result<()> {
        // Full wildcard DNS is impractical without a NRPT rule; that is applied
        // by the service installer via PowerShell. Surface guidance for now.
        Err(OsError::Unsupported(format!(
            "wildcard DNS for .{tld} requires an NRPT rule; sites fall back to the hosts file"
        )))
    }

    fn uninstall_resolver(&self, _tld: &str) -> Result<()> {
        Ok(())
    }

    fn trust_ca(&self, ca_cert: &Path) -> Result<()> {
        let status = Command::new("certutil")
            .args(["-addstore", "-f", "Root"])
            .arg(ca_cert)
            .status()?;
        if !status.success() {
            return Err(OsError::Command {
                cmd: "certutil -addstore Root".into(),
                detail: format!("exit status {status}"),
            });
        }
        Ok(())
    }

    fn untrust_ca(&self, _ca_cert: &Path) -> Result<()> {
        let _ = Command::new("certutil")
            .args(["-delstore", "Root", "Grove Local CA"])
            .status();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "windows"
    }
}

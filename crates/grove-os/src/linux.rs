//! Linux integration. v1 uses an `/etc/hosts`-independent approach
//! where possible via systemd-resolved; a full NetworkManager/per-domain
//! resolver hookup is tracked for fase 1/2.

use std::path::Path;
use std::process::Command;

use crate::{OsError, PlatformIntegration, Result};

#[derive(Default)]
pub struct Linux;

impl PlatformIntegration for Linux {
    fn install_resolver(&self, tld: &str, _dns_port: u16) -> Result<()> {
        // Preferred: register Grove's resolver for the TLD with systemd-resolved.
        // resolvectl needs an interface; a dedicated dummy link is created by the
        // service installer. Here we attempt the common case and surface a clear
        // error otherwise so `grove doctor` can guide the user.
        let status = Command::new("resolvectl")
            .args(["domain", "grove0", &format!("~{tld}")])
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => Err(OsError::Unsupported(format!(
                "automatic resolver setup for .{tld}; add '127.0.0.1 <site>.{tld}' to /etc/hosts as a fallback"
            ))),
        }
    }

    fn uninstall_resolver(&self, _tld: &str) -> Result<()> {
        Ok(())
    }

    fn trust_ca(&self, ca_cert: &Path) -> Result<()> {
        // Copy into the system anchors and refresh. Distro paths vary; this
        // covers Debian/Ubuntu. RHEL uses /etc/pki/ca-trust/source/anchors.
        let dest = Path::new("/usr/local/share/ca-certificates/grove-ca.crt");
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ca_cert, dest)?;
        let status = Command::new("update-ca-certificates").status()?;
        if !status.success() {
            return Err(OsError::Command {
                cmd: "update-ca-certificates".into(),
                detail: format!("exit status {status}"),
            });
        }
        Ok(())
    }

    fn untrust_ca(&self, _ca_cert: &Path) -> Result<()> {
        let dest = Path::new("/usr/local/share/ca-certificates/grove-ca.crt");
        let _ = std::fs::remove_file(dest);
        let _ = Command::new("update-ca-certificates")
            .arg("--fresh")
            .status();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "linux"
    }
}

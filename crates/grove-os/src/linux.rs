//! Linux integration: resolver through systemd-resolved, CA trust into the
//! distro's store and into the browsers' own NSS databases.
//!
//! Everything that touches the system is expressed first as a *plan* — the
//! exact commands — by a pure function, and then executed. The plans are what
//! the unit tests check, because the machines this code is developed on do not
//! run systemd, and "it compiled" is not a test of a resolver setup.
//!
//! ## What the first version got wrong
//!
//! It ran `resolvectl domain grove0 ~test` against a link named `grove0` that
//! nothing ever created, so every install fell to the "add it to /etc/hosts"
//! error. It set no DNS server on the link even if the link had existed. It
//! installed the CA only into Debian's path with Debian's command, and only
//! into the system store — which Chrome and Firefox on Linux do not read, so
//! the padlock stayed red on every site whatever the system said.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{OsError, PlatformIntegration, Result};

/// The dummy interface systemd-resolved's per-link settings hang off.
pub const RESOLVER_LINK: &str = "grove0";

/// Nickname the CA is stored under in NSS databases, so it can be removed.
pub const NSS_NICKNAME: &str = "Grove Local CA";

/// A sequence of commands, each as argv.
pub type Plan = Vec<Vec<String>>;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Route `*.<tld>` to Grove's DNS on loopback via systemd-resolved.
///
/// resolved scopes DNS servers and routing domains to *links*, so a dummy link
/// is created to carry them. `ip link add` fails when the link already exists;
/// the executor treats that one step as idempotent.
pub fn resolver_install_plan(tld: &str, dns_port: u16) -> Plan {
    vec![
        argv(&["ip", "link", "add", RESOLVER_LINK, "type", "dummy"]),
        argv(&["ip", "link", "set", RESOLVER_LINK, "up"]),
        vec![
            "resolvectl".into(),
            "dns".into(),
            RESOLVER_LINK.into(),
            format!("127.0.0.1:{dns_port}"),
        ],
        vec![
            "resolvectl".into(),
            "domain".into(),
            RESOLVER_LINK.into(),
            format!("~{tld}"),
        ],
    ]
}

/// Undo [`resolver_install_plan`].
pub fn resolver_uninstall_plan() -> Plan {
    vec![
        argv(&["resolvectl", "revert", RESOLVER_LINK]),
        argv(&["ip", "link", "del", RESOLVER_LINK]),
    ]
}

/// The same plan as one shell line, for a systemd `ExecStartPre=` so the link
/// and its settings come back after a reboot (neither persists on its own).
pub fn resolver_exec_start_pre(tld: &str, dns_port: u16) -> String {
    let steps: Vec<String> = resolver_install_plan(tld, dns_port)
        .into_iter()
        .enumerate()
        .map(|(i, argv)| {
            let cmd = argv.join(" ");
            // The link may already exist; every other step must succeed.
            if i == 0 {
                format!("{cmd} 2>/dev/null || true")
            } else {
                cmd
            }
        })
        .collect();
    format!("/bin/sh -c '{}'", steps.join("; "))
}

/// Where a distro keeps locally added CA anchors, and how it rebuilds the
/// bundle afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStore {
    /// Debian, Ubuntu and derivatives.
    Debian,
    /// Fedora, RHEL, CentOS, Arch (p11-kit): `update-ca-trust`.
    P11Kit,
}

impl TrustStore {
    pub fn anchor_path(self) -> PathBuf {
        match self {
            TrustStore::Debian => PathBuf::from("/usr/local/share/ca-certificates/grove-ca.crt"),
            TrustStore::P11Kit => PathBuf::from("/etc/pki/ca-trust/source/anchors/grove-ca.crt"),
        }
    }
    pub fn refresh_command(self) -> Vec<String> {
        match self {
            TrustStore::Debian => argv(&["update-ca-certificates"]),
            TrustStore::P11Kit => argv(&["update-ca-trust", "extract"]),
        }
    }
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// Pick the system trust store by which refresh command the distro ships.
pub fn detect_trust_store() -> Option<TrustStore> {
    if command_exists("update-ca-certificates") {
        Some(TrustStore::Debian)
    } else if command_exists("update-ca-trust") {
        Some(TrustStore::P11Kit)
    } else {
        None
    }
}

/// Add `cert` to every NSS database in `dbs` (Chrome's shared db, each Firefox
/// profile) under [`NSS_NICKNAME`] with the CA trust flag.
pub fn nss_trust_plan(cert: &Path, dbs: &[PathBuf]) -> Plan {
    dbs.iter()
        .map(|db| {
            vec![
                "certutil".into(),
                "-d".into(),
                format!("sql:{}", db.display()),
                "-A".into(),
                "-t".into(),
                "C,,".into(),
                "-n".into(),
                NSS_NICKNAME.into(),
                "-i".into(),
                cert.display().to_string(),
            ]
        })
        .collect()
}

/// Remove [`NSS_NICKNAME`] from every database in `dbs`.
pub fn nss_untrust_plan(dbs: &[PathBuf]) -> Plan {
    dbs.iter()
        .map(|db| {
            vec![
                "certutil".into(),
                "-d".into(),
                format!("sql:{}", db.display()),
                "-D".into(),
                "-n".into(),
                NSS_NICKNAME.into(),
            ]
        })
        .collect()
}

/// The NSS databases a user's browsers read: Chrome/Chromium's shared
/// `~/.pki/nssdb`, and one per Firefox profile (native and snap layouts).
/// Only existing directories, plus the Chrome one always — it is created if
/// missing so Chrome picks the CA up on next launch.
pub fn nss_databases(home: &Path) -> Vec<PathBuf> {
    let mut dbs = vec![home.join(".pki/nssdb")];
    for profiles in [
        home.join(".mozilla/firefox"),
        home.join("snap/firefox/common/.mozilla/firefox"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&profiles) {
            for e in entries.flatten() {
                let p = e.path();
                if p.join("cert9.db").exists() {
                    dbs.push(p);
                }
            }
        }
    }
    dbs
}

/// The home directory of the user who ran `sudo`, if any — the browsers to
/// trust the CA in are theirs, not root's.
fn invoking_user_home() -> Option<(PathBuf, Option<(u32, u32)>)> {
    let user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root");
    match user {
        Some(user) => {
            let out = Command::new("getent")
                .args(["passwd", &user])
                .output()
                .ok()?;
            let line = String::from_utf8_lossy(&out.stdout);
            let fields: Vec<&str> = line.trim().split(':').collect();
            let home = fields.get(5).map(PathBuf::from)?;
            let ids = match (fields.get(2), fields.get(3)) {
                (Some(u), Some(g)) => Some((u.parse().ok()?, g.parse().ok()?)),
                _ => None,
            };
            Some((home, ids))
        }
        None => std::env::var_os("HOME").map(|h| (PathBuf::from(h), None)),
    }
}

fn run(argv: &[String]) -> Result<()> {
    let (cmd, args) = argv.split_first().expect("non-empty argv");
    let out = Command::new(cmd).args(args).output()?;
    if !out.status.success() {
        return Err(OsError::Command {
            cmd: argv.join(" "),
            detail: format!(
                "exit status {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(())
}

#[derive(Default)]
pub struct Linux;

impl PlatformIntegration for Linux {
    fn install_resolver(&self, tld: &str, dns_port: u16) -> Result<()> {
        if !command_exists("resolvectl") {
            return Err(OsError::Unsupported(format!(
                "resolver setup for .{tld} needs systemd-resolved (`resolvectl`), which this \
                 system does not have. Point your resolver at 127.0.0.1:{dns_port} for .{tld}, \
                 or add sites to /etc/hosts"
            )));
        }
        for (i, step) in resolver_install_plan(tld, dns_port).iter().enumerate() {
            match run(step) {
                Ok(()) => {}
                // The dummy link is still there from a previous install.
                Err(_) if i == 0 => {}
                Err(e) => return Err(e),
            }
        }
        tracing::info!(
            link = RESOLVER_LINK,
            tld,
            dns_port,
            "installed systemd-resolved routing"
        );
        Ok(())
    }

    fn uninstall_resolver(&self, _tld: &str) -> Result<()> {
        // Best-effort: a missing link means nothing to undo.
        for step in resolver_uninstall_plan() {
            if let Err(e) = run(&step) {
                tracing::debug!(error = %e, "resolver teardown step");
            }
        }
        Ok(())
    }

    fn trust_ca(&self, ca_cert: &Path) -> Result<()> {
        // 1. The system store — what curl, PHP and everything using OpenSSL read.
        let store = detect_trust_store().ok_or_else(|| {
            OsError::Unsupported(
                "no known CA store: neither `update-ca-certificates` (Debian/Ubuntu) nor \
                 `update-ca-trust` (Fedora/RHEL/Arch) is installed"
                    .into(),
            )
        })?;
        let dest = store.anchor_path();
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ca_cert, &dest)?;
        run(&store.refresh_command())?;
        tracing::info!(store = ?store, anchor = %dest.display(), "root CA added to the system store");

        // 2. The browsers. Chrome and Firefox on Linux keep their own NSS
        // databases and do not consult the system store, so without this the
        // padlock stays red however green `curl` is. Best-effort: the system
        // store above is the requirement; this is reported, not fatal.
        if !command_exists("certutil") {
            tracing::warn!(
                "certutil (libnss3-tools / nss-tools) is not installed, so the CA was not \
                 added to Chrome's or Firefox's own stores — browsers will not trust it"
            );
            return Ok(());
        }
        let Some((home, ids)) = invoking_user_home() else {
            return Ok(());
        };
        let dbs = nss_databases(&home);
        let chrome_db = home.join(".pki/nssdb");
        if !chrome_db.join("cert9.db").exists() {
            std::fs::create_dir_all(&chrome_db)?;
            let _ = run(&[
                "certutil".into(),
                "-d".into(),
                format!("sql:{}", chrome_db.display()),
                "-N".into(),
                "--empty-password".into(),
            ]);
        }
        for step in nss_trust_plan(ca_cert, &dbs) {
            if let Err(e) = run(&step) {
                tracing::warn!(error = %e, "adding the CA to a browser NSS database");
            }
        }
        // Whatever certutil created under the user's home as root is theirs.
        if let Some((uid, gid)) = ids {
            for db in &dbs {
                own_tree(db, uid, gid);
            }
        }
        tracing::info!(
            databases = dbs.len(),
            "root CA added to browser NSS databases"
        );
        Ok(())
    }

    fn untrust_ca(&self, _ca_cert: &Path) -> Result<()> {
        if let Some(store) = detect_trust_store() {
            let _ = std::fs::remove_file(store.anchor_path());
            let _ = run(&store.refresh_command());
        }
        if command_exists("certutil") {
            if let Some((home, _)) = invoking_user_home() {
                for step in nss_untrust_plan(&nss_databases(&home)) {
                    let _ = run(&step);
                }
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "linux"
    }
}

fn own_tree(path: &Path, uid: u32, gid: u32) {
    let _ = Command::new("chown")
        .args(["-R", &format!("{uid}:{gid}")])
        .arg(path)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan the first version never had: a link to hang the settings on,
    /// a DNS server *and* a routing domain, in that order.
    #[test]
    fn the_resolver_plan_creates_the_link_then_routes_the_tld_to_grove() {
        let plan = resolver_install_plan("test", 53);
        let flat: Vec<String> = plan.iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            flat,
            vec![
                "ip link add grove0 type dummy",
                "ip link set grove0 up",
                "resolvectl dns grove0 127.0.0.1:53",
                "resolvectl domain grove0 ~test",
            ]
        );
        // A non-default DNS port reaches resolved as ip:port.
        assert!(resolver_install_plan("test", 15353)[2]
            .join(" ")
            .ends_with("127.0.0.1:15353"));
    }

    #[test]
    fn the_exec_start_pre_line_tolerates_an_existing_link_only() {
        let line = resolver_exec_start_pre("test", 53);
        assert!(line.starts_with("/bin/sh -c '"), "{line}");
        assert!(
            line.contains("ip link add grove0 type dummy 2>/dev/null || true; "),
            "{line}"
        );
        assert!(
            !line.contains("resolvectl dns grove0 127.0.0.1:53 2>/dev/null"),
            "the dns step must not be tolerated: {line}"
        );
        assert!(line.ends_with("resolvectl domain grove0 ~test'"), "{line}");
    }

    #[test]
    fn uninstall_reverts_the_link_settings_then_removes_it() {
        let flat: Vec<String> = resolver_uninstall_plan()
            .iter()
            .map(|a| a.join(" "))
            .collect();
        assert_eq!(flat, vec!["resolvectl revert grove0", "ip link del grove0"]);
    }

    #[test]
    fn each_trust_store_has_its_own_anchor_dir_and_refresh() {
        assert_eq!(
            TrustStore::Debian.anchor_path(),
            PathBuf::from("/usr/local/share/ca-certificates/grove-ca.crt")
        );
        assert_eq!(
            TrustStore::Debian.refresh_command(),
            vec!["update-ca-certificates"]
        );
        assert_eq!(
            TrustStore::P11Kit.anchor_path(),
            PathBuf::from("/etc/pki/ca-trust/source/anchors/grove-ca.crt")
        );
        assert_eq!(
            TrustStore::P11Kit.refresh_command(),
            vec!["update-ca-trust", "extract"]
        );
    }

    /// The browsers read NSS, not the system store. The plan must reach every
    /// database with the CA trust flag and a nickname it can be removed by.
    #[test]
    fn the_nss_plan_adds_the_ca_to_every_database_with_ca_trust() {
        let cert = PathBuf::from("/x/grove-ca.pem");
        let dbs = vec![
            PathBuf::from("/home/u/.pki/nssdb"),
            PathBuf::from("/home/u/.mozilla/firefox/abc.default"),
        ];
        let plan = nss_trust_plan(&cert, &dbs);
        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan[0].join(" "),
            "certutil -d sql:/home/u/.pki/nssdb -A -t C,, -n Grove Local CA -i /x/grove-ca.pem"
        );
        assert!(plan[1][2].ends_with("abc.default"));
        let undo = nss_untrust_plan(&dbs);
        assert_eq!(
            undo[0].join(" "),
            "certutil -d sql:/home/u/.pki/nssdb -D -n Grove Local CA"
        );
    }

    /// Chrome's database is always in the list (it is created if missing);
    /// Firefox profiles only when they exist and hold a cert9.db.
    #[test]
    fn nss_databases_finds_chrome_and_real_firefox_profiles() {
        let home = std::env::temp_dir().join(format!("grove-nss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let ff = home.join(".mozilla/firefox");
        std::fs::create_dir_all(ff.join("k3j2.default-release")).unwrap();
        std::fs::write(ff.join("k3j2.default-release/cert9.db"), b"").unwrap();
        std::fs::create_dir_all(ff.join("Crash Reports")).unwrap(); // no cert9.db

        let dbs = nss_databases(&home);
        assert_eq!(dbs[0], home.join(".pki/nssdb"));
        assert_eq!(dbs.len(), 2, "{dbs:?}");
        assert!(dbs[1].ends_with("k3j2.default-release"));
        let _ = std::fs::remove_dir_all(&home);
    }
}

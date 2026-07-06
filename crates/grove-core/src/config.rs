//! Declarative TOML configuration — the single source of truth.
//!
//! Only things that *cannot* be re-derived live here. Runtime state (which FPM
//! pool is hot, issued leaf certs, etc.) is kept out of config on purpose so the
//! file stays human-readable and diff-friendly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::driver::Driver;
use crate::error::{Error, Result};
use crate::paths::GrovePaths;

fn default_tld() -> String {
    "test".to_string()
}

fn default_php() -> String {
    "8.4".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,

    /// Directories whose immediate subdirectories each become a site.
    #[serde(default, rename = "parked")]
    pub parked: Vec<ParkedDir>,

    /// Explicitly linked / configured sites.
    #[serde(default, rename = "sites")]
    pub sites: Vec<SiteConfig>,

    /// Site names hidden from the list (removed from Grove without deleting
    /// their files). Re-appear if explicitly unforgotten.
    #[serde(default)]
    pub ignored: Vec<String>,

    /// Local service configuration (mail-catcher, …).
    #[serde(default)]
    pub services: Services,

    /// Public tunnel (`grove share`) defaults.
    #[serde(default)]
    pub tunnel: Tunnel,
}

/// Defaults for `grove share` so the flags can be omitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    /// Tunnel server control address. Defaults to the public Grove server.
    #[serde(default = "default_tunnel_server")]
    pub server: Option<String>,
    /// Shared secret matching the server's token (none for the open server).
    #[serde(default)]
    pub token: Option<String>,
}

fn default_tunnel_server() -> Option<String> {
    Some("grove.elyracode.com:7000".to_string())
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            server: default_tunnel_server(),
            token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Services {
    /// Run the built-in SMTP mail-catcher.
    #[serde(default = "default_true")]
    pub mail_enabled: bool,
    /// Port the mail-catcher's SMTP server listens on.
    #[serde(default = "default_mail_port")]
    pub mail_port: u16,
}

fn default_mail_port() -> u16 {
    1025
}

impl Default for Services {
    fn default() -> Self {
        Self {
            mail_enabled: true,
            mail_port: default_mail_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    #[serde(default = "default_tld")]
    pub tld: String,

    #[serde(default = "default_php")]
    pub default_php: String,

    #[serde(default = "default_true")]
    pub auto_start: bool,

    /// HTTP listen port (defaults to 80, overridable for rootless dev).
    #[serde(default = "default_http_port")]
    pub http_port: u16,

    /// HTTPS listen port (defaults to 443).
    #[serde(default = "default_https_port")]
    pub https_port: u16,

    /// DNS listen port (defaults to 53).
    #[serde(default = "default_dns_port")]
    pub dns_port: u16,

    /// Auto-discover Docker/OrbStack containers and serve them as `<name>.test`.
    #[serde(default = "default_true")]
    pub docker: bool,

    /// Load Xdebug into FPM pools for step-debugging (`grove debug on`).
    #[serde(default)]
    pub xdebug: bool,

    /// DBGp port the debugger/adapter listens on and Xdebug connects to.
    #[serde(default = "default_xdebug_port")]
    pub xdebug_port: u16,
}

fn default_xdebug_port() -> u16 {
    9003
}

fn default_http_port() -> u16 {
    80
}
fn default_https_port() -> u16 {
    443
}
fn default_dns_port() -> u16 {
    53
}

impl Default for General {
    fn default() -> Self {
        Self {
            tld: default_tld(),
            default_php: default_php(),
            auto_start: true,
            http_port: default_http_port(),
            https_port: default_https_port(),
            dns_port: default_dns_port(),
            docker: true,
            xdebug: false,
            xdebug_port: default_xdebug_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkedDir {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// PHP version override (isolate). Falls back to `general.default_php`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    /// Node.js version pinned for this site (used by tooling / dev servers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default)]
    pub secure: bool,
    /// Explicit driver override; otherwise auto-detected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<Driver>,
    /// For proxy driver: upstream URL the site forwards to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_to: Option<String>,
}

impl Config {
    /// Load config from the discovered path, creating a default if absent.
    pub fn load(paths: &GrovePaths) -> Result<Self> {
        Self::load_from(&paths.config_file())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let cfg: Config = toml::from_str(&raw).map_err(|source| Error::ConfigParse {
                    path: path.to_path_buf(),
                    source,
                })?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub fn save(&self, paths: &GrovePaths) -> Result<()> {
        paths.ensure()?;
        self.save_to(&paths.config_file())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)?;
        Ok(())
    }

    /// Expand `~` and environment variables in a parked/site path.
    pub fn expand(path: &Path) -> PathBuf {
        let as_str = path.to_string_lossy();
        PathBuf::from(shellexpand::tilde(&as_str).into_owned())
    }

    pub fn find_site(&self, name: &str) -> Option<&SiteConfig> {
        self.sites.iter().find(|s| s.name == name)
    }

    pub fn find_site_mut(&mut self, name: &str) -> Option<&mut SiteConfig> {
        self.sites.iter_mut().find(|s| s.name == name)
    }

    /// Add an explicit site, rejecting duplicates.
    pub fn add_site(&mut self, site: SiteConfig) -> Result<()> {
        if self.find_site(&site.name).is_some() {
            return Err(Error::DuplicateSite(site.name));
        }
        self.sites.push(site);
        Ok(())
    }

    pub fn remove_site(&mut self, name: &str) -> bool {
        let before = self.sites.len();
        self.sites.retain(|s| s.name != name);
        self.sites.len() != before
    }

    pub fn add_parked(&mut self, path: PathBuf) {
        let expanded = Self::expand(&path);
        if !self
            .parked
            .iter()
            .any(|p| Self::expand(&p.path) == expanded)
        {
            self.parked.push(ParkedDir { path });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_roundtrip() {
        let cfg = Config::default();
        let toml = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.general.tld, "test");
        assert_eq!(parsed.general.default_php, "8.4");
        assert_eq!(parsed.general.http_port, 80);
    }

    #[test]
    fn parses_prd_example() {
        let raw = r#"
[general]
tld = "test"
default_php = "8.4"
auto_start = true

[[parked]]
path = "~/Code"

[[sites]]
name = "inside-next"
path = "~/Code/inside-next"
php = "8.4"
secure = true
driver = "laravel"

[[sites]]
name = "frontend"
path = "~/Code/frontend"
driver = "proxy"
proxy_to = "http://127.0.0.1:5173"
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.parked.len(), 1);
        assert_eq!(cfg.sites.len(), 2);
        let frontend = cfg.find_site("frontend").unwrap();
        assert_eq!(frontend.driver, Some(crate::driver::Driver::Proxy));
        assert_eq!(frontend.proxy_to.as_deref(), Some("http://127.0.0.1:5173"));
    }

    #[test]
    fn no_duplicate_parked() {
        let mut cfg = Config::default();
        cfg.add_parked(PathBuf::from("~/Code"));
        cfg.add_parked(PathBuf::from("~/Code"));
        assert_eq!(cfg.parked.len(), 1);
    }
}

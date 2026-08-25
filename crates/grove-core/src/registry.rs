//! Resolves the effective set of sites from config + parked directories.
//!
//! The registry is pure: given a `Config` it deterministically produces the
//! list of `ResolvedSite`s. Explicit `[[sites]]` win over parked discovery when
//! names collide, so a `link` can override a `park`ed default.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{Config, SiteConfig};
use crate::driver::{self, Driver};
use crate::site::{ResolvedSite, SiteKind};

pub struct SiteRegistry {
    sites: BTreeMap<String, ResolvedSite>,
    tld: String,
}

/// The two site keys a hostname could name, given the TLD.
///
/// `api.myapp.test` with tld `test` yields `("myapp", "api.myapp")`: the
/// left-most label, then the whole remainder, which is the order
/// [`SiteRegistry::by_hostname`] tries them in. `None` when the host is not
/// under the TLD at all.
///
/// Extracted so the TLS layer can ask the same question the router does. Two
/// copies of this rule drifting apart is precisely how a certificate gets
/// issued for a name nothing will serve.
pub fn host_candidates<'a>(hostname: &'a str, tld: &str) -> Option<(&'a str, &'a str)> {
    let host = hostname.split(':').next().unwrap_or(hostname);
    let suffix = format!(".{tld}");
    let name = host.strip_suffix(&suffix)?;
    if name.is_empty() {
        return None;
    }
    let leaf = name.rsplit('.').next().unwrap_or(name);
    Some((leaf, name))
}

/// Which hostnames resolve to a site, readable without async.
#[derive(Debug, Clone, Default)]
pub struct KnownHosts {
    tld: String,
    names: std::collections::HashSet<String>,
}

impl KnownHosts {
    /// Whether `hostname` resolves to a site Grove serves.
    pub fn resolves(&self, hostname: &str) -> bool {
        match host_candidates(hostname, &self.tld) {
            Some((leaf, name)) => self.names.contains(leaf) || self.names.contains(name),
            None => false,
        }
    }

    /// The site hostname `hostname` belongs to — `api.myapp.test` → `myapp.test`.
    pub fn site_hostname(&self, hostname: &str) -> Option<String> {
        let (leaf, name) = host_candidates(hostname, &self.tld)?;
        let site = if self.names.contains(leaf) {
            leaf
        } else if self.names.contains(name) {
            name
        } else {
            return None;
        };
        Some(format!("{site}.{}", self.tld))
    }
}

impl SiteRegistry {
    /// Build the registry by resolving every parked subdirectory and explicit
    /// site in `config`.
    pub fn build(config: &Config) -> Self {
        let tld = config.general.tld.clone();
        let default_php = config.general.default_php.clone();
        let mut sites: BTreeMap<String, ResolvedSite> = BTreeMap::new();

        // 1. Parked directories: each immediate subdirectory becomes a site.
        for parked in &config.parked {
            let dir = Config::expand(&parked.path);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                tracing::warn!(path = %dir.display(), "parked dir unreadable, skipping");
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let Some(detected) = driver::detect(&path) else {
                    continue;
                };
                let plan = driver::plan(&path, detected);
                let resolved = ResolvedSite::from_parts(
                    name.to_string(),
                    &tld,
                    path.clone(),
                    plan,
                    default_php.clone(),
                    None,
                    false,
                    SiteKind::Parked,
                    None,
                );
                sites.insert(name.to_string(), resolved);
            }
        }

        // 2. Explicit sites override parked discovery on name collision.
        for sc in &config.sites {
            if let Some(resolved) = resolve_explicit(sc, &tld, &default_php) {
                sites.insert(resolved.name.clone(), resolved);
            }
        }

        // 3. Hide any sites the user has removed from the list (files kept).
        if !config.ignored.is_empty() {
            let ignored: std::collections::HashSet<&str> =
                config.ignored.iter().map(String::as_str).collect();
            sites.retain(|name, _| !ignored.contains(name.as_str()));
        }

        Self { sites, tld }
    }

    pub fn tld(&self) -> &str {
        &self.tld
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedSite> {
        self.sites.get(name)
    }

    /// Merge a Docker/OrbStack-discovered proxy site. Explicit config sites and
    /// parked sites take precedence on name collision.
    pub fn insert_docker(
        &mut self,
        name: &str,
        upstream: Option<&str>,
        id: Option<&str>,
        running: bool,
    ) {
        if self.sites.contains_key(name) {
            return;
        }
        let site = ResolvedSite::docker_proxy(
            name.to_string(),
            &self.tld,
            upstream.map(str::to_string),
            id.map(str::to_string),
            running,
        );
        self.sites.insert(name.to_string(), site);
    }

    /// Look up a site by its hostname (e.g. `myapp.test`).
    pub fn by_hostname(&self, hostname: &str) -> Option<&ResolvedSite> {
        let (leaf, name) = host_candidates(hostname, &self.tld)?;
        self.sites.get(leaf).or_else(|| self.sites.get(name))
    }

    /// A cheap, synchronously-readable view of which hostnames are ours.
    ///
    /// The TLS layer has to answer "is this a site?" inside a rustls callback,
    /// which is synchronous and cannot await the registry's async lock. This is
    /// the same question by the same rule, in a shape that callback can read.
    pub fn known_hosts(&self) -> KnownHosts {
        KnownHosts {
            tld: self.tld.clone(),
            names: self.sites.keys().cloned().collect(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ResolvedSite> {
        self.sites.values()
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}

fn resolve_explicit(sc: &SiteConfig, tld: &str, default_php: &str) -> Option<ResolvedSite> {
    let path = sc
        .path
        .as_ref()
        .map(|p| Config::expand(p))
        .unwrap_or_default();

    let driver = match sc.driver {
        Some(d) => d,
        None => driver::detect(&path).unwrap_or(Driver::Static),
    };

    // Proxy sites do not need a real path on disk.
    if driver != Driver::Proxy && !path.exists() {
        tracing::warn!(site = %sc.name, path = %path.display(), "linked site path missing");
    }

    let plan = driver::plan(&path, driver);
    let php = sc.php.clone().unwrap_or_else(|| default_php.to_string());

    Some(ResolvedSite::from_parts(
        sc.name.clone(),
        tld,
        path,
        plan,
        php,
        sc.node.clone(),
        sc.secure,
        SiteKind::Linked,
        sc.proxy_to.clone(),
    ))
}

/// Derive a site name from a directory path (its file name, lowercased).
pub fn name_from_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_lowercase())
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_candidates_follows_the_router_rule() {
        assert_eq!(
            host_candidates("myapp.test", "test"),
            Some(("myapp", "myapp"))
        );
        assert_eq!(
            host_candidates("api.myapp.test", "test"),
            Some(("myapp", "api.myapp"))
        );
        // A port is not part of the name.
        assert_eq!(
            host_candidates("myapp.test:8443", "test"),
            Some(("myapp", "myapp"))
        );
        // Not under the TLD at all.
        assert_eq!(host_candidates("bank.example", "test"), None);
        assert_eq!(host_candidates("test", "test"), None);
        assert_eq!(host_candidates(".test", "test"), None);
    }

    fn hosts(tld: &str, names: &[&str]) -> KnownHosts {
        KnownHosts {
            tld: tld.to_string(),
            names: names.iter().map(|n| n.to_string()).collect(),
        }
    }

    /// The finding: a machine-trusted certificate must not be mintable for a
    /// name Grove does not serve.
    #[test]
    fn foreign_names_do_not_resolve() {
        let h = hosts("test", &["myapp", "shop"]);
        for name in [
            "google.com",
            "bank.example",
            "myapp.test.evil.com",
            "notmyapp.test",
            "localhost",
            "",
        ] {
            assert!(!h.resolves(name), "{name} must not resolve to a site");
            assert_eq!(h.site_hostname(name), None, "{name}");
        }
    }

    #[test]
    fn sites_and_their_subdomains_resolve() {
        let h = hosts("test", &["myapp", "shop"]);
        for name in [
            "myapp.test",
            "api.myapp.test",
            "a.b.myapp.test",
            "shop.test",
        ] {
            assert!(h.resolves(name), "{name} should resolve");
        }
        // …and every one of them maps back to the site's own hostname, which is
        // what decides whether a certificate is persisted.
        assert_eq!(h.site_hostname("myapp.test").as_deref(), Some("myapp.test"));
        assert_eq!(
            h.site_hostname("api.myapp.test").as_deref(),
            Some("myapp.test")
        );
    }

    #[test]
    fn a_custom_tld_is_honoured() {
        let h = hosts("localhost", &["myapp"]);
        assert!(h.resolves("myapp.localhost"));
        assert!(!h.resolves("myapp.test"));
    }

    #[test]
    fn known_hosts_matches_the_registry_it_came_from() {
        let cfg = Config::default();
        let reg = SiteRegistry::build(&cfg);
        let known = reg.known_hosts();
        // Whatever the registry resolves, the mirror must resolve too — the two
        // drifting apart is how a certificate gets issued for a name nothing
        // will serve, or refused for one that would be.
        for site in reg.iter() {
            assert!(
                known.resolves(&site.hostname),
                "{} resolves in the registry but not in the mirror",
                site.hostname
            );
        }
        assert!(!known.resolves("bank.example"));
    }

    use super::*;
    use crate::config::{Config, ParkedDir};

    #[test]
    fn ignored_sites_are_hidden_but_files_kept() {
        let tmp = std::env::temp_dir().join(format!("grove-reg-test-{}", std::process::id()));
        let alpha = tmp.join("alpha");
        let beta = tmp.join("beta");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();

        let mut config = Config::default();
        config.parked.push(ParkedDir { path: tmp.clone() });

        let reg = SiteRegistry::build(&config);
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("beta").is_some());

        config.ignored.push("beta".into());
        let reg = SiteRegistry::build(&config);
        assert!(reg.get("alpha").is_some(), "alpha stays visible");
        assert!(reg.get("beta").is_none(), "beta is hidden");

        // Files are untouched on disk.
        assert!(beta.is_dir(), "forgetting must not delete files");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

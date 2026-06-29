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

        Self { sites, tld }
    }

    pub fn tld(&self) -> &str {
        &self.tld
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedSite> {
        self.sites.get(name)
    }

    /// Look up a site by its hostname (e.g. `myapp.test`).
    pub fn by_hostname(&self, hostname: &str) -> Option<&ResolvedSite> {
        let host = hostname.split(':').next().unwrap_or(hostname);
        // Strip the TLD; the remaining left-most label is the site name.
        let suffix = format!(".{}", self.tld);
        let name = host.strip_suffix(&suffix)?;
        // Support multi-label hosts like api.myapp.test → myapp.
        let leaf = name.rsplit('.').next().unwrap_or(name);
        self.sites.get(leaf).or_else(|| self.sites.get(name))
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

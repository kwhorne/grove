//! Tracks which PHP builds are known to Grove and how to invoke them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use grove_core::paths::GrovePaths;

/// A single registered PHP version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhpBuild {
    /// Version key used in config / `isolate` (e.g. "8.4").
    pub version: String,
    /// Path to the `php-fpm` binary.
    pub fpm_binary: PathBuf,
    /// Path to the `php` CLI binary (for `php -m`, version checks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_binary: Option<PathBuf>,
    /// Whether this build was registered by the user (bring-your-own).
    #[serde(default)]
    pub user_registered: bool,
}

impl PhpBuild {
    /// List loaded extensions by shelling out to the CLI (`php -m`).
    pub fn extensions(&self) -> Vec<String> {
        let Some(cli) = self.cli_binary.as_ref().or(Some(&self.fpm_binary)) else {
            return Vec::new();
        };
        let Ok(output) = std::process::Command::new(cli).arg("-m").output() else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('['))
            .collect()
    }
}

/// JSON-persisted registry of PHP builds, kept out of the declarative config
/// since it is re-derivable / machine-managed state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PhpRegistry {
    builds: BTreeMap<String, PhpBuild>,
}

impl PhpRegistry {
    fn file(paths: &GrovePaths) -> PathBuf {
        paths.runtimes_dir().join("php-builds.json")
    }

    pub fn load(paths: &GrovePaths) -> Self {
        let file = Self::file(paths);
        match std::fs::read_to_string(&file) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, paths: &GrovePaths) -> std::io::Result<()> {
        paths.ensure()?;
        let body = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(Self::file(paths), body)
    }

    pub fn register(&mut self, build: PhpBuild) {
        self.builds.insert(build.version.clone(), build);
    }

    pub fn get(&self, version: &str) -> Option<&PhpBuild> {
        self.builds.get(version)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PhpBuild> {
        self.builds.values()
    }

    /// Best-effort auto-discovery of php-fpm binaries on PATH / common dirs.
    /// Returns the number of new builds discovered.
    pub fn discover(&mut self) -> usize {
        let mut added = 0;
        for candidate in discover_candidates() {
            if let Some(build) = probe(&candidate) {
                if !self.builds.contains_key(&build.version) {
                    self.builds.insert(build.version.clone(), build);
                    added += 1;
                }
            }
        }
        added
    }
}

/// Probe a php-fpm binary for its version → build descriptor.
fn probe(fpm: &Path) -> Option<PhpBuild> {
    let output = std::process::Command::new(fpm)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // e.g. "PHP 8.4.3 (fpm-fcgi) ..."
    let version = text.split_whitespace().nth(1).and_then(|v| {
        let mut it = v.split('.');
        Some(format!("{}.{}", it.next()?, it.next()?))
    })?;
    let cli = fpm.parent().map(|d| d.join("php")).filter(|p| p.exists());
    Some(PhpBuild {
        version,
        fpm_binary: fpm.to_path_buf(),
        cli_binary: cli,
        user_registered: false,
    })
}

fn discover_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Common Homebrew / system / Herd locations.
    let fixed = [
        "/opt/homebrew/sbin/php-fpm",
        "/usr/local/sbin/php-fpm",
        "/usr/sbin/php-fpm",
    ];
    for f in fixed {
        let p = PathBuf::from(f);
        if p.exists() {
            out.push(p);
        }
    }
    // Versioned Homebrew kegs: /opt/homebrew/opt/php@8.x/sbin/php-fpm
    for base in ["/opt/homebrew/opt", "/usr/local/opt"] {
        if let Ok(entries) = std::fs::read_dir(base) {
            for e in entries.flatten() {
                let fpm = e.path().join("sbin/php-fpm");
                if fpm.exists() {
                    out.push(fpm);
                }
            }
        }
    }
    out
}

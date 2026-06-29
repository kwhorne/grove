//! Node.js runtime management (Herd-style Node panel).
//!
//! Like PHP, Node ships official self-contained binaries (node + npm + npx) per
//! platform on nodejs.org/dist. Grove downloads them into its own tree so the
//! user never installs Node, nvm or Homebrew separately.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use grove_core::paths::GrovePaths;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("unsupported platform for Node")]
    UnsupportedPlatform,
    #[error("no Node release found for {0}")]
    NoMatch(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, NodeError>;

/// A single installed Node version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBuild {
    /// Major version key (e.g. "22").
    pub major: String,
    /// Full version (e.g. "22.23.1").
    pub version: String,
    pub node_binary: PathBuf,
    pub npm_binary: PathBuf,
}

/// JSON-persisted registry of installed Node builds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NodeRegistry {
    builds: BTreeMap<String, NodeBuild>,
}

impl NodeRegistry {
    fn file(paths: &GrovePaths) -> PathBuf {
        paths.runtimes_dir().join("node-builds.json")
    }

    pub fn load(paths: &GrovePaths) -> Self {
        match std::fs::read_to_string(Self::file(paths)) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, paths: &GrovePaths) -> std::io::Result<()> {
        paths.ensure()?;
        let body = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(Self::file(paths), body)
    }

    pub fn register(&mut self, build: NodeBuild) {
        self.builds.insert(build.major.clone(), build);
    }

    pub fn iter(&self) -> impl Iterator<Item = &NodeBuild> {
        self.builds.values()
    }

    pub fn get(&self, major: &str) -> Option<&NodeBuild> {
        self.builds.get(major)
    }
}

/// `node-v{version}-{slug}` platform slug used in the dist filenames.
fn platform_slug() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        _ => Err(NodeError::UnsupportedPlatform),
    }
}

/// Install a Node version. `req` may be a major ("22") or exact ("22.23.1").
pub fn install(
    paths: &GrovePaths,
    registry: &mut NodeRegistry,
    req: &str,
    progress: impl Fn(&str),
) -> Result<NodeBuild> {
    let slug = platform_slug()?;
    progress(&format!("resolving Node {req}…"));
    let version = resolve_version(req)?;
    let major = version.split('.').next().unwrap_or(&version).to_string();

    let filename = format!("node-v{version}-{slug}.tar.gz");
    let url = format!("https://nodejs.org/dist/v{version}/{filename}");

    let dest = paths.runtimes_dir().join("node").join(&major);
    std::fs::create_dir_all(&dest)?;

    progress(&format!("downloading {filename}…"));
    let bytes = http_get(&url)?;
    progress("extracting…");
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    tar::Archive::new(decoder).unpack(&dest)?;

    let root = dest.join(format!("node-v{version}-{slug}")).join("bin");
    let build = NodeBuild {
        major: major.clone(),
        version: version.clone(),
        node_binary: root.join("node"),
        npm_binary: root.join("npm"),
    };
    registry.register(build.clone());
    registry.save(paths)?;
    progress(&format!("installed Node v{version}"));
    Ok(build)
}

/// Resolve a major or exact version to a concrete release via the dist index.
fn resolve_version(req: &str) -> Result<String> {
    // Exact x.y.z — use as-is.
    if req.split('.').count() == 3 {
        return Ok(req.to_string());
    }
    let index = http_get_string("https://nodejs.org/dist/index.json")?;
    let versions: serde_json::Value =
        serde_json::from_str(&index).map_err(|e| NodeError::Http(e.to_string()))?;
    let prefix = format!("v{}.", req.trim_start_matches('v'));
    // index.json is newest-first, so the first match is the latest.
    let found = versions
        .as_array()
        .and_then(|arr| {
            arr.iter().find_map(|r| {
                let v = r.get("version")?.as_str()?;
                v.starts_with(&prefix)
                    .then(|| v.trim_start_matches('v').to_string())
            })
        })
        .ok_or_else(|| NodeError::NoMatch(req.to_string()))?;
    Ok(found)
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| NodeError::Http(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(512 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn http_get_string(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| NodeError::Http(e.to_string()))?;
    resp.into_string().map_err(NodeError::Io)
}

/// Major versions Grove offers in the GUI (current LTS + recent lines).
pub const OFFERED_MAJORS: &[&str] = &["24", "22", "20", "18"];

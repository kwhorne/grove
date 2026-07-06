//! A fully resolved site — the runtime view the proxy/DNS layers consume.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::driver::{Driver, DriverPlan};

/// How a resolved site was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteKind {
    /// A subdirectory of a parked directory.
    Parked,
    /// An explicit `link`ed / configured site.
    Linked,
}

/// A site after merging config + parked discovery + driver detection.
///
/// This is what gets handed to the proxy on each request and serialized over
/// IPC / `--json` for the GUI and elyra-conductor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSite {
    pub name: String,
    pub hostname: String,
    pub path: PathBuf,
    pub document_root: PathBuf,
    pub driver: Driver,
    pub php: String,
    /// Node.js version pinned for this site, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub secure: bool,
    pub kind: SiteKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub front_controller: Option<PathBuf>,
    /// True when discovered from a Docker/OrbStack container.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub docker: bool,
    /// Container id, for start/stop control (docker sites only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_id: Option<String>,
    /// Whether the backing container is running (docker sites only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub docker_running: bool,
}

impl ResolvedSite {
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        name: String,
        tld: &str,
        path: PathBuf,
        plan: DriverPlan,
        php: String,
        node: Option<String>,
        secure: bool,
        kind: SiteKind,
        proxy_to: Option<String>,
    ) -> Self {
        let hostname = format!("{name}.{tld}");
        ResolvedSite {
            name,
            hostname,
            path,
            document_root: plan.document_root,
            driver: plan.driver,
            php,
            node,
            secure,
            kind,
            proxy_to,
            front_controller: plan.front_controller,
            docker: false,
            docker_id: None,
            docker_running: false,
        }
    }

    /// Build a proxy site discovered from a Docker/OrbStack container.
    pub fn docker_proxy(
        name: String,
        tld: &str,
        upstream: Option<String>,
        id: Option<String>,
        running: bool,
    ) -> Self {
        ResolvedSite {
            hostname: format!("{name}.{tld}"),
            name,
            path: PathBuf::new(),
            document_root: PathBuf::new(),
            driver: crate::driver::Driver::Proxy,
            php: String::new(),
            node: None,
            secure: true,
            kind: SiteKind::Linked,
            proxy_to: upstream,
            front_controller: None,
            docker: true,
            docker_id: id,
            docker_running: running,
        }
    }

    /// The URL a user would open in a browser.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.hostname)
    }
}

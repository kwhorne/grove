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
    pub secure: bool,
    pub kind: SiteKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub front_controller: Option<PathBuf>,
}

impl ResolvedSite {
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        name: String,
        tld: &str,
        path: PathBuf,
        plan: DriverPlan,
        php: String,
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
            secure,
            kind,
            proxy_to,
            front_controller: plan.front_controller,
        }
    }

    /// The URL a user would open in a browser.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.hostname)
    }
}

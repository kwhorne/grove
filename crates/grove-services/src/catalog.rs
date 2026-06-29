//! Catalog of bundled services Grove can download and supervise itself, so the
//! user never has to install MySQL/Redis/Postgres separately (PRD §6.5).
//!
//! Each entry knows where to fetch a portable, self-contained build per
//! platform and how to initialise + run it under `$GROVE_HOME/services`.

/// How a particular service is initialised and launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// Portable prebuilt binaries (initdb + postgres).
    Postgres,
    /// Built from source at install time (`make`), producing `src/redis-server`.
    Redis,
}

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    /// Stable key used in config / CLI (e.g. "postgres").
    pub key: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Grouping shown in the GUI ("Database", "Cache & Queue", …).
    pub category: &'static str,
    pub kind: ServiceKind,
    /// Default listen port.
    pub default_port: u16,
    /// Pinned version that Grove downloads.
    pub version: &'static str,
}

/// Everything Grove can bundle today. PostgreSQL ships self-contained binaries
/// for every platform; more services slot in here as portable builds are added.
pub const CATALOG: &[ServiceSpec] = &[
    ServiceSpec {
        key: "postgres",
        name: "PostgreSQL",
        category: "Database",
        kind: ServiceKind::Postgres,
        default_port: 5432,
        version: "18.4.0",
    },
    ServiceSpec {
        key: "redis",
        name: "Redis",
        category: "Cache & Queue",
        kind: ServiceKind::Redis,
        default_port: 6379,
        version: "7.4.2",
    },
];

pub fn spec(key: &str) -> Option<&'static ServiceSpec> {
    CATALOG.iter().find(|s| s.key == key)
}

/// Resolve the download URL for a service on the current platform.
pub fn download_url(spec: &ServiceSpec) -> Option<String> {
    match spec.kind {
        ServiceKind::Postgres => {
            let triple = postgres_triple()?;
            Some(format!(
                "https://github.com/theseus-rs/postgresql-binaries/releases/download/{v}/postgresql-{v}-{triple}.tar.gz",
                v = spec.version,
            ))
        }
        ServiceKind::Redis => Some(format!(
            "https://github.com/redis/redis/archive/refs/tags/{v}.tar.gz",
            v = spec.version,
        )),
    }
}

/// Top-level directory inside a service's archive.
pub fn archive_root(spec: &ServiceSpec) -> Option<String> {
    match spec.kind {
        ServiceKind::Postgres => {
            let triple = postgres_triple()?;
            Some(format!("postgresql-{}-{triple}", spec.version))
        }
        ServiceKind::Redis => Some(format!("redis-{}", spec.version)),
    }
}

fn postgres_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

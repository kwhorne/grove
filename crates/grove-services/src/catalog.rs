//! Catalog of bundled services Grove can download and supervise itself, so the
//! user never has to install MySQL/Redis/Postgres separately.
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
    /// Portable prebuilt binaries (mysqld --initialize-insecure).
    Mysql,
    /// A single static binary that speaks the MySQL wire protocol and keeps the
    /// whole database in one `.edb` file (`elyrasql serve`). Nothing to
    /// initialise: the file is created on first start.
    ElyraSql,
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
        key: "mysql",
        name: "MySQL",
        category: "Database",
        kind: ServiceKind::Mysql,
        default_port: 3306,
        version: "8.4.3",
    },
    ServiceSpec {
        key: "elyrasql",
        name: "ElyraSQL",
        category: "Database",
        kind: ServiceKind::ElyraSql,
        // Upstream's own default, and clear of MySQL's 3306 so both can run.
        default_port: 3307,
        version: "1.11.1",
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
        // The official release tarball, not GitHub's git-archive of the tag.
        //
        // The archive URL was convenient but unverifiable: GitHub generates those
        // tarballs on demand and does not promise their bytes stay stable — a
        // compression change on their side silently invalidates any pinned hash,
        // as one did across the ecosystem in 2023. `download.redis.io` serves a
        // fixed artefact, and Redis publishes its SHA-256 in the `redis-hashes`
        // repository. Both unpack to `redis-<version>/`, so nothing downstream
        // changes.
        ServiceKind::Redis => Some(format!(
            "https://download.redis.io/releases/redis-{v}.tar.gz",
            v = spec.version,
        )),
        ServiceKind::Mysql => {
            let plat = mysql_platform()?;
            // Use the CDN archive path directly; the dev.mysql.com redirect
            // 403s for non-browser clients.
            Some(format!(
                "https://cdn.mysql.com/archives/mysql-8.4/mysql-{v}-{plat}.tar.gz",
                v = spec.version,
            ))
        }
        // A GitHub release asset with a `.sha256` beside it, like PostgreSQL.
        ServiceKind::ElyraSql => {
            let slug = elyrasql_slug(std::env::consts::OS, std::env::consts::ARCH)?;
            Some(format!(
                "https://github.com/kwhorne/ElyraSQL/releases/download/v{v}/elyrasql-{v}-{slug}.tar.gz",
                v = spec.version,
            ))
        }
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
        ServiceKind::Mysql => Some(format!("mysql-{}-{}", spec.version, mysql_platform()?)),
        ServiceKind::ElyraSql => Some(format!(
            "elyrasql-{}-{}",
            spec.version,
            elyrasql_slug(std::env::consts::OS, std::env::consts::ARCH)?
        )),
    }
}

/// ElyraSQL's `<platform>-<arch>` asset slug. Upstream publishes Linux for both
/// architectures and macOS for Apple silicon only — there is no Intel macOS
/// build, so that combination is unsupported rather than guessed at.
fn elyrasql_slug(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("macos-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        _ => None,
    }
}

/// MySQL's platform slug. Only macOS ships a `.tar.gz`; Linux uses `.tar.xz`
/// (handled in a later iteration), so this returns `None` there for now.
fn mysql_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos14-arm64"),
        ("macos", "x86_64") => Some("macos14-x86_64"),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The asset name is `elyrasql-<version>-<platform>-<arch>.tar.gz` and it
    /// unpacks to a directory of the same name with the binary at its root.
    /// Both halves of that have to agree with the manager, which looks for
    /// `<root>/elyrasql`.
    #[test]
    fn elyrasql_downloads_a_release_asset_and_unpacks_to_its_own_name() {
        let spec = spec("elyrasql").expect("elyrasql is in the catalog");
        // Only meaningful where a build is published; the slug test below
        // covers the matrix without depending on the host.
        if let Some(url) = download_url(spec) {
            assert!(
                url.starts_with("https://github.com/kwhorne/ElyraSQL/releases/download/v1.11.1/elyrasql-1.11.1-"),
                "got {url}"
            );
            assert!(url.ends_with(".tar.gz"), "got {url}");
            let file = url.rsplit('/').next().unwrap();
            let root = archive_root(spec).expect("an archive root");
            assert_eq!(
                format!("{root}.tar.gz"),
                file,
                "the tarball unpacks to its own name"
            );
        }
    }

    #[test]
    fn elyrasql_is_published_for_three_targets_and_not_intel_macos() {
        assert_eq!(elyrasql_slug("macos", "aarch64"), Some("macos-aarch64"));
        assert_eq!(elyrasql_slug("linux", "x86_64"), Some("linux-x86_64"));
        assert_eq!(elyrasql_slug("linux", "aarch64"), Some("linux-aarch64"));
        assert_eq!(
            elyrasql_slug("macos", "x86_64"),
            None,
            "no Intel macOS build upstream"
        );
        assert_eq!(elyrasql_slug("windows", "x86_64"), None);
    }

    /// Two MySQL-protocol servers in one catalog must not want the same port.
    #[test]
    fn elyrasql_and_mysql_default_to_different_ports() {
        let a = spec("mysql").unwrap().default_port;
        let b = spec("elyrasql").unwrap().default_port;
        assert_ne!(a, b);
        assert_eq!(b, 3307, "upstream's own default");
    }
}

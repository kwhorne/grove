//! Which PHP extensions Grove expects a runtime to have — and a way to check.
//!
//! Grove's bundled PHP comes from prebuilt static-php-cli archives, and each
//! archive is compiled with a *fixed* extension set. That set is not a detail:
//! a missing `pdo_sqlite` breaks a fresh Laravel app before the first request,
//! and a missing `mysqli` means the WordPress driver can't connect at all. The
//! prebuilt sets Grove can choose from are also not supersets of each other
//! (see [`crate::install::Variant`]), so "which extensions do we actually have"
//! is a question Grove has to be able to answer out loud.
//!
//! This module is that answer: a curated catalogue of the extensions the PHP
//! ecosystem leans on, each tagged with what breaks without it, plus an audit
//! that diffs the catalogue against a build's real `php -m` output.

use crate::registry::PhpBuild;

/// How much a missing extension hurts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Laravel, WordPress or Composer itself won't run without it — or one of
    /// Grove's own advertised features (bundled MySQL/PostgreSQL, the WordPress
    /// driver) is dead on arrival.
    Required,
    /// Not fatal, but a normal project hits it: a package `require`s it, or a
    /// framework feature silently degrades.
    Recommended,
    /// Wanted by a real slice of the ecosystem, but you know when you need it.
    Optional,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Required => "required",
            Tier::Recommended => "recommended",
            Tier::Optional => "optional",
        }
    }
}

/// One catalogued extension.
#[derive(Debug, Clone, Copy)]
pub struct ExtInfo {
    /// Module name as `php -m` reports it, normalised to lowercase.
    pub name: &'static str,
    pub tier: Tier,
    /// What you lose without it. Written to be readable in a CLI table.
    pub why: &'static str,
}

/// The extensions Grove cares about, most-used first within each tier.
///
/// Deliberately *not* "every extension that exists": the point of the list is
/// that every line is worth acting on. Anything not listed is neither promised
/// nor warned about.
pub const CATALOGUE: &[ExtInfo] = &[
    // ---- Required -------------------------------------------------------
    ExtInfo {
        name: "ctype",
        tier: Tier::Required,
        why: "Laravel core (ext-ctype)",
    },
    ExtInfo {
        name: "curl",
        tier: Tier::Required,
        why: "HTTP client, Composer downloads",
    },
    ExtInfo {
        name: "dom",
        tier: Tier::Required,
        why: "Laravel core (ext-dom), PHPUnit",
    },
    ExtInfo {
        name: "fileinfo",
        tier: Tier::Required,
        why: "upload MIME detection, Storage",
    },
    ExtInfo {
        name: "filter",
        tier: Tier::Required,
        why: "Laravel core (ext-filter)",
    },
    ExtInfo {
        name: "hash",
        tier: Tier::Required,
        why: "hashing, signed URLs, sessions",
    },
    ExtInfo {
        name: "iconv",
        tier: Tier::Required,
        why: "charset conversion (ext-iconv)",
    },
    ExtInfo {
        name: "json",
        tier: Tier::Required,
        why: "config, API payloads, Composer",
    },
    ExtInfo {
        name: "mbstring",
        tier: Tier::Required,
        why: "Laravel core (ext-mbstring)",
    },
    ExtInfo {
        name: "openssl",
        tier: Tier::Required,
        why: "encryption, TLS, encrypted cookies",
    },
    ExtInfo {
        name: "pcre",
        tier: Tier::Required,
        why: "routing, validation — regex everywhere",
    },
    ExtInfo {
        name: "pdo",
        tier: Tier::Required,
        why: "every database driver builds on it",
    },
    ExtInfo {
        name: "pdo_sqlite",
        tier: Tier::Required,
        why: "Laravel's default DB and its test suites",
    },
    ExtInfo {
        name: "pdo_mysql",
        tier: Tier::Required,
        why: "Grove's bundled MySQL",
    },
    ExtInfo {
        name: "pdo_pgsql",
        tier: Tier::Required,
        why: "Grove's bundled PostgreSQL",
    },
    ExtInfo {
        name: "mysqli",
        tier: Tier::Required,
        why: "WordPress — its only MySQL driver",
    },
    ExtInfo {
        name: "phar",
        tier: Tier::Required,
        why: "Composer, cpx, packaged tools",
    },
    ExtInfo {
        name: "session",
        tier: Tier::Required,
        why: "Laravel core (ext-session)",
    },
    ExtInfo {
        name: "simplexml",
        tier: Tier::Required,
        why: "PHPUnit config, feed/XML packages",
    },
    ExtInfo {
        name: "tokenizer",
        tier: Tier::Required,
        why: "Laravel core (ext-tokenizer)",
    },
    ExtInfo {
        name: "xml",
        tier: Tier::Required,
        why: "Laravel core (ext-xml)",
    },
    ExtInfo {
        name: "xmlreader",
        tier: Tier::Required,
        why: "PHPUnit, XML-heavy packages",
    },
    ExtInfo {
        name: "xmlwriter",
        tier: Tier::Required,
        why: "PHPUnit coverage reports",
    },
    ExtInfo {
        name: "zip",
        tier: Tier::Required,
        why: "Composer unpacks packages with it",
    },
    ExtInfo {
        name: "zlib",
        tier: Tier::Required,
        why: "compression, Composer transport",
    },
    // ---- Recommended ----------------------------------------------------
    ExtInfo {
        name: "intl",
        tier: Tier::Recommended,
        why: "Laravel Number/dates, Filament, Nova",
    },
    ExtInfo {
        name: "opcache",
        tier: Tier::Recommended,
        why: "opcode cache — the single biggest FPM win",
    },
    ExtInfo {
        name: "pcntl",
        tier: Tier::Recommended,
        why: "queue workers, Horizon, Octane, grove dev",
    },
    ExtInfo {
        name: "posix",
        tier: Tier::Recommended,
        why: "process supervision and signals",
    },
    ExtInfo {
        name: "sockets",
        tier: Tier::Recommended,
        why: "Reverb, websockets, raw stream I/O",
    },
    ExtInfo {
        name: "redis",
        tier: Tier::Recommended,
        why: "phpredis for Grove's bundled Redis",
    },
    ExtInfo {
        name: "gd",
        tier: Tier::Recommended,
        why: "image resizing; WordPress media",
    },
    ExtInfo {
        name: "exif",
        tier: Tier::Recommended,
        why: "image orientation/metadata on upload",
    },
    ExtInfo {
        name: "bcmath",
        tier: Tier::Recommended,
        why: "exact decimal maths (money, tax)",
    },
    ExtInfo {
        name: "gmp",
        tier: Tier::Recommended,
        why: "big integers, crypto packages",
    },
    ExtInfo {
        name: "sodium",
        tier: Tier::Recommended,
        why: "modern crypto (sodium_*), passkeys",
    },
    ExtInfo {
        name: "readline",
        tier: Tier::Recommended,
        why: "history/editing in tinker and cpx tinker",
    },
    ExtInfo {
        name: "apcu",
        tier: Tier::Recommended,
        why: "in-process cache (Laravel apc store)",
    },
    ExtInfo {
        name: "xsl",
        tier: Tier::Recommended,
        why: "XSLT transforms (ext-xsl)",
    },
    ExtInfo {
        name: "soap",
        tier: Tier::Recommended,
        why: "SOAP integrations",
    },
    ExtInfo {
        name: "ftp",
        tier: Tier::Recommended,
        why: "Storage FTP disk",
    },
    ExtInfo {
        name: "bz2",
        tier: Tier::Recommended,
        why: "bzip2 archives",
    },
    ExtInfo {
        name: "calendar",
        tier: Tier::Recommended,
        why: "date conversion helpers",
    },
    // ---- Optional -------------------------------------------------------
    ExtInfo {
        name: "imagick",
        tier: Tier::Optional,
        why: "richer image processing than gd",
    },
    ExtInfo {
        name: "ldap",
        tier: Tier::Optional,
        why: "LDAP / Active Directory auth",
    },
    ExtInfo {
        name: "ffi",
        tier: Tier::Optional,
        why: "FFI-based packages",
    },
    ExtInfo {
        name: "igbinary",
        tier: Tier::Optional,
        why: "faster redis/session serialisation",
    },
    ExtInfo {
        name: "gettext",
        tier: Tier::Optional,
        why: "gettext translations (WordPress themes)",
    },
    ExtInfo {
        name: "dba",
        tier: Tier::Optional,
        why: "dbm-style key/value stores",
    },
    ExtInfo {
        name: "shmop",
        tier: Tier::Optional,
        why: "shared memory",
    },
    ExtInfo {
        name: "sysvsem",
        tier: Tier::Optional,
        why: "System V semaphores",
    },
    ExtInfo {
        name: "sysvshm",
        tier: Tier::Optional,
        why: "System V shared memory",
    },
    ExtInfo {
        name: "sysvmsg",
        tier: Tier::Optional,
        why: "System V message queues",
    },
    ExtInfo {
        name: "swoole",
        tier: Tier::Optional,
        why: "Laravel Octane (Swoole server)",
    },
    ExtInfo {
        name: "event",
        tier: Tier::Optional,
        why: "libevent loop for async packages",
    },
    ExtInfo {
        name: "imap",
        tier: Tier::Optional,
        why: "IMAP mailboxes (removed from PHP 8.4 core)",
    },
    ExtInfo {
        name: "xdebug",
        tier: Tier::Optional,
        why: "step-debugging (see grove debug status)",
    },
    ExtInfo {
        name: "tidy",
        tier: Tier::Optional,
        why: "HTML cleanup",
    },
    ExtInfo {
        name: "yaml",
        tier: Tier::Optional,
        why: "YAML parsing without a userland parser",
    },
    ExtInfo {
        name: "memcached",
        tier: Tier::Optional,
        why: "Memcached cache store",
    },
    ExtInfo {
        name: "mongodb",
        tier: Tier::Optional,
        why: "MongoDB driver",
    },
    ExtInfo {
        name: "zstd",
        tier: Tier::Optional,
        why: "zstd compression",
    },
    ExtInfo {
        name: "opentelemetry",
        tier: Tier::Optional,
        why: "OpenTelemetry auto-instrumentation",
    },
];

/// The extension set Grove builds *its own* PHP with.
///
/// This list exists because neither prebuilt static-php-cli archive is enough on
/// its own: `common` has no `intl` or `mysqli`, `bulk` has those but drops
/// `pdo_sqlite` and `pdo_pgsql`. Grove needs all of them, so it builds the union
/// (see `.github/workflows/php-build.yml`).
///
/// These are **static-php-cli** extension names (`spc dev:extensions`), which are
/// not always the `php -m` module names [`CATALOGUE`] compares against —
/// `mbregex` and `libxml` are separate entries here but fold into `mbstring` and
/// `libxml` at runtime, and `opcache` reports itself as `Zend OPcache`.
///
/// Everything here is already proven to build by one of the two prebuilt sets,
/// with one exception: `igbinary`, which neither ships. It is a dependency-free
/// PECL extension and the standard companion to `redis`, so it comes along.
pub const BUILD_SET: &[&str] = &[
    "apcu",
    "bcmath",
    "bz2",
    "calendar",
    "ctype",
    "curl",
    "dom",
    "exif",
    "fileinfo",
    "filter",
    "ftp",
    "gd",
    "gmp",
    "iconv",
    "igbinary",
    "intl",
    "libxml",
    "mbregex",
    "mbstring",
    "mysqli",
    "mysqlnd",
    "opcache",
    "openssl",
    "pcntl",
    "pdo",
    "pdo_mysql",
    "pdo_pgsql",
    "pdo_sqlite",
    "pgsql",
    "phar",
    "posix",
    "readline",
    "redis",
    "session",
    "shmop",
    "simplexml",
    "soap",
    "sockets",
    "sodium",
    "sqlite3",
    "sysvmsg",
    "sysvsem",
    "sysvshm",
    "tokenizer",
    "xml",
    "xmlreader",
    "xmlwriter",
    "xsl",
    "zip",
    "zlib",
];

/// A static-php-cli `craft.yml` that builds [`BUILD_SET`] for `php_version`.
///
/// Generated rather than committed so the extension list has exactly one home:
/// the CI workflow writes this file with `grove php craft`, and the same binary
/// that audits a build is the one that specified it.
pub fn craft_yml(php_version: &str) -> String {
    format!(
        "# Generated by `grove php craft` — do not edit by hand.\n\
         # Grove's PHP: the union of static-php-cli's `common` and `bulk` sets,\n\
         # because neither one has both the PDO SQLite/PostgreSQL drivers and\n\
         # intl/mysqli/sodium/readline/apcu/xsl.\n\
         php-version: {php_version}\n\
         extensions: {extensions}\n\
         sapi: cli,fpm\n\
         debug: true\n\
         download-options:\n\
        \x20 prefer-pre-built: true\n\
        \x20 retry: 5\n\
         craft-options:\n\
        \x20 doctor: true\n\
        \x20 download: true\n\
        \x20 build: true\n",
        php_version = php_version,
        extensions = BUILD_SET.join(","),
    )
}

/// Normalise a module name so `php -m` output and catalogue entries compare.
///
/// `php -m` prints registration names, not the lowercase ext slugs people write
/// in `composer.json`: `PDO`, `Phar`, `SimpleXML`, `Zend OPcache`. Composer's
/// own spelling (`ext-intl`) shows up in user-supplied lists, so strip that too.
pub fn normalise(module: &str) -> String {
    let m = module
        .trim()
        .trim_start_matches("ext-")
        .to_ascii_lowercase();
    match m.as_str() {
        "zend opcache" => "opcache".to_string(),
        other => other.to_string(),
    }
}

/// The result of comparing a build's loaded modules against [`CATALOGUE`].
#[derive(Debug, Clone, Default)]
pub struct Audit {
    /// Every module the build actually loads, normalised and sorted.
    pub loaded: Vec<String>,
    /// Catalogued extensions the build has.
    pub present: Vec<ExtInfo>,
    /// Catalogued extensions the build lacks.
    pub missing: Vec<ExtInfo>,
}

impl Audit {
    /// Missing entries at exactly `tier`.
    pub fn missing_at(&self, tier: Tier) -> Vec<ExtInfo> {
        self.missing
            .iter()
            .copied()
            .filter(|e| e.tier == tier)
            .collect()
    }

    /// Nothing required is absent.
    pub fn is_healthy(&self) -> bool {
        self.missing_at(Tier::Required).is_empty()
    }

    /// One-line summary for `grove php list` / `grove doctor`.
    pub fn summary(&self) -> String {
        let required = self.missing_at(Tier::Required).len();
        let recommended = self.missing_at(Tier::Recommended).len();
        if self.loaded.is_empty() {
            return "could not read `php -m`".to_string();
        }
        if required == 0 && recommended == 0 {
            return format!("{} modules, nothing missing", self.loaded.len());
        }
        let mut parts = Vec::new();
        if required > 0 {
            parts.push(format!("{required} required missing"));
        }
        if recommended > 0 {
            parts.push(format!("{recommended} recommended missing"));
        }
        format!("{} modules, {}", self.loaded.len(), parts.join(", "))
    }
}

/// Diff a list of loaded module names (raw `php -m` lines) against the catalogue.
pub fn audit_modules(modules: &[String]) -> Audit {
    let mut loaded: Vec<String> = modules.iter().map(|m| normalise(m)).collect();
    loaded.sort();
    loaded.dedup();

    // A build with no readable module list tells us nothing; don't report every
    // catalogued extension as missing when the real problem is that `php -m`
    // failed.
    if loaded.is_empty() {
        return Audit::default();
    }

    let (present, missing) = CATALOGUE
        .iter()
        .copied()
        .partition(|e| loaded.iter().any(|m| m == e.name));
    Audit {
        loaded,
        present,
        missing,
    }
}

/// Audit a registered build by asking its binary what it loads.
pub fn audit_build(build: &PhpBuild) -> Audit {
    audit_modules(&build.extensions())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_handles_php_m_spellings() {
        assert_eq!(normalise("Zend OPcache"), "opcache");
        assert_eq!(normalise("PDO"), "pdo");
        assert_eq!(normalise("SimpleXML"), "simplexml");
        assert_eq!(normalise("  Phar "), "phar");
        assert_eq!(normalise("ext-intl"), "intl");
    }

    #[test]
    fn catalogue_is_normalised_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for e in CATALOGUE {
            assert_eq!(e.name, normalise(e.name), "{} is not normalised", e.name);
            assert!(seen.insert(e.name), "{} listed twice", e.name);
            assert!(!e.why.is_empty());
        }
    }

    /// The exact module list of the `common` static build Grove installs today
    /// (php 8.5.8, macos-aarch64). Kept verbatim so the audit is tested against
    /// reality rather than against our idea of reality.
    fn common_build_modules() -> Vec<String> {
        [
            "bcmath",
            "bz2",
            "calendar",
            "Core",
            "ctype",
            "curl",
            "date",
            "dom",
            "exif",
            "fileinfo",
            "filter",
            "ftp",
            "gd",
            "gmp",
            "hash",
            "iconv",
            "json",
            "lexbor",
            "libxml",
            "mbstring",
            "mysqlnd",
            "openssl",
            "pcntl",
            "pcre",
            "PDO",
            "pdo_mysql",
            "pdo_pgsql",
            "pdo_sqlite",
            "pgsql",
            "Phar",
            "posix",
            "random",
            "redis",
            "Reflection",
            "session",
            "SimpleXML",
            "soap",
            "sockets",
            "SPL",
            "sqlite3",
            "standard",
            "tokenizer",
            "uri",
            "xml",
            "xmlreader",
            "xmlwriter",
            "Zend OPcache",
            "zip",
            "zlib",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Same, for the `bulk` build — which trades the PDO SQLite/PostgreSQL
    /// drivers for intl, mysqli and friends.
    fn bulk_build_modules() -> Vec<String> {
        [
            "apcu",
            "bcmath",
            "bz2",
            "calendar",
            "Core",
            "ctype",
            "curl",
            "date",
            "dba",
            "dom",
            "event",
            "exif",
            "fileinfo",
            "filter",
            "ftp",
            "gd",
            "gmp",
            "hash",
            "iconv",
            "imagick",
            "imap",
            "intl",
            "json",
            "lexbor",
            "libxml",
            "mbstring",
            "mysqli",
            "mysqlnd",
            "openssl",
            "opentelemetry",
            "pcntl",
            "pcre",
            "PDO",
            "pdo_mysql",
            "pgsql",
            "Phar",
            "posix",
            "protobuf",
            "random",
            "readline",
            "redis",
            "Reflection",
            "session",
            "shmop",
            "SimpleXML",
            "soap",
            "sockets",
            "sodium",
            "SPL",
            "sqlite3",
            "standard",
            "swoole",
            "sysvmsg",
            "sysvsem",
            "sysvshm",
            "tokenizer",
            "uri",
            "xml",
            "xmlreader",
            "xmlwriter",
            "xsl",
            "Zend OPcache",
            "zip",
            "zlib",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn common_build_is_missing_intl_and_mysqli() {
        let audit = audit_modules(&common_build_modules());
        let missing: Vec<_> = audit.missing.iter().map(|e| e.name).collect();
        assert!(
            missing.contains(&"mysqli"),
            "expected mysqli missing: {missing:?}"
        );
        assert!(
            missing.contains(&"intl"),
            "expected intl missing: {missing:?}"
        );
        assert!(missing.contains(&"sodium"));
        assert!(missing.contains(&"readline"));
        // …but it does have the PDO drivers Laravel and Grove's services need.
        let present: Vec<_> = audit.present.iter().map(|e| e.name).collect();
        assert!(present.contains(&"pdo_sqlite"));
        assert!(present.contains(&"pdo_pgsql"));
        assert!(
            present.contains(&"opcache"),
            "opcache is on despite not being listed in build-extensions.json"
        );
    }

    #[test]
    fn bulk_build_is_missing_the_pdo_drivers() {
        let audit = audit_modules(&bulk_build_modules());
        let missing: Vec<_> = audit.missing.iter().map(|e| e.name).collect();
        assert!(
            missing.contains(&"pdo_sqlite"),
            "expected pdo_sqlite missing: {missing:?}"
        );
        assert!(missing.contains(&"pdo_pgsql"));
        let present: Vec<_> = audit.present.iter().map(|e| e.name).collect();
        assert!(present.contains(&"intl"));
        assert!(present.contains(&"mysqli"));
    }

    /// Neither prebuilt variant is a superset of the other, so neither is
    /// "required-clean". This is the fact that justifies `grove php ext`.
    #[test]
    fn neither_prebuilt_variant_satisfies_every_required_extension() {
        assert!(!audit_modules(&common_build_modules()).is_healthy());
        assert!(!audit_modules(&bulk_build_modules()).is_healthy());
    }

    /// `php -m` of a real build from [`BUILD_SET`] — static-php-cli 2.8.5,
    /// PHP 8.4.24, linux-musl-aarch64. Recorded verbatim so this test fails if
    /// the build set ever stops delivering what it promises.
    fn grove_build_modules() -> Vec<String> {
        [
            "apcu",
            "bcmath",
            "bz2",
            "calendar",
            "Core",
            "ctype",
            "curl",
            "date",
            "dom",
            "exif",
            "fileinfo",
            "filter",
            "ftp",
            "gd",
            "gmp",
            "hash",
            "iconv",
            "igbinary",
            "intl",
            "json",
            "libxml",
            "mbstring",
            "mysqli",
            "mysqlnd",
            "openssl",
            "pcntl",
            "pcre",
            "PDO",
            "pdo_mysql",
            "pdo_pgsql",
            "pdo_sqlite",
            "pgsql",
            "Phar",
            "posix",
            "random",
            "readline",
            "redis",
            "Reflection",
            "session",
            "shmop",
            "SimpleXML",
            "soap",
            "sockets",
            "sodium",
            "SPL",
            "sqlite3",
            "standard",
            "sysvmsg",
            "sysvsem",
            "sysvshm",
            "tokenizer",
            "xml",
            "xmlreader",
            "xmlwriter",
            "xsl",
            "Zend OPcache",
            "zip",
            "zlib",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// The whole reason Grove builds its own PHP: nothing required is missing,
    /// and the six extensions `common` lacks are all there alongside the two
    /// PDO drivers `bulk` lacks.
    #[test]
    fn grove_build_has_no_required_or_recommended_gaps() {
        let audit = audit_modules(&grove_build_modules());
        assert!(
            audit.is_healthy(),
            "required missing: {:?}",
            audit
                .missing_at(Tier::Required)
                .iter()
                .map(|e| e.name)
                .collect::<Vec<_>>()
        );
        let present: Vec<&str> = audit.present.iter().map(|e| e.name).collect();
        for name in [
            "intl",
            "mysqli",
            "sodium",
            "readline",
            "apcu",
            "xsl",
            "pdo_sqlite",
            "pdo_pgsql",
            "opcache",
            "igbinary",
        ] {
            assert!(present.contains(&name), "{name} missing: {present:?}");
        }
        let recommended_gaps: Vec<&str> = audit
            .missing_at(Tier::Recommended)
            .iter()
            .map(|e| e.name)
            .collect();
        assert!(
            recommended_gaps.is_empty(),
            "recommended gaps left in Grove's own build: {recommended_gaps:?}"
        );
    }

    /// The build set is the whole point of building our own PHP: if it doesn't
    /// cover everything the audit calls required, we've shipped the same hole
    /// with extra steps.
    #[test]
    fn build_set_covers_every_required_extension() {
        // `php -m` names that the build set produces under a different spelling.
        let aliases = |name: &str| match name {
            "pcre" | "hash" | "json" => true, // always compiled into PHP
            _ => false,
        };
        let built: std::collections::BTreeSet<&str> = BUILD_SET.iter().copied().collect();
        let uncovered: Vec<&str> = CATALOGUE
            .iter()
            .filter(|e| e.tier == Tier::Required)
            .map(|e| e.name)
            .filter(|n| !built.contains(n) && !aliases(n))
            .collect();
        assert!(
            uncovered.is_empty(),
            "required but not built: {uncovered:?}"
        );
    }

    /// The six extensions this build set was created to add, spelled out so a
    /// future trim of the list has to be deliberate.
    #[test]
    fn build_set_includes_what_the_prebuilt_sets_each_miss() {
        for name in [
            // absent from `common`
            "intl",
            "mysqli",
            "sodium",
            "readline",
            "apcu",
            "xsl",
            // absent from `bulk`
            "pdo_sqlite",
            "pdo_pgsql",
        ] {
            assert!(BUILD_SET.contains(&name), "{name} missing from BUILD_SET");
        }
    }

    #[test]
    fn build_set_is_sorted_and_unique() {
        let mut sorted = BUILD_SET.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, BUILD_SET, "BUILD_SET must stay sorted and unique");
    }

    #[test]
    fn craft_yml_is_parseable_and_carries_the_set() {
        let yml = craft_yml("8.4");
        assert!(yml.contains("php-version: 8.4"));
        let line = yml
            .lines()
            .find(|l| l.starts_with("extensions: "))
            .expect("extensions line");
        let listed: Vec<&str> = line["extensions: ".len()..].split(',').collect();
        assert_eq!(listed, BUILD_SET);
        assert!(yml.contains("sapi: cli,fpm"));
        // Nested keys must be indented, or spc reads a flat mapping.
        assert!(yml.contains("\n  prefer-pre-built: true\n"), "{yml}");
        assert!(yml.contains("\n  doctor: true\n"), "{yml}");
    }

    #[test]
    fn empty_module_list_is_not_reported_as_everything_missing() {
        let audit = audit_modules(&[]);
        assert!(audit.missing.is_empty());
        assert!(audit.present.is_empty());
        assert_eq!(audit.summary(), "could not read `php -m`");
    }

    #[test]
    fn summary_counts_only_missing_tiers() {
        let mut modules = common_build_modules();
        modules.extend(
            ["intl", "mysqli", "sodium", "readline", "apcu", "xsl"]
                .iter()
                .map(|s| s.to_string()),
        );
        let audit = audit_modules(&modules);
        assert!(
            audit.is_healthy(),
            "missing: {:?}",
            audit.missing_at(Tier::Required)
        );
        assert!(audit.summary().contains("modules"));
    }
}

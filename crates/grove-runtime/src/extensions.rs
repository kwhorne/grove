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

/// Optional libraries the extensions above need asking for by name.
///
/// static-php-cli builds an extension's `lib-depends` and nothing else. `gd` depends on
/// `zlib` and `libpng` alone, and lists `libjpeg`, `freetype`, `libwebp` and `libavif` as
/// *suggestions* — so a build that names only `gd` produces a GD that cannot read or
/// write a JPEG. Nothing says so: `php -m` lists `gd`, `grove php ext` reports nothing
/// missing, and `imagejpeg()` is simply not a function.
///
/// Found from a Laravel suite where thirteen tests failed on
/// `imagejpeg function is not defined`, against a PHP whose `phpinfo()` cheerfully said
/// "GD Support => enabled". The extension audit cannot catch this class of gap, because
/// the module is present — it is the inside of the module that is short.
const BUILD_LIBS: &[&str] = &[
    // The three image formats a web application actually receives. A photograph from a
    // phone is a JPEG; a screenshot pasted into an issue is a PNG; everything that cares
    // about bytes over the wire is a WebP.
    "libjpeg",
    "libwebp",
    // Text drawn into an image — thumbnails with labels, generated avatars, captchas.
    "freetype",
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
         #\n\
         # `libs` is separate on purpose: spc builds an extension's lib-depends and not\n\
         # its lib-suggests, so gd without this line is a gd that cannot read a JPEG.\n\
         php-version: {php_version}\n\
         extensions: {extensions}\n\
         libs: {libs}\n\
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
        libs = BUILD_LIBS.join(","),
    )
}

/// Something a module is supposed to be able to do, and the PHP that proves it.
///
/// `php -m` answers "is the module loaded", which is a different question from "does
/// the module work". An extension is compiled against optional libraries, and leaving
/// one out produces a module that loads, registers, reports itself present, and cannot
/// do the thing people installed it for.
///
/// That is not hypothetical. Grove's own PHP shipped a `gd` built without libjpeg for
/// however long: `php -m` listed gd, `phpinfo()` said "GD Support => enabled", this
/// audit said "60 modules, nothing missing", and `imagejpeg()` did not exist. The
/// symptom reached the user as thirteen failing tests in an unrelated project on a
/// machine where nobody had changed anything.
///
/// So the catalogue above says which modules should be here, and this one says what
/// each should be able to do once it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// The module it belongs to, as [`CATALOGUE`] names it.
    pub ext: &'static str,
    /// What it is, in the words somebody would use to miss it.
    pub name: &'static str,
    /// A PHP expression that is truthy when the capability is there.
    ///
    /// Evaluated inside the build being audited, because that is the only place that
    /// can answer. Written defensively: it runs against a PHP that may be missing the
    /// very thing being asked about, so nothing here may fatal.
    pub probe: &'static str,
    /// What its absence costs, in the same register as `ExtInfo::why`.
    pub cost: &'static str,
}

/// The capabilities worth probing.
///
/// Deliberately short. Every entry is a case where a module genuinely ships hollow —
/// an optional library left out of the build — not a list of every function PHP has.
/// A probe that can never fail is a probe that only costs time.
pub const CAPABILITIES: &[Capability] = &[
    // The one that started this. gd's lib-depends are zlib and libpng; everything
    // below is a lib-suggests, which static builds leave out unless asked.
    Capability {
        ext: "gd",
        name: "JPEG images",
        probe: "function_exists('imagejpeg')",
        cost: "the commonest upload there is — every photograph from a phone",
    },
    Capability {
        ext: "gd",
        name: "WebP images",
        probe: "function_exists('imagewebp')",
        cost: "the format thumbnails should be written in",
    },
    Capability {
        ext: "gd",
        name: "FreeType text",
        probe: "function_exists('imagettftext')",
        cost: "text drawn into an image — labelled thumbnails, generated avatars",
    },
    // A curl without TLS resolves and connects and cannot speak to anything https.
    Capability {
        ext: "curl",
        name: "TLS",
        probe: "(bool) (curl_version()['features'] & CURL_VERSION_SSL)",
        cost: "every https request, which is all of them",
    },
    // PDO is the module; a driver is what talks to a database. The module is present
    // either way, and `grove php ext` could not tell them apart.
    Capability {
        ext: "pdo",
        name: "the SQLite driver",
        probe: "in_array('sqlite', PDO::getAvailableDrivers(), true)",
        cost: "Laravel's default test database",
    },
    Capability {
        ext: "pdo",
        name: "the MySQL driver",
        probe: "in_array('mysql', PDO::getAvailableDrivers(), true)",
        cost: "the database most of these applications actually run on",
    },
    // intl loads without ICU data and then formats nothing correctly.
    Capability {
        ext: "intl",
        name: "ICU data",
        probe: "class_exists('Collator') && Collator::create('en') !== null",
        cost: "dates, numbers and sorting in every locale — silently wrong, not absent",
    },
];

/// Which capabilities are worth asking this build about.
///
/// A separate function because the choosing is the part worth testing, and the asking
/// needs a binary. Written inline first, where no test could reach it without one --
/// and a fault injection that removed the filter altogether changed nothing anybody
/// could see.
pub fn capabilities_for(present: &[ExtInfo]) -> Vec<Capability> {
    CAPABILITIES
        .iter()
        .copied()
        .filter(|c| present.iter().any(|e| e.name == c.ext))
        .collect()
}

/// Ask a build which of [`CAPABILITIES`] it is missing.
///
/// One `php -r` for the lot: a probe per capability would be seven processes to answer
/// a question nobody asked out loud, and this already runs behind `grove php list`.
///
/// Only the modules the build actually has are probed. Asking a PHP without gd whether
/// it can write a JPEG produces a second complaint about the same absence, and the
/// module being missing is the more useful way to say it.
pub fn probe_capabilities(build: &PhpBuild, present: &[ExtInfo]) -> Vec<Capability> {
    let wanted = capabilities_for(present);

    if wanted.is_empty() {
        return Vec::new();
    }

    let Some(cli) = build.cli_binary.as_ref().or(Some(&build.fpm_binary)) else {
        return Vec::new();
    };

    /*
     * Each probe in its own try/catch, printing one line.
     *
     * A single expression would stop at the first capability that throws rather than
     * returning false -- and a build missing one of these is exactly the build whose
     * probe is most likely to fatal rather than answer.
     */
    let mut script = String::new();
    for (i, c) in wanted.iter().enumerate() {
        script.push_str(&format!(
            "try {{ echo {i}, ':', ({}) ? '1' : '0', \"\\n\"; }} catch (\\Throwable $e) {{ echo {i}, \":0\\n\"; }}\n",
            c.probe,
        ));
    }

    let Some(out) = crate::probe::output(cli, &["-r", &script]) else {
        return Vec::new();
    };

    let answers = String::from_utf8_lossy(&out.stdout);

    /*
     * A probe that produced no answer is not a missing capability.
     *
     * The binary may be one that cannot run `-r` at all, and reporting seven holes
     * because a process failed to start would be the same class of lie this whole
     * module exists to stop telling.
     */
    let mut seen = std::collections::BTreeMap::new();
    for line in answers.lines() {
        if let Some((i, v)) = line.split_once(':') {
            if let Ok(i) = i.trim().parse::<usize>() {
                seen.insert(i, v.trim() == "1");
            }
        }
    }

    if seen.is_empty() {
        return Vec::new();
    }

    wanted
        .iter()
        .enumerate()
        .filter(|(i, _)| seen.get(i) == Some(&false))
        .map(|(_, c)| *c)
        .collect()
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
    /// Things a present module cannot do — see [`CAPABILITIES`].
    ///
    /// Empty when the modules were read but the capabilities were not probed, which is
    /// every caller that only has a module list. `audit_build` fills it.
    pub hollow: Vec<Capability>,
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

    /// Nothing required is absent, and nothing present is hollow.
    ///
    /// A module that is there and cannot do its job is not a healthy build. It used to
    /// count as one, which is how a gd without JPEG passed for months.
    pub fn is_healthy(&self) -> bool {
        self.missing_at(Tier::Required).is_empty() && self.hollow.is_empty()
    }

    /// One-line summary for `grove php list` / `grove doctor`.
    pub fn summary(&self) -> String {
        let required = self.missing_at(Tier::Required).len();
        let recommended = self.missing_at(Tier::Recommended).len();
        if self.loaded.is_empty() {
            return "could not read `php -m`".to_string();
        }
        if required == 0 && recommended == 0 && self.hollow.is_empty() {
            return format!("{} modules, nothing missing", self.loaded.len());
        }
        let mut parts = Vec::new();
        if !self.hollow.is_empty() {
            /*
             * First, and worded as a hole rather than a count of absences.
             *
             * This line used to read "nothing missing" over a gd that could not write a
             * JPEG, because nothing *was* missing by the only measure it had. A module
             * that is present and hollow is the harder failure to find, so it leads.
             */
            parts.push(format!(
                "{} not working ({})",
                self.hollow.len(),
                self.hollow
                    .iter()
                    .map(|c| c.name)
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
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
        // Not probed here: this takes a module list, and a list cannot be asked
        // questions. `audit_build` has the binary and fills it in.
        hollow: Vec::new(),
    }
}

/// Audit a registered build by asking its binary what it loads — and what it can do.
pub fn audit_build(build: &PhpBuild) -> Audit {
    let mut audit = audit_modules(&build.extensions());

    // Only worth asking if the module list came back at all; a build that cannot answer
    // `php -m` will not answer `php -r` either, and two silences are not two problems.
    if !audit.loaded.is_empty() {
        audit.hollow = probe_capabilities(build, &audit.present);
    }

    audit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hollow module is not a healthy build, and the summary has to say so.
    ///
    /// The exact line this replaces: "60 modules, nothing missing", printed over a gd
    /// that could not write a JPEG, for as long as the audit only counted names.
    #[test]
    fn a_present_but_hollow_module_is_not_nothing_missing() {
        // A build that genuinely has everything, so "nothing missing" is the honest
        // baseline and the only thing the hollow entry changes is the hollowness.
        let everything: Vec<String> = CATALOGUE.iter().map(|e| e.name.to_string()).collect();
        let mut audit = audit_modules(&everything);

        assert!(audit.summary().ends_with("nothing missing"), "{}", audit.summary());
        assert!(audit.is_healthy());

        audit.hollow = vec![*CAPABILITIES
            .iter()
            .find(|c| c.ext == "gd" && c.name == "JPEG images")
            .expect("the jpeg capability")];

        let summary = audit.summary();

        assert!(
            !summary.contains("nothing missing"),
            "a build that cannot write a JPEG must not read as complete: {summary}"
        );
        assert!(summary.contains("JPEG images"), "{summary}");
        assert!(
            !audit.is_healthy(),
            "a module that is present and cannot do its job is not a healthy build"
        );
    }

    /// Every probe has to be valid PHP that answers for itself.
    ///
    /// A probe with a typo evaluates to a fatal, the catch turns that into `false`, and
    /// the audit reports a hole that is not there — which is the same lie as the one
    /// this module was written to stop, pointing the other way.
    #[test]
    fn every_capability_probe_is_a_single_defensible_expression() {
        for c in CAPABILITIES {
            assert!(
                CATALOGUE.iter().any(|e| e.name == c.ext),
                "{} probes `{}`, which is not in the catalogue — it would never be asked",
                c.name,
                c.ext,
            );
            assert!(!c.probe.contains(';'), "{}: a probe is one expression", c.name);

            /*
             * A probe has to actually ask PHP something.
             *
             * `probe: "true"` is valid PHP, passes every structural check here, and
             * turns the capability into a thing that can never be reported missing --
             * which is precisely the silence this module exists to break. So a probe
             * must name a function, a class or a constant.
             */
            assert!(
                c.probe.contains('(') || c.probe.contains("::") || c.probe.contains('_'),
                "{}: `{}` asks PHP nothing — it would always answer yes",
                c.name,
                c.probe,
            );
            assert!(!c.probe.contains("echo"), "{}: a probe returns, it does not print", c.name);
            assert!(!c.cost.is_empty(), "{}: say what its absence costs", c.name);
        }
    }

    /// Nothing is probed for a module the build does not have.
    ///
    /// Otherwise a PHP without gd is reported twice for one absence — once as a missing
    /// module, three more times as capabilities it was never going to have — and the
    /// useful line is buried under the restatement.
    /// Nothing is asked about a module the build does not have.
    ///
    /// Tested on the choosing rather than through a binary: the version of this that
    /// went through `probe_capabilities` passed with the filter deleted, because a
    /// build that cannot run returns nothing either way.
    #[test]
    fn only_present_modules_are_worth_asking_about() {
        let gd_only: Vec<ExtInfo> = CATALOGUE
            .iter()
            .copied()
            .filter(|e| e.name == "gd")
            .collect();

        let chosen = capabilities_for(&gd_only);

        assert!(!chosen.is_empty(), "gd has capabilities worth probing");
        assert!(
            chosen.iter().all(|c| c.ext == "gd"),
            "a build with only gd must not be asked about curl, pdo or intl: {:?}",
            chosen.iter().map(|c| c.ext).collect::<Vec<_>>(),
        );

        assert!(
            capabilities_for(&[]).is_empty(),
            "a build with nothing present has nothing to answer for"
        );
    }

    #[test]
    fn capabilities_are_only_probed_for_modules_that_are_present() {
        let build = PhpBuild {
            version: "8.5".into(),
            // Deliberately not a binary: this must not get as far as running one.
            fpm_binary: std::path::PathBuf::from("/nonexistent/php-fpm"),
            cli_binary: None,
            variant: None,
            user_registered: false,
        };

        let nothing_present: Vec<ExtInfo> = Vec::new();
        assert!(
            probe_capabilities(&build, &nothing_present).is_empty(),
            "with no modules present there is nothing to ask about"
        );

        // And with a module present but no runnable binary, it still claims nothing —
        // a process that could not start is not evidence of a missing capability.
        let gd: Vec<ExtInfo> = CATALOGUE.iter().copied().filter(|e| e.name == "gd").collect();
        assert!(
            probe_capabilities(&build, &gd).is_empty(),
            "a binary that cannot be run must not be reported as a build full of holes"
        );
    }

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

        /*
         * The libs line, and `libjpeg` by name.
         *
         * Without it spc builds gd from its lib-depends alone — zlib and libpng — and
         * the result is a PHP where `imagejpeg()` does not exist while `php -m` lists
         * `gd` and the extension audit reports nothing missing. Thirteen tests in a
         * Laravel suite failed on it before anybody worked out that the module being
         * present said nothing about what was inside it.
         */
        let libs = yml
            .lines()
            .find(|l| l.starts_with("libs: "))
            .expect("libs line");
        let listed: Vec<&str> = libs["libs: ".len()..].split(',').collect();
        assert_eq!(listed, BUILD_LIBS);
        assert!(listed.contains(&"libjpeg"), "gd without libjpeg cannot write a JPEG");
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

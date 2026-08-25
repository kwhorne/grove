//! Verifying what Grove downloaded before it runs it.
//!
//! Grove fetches binaries it then executes: PHP, Node, Composer, cpx, the
//! bundled databases. Every one of those paths was `get` → write → `chmod 0755`
//! → run, with no check that the bytes were the ones the publisher shipped. TLS
//! authenticates the *host*, not the artefact — so a compromised mirror, a
//! poisoned CDN edge, or a truncated response all arrived looking fine.
//!
//! ## Where the expected hash comes from
//!
//! Different publishers, all reachable over the same TLS session as the
//! download:
//!
//! | Source | Where |
//! | --- | --- |
//! | Node | `SHASUMS256.txt` next to the release |
//! | Composer | `composer-stable.phar.sha256` |
//! | cpx, Grove's own PHP | GitHub's release API `digest` field |
//! | PostgreSQL binaries | `<asset>.sha256` |
//!
//! Be precise about what that proves. A hash fetched from the same host as the
//! file detects storage corruption, a truncated transfer and a tampered artefact
//! — the CDN-layer and accidental failures. It does **not** defend against a
//! publisher whose account is taken over and who replaces both. Only Node
//! publishes an independent chain (a GPG signature over `SHASUMS256.txt`), and
//! verifying that needs an OpenPGP implementation and a pinned key, which this
//! does not do.
//!
//! Two sources publish nothing usable and are documented as unverified:
//! static-php-cli's upstream archives, which have no checksum at all, and the
//! Redis git-archive tarball, whose bytes GitHub does not promise to keep stable.

use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ChecksumError {
    #[error("{what}: expected sha256 {expected}, got {actual}")]
    Mismatch {
        what: String,
        expected: String,
        actual: String,
    },
    #[error("{what}: no sha256 published for {filename}")]
    NotPublished { what: String, filename: String },
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Check `bytes` against `expected`, which may carry a `sha256:` prefix.
///
/// Comparison is case-insensitive on the hex, since publishers differ.
pub fn verify(what: &str, bytes: &[u8], expected: &str) -> Result<(), ChecksumError> {
    let expected = expected
        .trim()
        .trim_start_matches("sha256:")
        .trim()
        .to_ascii_lowercase();
    let actual = sha256_hex(bytes);
    if actual == expected {
        return Ok(());
    }
    Err(ChecksumError::Mismatch {
        what: what.to_string(),
        expected,
        actual,
    })
}

/// Pull the hash for `filename` out of a published checksum document.
///
/// Handles the two shapes publishers actually use, so callers do not each need
/// their own parser:
///
/// - a bare hex digest, the whole file (Composer)
/// - `sha256sum` lines, `<hex>  <name>`, one per artefact (Node, PostgreSQL)
///
/// Matching is on the file name only, so a manifest listing `./node-v1.tar.gz`
/// or a full path still resolves.
pub fn expected_for(document: &str, filename: &str) -> Option<String> {
    let trimmed = document.trim();
    if is_hex_digest(trimmed) {
        return Some(trimmed.to_ascii_lowercase());
    }
    for line in document.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !is_hex_digest(hash) {
            continue;
        }
        // `*name` marks binary mode in some sha256sum output.
        let name = name.trim_start_matches('*');
        let base = name.rsplit('/').next().unwrap_or(name);
        if base == filename {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn is_hex_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The empty-input digest, so the hashing itself is pinned against a known
    /// value rather than only against itself.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn hashing_matches_the_known_vector() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_matching_hash_verifies() {
        assert!(verify("empty", b"", EMPTY_SHA256).is_ok());
        // Publishers differ on case and on the `sha256:` prefix GitHub uses.
        assert!(verify("empty", b"", &EMPTY_SHA256.to_uppercase()).is_ok());
        assert!(verify("empty", b"", &format!("sha256:{EMPTY_SHA256}")).is_ok());
        assert!(verify("empty", b"", &format!("  {EMPTY_SHA256}\n")).is_ok());
    }

    #[test]
    fn a_mismatch_names_both_hashes() {
        let err = verify("php-fpm", b"tampered", EMPTY_SHA256).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("php-fpm"), "{msg}");
        assert!(msg.contains(EMPTY_SHA256), "the expected hash: {msg}");
        assert!(msg.contains(&sha256_hex(b"tampered")), "the actual: {msg}");
    }

    /// Composer serves the digest alone, as the whole document.
    #[test]
    fn a_bare_digest_document_parses() {
        let doc = "5ee7125f8a30a34d246cefdc0bc85b8a783b28f2aec968994118512350d28027\n";
        assert_eq!(
            expected_for(doc, "composer-stable.phar").as_deref(),
            Some("5ee7125f8a30a34d246cefdc0bc85b8a783b28f2aec968994118512350d28027")
        );
    }

    /// Node ships one `sha256sum` line per artefact — dozens of them.
    #[test]
    fn the_right_line_is_picked_out_of_a_manifest() {
        let doc = "\
daa6ea9f6add07922013215bac6712a2b1e29b12e00d9bf4ed45e85eebf5e8e2  node-v24.10.0-aix-ppc64.tar.gz
fbc3d6e1e1d962450d058e918214373872cc4c46e08673f31c35932afac4a8c5  node-v24.10.0-darwin-arm64.tar.gz
1d721c81deac26a511a1fde66d76be73d608be5d5320680828edd0176c686ae1  node-v24.10.0-arm64.msi
";
        assert_eq!(
            expected_for(doc, "node-v24.10.0-darwin-arm64.tar.gz").as_deref(),
            Some("fbc3d6e1e1d962450d058e918214373872cc4c46e08673f31c35932afac4a8c5")
        );
        // A near-miss must not match: picking the wrong line would verify the
        // wrong artefact and pass.
        assert_eq!(expected_for(doc, "node-v24.10.0-darwin-x64.tar.gz"), None);
        assert_eq!(expected_for(doc, ""), None);
    }

    /// PostgreSQL's `.sha256` is one line naming its own asset.
    #[test]
    fn a_single_line_document_parses() {
        let doc = "a257bcdb8aa3301a13d6a5bcec48f8c9517045b7cbae71e50788b2615539e95b  postgresql-18.6.0-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            expected_for(doc, "postgresql-18.6.0-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("a257bcdb8aa3301a13d6a5bcec48f8c9517045b7cbae71e50788b2615539e95b")
        );
    }

    #[test]
    fn manifest_quirks_are_tolerated() {
        // Binary-mode marker, and a path rather than a bare name.
        let doc = format!("{EMPTY_SHA256} *./dist/thing.tar.gz");
        assert_eq!(
            expected_for(&doc, "thing.tar.gz").as_deref(),
            Some(EMPTY_SHA256)
        );
    }

    #[test]
    fn junk_documents_yield_nothing_rather_than_a_wrong_answer() {
        for doc in [
            "",
            "404 Not Found",
            "<html><body>error</body></html>",
            "not-a-hash  thing.tar.gz",
            // Too short to be a sha256 — must not be accepted as one.
            "abc123  thing.tar.gz",
        ] {
            assert_eq!(
                expected_for(doc, "thing.tar.gz"),
                None,
                "should not have parsed: {doc:?}"
            );
        }
    }
}

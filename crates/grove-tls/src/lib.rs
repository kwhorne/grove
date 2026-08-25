//! Local certificate authority + on-demand leaf issuance.
//!
//! Grove generates a single long-lived root CA on first run, which the user
//! trusts once via the OS trust store (`grove-os`). Per-site leaf certificates
//! are then minted on demand and cached, so enabling HTTPS for a new site never
//! requires another trust prompt.

use std::fs;
use std::path::Path;

/// Fallback TLD when no config can be read. Matches `[general].tld`'s default.
const DEFAULT_TLD: &str = "test";

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, GeneralSubtree, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use time::{Duration, OffsetDateTime};

use grove_core::paths::GrovePaths;
use grove_core::securefs;

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate generation: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("core: {0}")]
    Core(#[from] grove_core::Error),
}

pub type Result<T> = std::result::Result<T, TlsError>;

/// The in-memory root CA used to sign leaf certificates.
pub struct CertificateAuthority {
    /// The CA certificate as PEM — byte for byte the one on disk, and so the
    /// one the OS trust store was told to trust.
    cert_pem: String,
    key_pem: String,
    /// The signing identity: the CA's distinguished name, key-id method, key
    /// usages and private key, as rcgen wants them for signing a leaf.
    issuer: Issuer<'static, KeyPair>,
    /// The TLD this CA was constrained to when Grove generated it.
    ///
    /// `None` for a CA loaded from disk without a marker — either one Grove
    /// wrote before it constrained anything, or one brought from elsewhere.
    constrained_tld: Option<String>,
}

/// Records what a generated CA was constrained to.
///
/// A marker file rather than parsing the certificate back: the TLD is the thing
/// that has to be compared against config later, and reading it from our own
/// note is simpler and has no failure mode of its own. Its *absence* is the
/// signal that matters — that is a CA from before constraints existed.
#[derive(serde::Serialize, serde::Deserialize)]
struct CaMeta {
    constrained_tld: String,
}

fn meta_path(paths: &GrovePaths) -> std::path::PathBuf {
    paths.certs_dir().join("ca-meta.json")
}

impl CertificateAuthority {
    /// Load an existing CA from disk, or create+persist a new one.
    pub fn load_or_create(paths: &GrovePaths) -> Result<Self> {
        paths.ensure()?;
        let cert_path = paths.ca_cert();
        let key_path = paths.ca_key();

        // Half a CA is not a CA. Regenerating over a cert whose key has gone —
        // or a key whose cert has — mints a *new* signing identity while the
        // system trust store still holds the old certificate, so every site
        // starts failing TLS with nothing pointing at the cause. Say so instead.
        if cert_path.exists() != key_path.exists() {
            let (present, missing) = if cert_path.exists() {
                (cert_path.display(), key_path.display())
            } else {
                (key_path.display(), cert_path.display())
            };
            return Err(TlsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "incomplete root CA: {present} exists but {missing} is missing.                      Refusing to mint a new CA over a half-present one — the trust store                      still holds the old certificate. Remove both and re-run                      `sudo grove ca trust` to start over."
                ),
            )));
        }

        if cert_path.exists() && key_path.exists() {
            let key_pem = fs::read_to_string(&key_path)?;
            let key = KeyPair::from_pem(&key_pem)?;
            let cert_pem = fs::read_to_string(&cert_path)?;
            // The CA on disk is loaded as a signing identity, not re-created.
            // The previous shape had no way to express that: it parsed the PEM
            // into params and then called `self_signed`, which *minted a new
            // certificate* — new serial, new validity window — on every daemon
            // start and every CLI call. Leaves still chained (same DN, same
            // key), but `cert_pem()` returned a certificate that was not the
            // one on disk and not the one the trust store held.
            let issuer = Issuer::from_ca_cert_pem(&cert_pem, key)?;
            claim_key_for_root(&key_path);
            return Ok(Self {
                cert_pem,
                key_pem,
                issuer,
                constrained_tld: constrained_tld(paths),
            });
        }

        let ca = Self::generate_for_tld(&configured_tld(paths))?;
        ca.persist(paths)?;
        Ok(ca)
    }

    /// Generate a fresh root CA valid for ~20 years, able to sign only `.tld`.
    ///
    /// The name constraint is the point. This certificate goes into the
    /// **system** trust store, so without one it is a universal CA: anything
    /// holding its key could mint a certificate for `google.com`, a bank, any
    /// name at all, and this machine would believe it. Constrained to the TLD
    /// Grove serves, the worst it can assert is a local development site.
    ///
    /// RFC 5280 dNSName subtrees are suffix matches on label boundaries, so the
    /// bare TLD permits `myapp.test` and `api.myapp.test` while excluding
    /// `test.evil.com` — a name that merely ends in the same letters.
    pub fn generate_for_tld(tld: &str) -> Result<Self> {
        let mut params = CertificateParams::default();
        params.name_constraints = Some(NameConstraints {
            permitted_subtrees: vec![GeneralSubtree::DnsName(tld.to_string())],
            excluded_subtrees: Vec::new(),
        });
        // `Unconstrained` here is the *path length*, which is a different
        // constraint entirely — it says nothing about which names may be signed.
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Grove Local CA");
        dn.push(DnType::OrganizationName, "Elyra Grove");
        params.distinguished_name = dn;
        params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
        params.not_after = OffsetDateTime::now_utc() + Duration::days(365 * 20);

        let key = KeyPair::generate()?;
        let cert = params.self_signed(&key)?;
        let cert_pem = cert.pem();
        let key_pem = key.serialize_pem();
        Ok(Self {
            cert_pem,
            key_pem,
            issuer: Issuer::new(params, key),
            constrained_tld: Some(tld.to_string()),
        })
    }

    /// Generate a CA for the default TLD. Prefer [`Self::generate_for_tld`].
    pub fn generate() -> Result<Self> {
        Self::generate_for_tld(DEFAULT_TLD)
    }

    /// Write the CA cert (0644) and key (0600) to disk.
    ///
    /// The key goes out through [`securefs::write_private`], so it is `0600`
    /// from the first byte and the write refuses a symlink rather than letting a
    /// root daemon `chmod` and overwrite whatever it points at. `$GROVE_HOME`
    /// lives in the user's own home, so that destination is not ours to trust.
    pub fn persist(&self, paths: &GrovePaths) -> Result<()> {
        paths.ensure()?;
        let cert_path = paths.ca_cert();
        let key_path = paths.ca_key();
        refuse_symlinked_dir(&key_path)?;
        securefs::write_public(&cert_path, &self.cert_pem)?;
        securefs::write_private(&key_path, &self.key_pem)?;
        claim_key_for_root(&key_path);
        if let Some(tld) = &self.constrained_tld {
            let meta = CaMeta {
                constrained_tld: tld.clone(),
            };
            if let Ok(body) = serde_json::to_string_pretty(&meta) {
                let _ = securefs::write_public(&meta_path(paths), body);
            }
        }
        Ok(())
    }

    /// The TLD this CA may sign for, or `None` if it is unconstrained.
    pub fn constrained_tld(&self) -> Option<&str> {
        self.constrained_tld.as_deref()
    }

    pub fn cert_pem(&self) -> String {
        self.cert_pem.clone()
    }

    /// Issue a leaf certificate for the given DNS names, signed by this CA.
    /// Returns `(cert_pem, key_pem)`.
    pub fn issue_leaf(&self, names: &[String]) -> Result<(String, String)> {
        let mut params = CertificateParams::default();
        params.subject_alt_names = names
            .iter()
            .map(|n| SanType::DnsName(n.clone().try_into().expect("valid dns name")))
            .collect();
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            names.first().cloned().unwrap_or_default(),
        );
        params.distinguished_name = dn;
        params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
        // Keep leaves short-lived; the daemon renews them automatically.
        params.not_after = OffsetDateTime::now_utc() + Duration::days(397);

        let leaf_key = KeyPair::generate()?;
        let leaf = params.signed_by(&leaf_key, &self.issuer)?;
        Ok((leaf.pem(), leaf_key.serialize_pem()))
    }

    /// Issue (or load from cache) a leaf for one site hostname + wildcard.
    pub fn leaf_for_site(&self, paths: &GrovePaths, hostname: &str) -> Result<(String, String)> {
        let safe = hostname.replace('.', "_");
        let cert_path = paths.certs_dir().join(format!("{safe}.pem"));
        let key_path = paths.certs_dir().join(format!("{safe}.key"));

        if cert_path.exists() && key_path.exists() {
            let cert_pem = fs::read_to_string(&cert_path)?;
            // Reuse it only while it is still good for a while. `issue_leaf`
            // says the daemon renews these automatically, and it did not: the
            // cached pair was returned unconditionally, so 397 days after a site
            // was first served its certificate would simply expire and every
            // request to it would fail TLS.
            if !expires_within(&cert_pem, RENEW_BEFORE_DAYS) {
                return Ok((cert_pem, fs::read_to_string(&key_path)?));
            }
            tracing::info!(
                hostname,
                "leaf certificate is near expiry; issuing a replacement"
            );
        }

        let names = vec![hostname.to_string(), format!("*.{hostname}")];
        let (cert_pem, key_pem) = self.issue_leaf(&names)?;
        refuse_symlinked_dir(&key_path)?;
        securefs::write_public(&cert_path, &cert_pem)?;
        securefs::write_private(&key_path, &key_pem)?;
        Ok((cert_pem, key_pem))
    }
}

/// How long before expiry a leaf is replaced.
///
/// Comfortably longer than any browser session, so a certificate is never
/// swapped out from under a page that is already loaded, and short enough that
/// the vast majority of reuses are a plain file read.
const RENEW_BEFORE_DAYS: i64 = 30;

/// Whether a PEM certificate expires within `days`.
///
/// A certificate that cannot be parsed is treated as due for replacement:
/// re-issuing costs a keypair, while trusting something unreadable is how a
/// site ends up serving an expired certificate.
fn expires_within(cert_pem: &str, days: i64) -> bool {
    use x509_parser::prelude::*;

    let Ok((_, pem)) = parse_x509_pem(cert_pem.as_bytes()) else {
        return true;
    };
    let Ok(cert) = pem.parse_x509() else {
        return true;
    };
    let not_after = cert.validity().not_after.timestamp();
    let cutoff = OffsetDateTime::now_utc() + Duration::days(days);
    not_after <= cutoff.unix_timestamp()
}

/// The TLD a CA on disk was constrained to, if Grove recorded one.
pub fn constrained_tld(paths: &GrovePaths) -> Option<String> {
    let raw = fs::read_to_string(meta_path(paths)).ok()?;
    serde_json::from_str::<CaMeta>(&raw)
        .ok()
        .map(|m| m.constrained_tld)
}

/// The TLD Grove is configured to serve, falling back to the default.
fn configured_tld(paths: &GrovePaths) -> String {
    grove_core::Config::load(paths)
        .map(|c| c.general.tld)
        .unwrap_or_else(|_| DEFAULT_TLD.to_string())
}

/// Remove the CA and every leaf it signed, so the next load mints a fresh one.
///
/// Leaves have to go with it: they chain to the old CA, and a leaf whose issuer
/// the trust store no longer holds is a certificate error on every site rather
/// than a helpful one.
pub fn remove(paths: &GrovePaths) -> Result<()> {
    let _ = fs::remove_file(paths.ca_cert());
    let _ = fs::remove_file(paths.ca_key());
    let _ = fs::remove_file(meta_path(paths));
    let dir = paths.certs_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_leaf = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "pem" || e == "key" || e == "crt")
                .unwrap_or(false);
            if is_leaf && path != paths.ca_cert() && path != paths.ca_key() {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(())
}

/// Make the CA private key root-owned, whenever we are root to do it.
///
/// This key is the whole point of Grove's HTTPS: it is installed in the
/// **system** trust store, so whoever holds it can mint a certificate for any
/// name — `google.com` included — that this machine will believe. Nothing
/// unprivileged needs it: every caller that loads it is either the root daemon
/// or a command documented as `sudo`. Leaving it readable by the login user
/// meant a compromised `npm`/`composer` postinstall hook could walk off with it.
///
/// `0600` from [`securefs::write_private`] already stops other *users*; this is
/// what stops unprivileged code running as the login user. Ownership converges
/// rather than being enforced up front, because a first-run `grove init` without
/// `sudo` legitimately creates the CA as the user — the root daemon then claims
/// it on its next start. That leaves a window where the key is user-readable,
/// but only for a key the user created and therefore already had.
///
/// Note that the leaf keys are deliberately *not* included: `grove dev` hands
/// Vite its certificate and key on purpose, and that process runs as the user. A
/// stolen leaf key impersonates one local site; a stolen CA key impersonates
/// everything.
fn claim_key_for_root(key_path: &Path) {
    if !grove_core::privdrop::running_as_root() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let already_root = fs::metadata(key_path)
            .map(|m| m.uid() == 0)
            .unwrap_or(false);
        if already_root {
            return;
        }
        match std::os::unix::fs::chown(key_path, Some(0), Some(0)) {
            Ok(()) => tracing::info!(
                key = %key_path.display(),
                "took ownership of the root CA key so unprivileged code cannot read it"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                key = %key_path.display(),
                "could not take ownership of the root CA key"
            ),
        }
    }
}

/// Refuse to write into a directory that has been swapped for a symlink.
///
/// `O_NOFOLLOW` in [`securefs`] only guards the final path component, so it
/// cannot see a `certs/` that now points at `/etc`. Checking one level up covers
/// the realistic version of that attack; the durable fix is for this tree not to
/// sit inside a user-writable directory at all.
fn refuse_symlinked_dir(path: &Path) -> Result<()> {
    if securefs::parent_is_symlink(path) {
        return Err(TlsError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} is a symlink; refusing to write a private key through it",
                path.parent().unwrap_or(path).display()
            ),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension has to be *in the certificate*, not merely set on the
    /// params — this parses the DER back out and looks.
    #[test]
    fn the_ca_carries_a_dns_name_constraint() {
        use x509_parser::prelude::*;

        let ca = CertificateAuthority::generate_for_tld("test").unwrap();
        let (_, pem) = parse_x509_pem(ca.cert_pem().as_bytes()).expect("pem");
        let cert = pem.parse_x509().expect("der");

        let constraints = cert
            .extensions()
            .iter()
            .find_map(|e| match e.parsed_extension() {
                ParsedExtension::NameConstraints(nc) => Some(nc),
                _ => None,
            })
            .expect("the CA must carry a NameConstraints extension");

        let permitted: Vec<String> = constraints
            .permitted_subtrees
            .as_ref()
            .expect("permitted subtrees")
            .iter()
            .map(|s| format!("{:?}", s.base))
            .collect();
        assert!(
            permitted.iter().any(|p| p.contains("test")),
            "permitted subtrees were {permitted:?}"
        );
    }

    /// A CA generated for a custom TLD constrains itself to that one.
    #[test]
    fn the_constraint_follows_the_configured_tld() {
        use x509_parser::prelude::*;

        let ca = CertificateAuthority::generate_for_tld("localhost").unwrap();
        let (_, pem) = parse_x509_pem(ca.cert_pem().as_bytes()).unwrap();
        let cert = pem.parse_x509().unwrap();
        let nc = cert
            .extensions()
            .iter()
            .find_map(|e| match e.parsed_extension() {
                ParsedExtension::NameConstraints(nc) => Some(nc),
                _ => None,
            })
            .expect("NameConstraints");
        let permitted = format!("{:?}", nc.permitted_subtrees);
        assert!(permitted.contains("localhost"), "{permitted}");
        assert!(!permitted.contains("\"test\""), "{permitted}");
    }

    /// The constraint must not stop Grove doing its actual job.
    /// Generation records the TLD, and a fresh install is constrained without
    /// anyone having to ask.
    #[test]
    fn a_generated_ca_records_what_it_was_constrained_to() {
        let paths = scratch("meta");
        let ca = CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(ca.constrained_tld(), Some("test"));
        assert_eq!(constrained_tld(&paths).as_deref(), Some("test"));

        // And it survives a reload, which is what `grove doctor` reads.
        let reloaded = CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(reloaded.constrained_tld(), Some("test"));
        let _ = std::fs::remove_dir_all(paths.base());
    }

    /// A CA from before constraints existed has no marker — that absence is the
    /// signal `grove doctor` turns into a warning.
    #[test]
    fn a_legacy_ca_reports_no_constraint() {
        let paths = scratch("legacy");
        CertificateAuthority::load_or_create(&paths).unwrap();
        std::fs::remove_file(paths.certs_dir().join("ca-meta.json")).unwrap();

        assert_eq!(constrained_tld(&paths), None);
        let reloaded = CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(reloaded.constrained_tld(), None);
        let _ = std::fs::remove_dir_all(paths.base());
    }

    /// Rotation has to take the leaves with it: they chain to the old CA, and a
    /// leaf whose issuer is gone is a certificate error on every site.
    #[test]
    fn removing_the_ca_takes_its_leaves_too() {
        let paths = scratch("rotate");
        let ca = CertificateAuthority::load_or_create(&paths).unwrap();
        ca.leaf_for_site(&paths, "myapp.test").unwrap();
        let leaf = paths.certs_dir().join("myapp_test.pem");
        let leaf_key = paths.certs_dir().join("myapp_test.key");
        assert!(leaf.exists() && leaf_key.exists());

        remove(&paths).unwrap();
        assert!(
            !paths.ca_cert().exists(),
            "the CA certificate should be gone"
        );
        assert!(!paths.ca_key().exists(), "the CA key should be gone");
        assert!(!leaf.exists(), "a leaf signed by the old CA should be gone");
        assert!(!leaf_key.exists(), "its key too");
        assert_eq!(constrained_tld(&paths), None, "the marker should be gone");

        // And the next load mints a working CA rather than tripping the
        // half-present guard.
        let fresh = CertificateAuthority::load_or_create(&paths).unwrap();
        assert_ne!(fresh.cert_pem(), ca.cert_pem(), "it must be a new CA");
        assert!(fresh.issue_leaf(&["myapp.test".to_string()]).is_ok());
        let _ = std::fs::remove_dir_all(paths.base());
    }

    /// Build a leaf that expires in `days`, to drive the renewal check.
    fn leaf_expiring_in(ca: &CertificateAuthority, days: i64) -> String {
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::DnsName(
            "myapp.test".to_string().try_into().unwrap(),
        )];
        params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
        params.not_after = OffsetDateTime::now_utc() + Duration::days(days);
        let key = KeyPair::generate().unwrap();
        params.signed_by(&key, &ca.issuer).unwrap().pem()
    }

    #[test]
    fn expiry_is_judged_from_the_certificate() {
        let ca = CertificateAuthority::generate_for_tld("test").unwrap();
        assert!(
            !expires_within(&leaf_expiring_in(&ca, 200), RENEW_BEFORE_DAYS),
            "a certificate with 200 days left should be reused"
        );
        assert!(
            expires_within(&leaf_expiring_in(&ca, 5), RENEW_BEFORE_DAYS),
            "a certificate with 5 days left should be replaced"
        );
        // Unparseable input errs towards replacing: re-issuing costs a keypair,
        // serving something unreadable costs the site.
        assert!(expires_within("not a certificate", RENEW_BEFORE_DAYS));
        assert!(expires_within("", RENEW_BEFORE_DAYS));
    }

    /// The bug: `leaf_for_site` returned the cached pair unconditionally, so
    /// 397 days after a site was first served its certificate expired and every
    /// request to it failed TLS — despite the comment promising renewal.
    #[test]
    fn a_near_expiry_leaf_is_replaced_on_next_use() {
        let paths = scratch("renew");
        let ca = CertificateAuthority::load_or_create(&paths).unwrap();

        // Seed the cache with a certificate that is nearly up.
        let stale = leaf_expiring_in(&ca, 3);
        let cert_path = paths.certs_dir().join("myapp_test.pem");
        let key_path = paths.certs_dir().join("myapp_test.key");
        fs::write(&cert_path, &stale).unwrap();
        fs::write(&key_path, "placeholder").unwrap();

        let (served, key) = ca.leaf_for_site(&paths, "myapp.test").unwrap();
        assert_ne!(served, stale, "the stale certificate should not be served");
        assert_ne!(key, "placeholder", "its key should have been replaced too");
        assert!(!expires_within(&served, RENEW_BEFORE_DAYS));

        // And the replacement is what is now on disk, so the next call is a
        // plain file read.
        assert_eq!(fs::read_to_string(&cert_path).unwrap(), served);
        let (again, _) = ca.leaf_for_site(&paths, "myapp.test").unwrap();
        assert_eq!(again, served, "a fresh certificate should be reused as-is");
        let _ = std::fs::remove_dir_all(paths.base());
    }

    #[test]
    fn a_constrained_ca_still_signs_its_own_sites() {
        let ca = CertificateAuthority::generate_for_tld("test").unwrap();
        let (leaf, key) = ca
            .issue_leaf(&["myapp.test".to_string(), "*.myapp.test".to_string()])
            .expect("a site under the permitted TLD must still be signable");
        assert!(leaf.contains("BEGIN CERTIFICATE"));
        assert!(key.contains("PRIVATE KEY"));
    }

    #[test]
    fn generates_and_signs_leaf() {
        let ca = CertificateAuthority::generate().unwrap();
        assert!(ca.cert_pem().contains("BEGIN CERTIFICATE"));
        let (leaf, key) = ca
            .issue_leaf(&["myapp.test".to_string(), "*.myapp.test".to_string()])
            .unwrap();
        assert!(leaf.contains("BEGIN CERTIFICATE"));
        assert!(key.contains("PRIVATE KEY"));
    }

    fn scratch(name: &str) -> GrovePaths {
        let base = std::env::temp_dir().join(format!(
            "grove-ca-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        GrovePaths::with_base(&base)
    }

    /// A CA missing half of itself must not be silently replaced.
    ///
    /// The old behaviour regenerated both files, which mints a new signing
    /// identity while the system trust store still holds the old certificate —
    /// every site then fails TLS with nothing pointing at why.
    #[test]
    fn a_half_present_ca_is_refused_rather_than_replaced() {
        for missing_key in [true, false] {
            let paths = scratch(if missing_key { "nokey" } else { "nocert" });
            let ca = CertificateAuthority::load_or_create(&paths).unwrap();
            let original_cert = ca.cert_pem();

            let gone = if missing_key {
                paths.ca_key()
            } else {
                paths.ca_cert()
            };
            std::fs::remove_file(&gone).unwrap();

            let err = match CertificateAuthority::load_or_create(&paths) {
                Err(e) => e,
                Ok(_) => panic!("a half-present CA must not be regenerated"),
            };
            assert!(
                err.to_string().contains("incomplete root CA"),
                "unhelpful error: {err}"
            );

            // Crucially, the surviving half is untouched — the trust store's
            // certificate is still the one on disk.
            if !missing_key {
                assert!(!paths.ca_cert().exists(), "cert must not be re-minted");
            } else {
                assert_eq!(
                    fs::read_to_string(paths.ca_cert()).unwrap(),
                    original_cert,
                    "the trusted certificate must survive untouched"
                );
            }
            let _ = std::fs::remove_dir_all(paths.base());
        }
    }

    /// The key must never be group- or world-readable, whoever owns it.
    #[test]
    fn the_ca_key_is_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let paths = scratch("mode");
            CertificateAuthority::load_or_create(&paths).unwrap();
            let mode = fs::metadata(paths.ca_key()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "CA key mode is {mode:o}");
            let _ = std::fs::remove_dir_all(paths.base());
        }
    }

    /// Unprivileged, `claim_key_for_root` must be a no-op rather than an error —
    /// a first-run `grove init` without `sudo` has to keep working.
    #[test]
    fn claiming_the_key_is_a_no_op_without_root() {
        let paths = scratch("claim");
        CertificateAuthority::load_or_create(&paths).unwrap();
        let before = fs::metadata(paths.ca_key()).unwrap();
        claim_key_for_root(&paths.ca_key());
        let after = fs::metadata(paths.ca_key()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !grove_core::privdrop::running_as_root() {
                assert_eq!(before.uid(), after.uid(), "ownership must not change");
            }
        }
        // And the CA still loads afterwards.
        assert!(CertificateAuthority::load_or_create(&paths).is_ok());
        let _ = std::fs::remove_dir_all(paths.base());
    }

    #[test]
    fn reloading_keeps_the_certificate_that_is_on_disk() {
        // Regression: `load_or_create` used to parse the CA's PEM into params and
        // then call `self_signed`, minting a *new* certificate every time. The
        // CA that `cert_pem()` reported then differed from the file the OS trust
        // store had been pointed at.
        let base = std::env::temp_dir().join(format!(
            "grove-ca-reload-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let paths = GrovePaths::with_base(&base);

        let first = CertificateAuthority::load_or_create(&paths).unwrap();
        let on_disk = fs::read_to_string(paths.ca_cert()).unwrap();
        assert_eq!(first.cert_pem(), on_disk);

        let reloaded = CertificateAuthority::load_or_create(&paths).unwrap();
        assert_eq!(reloaded.cert_pem(), on_disk);

        // And it must still be able to sign: the issuer was rebuilt from PEM,
        // so a mistake here would show up as a leaf that does not chain.
        let (leaf, _) = reloaded.issue_leaf(&["myapp.test".to_string()]).unwrap();
        assert!(leaf.contains("BEGIN CERTIFICATE"));

        let _ = std::fs::remove_dir_all(&base);
    }
}

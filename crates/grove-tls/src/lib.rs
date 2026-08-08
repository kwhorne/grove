//! Local certificate authority + on-demand leaf issuance.
//!
//! Grove generates a single long-lived root CA on first run, which the user
//! trusts once via the OS trust store (`grove-os`). Per-site leaf certificates
//! are then minted on demand and cached, so enabling HTTPS for a new site never
//! requires another trust prompt.

use std::fs;
use std::path::Path;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};

use grove_core::paths::GrovePaths;

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
}

impl CertificateAuthority {
    /// Load an existing CA from disk, or create+persist a new one.
    pub fn load_or_create(paths: &GrovePaths) -> Result<Self> {
        paths.ensure()?;
        let cert_path = paths.ca_cert();
        let key_path = paths.ca_key();

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
            return Ok(Self {
                cert_pem,
                key_pem,
                issuer,
            });
        }

        let ca = Self::generate()?;
        ca.persist(paths)?;
        Ok(ca)
    }

    /// Generate a fresh root CA valid for ~20 years.
    pub fn generate() -> Result<Self> {
        let mut params = CertificateParams::default();
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
        })
    }

    /// Write the CA cert (0644) and key (0600) to disk.
    pub fn persist(&self, paths: &GrovePaths) -> Result<()> {
        paths.ensure()?;
        let cert_path = paths.ca_cert();
        let key_path = paths.ca_key();
        fs::write(&cert_path, &self.cert_pem)?;
        fs::write(&key_path, &self.key_pem)?;
        restrict_key_perms(&key_path)?;
        Ok(())
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
            return Ok((
                fs::read_to_string(&cert_path)?,
                fs::read_to_string(&key_path)?,
            ));
        }

        let names = vec![hostname.to_string(), format!("*.{hostname}")];
        let (cert_pem, key_pem) = self.issue_leaf(&names)?;
        fs::write(&cert_path, &cert_pem)?;
        fs::write(&key_path, &key_pem)?;
        restrict_key_perms(&key_path)?;
        Ok((cert_pem, key_pem))
    }
}

#[cfg(unix)]
fn restrict_key_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_key_perms(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

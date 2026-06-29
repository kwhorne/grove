//! SNI-based certificate resolution. On each TLS handshake we look at the
//! requested hostname and hand back a leaf certificate signed by Grove's CA,
//! minting (and caching) one on demand.

use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_tls::CertificateAuthority;

/// Resolves leaf certs for `*.test` hostnames using the local CA.
pub struct SniResolver {
    ca: Arc<CertificateAuthority>,
    paths: GrovePaths,
    cache: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniResolver").finish()
    }
}

impl SniResolver {
    pub fn new(ca: Arc<CertificateAuthority>, paths: GrovePaths) -> Self {
        Self {
            ca,
            paths,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn certified_key(&self, hostname: &str) -> Option<Arc<CertifiedKey>> {
        if let Some(found) = self.cache.lock().unwrap().get(hostname).cloned() {
            return Some(found);
        }

        let (cert_pem, key_pem) = self
            .ca
            .leaf_for_site(&self.paths, hostname)
            .map_err(|e| tracing::error!(error = %e, hostname, "leaf issuance failed"))
            .ok()?;

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .filter_map(|c| c.ok())
            .collect();
        let key: PrivateKeyDer<'static> =
            rustls_pemfile::private_key(&mut key_pem.as_bytes()).ok()??;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key).ok()?;
        let ck = Arc::new(CertifiedKey::new(certs, signing_key));
        self.cache
            .lock()
            .unwrap()
            .insert(hostname.to_string(), ck.clone());
        Some(ck)
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let hostname = client_hello.server_name()?.to_string();
        self.certified_key(&hostname)
    }
}

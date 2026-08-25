//! SNI-based certificate resolution. On each TLS handshake we look at the
//! requested hostname and hand back a leaf certificate signed by Grove's CA,
//! minting (and caching) one on demand.
//!
//! # Only for names Grove serves
//!
//! Grove's CA is installed in the **system** trust store, and this resolver used
//! to mint a leaf for whatever hostname the client asked for — no check against
//! the site registry at all. The HTTPS listener binds `0.0.0.0`, so anyone on
//! the same network who could get a connection here (a spoofed DNS answer, ARP
//! poisoning) could ask for `SNI: bank.example` and receive a **valid,
//! machine-trusted** certificate for it. The HTTP layer was already stricter: it
//! answers 404 for a host it does not know.
//!
//! Every name is now resolved against the registry first, by the same rule the
//! router uses, and an unknown one gets no certificate — which fails the
//! handshake, as it should.
//!
//! # Bounded work
//!
//! The registry answers for subdomains too (`api.myapp.test` → `myapp`), so
//! "known" is still an infinite set of names. Two limits keep that from being a
//! resource attack: the hot cache is capped, and only a site's *own* hostname is
//! persisted to disk. A made-up subdomain costs one keypair and a cache slot,
//! not a permanent pair of files.

use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::sync::Mutex;

use grove_core::paths::GrovePaths;
use grove_core::registry::KnownHosts;
use grove_tls::CertificateAuthority;

/// How many leaf certificates to keep hot.
///
/// One per site is the normal case; the rest are subdomains, which resolve to a
/// site but are otherwise unbounded — a peer can ask for `<random>.myapp.test`
/// as often as it likes. The cap turns unbounded growth into churn.
const MAX_CACHED_LEAVES: usize = 256;

/// Resolves leaf certs for `*.test` hostnames using the local CA.
pub struct SniResolver {
    ca: Arc<CertificateAuthority>,
    paths: GrovePaths,
    /// The registry's hostnames. Synchronous on purpose: rustls resolves inside
    /// a non-async callback.
    known_hosts: Arc<std::sync::RwLock<KnownHosts>>,
    cache: Mutex<Cache>,
}

/// The hot cache, with insertion order kept so it can be bounded.
#[derive(Default)]
struct Cache {
    by_host: HashMap<String, Arc<CertifiedKey>>,
    order: std::collections::VecDeque<String>,
}

impl Cache {
    fn get(&self, host: &str) -> Option<Arc<CertifiedKey>> {
        self.by_host.get(host).cloned()
    }

    fn insert(&mut self, host: String, key: Arc<CertifiedKey>) {
        if self.by_host.contains_key(&host) {
            return;
        }
        while self.order.len() >= MAX_CACHED_LEAVES {
            if let Some(oldest) = self.order.pop_front() {
                self.by_host.remove(&oldest);
            }
        }
        self.order.push_back(host.clone());
        self.by_host.insert(host, key);
    }
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniResolver").finish()
    }
}

impl SniResolver {
    pub fn new(
        ca: Arc<CertificateAuthority>,
        paths: GrovePaths,
        known_hosts: Arc<std::sync::RwLock<KnownHosts>>,
    ) -> Self {
        Self {
            ca,
            paths,
            known_hosts,
            cache: Mutex::new(Cache::default()),
        }
    }

    /// The site hostname `hostname` belongs to, or `None` if Grove serves no
    /// such site.
    ///
    /// A poisoned lock is treated as "unknown": refusing a handshake is a
    /// recoverable failure, and issuing a trusted certificate for an
    /// unverified name is not.
    fn site_hostname(&self, hostname: &str) -> Option<String> {
        match self.known_hosts.read() {
            Ok(hosts) => hosts.site_hostname(hostname),
            Err(_) => {
                tracing::error!("known-hosts lock poisoned; refusing to issue a certificate");
                None
            }
        }
    }

    fn certified_key(&self, hostname: &str) -> Option<Arc<CertifiedKey>> {
        if let Some(found) = self.cache.lock().ok()?.get(hostname) {
            return Some(found);
        }

        // Never mint for a name Grove does not serve. This is the check that
        // stops a machine-trusted certificate being handed out for any name a
        // client cares to ask for.
        let site_hostname = self.site_hostname(hostname)?;

        // Persist only a site's own certificate. Subdomains are legitimate but
        // unbounded, and a file per made-up name is a disk-fill waiting to
        // happen; they live in the capped cache until the daemon restarts.
        let issued = if site_hostname == hostname {
            self.ca.leaf_for_site(&self.paths, hostname)
        } else {
            self.ca
                .issue_leaf(&[hostname.to_string(), format!("*.{hostname}")])
        };
        let (cert_pem, key_pem) = issued
            .map_err(|e| tracing::error!(error = %e, hostname, "leaf issuance failed"))
            .ok()?;

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .filter_map(|c| c.ok())
            .collect();
        let key: PrivateKeyDer<'static> =
            rustls_pemfile::private_key(&mut key_pem.as_bytes()).ok()??;

        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key).ok()?;
        let ck = Arc::new(CertifiedKey::new(certs, signing_key));
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(hostname.to_string(), ck.clone());
        }
        Some(ck)
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let hostname = client_hello.server_name()?.to_string();
        let key = self.certified_key(&hostname);
        if key.is_none() {
            // Worth a line: from the client's side this is an opaque handshake
            // failure, and "you asked for a host I do not serve" is the answer.
            tracing::debug!(hostname, "no certificate: not a site Grove serves");
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(sites: &[&str]) -> SniResolver {
        let mut cfg = grove_core::Config::default();
        cfg.general.tld = "test".into();
        for name in sites {
            cfg.sites.push(grove_core::config::SiteConfig {
                name: (*name).to_string(),
                path: Some(std::path::PathBuf::from("/tmp")),
                php: None,
                node: None,
                secure: true,
                driver: Some(grove_core::Driver::Static),
                proxy_to: None,
            });
        }
        let registry = grove_core::SiteRegistry::build(&cfg);
        let known = Arc::new(std::sync::RwLock::new(registry.known_hosts()));

        let base = std::env::temp_dir().join(format!(
            "grove-sni-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let paths = GrovePaths::with_base(&base);
        let ca = Arc::new(grove_tls::CertificateAuthority::load_or_create(&paths).unwrap());
        SniResolver::new(ca, paths, known)
    }

    /// The finding. The HTTPS listener binds 0.0.0.0, so anyone who can reach it
    /// used to be able to ask for any name and get a machine-trusted certificate
    /// for it.
    #[test]
    fn no_certificate_is_minted_for_a_name_grove_does_not_serve() {
        let r = resolver(&["myapp"]);
        for name in [
            "google.com",
            "bank.example",
            "notmyapp.test",
            "myapp.test.evil.com",
        ] {
            assert!(
                r.certified_key(name).is_none(),
                "{name} should not have been issued a certificate"
            );
        }
        let _ = std::fs::remove_dir_all(r.paths.base());
    }

    #[test]
    fn a_real_site_and_its_subdomains_still_get_one() {
        let r = resolver(&["myapp"]);
        for name in ["myapp.test", "api.myapp.test"] {
            assert!(r.certified_key(name).is_some(), "{name} should be served");
        }
        let _ = std::fs::remove_dir_all(r.paths.base());
    }

    /// Only the site's own certificate is persisted; a made-up subdomain must
    /// not leave a permanent pair of files behind.
    #[test]
    fn only_a_sites_own_certificate_reaches_the_disk() {
        let r = resolver(&["myapp"]);
        assert!(r.certified_key("myapp.test").is_some());
        assert!(r.certified_key("scratch.myapp.test").is_some());

        let certs = r.paths.certs_dir();
        assert!(
            certs.join("myapp_test.pem").exists(),
            "site cert should persist"
        );
        assert!(
            !certs.join("scratch_myapp_test.pem").exists(),
            "a subdomain certificate must not be written to disk"
        );
        let _ = std::fs::remove_dir_all(r.paths.base());
    }

    /// Subdomains are unbounded, so the hot cache must not be.
    #[test]
    fn the_cache_is_bounded() {
        let r = resolver(&["myapp"]);
        for i in 0..(MAX_CACHED_LEAVES + 20) {
            assert!(r.certified_key(&format!("n{i}.myapp.test")).is_some());
        }
        let cache = r.cache.lock().unwrap();
        assert!(
            cache.by_host.len() <= MAX_CACHED_LEAVES,
            "cache grew to {}",
            cache.by_host.len()
        );
        assert_eq!(
            cache.by_host.len(),
            cache.order.len(),
            "bookkeeping drifted"
        );
        drop(cache);
        let _ = std::fs::remove_dir_all(r.paths.base());
    }

    /// A poisoned lock must fail closed: no certificate, rather than one for an
    /// unverified name.
    #[test]
    fn a_poisoned_known_hosts_lock_refuses_to_issue() {
        let r = resolver(&["myapp"]);
        let lock = r.known_hosts.clone();
        let _ = std::thread::spawn(move || {
            let _guard = lock.write().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(r.known_hosts.is_poisoned());
        assert!(r.certified_key("myapp.test").is_none());
        let _ = std::fs::remove_dir_all(r.paths.base());
    }
}

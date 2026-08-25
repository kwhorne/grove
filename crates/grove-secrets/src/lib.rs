//! grove-secrets — zero-knowledge, end-to-end encrypted team secrets.
//!
//! Secrets (a project's `.env`) are encrypted **on the client** to the public
//! keys of the current team members (the `age` X25519 recipients model). The
//! store — a mock file store here, a hosted backend in production — only ever
//! sees ciphertext and public keys. Reading requires a member's private key,
//! which never leaves their machine.
//!
//! This crate proves the trust anchor locally, with no backend:
//!   * the store never holds plaintext,
//!   * every current member can decrypt,
//!   * a removed member is locked out on the next re-encrypt.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, SecretsError>;

pub mod pin;

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("invalid key: {0}")]
    Key(String),
    #[error("no recipients for project {0:?} — nothing could decrypt it")]
    NoRecipients(String),
    #[error("not a member of {0:?} (your key can't decrypt these secrets)")]
    NotAMember(String),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("backend: {0}")]
    Http(String),
    #[error(
        "the recipient list for {project:?} does not match what you agreed to \
         (added: {added:?}, removed: {removed:?}). Refusing to encrypt. If this \
         change is expected, run `grove secret share`/`revoke` to record it; if \
         it is not, your backend may be compromised."
    )]
    RecipientsChanged {
        project: String,
        added: Vec<String>,
        removed: Vec<String>,
    },
    #[error(
        "{project:?} came back at version {served}, older than the {expected} \
         already seen — the backend may be replaying an old payload"
    )]
    Rollback {
        project: String,
        served: u64,
        expected: u64,
    },
}

// ---------------------------------------------------------------------------
// Identities & public keys
// ---------------------------------------------------------------------------

/// A member's key pair. The private half never leaves the machine.
pub struct Identity {
    inner: age::x25519::Identity,
}

impl Identity {
    /// Generate a fresh member identity.
    pub fn generate() -> Self {
        Self {
            inner: age::x25519::Identity::generate(),
        }
    }

    /// Load an identity from its secret string (`AGE-SECRET-KEY-1…`).
    pub fn from_secret(secret: &str) -> Result<Self> {
        let inner = secret
            .trim()
            .parse::<age::x25519::Identity>()
            .map_err(|e| SecretsError::Key(e.to_string()))?;
        Ok(Self { inner })
    }

    /// The secret string to persist locally (treat like an SSH private key).
    pub fn secret_string(&self) -> String {
        use age::secrecy::ExposeSecret;
        self.inner.to_string().expose_secret().to_string()
    }

    /// This identity's shareable public key.
    pub fn public(&self) -> PublicKey {
        PublicKey(self.inner.to_public().to_string())
    }
}

/// What actually gets encrypted: the secrets plus the metadata that makes a
/// rollback visible.
///
/// The version lives *inside* the ciphertext on purpose. The server stores an
/// opaque blob and cannot edit it, so it cannot serve an old payload while
/// claiming it is current — the client compares the decrypted version against
/// the highest it has accepted and refuses anything lower.
///
/// Before this, a payload was a bare map with no version and no timestamp, so
/// replaying yesterday's blob — restoring a secret that had since been rotated,
/// or reinstating a member's access — was indistinguishable from a normal fetch.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u64,
    /// Unix seconds. Not used for any decision — a server could not be trusted
    /// with time anyway — but it tells a human when a payload was written.
    written_at: i64,
    env: BTreeMap<String, String>,
}

impl Envelope {
    /// Read a stored payload, accepting the pre-envelope format.
    ///
    /// Blobs written before this existed are bare maps. They are read as version
    /// 0, which is below every version an envelope carries, so the first write
    /// after an upgrade moves the project forward and it never goes back.
    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Ok(Self {
                version: 0,
                written_at: 0,
                env: BTreeMap::new(),
            });
        }
        if let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) {
            return Ok(envelope);
        }
        let env: BTreeMap<String, String> = serde_json::from_slice(bytes)?;
        Ok(Self {
            version: 0,
            written_at: 0,
            env,
        })
    }
}

/// A member's public key (`age1…`) — safe to store on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey(pub String);

impl PublicKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn to_recipient(&self) -> Result<age::x25519::Recipient> {
        self.0
            .trim()
            .parse::<age::x25519::Recipient>()
            .map_err(|e| SecretsError::Key(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Raw encrypt / decrypt (the age recipients model)
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` so that any of `recipients` can decrypt it.
pub fn encrypt(plaintext: &[u8], recipients: &[PublicKey]) -> Result<Vec<u8>> {
    if recipients.is_empty() {
        return Err(SecretsError::Crypto("no recipients".into()));
    }
    // age 0.12 takes borrowed recipients, so they are parsed into an owned Vec
    // first and lent out as trait objects — no boxing, and one allocation for the
    // whole set instead of one per recipient.
    let parsed: Vec<age::x25519::Recipient> = recipients
        .iter()
        .map(|r| r.to_recipient())
        .collect::<Result<_>>()?;

    let encryptor =
        age::Encryptor::with_recipients(parsed.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|e| SecretsError::Crypto(e.to_string()))?;
    let mut out = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut out)
        .map_err(|e| SecretsError::Crypto(e.to_string()))?;
    writer.write_all(plaintext)?;
    writer
        .finish()
        .map_err(|e| SecretsError::Crypto(e.to_string()))?;
    Ok(out)
}

/// Decrypt `ciphertext` with a member's identity.
pub fn decrypt(ciphertext: &[u8], identity: &Identity) -> Result<Vec<u8>> {
    // `new_buffered` rather than `new`: the input is a `&[u8]`, which is already
    // `BufRead`, and age's own docs point at the buffered parser for it — `new`
    // uses a slower one that avoids overreading on arbitrary readers.
    let decryptor = age::Decryptor::new_buffered(ciphertext)
        .map_err(|e| SecretsError::Crypto(e.to_string()))?;
    // age 0.12 folded the Recipients/Passphrase enum into one type, so the case
    // Grove does not support is now a question rather than a match arm.
    if decryptor.is_scrypt() {
        return Err(SecretsError::Crypto(
            "passphrase-encrypted, not supported".into(),
        ));
    }
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity.inner as &dyn age::Identity))
        .map_err(|e| SecretsError::Crypto(e.to_string()))?;
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// The .env payload
// ---------------------------------------------------------------------------

/// A project's secret key/value pairs (what ends up in `.env`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSecrets(pub BTreeMap<String, String>);

impl EnvSecrets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }

    /// Wrap the secrets in a versioned envelope for storage.
    fn to_envelope_bytes(&self, version: u64, written_at: i64) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&Envelope {
            version,
            written_at,
            env: self.0.clone(),
        })?)
    }

    /// Render as a `.env` file body.
    pub fn to_dotenv(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.0 {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Store abstraction (mock here; a hosted backend in production)
// ---------------------------------------------------------------------------

/// The zero-knowledge store: it only ever holds ciphertext + public keys.
pub trait SecretStore {
    fn put_env(&self, project: &str, ciphertext: &[u8]) -> Result<()>;
    fn get_env(&self, project: &str) -> Result<Option<Vec<u8>>>;
    fn put_recipients(&self, project: &str, recipients: &[PublicKey]) -> Result<()>;
    fn get_recipients(&self, project: &str) -> Result<Vec<PublicKey>>;
}

/// A filesystem-backed mock of the hosted backend, for local verification.
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn project_dir(&self, project: &str) -> PathBuf {
        self.root.join(project)
    }
}

impl SecretStore for FileStore {
    fn put_env(&self, project: &str, ciphertext: &[u8]) -> Result<()> {
        let dir = self.project_dir(project);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("env.age"), ciphertext)?;
        Ok(())
    }

    fn get_env(&self, project: &str) -> Result<Option<Vec<u8>>> {
        let path = self.project_dir(project).join("env.age");
        match std::fs::read(path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put_recipients(&self, project: &str, recipients: &[PublicKey]) -> Result<()> {
        let dir = self.project_dir(project);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("recipients.json"), serde_json::to_vec(recipients)?)?;
        Ok(())
    }

    fn get_recipients(&self, project: &str) -> Result<Vec<PublicKey>> {
        let path = self.project_dir(project).join("recipients.json");
        match std::fs::read(path) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// The client — ties identity + store together into the team workflow
// ---------------------------------------------------------------------------

/// A member's view of the team secrets, backed by a [`SecretStore`].
pub struct SecretsClient<S: SecretStore> {
    store: S,
    identity: Identity,
    /// What this client has agreed to, locally. `None` disables pinning, which
    /// only the crate's own tests do — a real client always has one.
    pins: Option<pin::PinStore>,
}

impl<S: SecretStore> SecretsClient<S> {
    pub fn new(store: S, identity: Identity) -> Self {
        Self {
            store,
            identity,
            pins: None,
        }
    }

    /// Record membership decisions under `dir`, and enforce them.
    ///
    /// Without this the client trusts whatever recipient list the server hands
    /// back, which is the hole this closes: a compromised backend that appends
    /// its own key gets the next `set` encrypted to it.
    pub fn with_pins(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.pins = Some(pin::PinStore::new(dir));
        self
    }

    /// The recipients to encrypt to — from local agreement, not from the server.
    ///
    /// The server's list is consulted only to seed the pin the first time a
    /// project is seen. After that it is compared, never obeyed: if the two have
    /// diverged this refuses rather than guessing which is right, because both
    /// directions are someone's access changing without a local decision.
    fn agreed_recipients(&self, project: &str) -> Result<Vec<PublicKey>> {
        let offered = self.store.get_recipients(project)?;
        let Some(pins) = &self.pins else {
            return Ok(offered);
        };
        match pins.check(project, &offered) {
            pin::PinCheck::FirstUse => {
                pins.agree(project, &offered)?;
                Ok(offered)
            }
            pin::PinCheck::Match => Ok(offered),
            pin::PinCheck::Diverged { added, removed } => Err(SecretsError::RecipientsChanged {
                project: project.to_string(),
                added,
                removed,
            }),
        }
    }

    pub fn public(&self) -> PublicKey {
        self.identity.public()
    }

    /// Create a project with an initial member set (empty secrets).
    pub fn init_project(&self, project: &str, members: &[PublicKey]) -> Result<()> {
        self.store.put_recipients(project, members)?;
        if let Some(pins) = &self.pins {
            pins.agree(project, members)?;
        }
        self.write_env_versioned(project, &EnvSecrets::new(), members, 1)
    }

    /// Fetch + decrypt this project's secrets (requires membership).
    pub fn pull(&self, project: &str) -> Result<EnvSecrets> {
        self.pull_versioned(project).map(|(env, _)| env)
    }

    /// [`Self::pull`], with the payload's version.
    ///
    /// Refuses a version below the highest already accepted. The server stores
    /// an opaque blob and cannot edit the version inside it, so serving an old
    /// one is the only rollback available to it — and this is what makes that
    /// visible instead of silent.
    fn pull_versioned(&self, project: &str) -> Result<(EnvSecrets, u64)> {
        let Some(ciphertext) = self.store.get_env(project)? else {
            return Ok((EnvSecrets::new(), 0));
        };
        let plaintext = decrypt(&ciphertext, &self.identity)
            .map_err(|_| SecretsError::NotAMember(project.to_string()))?;
        let envelope = Envelope::parse(&plaintext)?;
        if let Some(pins) = &self.pins {
            let highest = pins.highest_version(project);
            if envelope.version < highest {
                return Err(SecretsError::Rollback {
                    project: project.to_string(),
                    served: envelope.version,
                    expected: highest,
                });
            }
            pins.observe_version(project, envelope.version)?;
        }
        Ok((EnvSecrets(envelope.env), envelope.version))
    }

    /// Set one secret and re-encrypt to the current members.
    pub fn set(&self, project: &str, key: &str, value: &str) -> Result<()> {
        let (mut env, version) = self.pull_versioned(project)?;
        env.set(key, value);
        let members = self.agreed_recipients(project)?;
        self.write_env_versioned(project, &env, &members, version + 1)
    }

    /// Add a member: re-encrypt the current secrets to include their key.
    /// Add a member: re-encrypt the current secrets to include their key.
    ///
    /// This is the deliberate act pinning requires — the new list is built from
    /// what was already agreed plus this key, so it also repairs a pin that has
    /// diverged, on the authority of whoever ran the command.
    pub fn add_member(&self, project: &str, member: PublicKey) -> Result<()> {
        let (env, version) = self.pull_versioned(project)?;
        let mut members = self.pinned_or_offered(project)?;
        if !members.contains(&member) {
            members.push(member);
        }
        self.store.put_recipients(project, &members)?;
        if let Some(pins) = &self.pins {
            pins.agree(project, &members)?;
        }
        self.write_env_versioned(project, &env, &members, version + 1)
    }

    /// Remove a member: re-encrypt without their key (they lose access).
    pub fn remove_member(&self, project: &str, member: &PublicKey) -> Result<()> {
        let (env, version) = self.pull_versioned(project)?;
        let members: Vec<PublicKey> = self
            .pinned_or_offered(project)?
            .into_iter()
            .filter(|m| m != member)
            .collect();
        self.store.put_recipients(project, &members)?;
        if let Some(pins) = &self.pins {
            pins.agree(project, &members)?;
        }
        self.write_env_versioned(project, &env, &members, version + 1)
    }

    /// The agreed list if there is one, else what the server offers.
    ///
    /// Used by the membership commands, which are allowed to start from a
    /// diverged state — refusing there would leave no way to repair a pin.
    fn pinned_or_offered(&self, project: &str) -> Result<Vec<PublicKey>> {
        if let Some(pins) = &self.pins {
            if let Some(p) = pins.load(project) {
                return Ok(p.recipients.into_iter().map(PublicKey).collect());
            }
        }
        self.store.get_recipients(project)
    }

    pub fn members(&self, project: &str) -> Result<Vec<PublicKey>> {
        self.store.get_recipients(project)
    }

    fn write_env_versioned(
        &self,
        project: &str,
        env: &EnvSecrets,
        members: &[PublicKey],
        version: u64,
    ) -> Result<()> {
        if members.is_empty() {
            return Err(SecretsError::NoRecipients(project.to_string()));
        }
        let written_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let ciphertext = encrypt(&env.to_envelope_bytes(version, written_at)?, members)?;
        self.store.put_env(project, &ciphertext)?;
        if let Some(pins) = &self.pins {
            pins.observe_version(project, version)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTTP store — talks to the grove-teams backend (feature = "http")
// ---------------------------------------------------------------------------

/// A [`SecretStore`] backed by the hosted grove-teams API. The license key is
/// sent as a bearer token; the server verifies it and enforces entitlement.
/// Only ciphertext + public keys ever leave the machine.
#[cfg(feature = "http")]
pub struct HttpStore {
    base: String,
    token: String,
}

#[cfg(feature = "http")]
impl HttpStore {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        req.set("Authorization", &format!("Bearer {}", self.token))
    }

    fn url(&self, project: &str, leaf: &str) -> String {
        format!("{}/v1/projects/{}/{}", self.base, urlencode(project), leaf)
    }
}

#[cfg(feature = "http")]
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

#[cfg(feature = "http")]
impl SecretStore for HttpStore {
    fn put_env(&self, project: &str, ciphertext: &[u8]) -> Result<()> {
        self.auth(ureq::put(&self.url(project, "env")))
            .send_bytes(ciphertext)
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        Ok(())
    }

    fn get_env(&self, project: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .auth(ureq::get(&self.url(project, "env")))
            .call()
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        if resp.status() == 204 {
            return Ok(None);
        }
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        Ok(Some(buf))
    }

    fn put_recipients(&self, project: &str, recipients: &[PublicKey]) -> Result<()> {
        let keys: Vec<&str> = recipients.iter().map(|r| r.as_str()).collect();
        self.auth(ureq::put(&self.url(project, "recipients")))
            .send_json(ureq::json!(keys))
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        Ok(())
    }

    fn get_recipients(&self, project: &str) -> Result<Vec<PublicKey>> {
        let resp = self
            .auth(ureq::get(&self.url(project, "recipients")))
            .call()
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        let keys: Vec<String> = resp
            .into_json()
            .map_err(|e| SecretsError::Http(e.to_string()))?;
        Ok(keys.into_iter().map(PublicKey).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "grove-secrets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// An age file produced by the `age` 0.10 crate Grove shipped before, with
    /// the identity that can open it. Kept as bytes so the upgrade to 0.12 has to
    /// prove it can still read what earlier versions of Grove wrote — these are
    /// real secrets on real users' disks, and a format regression would be
    /// silent until someone needed their `.env` back.
    const V0_10_SECRET: &str =
        "AGE-SECRET-KEY-1N9YZV3KCP6RCYGKFAGUSPL7K0KAW8KLFT4WLTPDE6CYGSXKXGV3SZP5LRW";
    const V0_10_CIPHERTEXT_HEX: &str = concat!(
        "6167652d656e6372797074696f6e2e6f72672f76310a2d3e20583235353139204d3736557533",
        "4a6d74374a4376344656686b6d3637484c63795276713649456e4c6354374e3065594d516f0a",
        "557074505a514f734157524350792b707071494138635432442f76544c496e4c447259683771",
        "74747368590a2d3e206d31404d372d6772656173650a70786b53387972374a3472625a336c39",
        "41626564426b41797441672f5252716f2f346c7242527a512b63380a2d2d2d2039343646334b",
        "6c4b30654b794f7666396b5655386f566477616c33684e36714549506f66315378367572450a",
        "e1503e7151b2d4d0fb9960c0383cfe392461e04a3a4e4700d95def1dc241f0fa1d434d69e7f3",
        "ac1947e6a78f34eb68dc902f4e7579762b46b31c437079524e2f172db626f890b0",
    );

    #[test]
    fn reads_files_written_by_the_previous_age_version() {
        let bytes: Vec<u8> = (0..V0_10_CIPHERTEXT_HEX.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&V0_10_CIPHERTEXT_HEX[i..i + 2], 16).unwrap())
            .collect();
        let id = Identity::from_secret(V0_10_SECRET).unwrap();
        let plain = decrypt(&bytes, &id).expect("age 0.12 must read an age 0.10 file");
        assert_eq!(
            String::from_utf8(plain).unwrap(),
            "DB_PASSWORD=hunter2\nAPP_KEY=base64:abc\n"
        );
    }

    #[test]
    fn identity_roundtrips() {
        let id = Identity::generate();
        let restored = Identity::from_secret(&id.secret_string()).unwrap();
        assert_eq!(id.public(), restored.public());
        assert!(id.public().as_str().starts_with("age1"));
    }

    #[test]
    fn two_members_share_secrets_and_removal_locks_out() {
        let root = tmp();

        // Two teammates, each with their own identity.
        let alice = Identity::generate();
        let bob = Identity::generate();
        let alice_pub = alice.public();
        let bob_pub = bob.public();

        let alice_client = SecretsClient::new(FileStore::new(&root), alice);
        // Bob uses the SAME store (simulating the shared backend).
        let bob_client = SecretsClient::new(FileStore::new(&root), bob);

        // Alice creates the project with both members and sets a secret.
        alice_client
            .init_project("billing", &[alice_pub.clone(), bob_pub.clone()])
            .unwrap();
        alice_client
            .set("billing", "DB_PASSWORD", "s3cr3t-value")
            .unwrap();

        // 1) The store never holds plaintext.
        let on_disk = std::fs::read(root.join("billing/env.age")).unwrap();
        assert!(
            !String::from_utf8_lossy(&on_disk).contains("s3cr3t-value"),
            "ciphertext must not contain the plaintext secret"
        );
        assert!(on_disk.starts_with(b"age-encryption.org/v1"));

        // 2) Both current members can decrypt.
        assert_eq!(
            alice_client.pull("billing").unwrap().get("DB_PASSWORD"),
            Some("s3cr3t-value")
        );
        assert_eq!(
            bob_client.pull("billing").unwrap().get("DB_PASSWORD"),
            Some("s3cr3t-value")
        );

        // 3) Remove Bob and re-encrypt → Bob is locked out, Alice still reads.
        alice_client.remove_member("billing", &bob_pub).unwrap();
        assert!(
            matches!(bob_client.pull("billing"), Err(SecretsError::NotAMember(_))),
            "a removed member must not be able to decrypt"
        );
        assert_eq!(
            alice_client.pull("billing").unwrap().get("DB_PASSWORD"),
            Some("s3cr3t-value")
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod trust_tests {
    use super::*;
    use std::path::Path;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grove-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A client with pinning on, backed by a local store, as the CLI builds it.
    fn client(dir: &Path, id: Identity) -> SecretsClient<FileStore> {
        SecretsClient::new(FileStore::new(dir.join("store")), id).with_pins(dir.join("pins"))
    }

    /// The finding, end to end: a backend that appends its own key to the
    /// recipient list must not get the next `set` encrypted to it.
    #[test]
    fn a_backend_that_adds_a_recipient_is_refused() {
        let dir = scratch("added");
        let me = Identity::generate();
        let my_key = me.public();
        let attacker = Identity::generate().public();
        let c = client(&dir, me);

        c.init_project("proj", std::slice::from_ref(&my_key))
            .unwrap();
        c.set("proj", "APP_KEY", "original").unwrap();

        // The backend rewrites the recipient list behind our back.
        let store = FileStore::new(dir.join("store"));
        store
            .put_recipients("proj", &[my_key, attacker.clone()])
            .unwrap();

        let err = c.set("proj", "APP_KEY", "secret").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not match"), "{msg}");
        assert!(msg.contains(attacker.as_str()), "the added key: {msg}");

        // And the stored secret is still the one from before the attempt, so
        // nothing was encrypted to the attacker.
        assert_eq!(c.pull("proj").unwrap().get("APP_KEY"), Some("original"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A silent removal is refused too — someone losing access without a local
    /// decision is the same class of problem.
    #[test]
    fn a_backend_that_drops_a_recipient_is_refused() {
        let dir = scratch("removed");
        let me = Identity::generate();
        let my_key = me.public();
        let mate = Identity::generate().public();
        let c = client(&dir, me);

        c.init_project("proj", &[my_key.clone(), mate]).unwrap();
        FileStore::new(dir.join("store"))
            .put_recipients("proj", &[my_key])
            .unwrap();

        let err = c.set("proj", "K", "v").unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Adding a teammate deliberately is the way through, and it repairs the pin.
    #[test]
    fn sharing_deliberately_records_the_new_member() {
        let dir = scratch("share");
        let me = Identity::generate();
        let my_key = me.public();
        let mate = Identity::generate();
        let mate_key = mate.public();
        let c = client(&dir, me);

        c.init_project("proj", &[my_key]).unwrap();
        c.add_member("proj", mate_key).unwrap();

        // Now a plain set works, and the teammate can read it.
        c.set("proj", "APP_KEY", "shared").unwrap();
        let theirs = client(&dir, mate).pull("proj").unwrap();
        assert_eq!(theirs.get("APP_KEY"), Some("shared"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Replay: the backend serves yesterday's blob. The version inside the
    /// ciphertext is one it cannot edit, so the client notices.
    #[test]
    fn an_old_payload_is_refused() {
        let dir = scratch("rollback");
        let me = Identity::generate();
        let my_key = me.public();
        let c = client(&dir, me);
        let store = FileStore::new(dir.join("store"));

        c.init_project("proj", &[my_key]).unwrap();
        c.set("proj", "APP_KEY", "rotated-away").unwrap();
        let old = store.get_env("proj").unwrap().unwrap();
        c.set("proj", "APP_KEY", "current").unwrap();

        // Put the earlier ciphertext back.
        store.put_env("proj", &old).unwrap();

        let err = c.pull("proj").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("older than"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Payloads written before the envelope existed are bare maps. They must
    /// still read, as version 0, so an upgrade does not lock anyone out.
    #[test]
    fn a_pre_envelope_payload_still_reads() {
        let dir = scratch("legacy");
        let me = Identity::generate();
        let my_key = me.public();
        let c = client(&dir, me);
        let store = FileStore::new(dir.join("store"));

        // A blob in the old shape, encrypted to us.
        let legacy = serde_json::to_vec(&BTreeMap::from([(
            "APP_KEY".to_string(),
            "from-before".to_string(),
        )]))
        .unwrap();
        store
            .put_recipients("proj", std::slice::from_ref(&my_key))
            .unwrap();
        store
            .put_env("proj", &encrypt(&legacy, &[my_key]).unwrap())
            .unwrap();

        assert_eq!(c.pull("proj").unwrap().get("APP_KEY"), Some("from-before"));
        // And the first write after the upgrade moves it forward.
        c.set("proj", "APP_KEY", "after").unwrap();
        assert_eq!(c.pull("proj").unwrap().get("APP_KEY"), Some("after"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// First use trusts what the server says — there is nothing else to go on —
    /// and remembers it, so the *second* use is protected.
    #[test]
    fn first_use_seeds_the_pin() {
        let dir = scratch("tofu");
        let me = Identity::generate();
        let my_key = me.public();
        let attacker = Identity::generate().public();
        let store = FileStore::new(dir.join("store"));
        store
            .put_recipients("proj", std::slice::from_ref(&my_key))
            .unwrap();

        let c = client(&dir, me);
        c.set("proj", "K", "v").unwrap(); // seeds the pin

        store.put_recipients("proj", &[my_key, attacker]).unwrap();
        assert!(
            c.set("proj", "K", "v2").is_err(),
            "the pin seeded on first use must protect the second"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

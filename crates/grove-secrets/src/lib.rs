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
    let boxed: Vec<Box<dyn age::Recipient + Send>> = recipients
        .iter()
        .map(|r| {
            r.to_recipient()
                .map(|x| Box::new(x) as Box<dyn age::Recipient + Send>)
        })
        .collect::<Result<_>>()?;

    let encryptor = age::Encryptor::with_recipients(boxed)
        .ok_or_else(|| SecretsError::Crypto("no recipients".into()))?;
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
    let decryptor =
        match age::Decryptor::new(ciphertext).map_err(|e| SecretsError::Crypto(e.to_string()))? {
            age::Decryptor::Recipients(d) => d,
            age::Decryptor::Passphrase(_) => {
                return Err(SecretsError::Crypto(
                    "passphrase-encrypted, not supported".into(),
                ))
            }
        };
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

    fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.0)?)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        Ok(Self(serde_json::from_slice(bytes)?))
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
}

impl<S: SecretStore> SecretsClient<S> {
    pub fn new(store: S, identity: Identity) -> Self {
        Self { store, identity }
    }

    pub fn public(&self) -> PublicKey {
        self.identity.public()
    }

    /// Create a project with an initial member set (empty secrets).
    pub fn init_project(&self, project: &str, members: &[PublicKey]) -> Result<()> {
        self.store.put_recipients(project, members)?;
        self.write_env(project, &EnvSecrets::new(), members)
    }

    /// Fetch + decrypt this project's secrets (requires membership).
    pub fn pull(&self, project: &str) -> Result<EnvSecrets> {
        let Some(ciphertext) = self.store.get_env(project)? else {
            return Ok(EnvSecrets::new());
        };
        let plaintext = decrypt(&ciphertext, &self.identity)
            .map_err(|_| SecretsError::NotAMember(project.to_string()))?;
        EnvSecrets::from_bytes(&plaintext)
    }

    /// Set one secret and re-encrypt to the current members.
    pub fn set(&self, project: &str, key: &str, value: &str) -> Result<()> {
        let mut env = self.pull(project)?;
        env.set(key, value);
        let members = self.store.get_recipients(project)?;
        self.write_env(project, &env, &members)
    }

    /// Add a member: re-encrypt the current secrets to include their key.
    pub fn add_member(&self, project: &str, member: PublicKey) -> Result<()> {
        let env = self.pull(project)?;
        let mut members = self.store.get_recipients(project)?;
        if !members.contains(&member) {
            members.push(member);
        }
        self.store.put_recipients(project, &members)?;
        self.write_env(project, &env, &members)
    }

    /// Remove a member: re-encrypt without their key (they lose access).
    pub fn remove_member(&self, project: &str, member: &PublicKey) -> Result<()> {
        let env = self.pull(project)?;
        let members: Vec<PublicKey> = self
            .store
            .get_recipients(project)?
            .into_iter()
            .filter(|m| m != member)
            .collect();
        self.store.put_recipients(project, &members)?;
        self.write_env(project, &env, &members)
    }

    pub fn members(&self, project: &str) -> Result<Vec<PublicKey>> {
        self.store.get_recipients(project)
    }

    fn write_env(&self, project: &str, env: &EnvSecrets, members: &[PublicKey]) -> Result<()> {
        if members.is_empty() {
            return Err(SecretsError::NoRecipients(project.to_string()));
        }
        let ciphertext = encrypt(&env.to_bytes()?, members)?;
        self.store.put_env(project, &ciphertext)
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

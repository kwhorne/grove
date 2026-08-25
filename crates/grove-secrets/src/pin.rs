//! Remembering, locally, who a project's secrets may be encrypted to.
//!
//! The crypto in this crate is sound — real age/X25519, authenticated
//! encryption, nothing home-made. The weakness was never the algorithms; it was
//! that the *server* decided the recipient list:
//!
//! ```ignore
//! let members = self.store.get_recipients(project)?;   // the server's answer
//! self.write_env(project, &env, &members)              // …and we encrypt to it
//! ```
//!
//! A compromised backend adds its own public key to that list, and the next
//! `grove secret set` encrypts the whole `.env` to the attacker. Nothing about
//! that is detectable from the ciphertext, because it is a perfectly valid
//! encryption to a recipient the client was told about.
//!
//! So the client keeps its own record. The server's list is used to *seed* that
//! record the first time a project is seen, and never again to decide anything:
//! from then on the recipients are whoever the client last deliberately agreed
//! to, changed only through `add_member` / `remove_member`.
//!
//! The same file carries the highest payload version seen, which is what makes a
//! rollback visible — see [`crate::Envelope`].
//!
//! ## What this costs
//!
//! A legitimate new teammate is refused until someone runs `grove secret share`.
//! That is the point rather than a side effect: the whole value is that widening
//! the set is a decision somebody makes, not something a server can announce.
//!
//! ## Where it lives
//!
//! In the user's own `~/.grove`, not `$GROVE_HOME`. The root daemon has no part
//! in secret sync, and a record of who you trust should not sit in a tree owned
//! by a different user — the same reasoning that put the cpx PHAR there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{PublicKey, Result, SecretsError};

/// What the client remembers about one project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectPin {
    /// The recipients this client agreed to, newest agreement wins.
    pub recipients: Vec<String>,
    /// Highest payload version seen. A lower one is a rollback.
    #[serde(default)]
    pub highest_version: u64,
}

impl ProjectPin {
    fn keys(&self) -> BTreeSet<&str> {
        self.recipients.iter().map(|s| s.as_str()).collect()
    }
}

/// Per-project pins on disk.
#[derive(Debug, Clone)]
pub struct PinStore {
    dir: PathBuf,
}

/// How the server's recipient list compares to what the client agreed to.
#[derive(Debug, PartialEq, Eq)]
pub enum PinCheck {
    /// Nothing recorded yet — trust on first use, and remember it.
    FirstUse,
    /// The server agrees with the client.
    Match,
    /// It does not. Both directions are reported, because both matter: an
    /// addition is someone gaining access, a removal is someone losing it, and
    /// neither should happen without a local decision.
    Diverged {
        added: Vec<String>,
        removed: Vec<String>,
    },
}

impl PinStore {
    /// Pins under `dir`, which the caller places in the user's own home.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, project: &str) -> PathBuf {
        // Project names reach us from the command line, so keep them from
        // walking out of the directory.
        let safe: String = project
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    /// What the client last agreed to for `project`, if anything.
    pub fn load(&self, project: &str) -> Option<ProjectPin> {
        let raw = std::fs::read_to_string(self.path(project)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Record `pin` for `project`.
    pub fn save(&self, project: &str, pin: &ProjectPin) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(SecretsError::Io)?;
        let body = serde_json::to_string_pretty(pin)?;
        write_private(&self.path(project), body.as_bytes()).map_err(SecretsError::Io)
    }

    /// Compare the server's recipient list against what was agreed.
    pub fn check(&self, project: &str, server: &[PublicKey]) -> PinCheck {
        let Some(pin) = self.load(project) else {
            return PinCheck::FirstUse;
        };
        let pinned = pin.keys();
        let offered: BTreeSet<&str> = server.iter().map(|k| k.as_str()).collect();
        if pinned == offered {
            return PinCheck::Match;
        }
        PinCheck::Diverged {
            added: offered
                .difference(&pinned)
                .map(|s| (*s).to_string())
                .collect(),
            removed: pinned
                .difference(&offered)
                .map(|s| (*s).to_string())
                .collect(),
        }
    }

    /// Replace the agreed recipient list, keeping the version watermark.
    pub fn agree(&self, project: &str, recipients: &[PublicKey]) -> Result<()> {
        let mut pin = self.load(project).unwrap_or_default();
        pin.recipients = recipients.iter().map(|k| k.as_str().to_string()).collect();
        self.save(project, &pin)
    }

    /// Raise the version watermark. Never lowers it.
    pub fn observe_version(&self, project: &str, version: u64) -> Result<()> {
        let mut pin = self.load(project).unwrap_or_default();
        if version <= pin.highest_version {
            return Ok(());
        }
        pin.highest_version = version;
        self.save(project, &pin)
    }

    /// The highest version this client has accepted for `project`.
    pub fn highest_version(&self, project: &str) -> u64 {
        self.load(project).map(|p| p.highest_version).unwrap_or(0)
    }
}

/// Write a file only its owner can read.
///
/// A pin is not a secret in itself — the keys in it are public — but it records
/// who you trust, and leaking that is leaking your team roster.
#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents)
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> PinStore {
        let dir = std::env::temp_dir().join(format!("grove-pin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        PinStore::new(dir)
    }

    fn key(n: u8) -> PublicKey {
        // Not a real age key; the pin store only ever compares strings.
        PublicKey(format!("age1testkey{n:02}"))
    }

    #[test]
    fn an_unknown_project_is_trusted_on_first_use() {
        let s = store("first");
        assert_eq!(s.check("proj", &[key(1), key(2)]), PinCheck::FirstUse);
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    #[test]
    fn an_agreed_list_matches_regardless_of_order() {
        let s = store("order");
        s.agree("proj", &[key(1), key(2)]).unwrap();
        assert_eq!(s.check("proj", &[key(2), key(1)]), PinCheck::Match);
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    /// The finding: a backend that adds its own key must be caught, and the
    /// offending key named.
    #[test]
    fn an_added_recipient_is_reported() {
        let s = store("added");
        s.agree("proj", &[key(1)]).unwrap();
        match s.check("proj", &[key(1), key(9)]) {
            PinCheck::Diverged { added, removed } => {
                assert_eq!(added, vec![key(9).as_str().to_string()]);
                assert!(removed.is_empty());
            }
            other => panic!("expected divergence, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    /// A silent *removal* matters too: it is someone losing access without a
    /// local decision, and it would go unnoticed if only additions were checked.
    #[test]
    fn a_removed_recipient_is_reported() {
        let s = store("removed");
        s.agree("proj", &[key(1), key(2)]).unwrap();
        match s.check("proj", &[key(1)]) {
            PinCheck::Diverged { added, removed } => {
                assert!(added.is_empty());
                assert_eq!(removed, vec![key(2).as_str().to_string()]);
            }
            other => panic!("expected divergence, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    #[test]
    fn agreeing_again_replaces_the_list_but_keeps_the_watermark() {
        let s = store("agree");
        s.agree("proj", &[key(1)]).unwrap();
        s.observe_version("proj", 7).unwrap();
        s.agree("proj", &[key(1), key(2)]).unwrap();

        assert_eq!(s.check("proj", &[key(1), key(2)]), PinCheck::Match);
        assert_eq!(
            s.highest_version("proj"),
            7,
            "changing membership must not reset rollback protection"
        );
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    #[test]
    fn the_version_watermark_only_rises() {
        let s = store("version");
        s.observe_version("proj", 5).unwrap();
        s.observe_version("proj", 3).unwrap();
        assert_eq!(
            s.highest_version("proj"),
            5,
            "an older version must not win"
        );
        s.observe_version("proj", 9).unwrap();
        assert_eq!(s.highest_version("proj"), 9);
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    /// Project names come from the command line.
    #[test]
    fn a_project_name_cannot_escape_the_pin_directory() {
        let s = store("escape");
        for hostile in ["../../etc/passwd", "a/b", "..", "with spaces"] {
            let p = s.path(hostile);
            assert_eq!(
                p.parent(),
                Some(s.dir.as_path()),
                "{hostile:?} escaped to {p:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&s.dir);
    }

    #[test]
    fn pins_survive_a_round_trip() {
        let s = store("roundtrip");
        s.agree("proj", &[key(1), key(2)]).unwrap();
        s.observe_version("proj", 4).unwrap();
        let reloaded = PinStore::new(s.dir.clone());
        assert_eq!(reloaded.check("proj", &[key(1), key(2)]), PinCheck::Match);
        assert_eq!(reloaded.highest_version("proj"), 4);
        let _ = std::fs::remove_dir_all(&s.dir);
    }
}

//! Point-in-time database snapshots for Grove's bundled databases.
//!
//! Snapshots live under `$GROVE_HOME/snapshots/` with a small JSON index, so you
//! can snapshot before a risky migration and roll back in one command. Grove
//! owns the DB service, so this needs zero configuration. MySQL and PostgreSQL
//! snapshots are plain SQL dumps; an ElyraSQL snapshot is a complete copy of its
//! single database file (`.edb`), taken hot via `BACKUP TO`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::OffsetDateTime;

use grove_core::paths::GrovePaths;

use crate::manager::{Result, ServiceError, ServiceManager};

/// A stored snapshot's metadata (also surfaced over IPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub engine: String,
    pub database: String,
    pub file: String,
    pub created: String,
    pub note: String,
    pub bytes: u64,
}

/// Reads/writes the snapshot index and drives dump/restore.
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(paths: &GrovePaths) -> Self {
        Self {
            dir: paths.base().join("snapshots"),
        }
    }

    fn index(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    pub fn list(&self) -> Vec<Snapshot> {
        grove_core::securefs::read_json_or_quarantine(&self.index())
    }

    fn save(&self, list: &[Snapshot]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".into());
        grove_core::securefs::write_public_atomic(&self.index(), json)?;
        Ok(())
    }

    /// Snapshot a database (MySQL may pass `None` for all user databases).
    pub fn create(
        &self,
        services: &ServiceManager,
        engine: &str,
        database: Option<&str>,
        note: &str,
    ) -> Result<Snapshot> {
        std::fs::create_dir_all(&self.dir)?;
        // Held until the snapshot is in the index (or has failed), so a second
        // snapshot started in the same second cannot be handed the same id.
        let reservation = Reservation::new(&self.list());
        let id = reservation.id.clone();
        // ElyraSQL has one database and the snapshot is the whole file.
        let label = if engine == "elyrasql" {
            crate::manager::ELYRASQL_DATABASE
        } else {
            database.unwrap_or("(all)")
        };
        let slug: String = label
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let file = snapshot_file_name(engine, &slug, &id);
        let path = self.dir.join(&file);

        match engine {
            "mysql" => services.snapshot_mysql(database, &path)?,
            "elyrasql" => services.snapshot_elyrasql(&path)?,
            "postgres" => {
                let db = database.ok_or_else(|| {
                    ServiceError::Init("PostgreSQL snapshots need a database name (--db)".into())
                })?;
                services.snapshot_postgres(db, &path)?;
            }
            other => return Err(ServiceError::Unknown(other.into())),
        }

        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let snap = Snapshot {
            id,
            engine: engine.into(),
            database: label.into(),
            file,
            created: now_iso(),
            note: note.into(),
            bytes,
        };
        let mut list = self.list();
        list.push(snap.clone());
        self.save(&list)?;
        Ok(snap)
    }

    pub fn restore(&self, services: &ServiceManager, id: &str) -> Result<Snapshot> {
        let snap = self
            .list()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| ServiceError::NoSnapshot(id.to_string()))?;
        let path = self.dir.join(&snap.file);
        match snap.engine.as_str() {
            "mysql" => services.restore_mysql(&path)?,
            "postgres" => services.restore_postgres(&path)?,
            "elyrasql" => services.restore_elyrasql(&path)?,
            other => return Err(ServiceError::Unknown(other.into())),
        }
        Ok(snap)
    }

    pub fn remove(&self, id: &str) -> Result<Snapshot> {
        let mut list = self.list();
        let idx = list
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| ServiceError::NoSnapshot(id.to_string()))?;
        let snap = list.remove(idx);
        let _ = std::fs::remove_file(self.dir.join(&snap.file));
        self.save(&list)?;
        Ok(snap)
    }
}

/// The file a snapshot is stored in. The extension says what it is: a SQL dump
/// for the servers that dump, a database file for the one that copies.
fn snapshot_file_name(engine: &str, slug: &str, id: &str) -> String {
    let ext = if engine == "elyrasql" { "edb" } else { "sql" };
    format!("{engine}-{slug}-{id}.{ext}")
}

fn unique_id() -> String {
    let now = OffsetDateTime::now_utc();
    let fmt = format_description!("[year][month][day]-[hour][minute][second]");
    now.format(&fmt)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}

/// The first of `base`, `base-2`, `base-3`, … that `taken` does not claim.
///
/// Ids are to-the-second timestamps, which is what makes them readable — and
/// what made two snapshots in the same second share one. They shared the file
/// name too, so the second dump overwrote the first on disk while the index
/// kept both entries, pointing at the same file under two different notes.
/// A sandboxed migration takes a snapshot before it runs; two of those in quick
/// succession was enough.
fn next_free_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always has a free suffix")
}

/// Ids handed out but not yet written to the index. The daemon serves requests
/// concurrently, and two creates can read the same index before either saves.
static RESERVED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// An id nobody else has, released when the snapshot is recorded or abandoned.
struct Reservation {
    id: String,
}

impl Reservation {
    fn new(existing: &[Snapshot]) -> Self {
        let mut reserved = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
        let id = next_free_id(&unique_id(), |c| {
            existing.iter().any(|s| s.id == c) || reserved.iter().any(|r| r == c)
        });
        reserved.push(id.clone());
        Self { id }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut reserved = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
        reserved.retain(|r| r != &self.id);
    }
}

fn now_iso() -> String {
    let now = OffsetDateTime::now_utc();
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");
    now.format(&fmt).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_id_gets_the_next_free_suffix() {
        assert_eq!(
            next_free_id("20260923-120000", |_| false),
            "20260923-120000"
        );
        assert_eq!(
            next_free_id("20260923-120000", |c| c == "20260923-120000"),
            "20260923-120000-2"
        );
        let taken = ["20260923-120000", "20260923-120000-2", "20260923-120000-3"];
        assert_eq!(
            next_free_id("20260923-120000", |c| taken.contains(&c)),
            "20260923-120000-4"
        );
    }

    /// Two snapshots started at the same moment, as two sandboxed migrations
    /// in a row do: neither is in the index yet, and they must still get
    /// different ids — and therefore different files.
    #[test]
    fn concurrent_reservations_never_share_an_id() {
        let first = Reservation::new(&[]);
        let second = Reservation::new(&[]);
        let third = Reservation::new(&[]);
        assert_ne!(first.id, second.id);
        assert_ne!(second.id, third.id);
        assert_ne!(first.id, third.id);
        // An id already in the index is never handed out either.
        let existing = Snapshot {
            id: unique_id(),
            engine: "mysql".into(),
            database: "app".into(),
            file: "x.sql".into(),
            created: String::new(),
            note: String::new(),
            bytes: 0,
        };
        let fourth = Reservation::new(std::slice::from_ref(&existing));
        assert_ne!(fourth.id, existing.id);
    }

    #[test]
    fn index_roundtrip_and_remove() {
        let tmp = std::env::temp_dir().join(format!("grove-snap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = GrovePaths::with_base(&tmp);
        let store = SnapshotStore::new(&paths);

        assert!(store.list().is_empty());

        // Seed one entry with a backing file, as create() would.
        std::fs::create_dir_all(&store.dir).unwrap();
        let file = "mysql-app-20260101-000000.sql";
        std::fs::write(store.dir.join(file), b"-- dump").unwrap();
        let snap = Snapshot {
            id: "20260101-000000".into(),
            engine: "mysql".into(),
            database: "app".into(),
            file: file.into(),
            created: "2026-01-01 00:00:00 UTC".into(),
            note: "before migrate".into(),
            bytes: 7,
        };
        store.save(std::slice::from_ref(&snap)).unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "20260101-000000");
        assert_eq!(listed[0].note, "before migrate");

        let removed = store.remove("20260101-000000").unwrap();
        assert_eq!(removed.id, "20260101-000000");
        assert!(store.list().is_empty());
        assert!(!store.dir.join(file).exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn snapshot_files_carry_the_extension_of_what_they_hold() {
        assert_eq!(snapshot_file_name("mysql", "app", "1"), "mysql-app-1.sql");
        assert_eq!(
            snapshot_file_name("postgres", "app", "1"),
            "postgres-app-1.sql"
        );
        assert_eq!(
            snapshot_file_name("elyrasql", "elyra", "1"),
            "elyrasql-elyra-1.edb",
            "an ElyraSQL snapshot is the database file itself, not a dump"
        );
    }
}

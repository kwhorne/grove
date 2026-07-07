//! Point-in-time database snapshots for Grove's bundled MySQL / PostgreSQL.
//!
//! Snapshots are plain SQL dumps stored under `$GROVE_HOME/snapshots/` with a
//! small JSON index, so you can snapshot before a risky migration and roll back
//! in one command. Grove owns the DB service, so this needs zero configuration.

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
        std::fs::read_to_string(self.index())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, list: &[Snapshot]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".into());
        std::fs::write(self.index(), json)?;
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
        let id = unique_id();
        let label = database.unwrap_or("(all)");
        let slug: String = label
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let file = format!("{engine}-{slug}-{id}.sql");
        let path = self.dir.join(&file);

        match engine {
            "mysql" => services.snapshot_mysql(database, &path)?,
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
            .ok_or_else(|| ServiceError::Unknown(format!("snapshot {id}")))?;
        let path = self.dir.join(&snap.file);
        match snap.engine.as_str() {
            "mysql" => services.restore_mysql(&path)?,
            "postgres" => services.restore_postgres(&path)?,
            other => return Err(ServiceError::Unknown(other.into())),
        }
        Ok(snap)
    }

    pub fn remove(&self, id: &str) -> Result<Snapshot> {
        let mut list = self.list();
        let idx = list
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| ServiceError::Unknown(format!("snapshot {id}")))?;
        let snap = list.remove(idx);
        let _ = std::fs::remove_file(self.dir.join(&snap.file));
        self.save(&list)?;
        Ok(snap)
    }
}

fn unique_id() -> String {
    let now = OffsetDateTime::now_utc();
    let fmt = format_description!("[year][month][day]-[hour][minute][second]");
    now.format(&fmt)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
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
}

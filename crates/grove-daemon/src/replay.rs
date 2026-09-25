//! Replaying a request against the same data every time.
//!
//! `grove replay <id>` sends a captured request again, but a request that
//! writes — a checkout, a form post, a webhook — finds the database it changed
//! the last time, so the second run is not the first run again: the order
//! exists, the email is taken, the webhook was already applied. With
//! `--same-data` the first replay takes a snapshot of the site's database and
//! every later one puts it back first, so each run starts from the same place
//! and a fix can be tried again and again.
//!
//! The starting point is the database as it was at that first `--same-data`
//! replay, not as it was when the request was first made: Grove does not
//! snapshot on every request. Baselines are kept in memory, keyed by request
//! id, like the request log itself; a daemon restart forgets both, and the
//! files left behind are swept at startup.

use std::path::{Path, PathBuf};

use grove_core::paths::GrovePaths;
use grove_services::ServiceManager;

use crate::branches::{resolve_database, Engine};

/// The note snapshots taken here carry, so a sweep can tell them apart.
const NOTE: &str = "replay baseline for request";

/// Where one request's baseline lives.
pub enum Baseline {
    /// A snapshot in the ordinary snapshot store.
    Mysql { snapshot: String, database: String },
    /// A copy of the database file (and its `-wal`/`-shm`, if any). `copy`
    /// may not exist: the site had no database file yet, and restoring then
    /// means removing the one the replays made.
    Sqlite { live: PathBuf, copy: PathBuf },
}

impl Baseline {
    fn describe(&self) -> String {
        match self {
            Baseline::Mysql { database, .. } => format!("MySQL `{database}`"),
            Baseline::Sqlite { live, .. } => format!("SQLite {}", live.display()),
        }
    }
}

fn sqlite_dir(paths: &GrovePaths) -> PathBuf {
    paths.base().join("snapshots").join("replay")
}

/// The file and its sidecars, in the order SQLite would want them gone.
fn with_sidecars(file: &Path) -> [PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = file.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    [file.to_path_buf(), with("-wal"), with("-shm")]
}

fn remove_sqlite(file: &Path) -> std::io::Result<()> {
    for f in with_sidecars(file) {
        match std::fs::remove_file(&f) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Take the baseline for request `id` of the site at `project`.
pub fn take(
    paths: &GrovePaths,
    services: &ServiceManager,
    project: &Path,
    id: u64,
) -> anyhow::Result<Baseline> {
    let (engine, database) = resolve_database(project, services)?;
    match engine {
        Engine::Mysql => {
            let snap = grove_services::SnapshotStore::new(paths).create(
                services,
                "mysql",
                Some(&database),
                &format!("{NOTE} {id}"),
            )?;
            Ok(Baseline::Mysql {
                snapshot: snap.id,
                database,
            })
        }
        Engine::Sqlite => {
            let live = PathBuf::from(database);
            let name = live
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_else(|| "database.sqlite".into());
            let copy = sqlite_dir(paths).join(id.to_string()).join(name);
            // A leftover from an earlier daemon holds a different moment.
            if let Some(dir) = copy.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
            grove_services::sqlite_copy(&live, &copy)?;
            Ok(Baseline::Sqlite { live, copy })
        }
    }
}

/// Put the database back the way it was when `baseline` was taken.
pub fn restore(
    paths: &GrovePaths,
    services: &ServiceManager,
    baseline: &Baseline,
) -> anyhow::Result<()> {
    match baseline {
        Baseline::Mysql { snapshot, .. } => {
            grove_services::SnapshotStore::new(paths).restore(services, snapshot)?;
        }
        Baseline::Sqlite { live, copy } => {
            remove_sqlite(live)?;
            grove_services::sqlite_copy(copy, live)?;
        }
    }
    Ok(())
}

/// Throw a baseline away.
pub fn forget(paths: &GrovePaths, baseline: &Baseline) {
    match baseline {
        Baseline::Mysql { snapshot, .. } => {
            let _ = grove_services::SnapshotStore::new(paths).remove(snapshot);
        }
        Baseline::Sqlite { copy, .. } => {
            if let Some(dir) = copy.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

/// Remove baselines an earlier daemon left behind. Their request ids died
/// with it, so nothing can name them any more.
pub fn sweep(paths: &GrovePaths) {
    let store = grove_services::SnapshotStore::new(paths);
    for snap in store.list() {
        if snap.note.starts_with(NOTE) {
            let _ = store.remove(&snap.id);
        }
    }
    let _ = std::fs::remove_dir_all(sqlite_dir(paths));
}

/// The line `grove replay --same-data` prints about the data.
pub fn said(baseline: &Baseline, taken: bool) -> String {
    if taken {
        format!(
            "baseline taken of {}; later --same-data replays start from it",
            baseline.describe()
        )
    } else {
        format!("{} put back to the baseline first", baseline.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "grove-replay-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The SQLite round trip: what a replay wrote is gone after a restore,
    /// sidecars included, and a stale `-wal` cannot replay itself back in.
    #[test]
    fn a_sqlite_baseline_undoes_what_the_replay_wrote() {
        let dir = scratch("sqlite");
        let live = dir.join("database.sqlite");
        std::fs::write(&live, b"before").unwrap();
        let copy = dir.join("baseline").join("database.sqlite");
        grove_services::sqlite_copy(&live, &copy).unwrap();
        let baseline = Baseline::Sqlite {
            live: live.clone(),
            copy,
        };

        std::fs::write(&live, b"after").unwrap();
        std::fs::write(dir.join("database.sqlite-wal"), b"pending").unwrap();
        let paths = GrovePaths::with_base(dir.join("home"));
        let services = ServiceManager::new(paths.clone());
        restore(&paths, &services, &baseline).unwrap();

        assert_eq!(std::fs::read(&live).unwrap(), b"before");
        assert!(!dir.join("database.sqlite-wal").exists());
        // And it holds for the next run too.
        std::fs::write(&live, b"again").unwrap();
        restore(&paths, &services, &baseline).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"before");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A site with no database file yet gets none back: restoring removes
    /// the one the replay created.
    #[test]
    fn a_missing_file_is_restored_as_missing() {
        let dir = scratch("missing");
        let live = dir.join("database.sqlite");
        let baseline = Baseline::Sqlite {
            live: live.clone(),
            copy: dir.join("baseline").join("database.sqlite"),
        };
        std::fs::write(&live, b"made by the replay").unwrap();
        let paths = GrovePaths::with_base(dir.join("home"));
        let services = ServiceManager::new(paths.clone());
        restore(&paths, &services, &baseline).unwrap();
        assert!(!live.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

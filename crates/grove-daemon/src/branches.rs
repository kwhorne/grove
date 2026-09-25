//! A site's database that follows its git branch.
//!
//! The mechanics of parking one branch's data and bringing another's back live
//! in `grove_services::branches`. This is everything around them: which sites
//! opted in, noticing that a checkout changed, holding the site's traffic while
//! its tables move, and making sure a switch that was interrupted — a crash, a
//! reboot, a killed daemon — is finished or undone on the next start rather
//! than left half-done.
//!
//! ## When a switch happens
//!
//! Every second, for each site that follows its branch, the daemon reads the
//! checkout's `HEAD`. A switch happens when `HEAD` names a local branch other
//! than the one whose data is live, *and has done so for a whole second*: a
//! checkout rewrites a lot of files, and acting on the first glimpse of a new
//! `HEAD` would move the database under a checkout still in progress.
//!
//! A detached `HEAD` never causes a switch. Git detaches during a rebase, a
//! bisect and `git checkout <sha>`, and the database should sit still through
//! all of them until a real branch is checked out again.
//!
//! ## What a switch holds back
//!
//! The site answers 503 with `Retry-After` for the duration, and its `grove
//! dev` processes — a queue worker is the usual one — are stopped first and
//! started again after. That keeps every write that arrives *through Grove*
//! out of the window where it could land in the wrong branch's copy, and it
//! means the queue worker afterwards runs the new branch's code against the new
//! branch's data. A database client writing directly at the same moment is
//! outside what Grove can hold; for MySQL a swap is one atomic statement, so it
//! can only ever see one branch or the other.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use grove_core::git::{self, Head};
use grove_core::paths::GrovePaths;
use grove_ipc::protocol::{FollowedDatabase, ParkedBranch};
use grove_services::{BranchError, BranchStore, ServiceManager};

use crate::state::DaemonState;

/// How often each followed checkout's `HEAD` is read.
const POLL: Duration = Duration::from_secs(1);
/// How long a new `HEAD` has to stay put before the database follows it.
const SETTLE: Duration = Duration::from_secs(1);
/// After a switch that could not happen because something held the database.
const RETRY_BUSY: Duration = Duration::from_secs(10);
/// After one refused outright: something needs fixing, not retrying.
const RETRY_REFUSED: Duration = Duration::from_secs(60);

/// Which engine holds a followed database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Mysql,
    Sqlite,
}

impl Engine {
    fn as_str(self) -> &'static str {
        match self {
            Engine::Mysql => "mysql",
            Engine::Sqlite => "sqlite",
        }
    }
}

/// A switch that has been started and not yet recorded as finished.
///
/// Written *before* anything moves, cleared after the new state is saved, so a
/// daemon that dies in between knows on its next start exactly which switch to
/// finish or undo — and never has to guess from what the database looks like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    pub from: String,
    pub to: String,
    /// The target branch had no parked copy: this was a copy, not a swap.
    pub first_visit: bool,
}

/// One site that follows its branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Followed {
    /// `grove db branches off` clears this rather than forgetting the site:
    /// the parked copies are still there, and forgetting which branch each
    /// belongs to would strand them — they are named by a hash of the branch.
    #[serde(default = "following_by_default")]
    pub enabled: bool,
    pub engine: Engine,
    /// The MySQL schema name, or the SQLite file's absolute path.
    pub database: String,
    /// The branch whose data is in the live database.
    pub live: String,
    /// Branches with a copy waiting.
    #[serde(default)]
    pub parked: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switching: Option<Switch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

fn following_by_default() -> bool {
    true
}

/// Everything that follows its branch, persisted as one small JSON file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchState {
    #[serde(default)]
    pub sites: BTreeMap<String, Followed>,
}

impl BranchState {
    fn path(paths: &GrovePaths) -> PathBuf {
        paths.base().join("branch-databases.json")
    }

    pub fn load(paths: &GrovePaths) -> Self {
        std::fs::read_to_string(Self::path(paths))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Atomically: a torn write would forget which branch's data is live.
    pub fn save(&self, paths: &GrovePaths) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        grove_core::securefs::write_public_atomic(&Self::path(paths), json)?;
        Ok(())
    }
}

/// Where a SQLite site's parked copies live: inside Grove's home, not the
/// project, so nothing new appears in a working tree that git would notice.
fn sqlite_parked_dir(paths: &GrovePaths, site: &str) -> PathBuf {
    paths.base().join("branch-databases").join(site)
}

/// Whether reaching a followed database may start MySQL to do it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    /// A switch or an explicit action: MySQL has to be up.
    IfNeeded,
    /// A status read: report what is there, never start a server to find out.
    Never,
}

/// The store for a followed database.
fn store_for(
    paths: &GrovePaths,
    services: &ServiceManager,
    site: &str,
    f: &Followed,
    start: Start,
) -> anyhow::Result<BranchStore> {
    Ok(match f.engine {
        Engine::Mysql => BranchStore::Mysql {
            database: f.database.clone(),
            port: match start {
                Start::IfNeeded => services.ready_port("mysql")?,
                Start::Never => services
                    .port_of("mysql")
                    .ok_or_else(|| anyhow::anyhow!("MySQL is not in the catalog"))?,
            },
        },
        Engine::Sqlite => BranchStore::Sqlite {
            file: PathBuf::from(&f.database),
            parked_dir: sqlite_parked_dir(paths, site),
        },
    })
}

/// Work out, from the project's `.env`, which database would follow — and
/// refuse anything that is not one of Grove's own.
///
/// Only MySQL on Grove's bundled server and SQLite files are supported. A
/// remote MySQL is refused because swapping tables on a server someone else
/// uses is not a local-development decision; PostgreSQL and ElyraSQL are
/// refused because they are not written yet, and saying so beats a switch that
/// silently does nothing.
pub(crate) fn resolve_database(
    project: &Path,
    services: &ServiceManager,
) -> anyhow::Result<(Engine, String)> {
    let Some(cfg) = e_db::from_env(project) else {
        anyhow::bail!(
            "{} has no database in its .env (DB_CONNECTION), so there is nothing to follow",
            project.display()
        );
    };
    match cfg.engine.as_str() {
        "sqlite" => {
            if cfg.path.is_empty() {
                anyhow::bail!("could not work out the SQLite file from .env");
            }
            Ok((Engine::Sqlite, cfg.path))
        }
        "mysql" => {
            let loopback = matches!(cfg.host.as_str(), "127.0.0.1" | "localhost" | "::1" | "");
            let grove_port = services.port_of("mysql");
            if services.port_of("elyrasql") == Some(cfg.port) {
                anyhow::bail!(
                    "this site uses ElyraSQL (port {}); following branches supports MySQL and \
                     SQLite so far",
                    cfg.port
                );
            }
            if !loopback || grove_port != Some(cfg.port) {
                anyhow::bail!(
                    "this site's MySQL is {}:{}, not Grove's own server{} — Grove only moves \
                     tables on the server it runs",
                    if cfg.host.is_empty() {
                        "127.0.0.1"
                    } else {
                        &cfg.host
                    },
                    cfg.port,
                    grove_port.map(|p| format!(" on :{p}")).unwrap_or_default()
                );
            }
            if cfg.database.is_empty() {
                anyhow::bail!("DB_DATABASE is empty in .env");
            }
            Ok((Engine::Mysql, cfg.database))
        }
        other => anyhow::bail!(
            "this site uses {other}; following branches supports MySQL and SQLite so far"
        ),
    }
}

/// The site's project directory, from the live registry.
async fn project_of(state: &DaemonState, site: &str) -> Option<PathBuf> {
    let registry = state.shared.registry.read().await;
    registry
        .get(site)
        .map(|s| s.path.clone())
        .filter(|p| !p.as_os_str().is_empty())
}

// ---- the actions `grove db branches` exposes ---------------------------------

/// Start following. Nothing moves: the live data simply becomes the current
/// branch's, and the first switch away from it parks a copy.
pub async fn enable(state: &DaemonState, site: &str) -> anyhow::Result<String> {
    let project = project_of(state, site)
        .await
        .ok_or_else(|| anyhow::anyhow!("no site named {site}"))?;
    let branch = match git::head(&project) {
        Head::Branch(b) => b,
        Head::Detached(at) => anyhow::bail!(
            "{} is not on a branch (detached at {}) — check one out first",
            project.display(),
            short(&at)
        ),
        Head::NotARepo => anyhow::bail!("{} is not a git checkout", project.display()),
    };
    let (engine, database) = resolve_database(&project, &state.services)?;

    let _guard = state.branch_lock.lock().await;
    let mut all = BranchState::load(&state.paths);
    if let Some(existing) = all.sites.get(site).cloned() {
        if existing.enabled {
            return Ok(format!(
                "{site} already follows its branch ({} is live)",
                existing.live
            ));
        }
        return resume(state, &mut all, site, existing, &branch).await;
    }
    // Two worktrees of one project are two sites with the same `.env`. If
    // both followed, each checkout would swap the other's data out from
    // under it.
    if let Some((other, _)) = all
        .sites
        .iter()
        .find(|(name, f)| *name != site && f.engine == engine && f.database == database)
    {
        anyhow::bail!(
            "{other} already follows this same database ({database}); two sites cannot both \
             move it — use a different DB_DATABASE for one of them"
        );
    }
    let followed = Followed {
        enabled: true,
        engine,
        database: database.clone(),
        live: branch.clone(),
        parked: BTreeSet::new(),
        switching: None,
        last_error: None,
    };
    let store = store_for(
        &state.paths,
        &state.services,
        site,
        &followed,
        Start::IfNeeded,
    )?;
    store.preflight().await?;
    all.sites.insert(site.to_string(), followed);
    all.save(&state.paths)?;
    Ok(format!(
        "{site} now follows its git branch: {database} holds {branch}'s data. Check out another \
         branch and Grove parks this one and gives that branch its own copy."
    ))
}

/// Turn a paused site back on.
///
/// Nothing moved while it was off, so the live database still holds the data
/// of the branch that was live then — used, since, by whatever was checked
/// out. Three cases:
///
/// - still on that branch: carry on exactly where it stopped;
/// - on a branch that has its own parked copy: refuse, because two copies
///   would claim that branch, and choosing between them is not Grove's call;
/// - on any other branch: the live data becomes that branch's, just as on the
///   first `on`, and the branch that used to own it has no copy any more.
async fn resume(
    state: &DaemonState,
    all: &mut BranchState,
    site: &str,
    mut f: Followed,
    head: &str,
) -> anyhow::Result<String> {
    if f.parked.contains(head) && f.live != head {
        anyhow::bail!(
            "{head} has a parked copy, and the live database would also count as {head}'s. Check \
             out {} (whose data is live) and run this again, or drop {head}'s copy first with \
             `grove db branches drop {head}`",
            f.live
        );
    }
    let store = store_for(&state.paths, &state.services, site, &f, Start::IfNeeded)?;
    store.preflight().await?;
    let note = if f.live == head {
        format!("{site} follows its branch again; {head} is live, as it was")
    } else {
        let before = std::mem::replace(&mut f.live, head.to_string());
        format!(
            "{site} follows its branch again. The live database was {before}'s when following \
             stopped and has been used by other checkouts since, so it now counts as {head}'s; \
             {before} has no copy of its own any more"
        )
    };
    f.enabled = true;
    f.last_error = None;
    let kept = f.parked.len();
    all.sites.insert(site.to_string(), f);
    all.save(&state.paths)?;
    Ok(if kept == 0 {
        note
    } else {
        format!(
            "{note}. {kept} parked cop{} still available.",
            if kept == 1 { "y is" } else { "ies are" }
        )
    })
}

/// Stop following. The live database keeps whatever branch is in it; parked
/// copies are left alone, because deleting data is never a side effect.
pub async fn disable(state: &DaemonState, site: &str) -> anyhow::Result<String> {
    let _guard = state.branch_lock.lock().await;
    let mut all = BranchState::load(&state.paths);
    let Some(entry) = all.sites.get_mut(site) else {
        return Ok(format!("{site} does not follow its branch"));
    };
    if !entry.enabled {
        return Ok(format!("{site} is already not following its branch"));
    }
    entry.enabled = false;
    let f = entry.clone();
    all.save(&state.paths)?;
    let kept = if f.parked.is_empty() {
        String::new()
    } else {
        format!(
            " Parked copies kept for: {}. `grove db branches on` picks them up again; \
             `grove db branches drop <branch>` deletes one.",
            f.parked.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    };
    Ok(format!(
        "{site} no longer follows its branch; {} keeps {}'s data.{kept}",
        f.database, f.live
    ))
}

/// Delete one parked copy. The only action that destroys data, and only ever
/// on an explicit request for that branch by name.
pub async fn drop_branch(state: &DaemonState, site: &str, branch: &str) -> anyhow::Result<String> {
    let _guard = state.branch_lock.lock().await;
    let mut all = BranchState::load(&state.paths);
    let f = all
        .sites
        .get(site)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{site} does not follow its branch"))?;
    if f.live == branch {
        anyhow::bail!("{branch} is the live branch; its data is the site's database, not a copy");
    }
    if !f.parked.contains(branch) {
        anyhow::bail!("{site} has no parked copy for {branch}");
    }
    let store = store_for(&state.paths, &state.services, site, &f, Start::IfNeeded)?;
    store.drop_parked(branch).await?;
    if let Some(entry) = all.sites.get_mut(site) {
        entry.parked.remove(branch);
    }
    all.save(&state.paths)?;
    Ok(format!("dropped {site}'s parked copy for {branch}"))
}

/// What is live, what is checked out, and what is parked.
pub async fn status(
    state: &DaemonState,
    only: Option<&str>,
) -> anyhow::Result<Vec<FollowedDatabase>> {
    let all = BranchState::load(&state.paths);
    let mut out = Vec::new();
    for (site, f) in &all.sites {
        if only.is_some_and(|s| s != site) {
            continue;
        }
        let project = project_of(state, site).await;
        let head = match project.as_deref().map(git::head) {
            Some(Head::Branch(b)) => b,
            Some(Head::Detached(at)) => format!("detached at {}", short(&at)),
            Some(Head::NotARepo) => "not a git repository".to_string(),
            None => "site not found".to_string(),
        };
        let store = store_for(&state.paths, &state.services, site, f, Start::Never).ok();
        let mut parked = Vec::new();
        for branch in &f.parked {
            let size = match &store {
                Some(store) => store.parked_size(branch).await.unwrap_or_default(),
                None => Default::default(),
            };
            parked.push(ParkedBranch {
                branch: branch.clone(),
                tables: size.tables,
                bytes: size.bytes,
                branch_exists: project
                    .as_deref()
                    .map(|p| git::branch_exists(p, branch))
                    .unwrap_or(false),
            });
        }
        out.push(FollowedDatabase {
            site: site.clone(),
            following: f.enabled,
            engine: f.engine.as_str().to_string(),
            database: f.database.clone(),
            live: f.live.clone(),
            head,
            parked,
            last_error: f.last_error.clone(),
        });
    }
    Ok(out)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

// ---- following --------------------------------------------------------------

/// Finish or undo a switch a previous daemon started and did not record.
///
/// Called once at startup, before the poller runs. The intent written before
/// the switch says what was attempted; the store says how far it got.
pub async fn reconcile(state: &DaemonState) {
    let _guard = state.branch_lock.lock().await;
    let mut all = BranchState::load(&state.paths);
    let mut changed = false;
    for (site, f) in all.sites.iter_mut() {
        let Some(sw) = f.switching.clone() else {
            continue;
        };
        let Ok(store) = store_for(&state.paths, &state.services, site, f, Start::IfNeeded) else {
            continue;
        };
        let outcome = if sw.first_visit {
            // A copy into the outgoing branch's parking spot. Whatever of it
            // exists is ours and possibly incomplete; the live data was never
            // touched, so dropping it and trying again is always safe.
            let _ = store.drop_parked(&sw.from).await;
            "undone (the copy is taken again on the next check)"
        } else {
            let _ = store.resume_swap(&sw.from, &sw.to).await;
            let from_parked = store.has_parked(&sw.from).await.unwrap_or(false);
            let to_parked = store.has_parked(&sw.to).await.unwrap_or(true);
            if from_parked && !to_parked {
                f.live = sw.to.clone();
                f.parked.insert(sw.from.clone());
                f.parked.remove(&sw.to);
                "finished"
            } else {
                "had not started"
            }
        };
        tracing::warn!(site, from = %sw.from, to = %sw.to, outcome, "reconciled an interrupted database switch");
        f.switching = None;
        changed = true;
    }
    if changed {
        if let Err(e) = all.save(&state.paths) {
            tracing::error!(error = %e, "could not save branch-database state after reconciling");
        }
    }
}

/// Watch every followed checkout and move its database when the branch changes.
pub async fn follow(state: Arc<DaemonState>) {
    // `branch` first seen, and when: a switch waits until HEAD has settled.
    let mut seen: HashMap<String, (String, Instant)> = HashMap::new();
    let mut retry_at: HashMap<String, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(POLL).await;
        let all = BranchState::load(&state.paths);
        for (site, f) in &all.sites {
            if !f.enabled {
                seen.remove(site);
                continue;
            }
            let Some(project) = project_of(&state, site).await else {
                continue;
            };
            let Head::Branch(head) = git::head(&project) else {
                // Detached or gone: stay exactly where we are.
                seen.remove(site);
                continue;
            };
            if head == f.live {
                seen.remove(site);
                continue;
            }
            match decide(seen.get(site), &head, Instant::now()) {
                Decision::Wait => {
                    seen.entry(site.clone())
                        .and_modify(|e| {
                            if e.0 != head {
                                *e = (head.clone(), Instant::now());
                            }
                        })
                        .or_insert_with(|| (head.clone(), Instant::now()));
                    continue;
                }
                Decision::Switch => {}
            }
            if retry_at.get(site).is_some_and(|t| Instant::now() < *t) {
                continue;
            }
            match switch(&state, site, &head).await {
                Ok(()) => {
                    retry_at.remove(site);
                }
                Err(e) => {
                    let wait = match e.downcast_ref::<BranchError>() {
                        Some(BranchError::Busy(_)) => RETRY_BUSY,
                        _ => RETRY_REFUSED,
                    };
                    tracing::warn!(site, to = %head, error = %e, "database did not follow the branch");
                    retry_at.insert(site.clone(), Instant::now() + wait);
                }
            }
            seen.remove(site);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Wait,
    Switch,
}

/// Has `head` been the checkout long enough to act on?
fn decide(seen: Option<&(String, Instant)>, head: &str, now: Instant) -> Decision {
    match seen {
        Some((branch, since)) if branch == head && now.duration_since(*since) >= SETTLE => {
            Decision::Switch
        }
        _ => Decision::Wait,
    }
}

/// Move `site`'s database to `to`: pause, park, bring back, resume.
async fn switch(state: &DaemonState, site: &str, to: &str) -> anyhow::Result<()> {
    let _guard = state.branch_lock.lock().await;
    let mut all = BranchState::load(&state.paths);
    let Some(f) = all.sites.get(site).cloned() else {
        return Ok(());
    };
    if f.live == to {
        return Ok(());
    }
    let store = store_for(&state.paths, &state.services, site, &f, Start::IfNeeded)?;
    let first_visit = !store.has_parked(to).await?;
    let from = f.live.clone();

    // The intent, on disk, before anything moves.
    if let Some(entry) = all.sites.get_mut(site) {
        entry.switching = Some(Switch {
            from: from.clone(),
            to: to.to_string(),
            first_visit,
        });
    }
    all.save(&state.paths)?;

    let started = Instant::now();
    state.shared.pause(
        site,
        format!("Grove is switching this site's database to branch {to}"),
    );
    let dev_was_running = state.dev.list().await.iter().any(|s| s == site);
    if dev_was_running {
        let _ = state.dev.stop(site).await;
    }

    let result: Result<(), BranchError> = async {
        store.preflight().await?;
        let holders = store.holders();
        if !holders.is_empty() {
            let who: Vec<String> = holders
                .iter()
                .map(|(pid, name)| format!("{name} (pid {pid})"))
                .collect();
            return Err(BranchError::Busy(format!(
                "the database file is open in {} — close it and Grove will try again",
                who.join(", ")
            )));
        }
        if first_visit {
            store.park_copy(&from).await
        } else {
            store.swap(&from, to).await
        }
    }
    .await;

    state.shared.resume(site);
    if dev_was_running {
        let site_now = state.shared.registry.read().await.get(site).cloned();
        if let Some(resolved) = site_now {
            if let Err(e) = state.dev.start(&resolved, &state.paths).await {
                tracing::warn!(site, error = %e, "could not restart dev processes after the switch");
            }
        }
    }

    let mut all = BranchState::load(&state.paths);
    let Some(entry) = all.sites.get_mut(site) else {
        return Ok(());
    };
    entry.switching = None;
    match result {
        Ok(()) => {
            entry.live = to.to_string();
            entry.parked.insert(from.clone());
            entry.parked.remove(to);
            entry.last_error = None;
            all.save(&state.paths)?;
            tracing::info!(
                site,
                from = %from,
                to,
                how = if first_visit { "first visit: parked a copy" } else { "swapped" },
                ms = started.elapsed().as_millis() as u64,
                "database followed the branch"
            );
            Ok(())
        }
        Err(e) => {
            entry.last_error = Some(format!("switching to {to}: {e}"));
            all.save(&state.paths)?;
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A checkout rewrites many files; the database waits for `HEAD` to have
    /// said the same thing for a second before following it.
    #[test]
    fn a_new_head_has_to_settle_before_the_database_follows() {
        let now = Instant::now();
        assert_eq!(decide(None, "feature", now), Decision::Wait);
        let just_seen = ("feature".to_string(), now);
        assert_eq!(decide(Some(&just_seen), "feature", now), Decision::Wait);
        let settled = ("feature".to_string(), now - SETTLE);
        assert_eq!(decide(Some(&settled), "feature", now), Decision::Switch);
        // A HEAD that changed again restarts the wait.
        assert_eq!(decide(Some(&settled), "other", now), Decision::Wait);
    }

    /// The state file round-trips, and an older one without the newer fields
    /// still loads — it is read by whichever Grove version runs next.
    #[test]
    fn state_round_trips_and_tolerates_missing_fields() {
        let base = std::env::temp_dir().join(format!("grove-bstate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = GrovePaths::with_base(&base);
        paths.ensure().unwrap();
        assert_eq!(BranchState::load(&paths), BranchState::default());

        let mut s = BranchState::default();
        s.sites.insert(
            "shop".into(),
            Followed {
                enabled: true,
                engine: Engine::Mysql,
                database: "shop".into(),
                live: "main".into(),
                parked: ["feature/x".to_string()].into_iter().collect(),
                switching: Some(Switch {
                    from: "main".into(),
                    to: "feature/x".into(),
                    first_visit: false,
                }),
                last_error: None,
            },
        );
        s.save(&paths).unwrap();
        assert_eq!(BranchState::load(&paths), s);

        std::fs::write(
            BranchState::path(&paths),
            r#"{"sites":{"blog":{"engine":"sqlite","database":"/x/db.sqlite","live":"main"}}}"#,
        )
        .unwrap();
        let old = BranchState::load(&paths);
        assert_eq!(old.sites["blog"].engine, Engine::Sqlite);
        assert!(old.sites["blog"].parked.is_empty());
        assert!(
            old.sites["blog"].enabled,
            "an entry from before `off` existed is following"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

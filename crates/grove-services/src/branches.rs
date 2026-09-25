//! Parking a site's database under a git branch, and bringing another back.
//!
//! Switching branches changes the code on disk in a second. It does not change
//! the database, so a feature branch's migrations land in the same tables
//! `main` uses, and going back to `main` means `migrate:fresh` or a broken app.
//! This module keeps one database per branch *under the name the app already
//! uses*: the live database always holds the checked-out branch's data, and
//! every other branch's data waits beside it.
//!
//! ## Why the name never changes
//!
//! The obvious alternative — a database per branch, and point the app at it by
//! injecting `DB_DATABASE` into the requests Grove proxies — only reaches the
//! requests Grove proxies. `php artisan migrate` in a terminal, a test run, a
//! database client and a queue worker started by hand all read `.env`, and
//! would all still hit the old branch's database. Swapping the *contents*
//! under a fixed name is invisible to every one of them.
//!
//! ## How each engine swaps
//!
//! - **MySQL** has no `RENAME DATABASE`, but one `RENAME TABLE` statement can
//!   move any number of tables between schemas, atomically, as a metadata
//!   operation. So a swap is a single statement: every live table into the
//!   outgoing branch's parked schema, every incoming branch's table into the
//!   live one. No data is copied, a 2 GB database swaps as fast as an empty
//!   one, and there is no moment where the app sees half of each.
//! - **SQLite** is a file (with `-wal`/`-shm` beside it in WAL mode). A swap is
//!   two renames of the set.
//!
//! The first visit to a branch is the one case that copies: the branch you
//! came from gets a parked copy of the live data, and the live data carries on
//! as the new branch's starting point — which is what branching off `main`
//! means.
//!
//! ## What is refused rather than guessed at
//!
//! Views, triggers, stored routines and events belong to a MySQL schema, not to
//! a table, and `RENAME TABLE` either refuses to move them (views, tables with
//! triggers) or leaves them behind acting on the other branch's data
//! (routines, events). A database that has any is refused up front, with the
//! list, instead of half-followed.

use std::path::{Path, PathBuf};

use sqlx::mysql::{MySqlConnectOptions, MySqlConnection};
use sqlx::{raw_sql, AssertSqlSafe, Connection, Row};

// Two ways into MySQL, and which one a statement takes is not a style choice.
// `sqlx::query` sends a prepared statement, which is what binding a value
// needs. MySQL will not prepare `USE`, and refuses a good deal of DDL the same
// way (error 1295), so every statement here that binds nothing — which is all
// the DDL and every `RENAME` — goes as plain text through `raw_sql`.

/// Why a switch did not happen.
#[derive(Debug, thiserror::Error)]
pub enum BranchError {
    /// This database cannot be followed safely; nothing was touched.
    #[error("{0}")]
    Refused(String),
    /// Something else holds the database right now; nothing was touched, and
    /// the same switch can simply be tried again.
    #[error("{0}")]
    Busy(String),
    #[error("database: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BranchError>;

/// A short, stable name for a branch, safe to put in a schema or file name.
///
/// Branch names can hold `/`, spaces, and anything else git allows, and they
/// can be longer than MySQL's 64-character identifier limit. A hash of the
/// name has none of those problems; the mapping back to the real name is kept
/// by the caller.
pub fn branch_id(branch: &str) -> String {
    grove_core::checksum::sha256_hex(branch.as_bytes())[..8].to_string()
}

const PARKED_MARKER: &str = "__gb_";

/// The schema a MySQL database's copy for `branch` is parked in.
///
/// `<database>__gb_<id>`, with the database part shortened as needed so the
/// whole stays within MySQL's 64-character limit.
pub fn parked_schema(database: &str, branch: &str) -> String {
    let suffix = format!("{PARKED_MARKER}{}", branch_id(branch));
    let keep = 64 - suffix.chars().count();
    let base: String = database.chars().take(keep).collect();
    format!("{base}{suffix}")
}

/// Is `name` a schema this module created? Checked before every `DROP`, so a
/// bug elsewhere can never turn into dropping someone's real database.
pub fn is_parked_schema(name: &str) -> bool {
    has_marker(name, PARKED_MARKER)
}

const TRY_MARKER: &str = "__gt_";

/// The database a `grove try` of `branch` gets: `<database>__gt_<id>`.
///
/// A different marker from a parked copy on purpose. A parked copy is a
/// branch's data waiting to be swapped back under the real name; a try's copy
/// is a second, independent database that a second checkout uses at the same
/// time. Mixing them up would let `grove db branches` swap a try's data into
/// the live database.
pub fn try_schema(database: &str, branch: &str) -> String {
    let suffix = format!("{TRY_MARKER}{}", branch_id(branch));
    let keep = 64 - suffix.chars().count();
    let base: String = database.chars().take(keep).collect();
    format!("{base}{suffix}")
}

/// Is `name` a database `grove try` created? The guard on its `DROP`.
pub fn is_try_schema(name: &str) -> bool {
    has_marker(name, TRY_MARKER)
}

fn has_marker(name: &str, marker: &str) -> bool {
    match name.rsplit_once(marker) {
        Some((base, id)) => {
            !base.is_empty() && id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// Copy MySQL database `from` into a new database `to` on Grove's server, as
/// a faithful copy: the same tables, rows, generated columns and foreign keys.
///
/// Refuses a `to` that already holds tables, and a `from` with views, triggers,
/// routines or events, which a table-by-table copy would silently leave out.
/// On failure the half-made `to` is removed, since this created it.
pub async fn mysql_clone_database(port: u16, from: &str, to: &str) -> Result<()> {
    let mut conn = mysql_connect(port).await?;
    let blockers = mysql_blockers(&mut conn, from).await?;
    if !blockers.is_empty() {
        return Err(BranchError::Refused(format!(
            "{from} has {}, which a copy would leave behind",
            blockers.join(", ")
        )));
    }
    let meta = mysql_schema(&mut conn, from)
        .await?
        .ok_or_else(|| BranchError::Refused(format!("there is no database {from} to copy")))?;
    let created = mysql_prepare_parking(&mut conn, to, Some(meta)).await?;
    let result = mysql_copy_tables(&mut conn, from, to).await;
    if result.is_err() && created {
        let _ = raw_sql(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {}",
            quote_ident(to)
        )))
        .execute(&mut conn)
        .await;
    }
    let _ = conn.close().await;
    result
}

/// Drop a database `grove try` made. Anything without the try marker is
/// refused, whatever the caller asked for.
pub async fn mysql_drop_try(port: u16, name: &str) -> Result<()> {
    if !is_try_schema(name) {
        return Err(BranchError::Refused(format!(
            "{name} is not a database `grove try` created; refusing to drop it"
        )));
    }
    let mut conn = mysql_connect(port).await?;
    raw_sql(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {}",
        quote_ident(name)
    )))
    .execute(&mut conn)
    .await?;
    conn.close().await?;
    Ok(())
}

/// A MySQL identifier, quoted: backticks around it, and any backtick inside
/// doubled, which is the only escape MySQL identifiers have.
pub fn quote_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// The one statement that swaps two branches' tables, or `None` when there is
/// nothing to move.
///
/// All pairs go in a single `RENAME TABLE`, which MySQL executes atomically: a
/// concurrent reader sees either the old set of live tables or the new one.
pub fn rename_statement(
    live: &str,
    park_as: &str,
    live_tables: &[String],
    bring: &str,
    bring_tables: &[String],
) -> Option<String> {
    let moves: Vec<String> = live_tables
        .iter()
        .map(|t| (live, park_as, t))
        .chain(bring_tables.iter().map(|t| (bring, live, t)))
        .map(|(from, to, t)| {
            format!(
                "{}.{} TO {}.{}",
                quote_ident(from),
                quote_ident(t),
                quote_ident(to),
                quote_ident(t)
            )
        })
        .collect();
    (!moves.is_empty()).then(|| format!("RENAME TABLE {}", moves.join(", ")))
}

/// How big a parked copy is, for `grove db branches`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParkedSize {
    pub tables: u64,
    pub bytes: u64,
}

/// One followed database, in the engine that holds it.
#[derive(Debug, Clone)]
pub enum BranchStore {
    /// A schema on Grove's own MySQL, on `port`.
    Mysql { database: String, port: u16 },
    /// A SQLite file, with parked copies kept in `parked_dir`.
    Sqlite { file: PathBuf, parked_dir: PathBuf },
}

impl BranchStore {
    /// Refuse, up front, a database this cannot follow without losing
    /// something. Nothing is moved either way.
    pub async fn preflight(&self) -> Result<()> {
        match self {
            BranchStore::Mysql { database, port } => {
                let mut conn = mysql_connect(*port).await?;
                let blockers = mysql_blockers(&mut conn, database).await?;
                conn.close().await?;
                if blockers.is_empty() {
                    Ok(())
                } else {
                    Err(BranchError::Refused(format!(
                        "{database} has {} — these belong to the schema rather than to a table, so \
                         swapping tables would leave them acting on the wrong branch's data",
                        blockers.join(", ")
                    )))
                }
            }
            BranchStore::Sqlite { .. } => Ok(()),
        }
    }

    /// Is there a parked copy of `branch`?
    pub async fn has_parked(&self, branch: &str) -> Result<bool> {
        match self {
            BranchStore::Mysql { database, port } => {
                let mut conn = mysql_connect(*port).await?;
                let found = mysql_schema(&mut conn, &parked_schema(database, branch))
                    .await?
                    .is_some();
                conn.close().await?;
                Ok(found)
            }
            BranchStore::Sqlite { parked_dir, .. } => {
                let (file, marker) = sqlite_parked(parked_dir, branch);
                Ok(file.exists() || marker.exists())
            }
        }
    }

    /// The live database goes into `from`'s parking spot; `to`'s parked copy
    /// becomes live. For MySQL that is one atomic statement.
    pub async fn swap(&self, from: &str, to: &str) -> Result<()> {
        match self {
            BranchStore::Mysql { database, port } => mysql_swap(*port, database, from, to).await,
            BranchStore::Sqlite { file, parked_dir } => sqlite_swap(file, parked_dir, from, to),
        }
    }

    /// Complete a swap that was interrupted between its steps.
    ///
    /// MySQL's swap is one atomic statement, so there is never anything to
    /// finish. SQLite's is two renames: if the first happened (the outgoing
    /// branch is parked) and the second did not (no live file, the incoming
    /// copy still parked), this does the second. Safe to call when either both
    /// or neither step ran; it then does nothing.
    pub async fn resume_swap(&self, from: &str, to: &str) -> Result<()> {
        let BranchStore::Sqlite { file, parked_dir } = self else {
            return Ok(());
        };
        let (from_file, from_marker) = sqlite_parked(parked_dir, from);
        let (to_file, to_marker) = sqlite_parked(parked_dir, to);
        let first_step_done = from_file.exists() || from_marker.exists();
        if first_step_done && !file.exists() && to_file.exists() {
            move_set(&to_file, file)?;
        }
        if first_step_done {
            remove_if_present(&to_marker)?;
        }
        Ok(())
    }

    /// First visit to a branch: park a *copy* of the live data under `from`,
    /// and leave the live data as the new branch's starting point.
    pub async fn park_copy(&self, from: &str) -> Result<()> {
        match self {
            BranchStore::Mysql { database, port } => mysql_park_copy(*port, database, from).await,
            BranchStore::Sqlite { file, parked_dir } => sqlite_park_copy(file, parked_dir, from),
        }
    }

    /// Delete `branch`'s parked copy. Only ever called on an explicit request.
    pub async fn drop_parked(&self, branch: &str) -> Result<()> {
        match self {
            BranchStore::Mysql { database, port } => {
                let schema = parked_schema(database, branch);
                let mut conn = mysql_connect(*port).await?;
                mysql_drop_parked(&mut conn, &schema).await?;
                conn.close().await?;
                Ok(())
            }
            BranchStore::Sqlite { parked_dir, .. } => {
                let (file, marker) = sqlite_parked(parked_dir, branch);
                for part in sidecars(&file) {
                    remove_if_present(&part)?;
                }
                remove_if_present(&marker)?;
                Ok(())
            }
        }
    }

    /// How much `branch`'s parked copy takes up.
    pub async fn parked_size(&self, branch: &str) -> Result<ParkedSize> {
        match self {
            BranchStore::Mysql { database, port } => {
                let mut conn = mysql_connect(*port).await?;
                let size = mysql_size(&mut conn, &parked_schema(database, branch)).await?;
                conn.close().await?;
                Ok(size)
            }
            BranchStore::Sqlite { parked_dir, .. } => {
                let (file, _) = sqlite_parked(parked_dir, branch);
                let bytes = sidecars(&file)
                    .iter()
                    .filter_map(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .sum();
                Ok(ParkedSize {
                    tables: u64::from(file.exists()),
                    bytes,
                })
            }
        }
    }

    /// Processes other than this one that have the live database open.
    ///
    /// Only meaningful for SQLite, where a writer holding the file across a
    /// rename would keep writing into whichever branch's copy it opened.
    /// MySQL's equivalent is a lock the `RENAME` waits on, which surfaces as
    /// [`BranchError::Busy`] from the swap itself.
    pub fn holders(&self) -> Vec<(u32, String)> {
        match self {
            BranchStore::Sqlite { file, .. } => open_by_others(file),
            BranchStore::Mysql { .. } => Vec::new(),
        }
    }
}

// ---- MySQL ----------------------------------------------------------------

async fn mysql_connect(port: u16) -> Result<MySqlConnection> {
    // The server's own defaults, not sqlx's: `PIPES_AS_CONCAT` would change
    // what a generated-column expression means when its `CREATE TABLE` is
    // replayed into a parked copy, and a forced time zone has no business in
    // a byte-for-byte copy.
    let options = MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .username("root")
        .pipes_as_concat(false)
        .no_engine_substitution(false)
        .timezone(None);
    Ok(MySqlConnection::connect_with(&options).await?)
}

/// Read a text column that MySQL may hand back as either a string or bytes —
/// `information_schema` and `SHOW CREATE TABLE` vary by version and collation.
fn text(row: &sqlx::mysql::MySqlRow, idx: usize) -> Result<String> {
    if let Ok(s) = row.try_get::<String, _>(idx) {
        return Ok(s);
    }
    let bytes: Vec<u8> = row.try_get(idx)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `(charset, collation)` of a schema, or `None` if it does not exist.
async fn mysql_schema(conn: &mut MySqlConnection, name: &str) -> Result<Option<(String, String)>> {
    let row = sqlx::query(
        "SELECT DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME \
         FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => Ok(Some((text(&row, 0)?, text(&row, 1)?))),
        None => Ok(None),
    }
}

async fn mysql_tables(conn: &mut MySqlConnection, schema: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT TABLE_NAME FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(|r| text(r, 0)).collect()
}

async fn mysql_count(conn: &mut MySqlConnection, sql: &'static str, schema: &str) -> Result<i64> {
    Ok(sqlx::query(sql)
        .bind(schema)
        .fetch_one(&mut *conn)
        .await?
        .try_get::<i64, _>(0)?)
}

/// What in `schema` would not survive a table-by-table swap.
async fn mysql_blockers(conn: &mut MySqlConnection, schema: &str) -> Result<Vec<String>> {
    let checks: [(&str, &'static str); 4] = [
        (
            "views",
            "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'VIEW'",
        ),
        (
            "triggers",
            "SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ?",
        ),
        (
            "stored routines",
            "SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ?",
        ),
        (
            "events",
            "SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ?",
        ),
    ];
    let mut found = Vec::new();
    for (what, sql) in checks {
        let n = mysql_count(conn, sql, schema).await?;
        if n > 0 {
            found.push(format!("{n} {what}"));
        }
    }
    Ok(found)
}

async fn mysql_size(conn: &mut MySqlConnection, schema: &str) -> Result<ParkedSize> {
    let row = sqlx::query(
        "SELECT COUNT(*), CAST(COALESCE(SUM(DATA_LENGTH + INDEX_LENGTH), 0) AS UNSIGNED) \
         FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?",
    )
    .bind(schema)
    .fetch_one(&mut *conn)
    .await?;
    Ok(ParkedSize {
        tables: row.try_get::<i64, _>(0)?.max(0) as u64,
        bytes: row.try_get::<u64, _>(1)?,
    })
}

/// A charset or collation name, as MySQL reported it, checked before it is put
/// into a statement unquoted (which is the only way `CHARACTER SET` takes it).
fn plain_name(name: &str) -> Result<&str> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(name)
    } else {
        Err(BranchError::Refused(format!(
            "unexpected charset or collation name {name:?}"
        )))
    }
}

/// Create `schema` like `like`, or confirm an existing one is empty. A parking
/// spot that already holds tables means the bookkeeping and the server
/// disagree, and moving tables into it would mix two branches.
async fn mysql_prepare_parking(
    conn: &mut MySqlConnection,
    schema: &str,
    like: Option<(String, String)>,
) -> Result<bool> {
    if mysql_schema(conn, schema).await?.is_some() {
        if !mysql_tables(conn, schema).await?.is_empty() {
            return Err(BranchError::Refused(format!(
                "{schema} already holds tables, so it cannot receive another branch's"
            )));
        }
        return Ok(false);
    }
    let options = match &like {
        Some((cs, coll)) => format!(
            " CHARACTER SET {} COLLATE {}",
            plain_name(cs)?,
            plain_name(coll)?
        ),
        None => String::new(),
    };
    raw_sql(AssertSqlSafe(format!(
        "CREATE DATABASE {}{options}",
        quote_ident(schema)
    )))
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

async fn mysql_drop_parked(conn: &mut MySqlConnection, schema: &str) -> Result<()> {
    if !is_parked_schema(schema) {
        return Err(BranchError::Refused(format!(
            "{schema} is not a parked branch database; refusing to drop it"
        )));
    }
    raw_sql(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {}",
        quote_ident(schema)
    )))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// MySQL's "lock wait timeout exceeded": another session holds a metadata lock,
/// typically an open transaction in a database client.
fn is_lock_timeout(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("HY000") && d.message().contains("Lock wait timeout"))
        || e.to_string().contains("1205")
}

async fn mysql_swap(port: u16, live: &str, from: &str, to: &str) -> Result<()> {
    let mut conn = mysql_connect(port).await?;
    // A metadata lock held by someone else — an open transaction in a GUI
    // client — makes `RENAME TABLE` wait, by default for a year. Five seconds,
    // then give up and let the caller try again.
    raw_sql("SET SESSION lock_wait_timeout = 5")
        .execute(&mut conn)
        .await?;

    let park_as = parked_schema(live, from);
    let bring = parked_schema(live, to);
    let live_meta = mysql_schema(&mut conn, live).await?;
    if live_meta.is_none() {
        // Never migrated: there is nothing to park, but the app's database
        // must exist for the incoming tables to land in.
        raw_sql(AssertSqlSafe(format!(
            "CREATE DATABASE {}",
            quote_ident(live)
        )))
        .execute(&mut conn)
        .await?;
    }
    let live_meta = mysql_schema(&mut conn, live).await?;
    mysql_prepare_parking(&mut conn, &park_as, live_meta).await?;

    let live_tables = mysql_tables(&mut conn, live).await?;
    let bring_tables = mysql_tables(&mut conn, &bring).await?;
    if let Some(stmt) = rename_statement(live, &park_as, &live_tables, &bring, &bring_tables) {
        if let Err(e) = raw_sql(AssertSqlSafe(stmt)).execute(&mut conn).await {
            return Err(if is_lock_timeout(&e) {
                BranchError::Busy(format!(
                    "another connection holds a lock on {live} (an open transaction in a database \
                     client?) — Grove will try again"
                ))
            } else {
                e.into()
            });
        }
    }

    // The incoming branch's parking spot is empty now; remove it so the
    // bookkeeping and the server agree on which copies exist.
    if mysql_tables(&mut conn, &bring).await?.is_empty() {
        mysql_drop_parked(&mut conn, &bring).await?;
    }
    conn.close().await?;
    Ok(())
}

async fn mysql_park_copy(port: u16, live: &str, from: &str) -> Result<()> {
    let mut conn = mysql_connect(port).await?;
    let park_as = parked_schema(live, from);
    let Some(meta) = mysql_schema(&mut conn, live).await? else {
        // Nothing to copy: park an empty schema, so that coming back to this
        // branch brings back "no tables" rather than whatever is live then.
        mysql_prepare_parking(&mut conn, &park_as, None).await?;
        conn.close().await?;
        return Ok(());
    };
    let created = mysql_prepare_parking(&mut conn, &park_as, Some(meta)).await?;

    let result = mysql_copy_tables(&mut conn, live, &park_as).await;
    if result.is_err() && created {
        // Ours, and incomplete: leave nothing half-copied behind.
        let _ = mysql_drop_parked(&mut conn, &park_as).await;
    }
    let _ = conn.close().await;
    result
}

async fn mysql_copy_tables(conn: &mut MySqlConnection, from: &str, to: &str) -> Result<()> {
    // Copy what is there, as it is: no strict-mode rejection of a legacy zero
    // date, no foreign keys checked against tables that have not arrived yet,
    // no unique checks on data that was already unique.
    for stmt in [
        "SET SESSION sql_mode = ''",
        "SET SESSION foreign_key_checks = 0",
        "SET SESSION unique_checks = 0",
    ] {
        raw_sql(stmt).execute(&mut *conn).await?;
    }
    let tables = mysql_tables(conn, from).await?;

    // Every CREATE first, then every INSERT: DDL commits implicitly, and
    // creating the whole schema before moving rows keeps foreign keys between
    // tables resolvable in any order.
    raw_sql(AssertSqlSafe(format!("USE {}", quote_ident(to))))
        .execute(&mut *conn)
        .await?;
    for table in &tables {
        let row = raw_sql(AssertSqlSafe(format!(
            "SHOW CREATE TABLE {}.{}",
            quote_ident(from),
            quote_ident(table)
        )))
        .fetch_one(&mut *conn)
        .await?;
        let ddl = text(&row, 1)?;
        raw_sql(AssertSqlSafe(ddl)).execute(&mut *conn).await?;
    }
    for table in &tables {
        // Generated columns are computed, and inserting into one is an error;
        // `GENERATION_EXPRESSION` is empty for every column that is not.
        let rows = sqlx::query(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND GENERATION_EXPRESSION = '' \
             ORDER BY ORDINAL_POSITION",
        )
        .bind(from)
        .bind(table)
        .fetch_all(&mut *conn)
        .await?;
        let cols: Vec<String> = rows
            .iter()
            .map(|r| text(r, 0).map(|c| quote_ident(&c)))
            .collect::<Result<_>>()?;
        if cols.is_empty() {
            continue;
        }
        let list = cols.join(", ");
        raw_sql(AssertSqlSafe(format!(
            "INSERT INTO {to}.{t} ({list}) SELECT {list} FROM {from}.{t}",
            to = quote_ident(to),
            from = quote_ident(from),
            t = quote_ident(table),
        )))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

// ---- SQLite ---------------------------------------------------------------

/// A database file and the two files SQLite keeps beside it in WAL mode. They
/// are one database, and have to move together or not at all.
fn sidecars(file: &Path) -> [PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = file.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    [file.to_path_buf(), with("-wal"), with("-shm")]
}

/// Where `branch`'s copy is parked, and the marker that records "this branch
/// had no database file at all".
fn sqlite_parked(parked_dir: &Path, branch: &str) -> (PathBuf, PathBuf) {
    let id = branch_id(branch);
    (
        parked_dir.join(format!("{id}.sqlite")),
        parked_dir.join(format!("{id}.absent")),
    )
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `rename`, or copy-then-remove when the two paths are on different volumes.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc_exdev()) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

/// `EXDEV`, which is 18 on both macOS and Linux.
fn libc_exdev() -> i32 {
    18
}

/// Move the parts of `from` that exist onto `to`, clearing any stale sidecar
/// at the destination so a leftover `-wal` cannot be replayed into the wrong
/// branch's database.
fn move_set(from: &Path, to: &Path) -> std::io::Result<()> {
    for (src, dst) in sidecars(from).iter().zip(sidecars(to).iter()) {
        if src.exists() {
            move_file(src, dst)?;
        } else {
            remove_if_present(dst)?;
        }
    }
    Ok(())
}

fn sqlite_swap(live: &Path, parked_dir: &Path, from: &str, to: &str) -> Result<()> {
    std::fs::create_dir_all(parked_dir)?;
    let (park_file, park_marker) = sqlite_parked(parked_dir, from);
    let (bring_file, bring_marker) = sqlite_parked(parked_dir, to);
    if park_file.exists() || park_marker.exists() {
        return Err(BranchError::Refused(format!(
            "a parked copy for {from} already exists, so the live database cannot be parked there"
        )));
    }
    // Park the live database — or record that there was none.
    if live.exists() {
        move_set(live, &park_file)?;
    } else {
        std::fs::write(&park_marker, b"")?;
    }
    // Bring the other branch's back — or leave no file, if it had none.
    if bring_file.exists() {
        move_set(&bring_file, live)?;
    }
    remove_if_present(&bring_marker)?;
    Ok(())
}

fn sqlite_park_copy(live: &Path, parked_dir: &Path, from: &str) -> Result<()> {
    std::fs::create_dir_all(parked_dir)?;
    let (park_file, park_marker) = sqlite_parked(parked_dir, from);
    if park_file.exists() || park_marker.exists() {
        return Err(BranchError::Refused(format!(
            "a parked copy for {from} already exists"
        )));
    }
    if !live.exists() {
        std::fs::write(&park_marker, b"")?;
        return Ok(());
    }
    for (src, dst) in sidecars(live).iter().zip(sidecars(&park_file).iter()) {
        if src.exists() {
            if let Err(e) = std::fs::copy(src, dst) {
                for part in sidecars(&park_file) {
                    let _ = std::fs::remove_file(part);
                }
                return Err(e.into());
            }
        }
    }
    Ok(())
}

/// Copy a SQLite database — the file and whichever of `-wal`/`-shm` exist —
/// to `to`, which must not exist yet. For `grove try`, whose checkout needs a
/// database of its own that starts from the main checkout's data.
pub fn sqlite_copy(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        return Err(BranchError::Refused(format!(
            "{} already exists; not overwriting it",
            to.display()
        )));
    }
    if !from.exists() {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for (src, dst) in sidecars(from).iter().zip(sidecars(to).iter()) {
        if src.exists() {
            std::fs::copy(src, dst)?;
        }
    }
    Ok(())
}

/// Other processes with `file` open, from `lsof`, as `(pid, command)`.
fn open_by_others(file: &Path) -> Vec<(u32, String)> {
    if !file.exists() {
        return Vec::new();
    }
    let Ok(out) = std::process::Command::new("lsof")
        .args(["-F", "pc", "--"])
        .arg(file)
        .output()
    else {
        return Vec::new();
    };
    let me = std::process::id();
    let mut found = Vec::new();
    let mut pid: Option<u32> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(c), Some(p)) = (line.strip_prefix('c'), pid) {
            if p != me {
                found.push((p, c.to_string()));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_id_is_short_stable_and_distinct() {
        assert_eq!(branch_id("main"), branch_id("main"));
        assert_ne!(branch_id("main"), branch_id("feature/main"));
        assert_eq!(branch_id("feature/with spaces and ünïcode").len(), 8);
    }

    /// MySQL refuses identifiers over 64 characters, and a database name can
    /// already be 64. The parked name must still fit, and still be recognised
    /// as ours — that recognition is what guards every `DROP`.
    #[test]
    fn parked_schema_names_fit_and_are_recognised() {
        let long = "d".repeat(64);
        let parked = parked_schema(&long, "feature/x");
        assert!(parked.chars().count() <= 64, "{parked}");
        assert!(is_parked_schema(&parked));
        assert!(is_parked_schema(&parked_schema("shop", "main")));
        assert_ne!(parked_schema("shop", "main"), parked_schema("shop", "dev"));

        for real in [
            "shop",
            "shop__gb_",
            "shop__gb_xyz",
            "__gb_12345678",
            "shop__gb_1234567g",
        ] {
            assert!(
                !is_parked_schema(real),
                "{real} must not look like a parked copy"
            );
        }
    }

    /// A try's database and a parked copy must never be mistaken for each
    /// other: each guard accepts only its own.
    #[test]
    fn try_and_parked_names_are_told_apart() {
        let t = try_schema("shop", "feature/x");
        let p = parked_schema("shop", "feature/x");
        assert!(is_try_schema(&t) && !is_parked_schema(&t), "{t}");
        assert!(is_parked_schema(&p) && !is_try_schema(&p), "{p}");
        assert!(try_schema(&"d".repeat(64), "b").chars().count() <= 64);
        assert!(!is_try_schema("shop"));
    }

    #[test]
    fn identifiers_escape_their_own_quote() {
        assert_eq!(quote_ident("orders"), "`orders`");
        assert_eq!(quote_ident("we`ird"), "`we``ird`");
    }

    /// The swap is one statement: both directions in a single `RENAME TABLE`,
    /// so no reader ever sees half of each branch.
    #[test]
    fn a_swap_is_one_statement_moving_both_ways() {
        let stmt = rename_statement(
            "shop",
            "shop__gb_aaaaaaaa",
            &["orders".into(), "users".into()],
            "shop__gb_bbbbbbbb",
            &["users".into(), "invoices".into()],
        )
        .unwrap();
        assert_eq!(
            stmt,
            "RENAME TABLE `shop`.`orders` TO `shop__gb_aaaaaaaa`.`orders`, \
             `shop`.`users` TO `shop__gb_aaaaaaaa`.`users`, \
             `shop__gb_bbbbbbbb`.`users` TO `shop`.`users`, \
             `shop__gb_bbbbbbbb`.`invoices` TO `shop`.`invoices`"
        );
        assert_eq!(rename_statement("a", "b", &[], "c", &[]), None);
    }

    /// A table name is whatever a migration called it; it goes into the
    /// statement quoted, and cannot end the identifier early.
    #[test]
    fn a_hostile_table_name_stays_one_identifier() {
        let stmt = rename_statement("a", "b", &["x` TO `mysql`.`user".into()], "c", &[]).unwrap();
        assert_eq!(
            stmt,
            "RENAME TABLE `a`.`x`` TO ``mysql``.``user` TO `b`.`x`` TO ``mysql``.``user`"
        );
    }

    fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("grove-branches-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("database")).unwrap();
        let live = root.join("database/database.sqlite");
        let parked = root.join("parked");
        (root, live, parked)
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|_| "<missing>".into())
    }

    /// Main's data parked on the way out, the feature branch's brought back on
    /// the way in, and main's again on the way back — with the `-wal` moving as
    /// part of the database it belongs to.
    #[test]
    fn sqlite_round_trips_between_branches() {
        let (root, live, parked) = scratch("roundtrip");
        std::fs::write(&live, "main-data").unwrap();
        std::fs::write(format!("{}-wal", live.display()), "main-wal").unwrap();

        // First visit to `feature`: main gets a copy, live carries on.
        sqlite_park_copy(&live, &parked, "main").unwrap();
        assert_eq!(read(&live), "main-data");
        std::fs::write(&live, "feature-data").unwrap();
        std::fs::remove_file(format!("{}-wal", live.display())).unwrap();

        // Back to main: feature parked, main returned, wal and all.
        sqlite_swap(&live, &parked, "feature", "main").unwrap();
        assert_eq!(read(&live), "main-data");
        assert_eq!(
            read(Path::new(&format!("{}-wal", live.display()))),
            "main-wal"
        );

        // And to feature again: exactly what was left there.
        sqlite_swap(&live, &parked, "main", "feature").unwrap();
        assert_eq!(read(&live), "feature-data");
        assert!(
            !Path::new(&format!("{}-wal", live.display())).exists(),
            "main's -wal must not be replayed into feature's database"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A daemon killed between the two renames leaves no live file at all. The
    /// resume brings the incoming branch's copy into place, and does nothing
    /// on a swap that had not started.
    #[tokio::test]
    async fn an_interrupted_sqlite_swap_is_finished_on_resume() {
        let (root, live, parked) = scratch("resume");
        std::fs::create_dir_all(&parked).unwrap();
        std::fs::write(&live, "main-data").unwrap();
        std::fs::write(sqlite_parked(&parked, "feature").0, "feature-data").unwrap();
        let store = BranchStore::Sqlite {
            file: live.clone(),
            parked_dir: parked.clone(),
        };

        // Not started: nothing to do.
        store.resume_swap("main", "feature").await.unwrap();
        assert_eq!(read(&live), "main-data");

        // First step only, as if the daemon died right after it.
        move_set(&live, &sqlite_parked(&parked, "main").0).unwrap();
        assert!(!live.exists());
        store.resume_swap("main", "feature").await.unwrap();
        assert_eq!(read(&live), "feature-data");
        assert_eq!(read(&sqlite_parked(&parked, "main").0), "main-data");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A branch that never had a database file gets none back, rather than
    /// whatever happened to be live when it was left.
    #[test]
    fn sqlite_remembers_a_branch_that_had_no_database() {
        let (root, live, parked) = scratch("absent");
        sqlite_park_copy(&live, &parked, "empty").unwrap();
        std::fs::write(&live, "other-data").unwrap();
        sqlite_swap(&live, &parked, "other", "empty").unwrap();
        assert!(!live.exists(), "the branch had no database file");
        sqlite_swap(&live, &parked, "empty", "other").unwrap();
        assert_eq!(read(&live), "other-data");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A parking spot that is already taken means the bookkeeping is wrong;
    /// the swap refuses and moves nothing.
    #[test]
    fn sqlite_refuses_to_overwrite_a_parked_copy() {
        let (root, live, parked) = scratch("refuse");
        std::fs::write(&live, "live").unwrap();
        std::fs::create_dir_all(&parked).unwrap();
        std::fs::write(sqlite_parked(&parked, "main").0, "already-there").unwrap();
        assert!(matches!(
            sqlite_swap(&live, &parked, "main", "feature"),
            Err(BranchError::Refused(_))
        ));
        assert_eq!(read(&live), "live", "nothing moved");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file nobody else has open reports no holders, and `lsof` reports the
    /// process that does hold it.
    #[test]
    fn holders_sees_another_process_with_the_file_open() {
        let (root, live, _) = scratch("holders");
        std::fs::write(&live, "x").unwrap();
        let store = BranchStore::Sqlite {
            file: live.clone(),
            parked_dir: root.join("parked"),
        };
        assert!(store.holders().is_empty());
        let mut child = std::process::Command::new("tail")
            .arg("-f")
            .arg(&live)
            .spawn()
            .unwrap();
        let mut seen = Vec::new();
        for _ in 0..30 {
            seen = store.holders();
            if !seen.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        if std::process::Command::new("lsof")
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("skipped: no lsof on this machine");
        } else {
            assert!(seen.iter().any(|(pid, _)| *pid == child.id()), "{seen:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Against a real MySQL, when one is offered: `GROVE_TEST_MYSQL_PORT`
    /// pointing at a server where `root` has no password. It creates and drops
    /// its own schema and touches nothing else.
    #[tokio::test]
    async fn mysql_swaps_and_copies_against_a_real_server() {
        let Some(port) = std::env::var("GROVE_TEST_MYSQL_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
        else {
            eprintln!("skipped: set GROVE_TEST_MYSQL_PORT to run against a real MySQL");
            return;
        };
        let db = format!("grove_bt_{}", std::process::id());
        let store = BranchStore::Mysql {
            database: db.clone(),
            port,
        };
        let mut conn = mysql_connect(port).await.unwrap();
        for stmt in [
            format!("DROP DATABASE IF EXISTS {}", quote_ident(&db)),
            format!("CREATE DATABASE {} CHARACTER SET utf8mb4", quote_ident(&db)),
            format!(
                "CREATE TABLE {}.users (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(20), \
                 created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, \
                 upper_name VARCHAR(20) AS (UPPER(name)) VIRTUAL)",
                quote_ident(&db)
            ),
            format!(
                "CREATE TABLE {db}.posts (id INT PRIMARY KEY, user_id INT, \
                 FOREIGN KEY (user_id) REFERENCES {db}.users(id))",
                db = quote_ident(&db)
            ),
            format!(
                "INSERT INTO {}.users (name) VALUES ('ada'), ('bob')",
                quote_ident(&db)
            ),
            format!("INSERT INTO {}.posts VALUES (1, 1)", quote_ident(&db)),
        ] {
            raw_sql(AssertSqlSafe(stmt))
                .execute(&mut conn)
                .await
                .unwrap();
        }
        store.preflight().await.unwrap();

        // First visit to `feature`: main parked as a faithful copy.
        store.park_copy("main").await.unwrap();
        let main_copy = parked_schema(&db, "main");
        let copied: i64 = raw_sql(AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {}.users WHERE upper_name = 'ADA'",
            quote_ident(&main_copy)
        )))
        .fetch_one(&mut conn)
        .await
        .unwrap()
        .get(0);
        assert_eq!(copied, 1, "generated columns recomputed, rows intact");

        // The feature branch changes its schema.
        raw_sql(AssertSqlSafe(format!(
            "CREATE TABLE {}.invoices (id INT PRIMARY KEY)",
            quote_ident(&db)
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        // Back to main: invoices leaves with feature, users/posts come back.
        store.swap("feature", "main").await.unwrap();
        assert_eq!(
            mysql_tables(&mut conn, &db).await.unwrap(),
            vec!["posts".to_string(), "users".to_string()]
        );
        assert!(
            !store.has_parked("main").await.unwrap(),
            "main is live, not parked"
        );
        assert!(store.has_parked("feature").await.unwrap());
        assert!(mysql_tables(&mut conn, &parked_schema(&db, "feature"))
            .await
            .unwrap()
            .contains(&"invoices".to_string()));

        // A view blocks following, by name, before anything moves.
        raw_sql(AssertSqlSafe(format!(
            "CREATE VIEW {}.v AS SELECT 1",
            quote_ident(&db)
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        assert!(matches!(
            store.preflight().await,
            Err(BranchError::Refused(_))
        ));

        // Clean up everything this test made, and nothing else.
        store.drop_parked("feature").await.unwrap();
        raw_sql(AssertSqlSafe(format!("DROP DATABASE {}", quote_ident(&db))))
            .execute(&mut conn)
            .await
            .unwrap();
        // The guard on DROP: a real database name is refused outright.
        assert!(matches!(
            mysql_drop_parked(&mut conn, "mysql").await,
            Err(BranchError::Refused(_))
        ));
        conn.close().await.unwrap();
    }
}

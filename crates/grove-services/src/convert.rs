//! Cross-dialect database conversion between MySQL, PostgreSQL and SQLite.
//!
//! Scope (v1): tables, columns (mapped by category), primary keys and row data.
//! Values are transferred as text (blobs as bytes), which reliably carries
//! dates, decimals, JSON, UUIDs, etc. across dialects. Views, stored routines,
//! triggers, foreign keys and sequences are not recreated.

use serde::{Deserialize, Serialize};
use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row};

use crate::manager::{Result, ServiceError};

/// A database endpoint for a conversion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbConnSpec {
    /// "mysql" | "postgres" | "sqlite"
    pub kind: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub database: String,
    /// Absolute file path for SQLite.
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mysql,
    Postgres,
    Sqlite,
}

impl DbConnSpec {
    fn kind(&self) -> Result<Kind> {
        match self.kind.as_str() {
            "mysql" => Ok(Kind::Mysql),
            "postgres" | "postgresql" | "pgsql" => Ok(Kind::Postgres),
            "sqlite" => Ok(Kind::Sqlite),
            other => Err(ServiceError::Init(format!(
                "unknown database kind: {other}"
            ))),
        }
    }

    /// Build a sqlx connection URL. `create` opens SQLite read-write (creating
    /// the file); otherwise read-only.
    fn url(&self, create: bool) -> Result<String> {
        Ok(match self.kind()? {
            Kind::Mysql => {
                let auth = if self.password.is_empty() {
                    self.user.clone()
                } else {
                    format!("{}:{}", self.user, self.password)
                };
                format!(
                    "mysql://{auth}@{}:{}/{}",
                    self.host, self.port, self.database
                )
            }
            Kind::Postgres => {
                let auth = if self.password.is_empty() {
                    self.user.clone()
                } else {
                    format!("{}:{}", self.user, self.password)
                };
                format!(
                    "postgres://{auth}@{}:{}/{}",
                    self.host, self.port, self.database
                )
            }
            Kind::Sqlite => {
                if self.path.is_empty() {
                    return Err(ServiceError::Init("SQLite path is required".into()));
                }
                let mode = if create { "rwc" } else { "ro" };
                format!("sqlite://{}?mode={mode}", self.path)
            }
        })
    }
}

/// Value category, normalised across dialects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cat {
    Integer,
    Real,
    Numeric,
    Boolean,
    Blob,
    Text,
}

fn categorize(sql_type: &str) -> Cat {
    let t = sql_type.to_lowercase();
    if t.contains("bool") {
        Cat::Boolean
    } else if t.contains("blob") || t.contains("binary") || t.contains("bytea") {
        Cat::Blob
    } else if t.contains("float") || t.contains("double") || t.contains("real") {
        Cat::Real
    } else if t.contains("decimal") || t.contains("numeric") {
        Cat::Numeric
    } else if t.contains("int") || t.contains("serial") {
        Cat::Integer
    } else {
        Cat::Text
    }
}

fn target_type(cat: Cat, target: Kind) -> &'static str {
    match (target, cat) {
        (Kind::Sqlite, Cat::Integer | Cat::Boolean) => "INTEGER",
        (Kind::Sqlite, Cat::Real) => "REAL",
        (Kind::Sqlite, Cat::Numeric) => "NUMERIC",
        (Kind::Sqlite, Cat::Blob) => "BLOB",
        (Kind::Sqlite, Cat::Text) => "TEXT",
        (Kind::Mysql, Cat::Integer) => "BIGINT",
        (Kind::Mysql, Cat::Boolean) => "TINYINT(1)",
        (Kind::Mysql, Cat::Real) => "DOUBLE",
        (Kind::Mysql, Cat::Numeric) => "DECIMAL(38,10)",
        (Kind::Mysql, Cat::Blob) => "LONGBLOB",
        (Kind::Mysql, Cat::Text) => "LONGTEXT",
        (Kind::Postgres, Cat::Integer) => "BIGINT",
        (Kind::Postgres, Cat::Boolean) => "BOOLEAN",
        (Kind::Postgres, Cat::Real) => "DOUBLE PRECISION",
        (Kind::Postgres, Cat::Numeric) => "NUMERIC",
        (Kind::Postgres, Cat::Blob) => "BYTEA",
        (Kind::Postgres, Cat::Text) => "TEXT",
    }
}

fn quote(ident: &str, kind: Kind) -> String {
    match kind {
        Kind::Mysql => format!("`{}`", ident.replace('`', "``")),
        Kind::Postgres | Kind::Sqlite => format!("\"{}\"", ident.replace('"', "\"\"")),
    }
}

struct Column {
    name: String,
    cat: Cat,
    pk: bool,
}

/// Convert every table + its data from `source` into `target`.
pub async fn convert(
    source: &DbConnSpec,
    target: &DbConnSpec,
    progress: impl Fn(&str),
) -> Result<String> {
    sqlx::any::install_default_drivers();
    let (sk, tk) = (source.kind()?, target.kind()?);

    let src = AnyPoolOptions::new()
        .max_connections(2)
        .connect(&source.url(false)?)
        .await
        .map_err(|e| ServiceError::Init(format!("connecting to source: {e}")))?;
    let dst = AnyPoolOptions::new()
        .max_connections(2)
        .connect(&target.url(true)?)
        .await
        .map_err(|e| ServiceError::Init(format!("connecting to target: {e}")))?;

    let tables = list_tables(&src, sk).await?;
    if tables.is_empty() {
        return Ok("No tables found in the source database.".into());
    }

    let mut total_rows = 0u64;
    for table in &tables {
        progress(&format!("converting `{table}`…"));
        let cols = columns(&src, sk, table).await?;
        if cols.is_empty() {
            continue;
        }

        // Recreate the table in the target.
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {}", quote(table, tk)))
            .execute(&dst)
            .await;
        sqlx::query(&create_table_sql(table, &cols, tk))
            .execute(&dst)
            .await
            .map_err(|e| ServiceError::Init(format!("creating {table}: {e}")))?;

        total_rows += copy_rows(&src, &dst, table, &cols, sk, tk).await?;
    }

    Ok(format!(
        "Converted {} table(s) and {} row(s) into the {} database.",
        tables.len(),
        total_rows,
        target.kind
    ))
}

async fn list_tables(pool: &AnyPool, kind: Kind) -> Result<Vec<String>> {
    let sql = match kind {
        Kind::Mysql => {
            "SELECT CAST(table_name AS CHAR) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
        Kind::Postgres => {
            "SELECT tablename FROM pg_catalog.pg_tables \
             WHERE schemaname = 'public' ORDER BY tablename"
        }
        Kind::Sqlite => {
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' ORDER BY name"
        }
    };
    let rows = sqlx::query(sql)
        .fetch_all(pool)
        .await
        .map_err(|e| ServiceError::Init(format!("listing tables: {e}")))?;
    Ok(rows
        .iter()
        .map(|r| read_string(r, 0))
        .filter(|s| !s.is_empty())
        .collect())
}

async fn columns(pool: &AnyPool, kind: Kind, table: &str) -> Result<Vec<Column>> {
    match kind {
        Kind::Sqlite => {
            let rows = sqlx::query(&format!("PRAGMA table_info({})", quote(table, kind)))
                .fetch_all(pool)
                .await
                .map_err(|e| ServiceError::Init(format!("reading {table}: {e}")))?;
            Ok(rows
                .iter()
                .map(|r| {
                    let name = read_string(r, 1);
                    let ty = read_string(r, 2);
                    let pk = read_i64(r, 5);
                    Column {
                        name,
                        cat: categorize(&ty),
                        pk: pk > 0,
                    }
                })
                .filter(|c| !c.name.is_empty())
                .collect())
        }
        Kind::Mysql | Kind::Postgres => {
            let schema = if kind == Kind::Mysql {
                "table_schema = DATABASE()"
            } else {
                "table_schema = 'public'"
            };
            let _ = schema;
            // Any uses `?` placeholders and translates per backend.
            // information_schema string columns are special types the Any driver
            // can't decode directly — cast them to plain text.
            let rows = if kind == Kind::Mysql {
                let my = "SELECT CAST(column_name AS CHAR), CAST(data_type AS CHAR), \
                          CASE WHEN column_key = 'PRI' THEN 1 ELSE 0 END \
                          FROM information_schema.columns \
                          WHERE table_schema = DATABASE() AND table_name = ? \
                          ORDER BY ordinal_position";
                sqlx::query(my).bind(table).fetch_all(pool).await
            } else {
                let pg = "SELECT column_name::text, data_type::text, 0 \
                          FROM information_schema.columns \
                          WHERE table_schema = 'public' AND table_name = ? \
                          ORDER BY ordinal_position";
                sqlx::query(pg).bind(table).fetch_all(pool).await
            }
            .map_err(|e| ServiceError::Init(format!("reading {table}: {e}")))?;

            let mut cols: Vec<Column> = rows
                .iter()
                .map(|r| {
                    let name = read_string(r, 0);
                    let ty = read_string(r, 1);
                    let pk = read_i64(r, 2);
                    Column {
                        name,
                        cat: categorize(&ty),
                        pk: pk > 0,
                    }
                })
                .filter(|c| !c.name.is_empty())
                .collect();

            if kind == Kind::Postgres {
                let pk_sql = "SELECT a.attname FROM pg_index i \
                    JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                    WHERE i.indrelid = ?::regclass AND i.indisprimary";
                if let Ok(pk_rows) = sqlx::query(pk_sql).bind(table).fetch_all(pool).await {
                    let pks: Vec<String> = pk_rows
                        .iter()
                        .filter_map(|r| r.try_get::<String, _>(0).ok())
                        .collect();
                    for c in &mut cols {
                        if pks.contains(&c.name) {
                            c.pk = true;
                        }
                    }
                }
            }
            Ok(cols)
        }
    }
}

fn create_table_sql(table: &str, cols: &[Column], target: Kind) -> String {
    let mut defs: Vec<String> = cols
        .iter()
        .map(|c| format!("{} {}", quote(&c.name, target), target_type(c.cat, target)))
        .collect();
    let pks: Vec<String> = cols
        .iter()
        .filter(|c| c.pk)
        .map(|c| quote(&c.name, target))
        .collect();
    if !pks.is_empty() {
        defs.push(format!("PRIMARY KEY ({})", pks.join(", ")));
    }
    format!(
        "CREATE TABLE {} ({})",
        quote(table, target),
        defs.join(", ")
    )
}

async fn copy_rows(
    src: &AnyPool,
    dst: &AnyPool,
    table: &str,
    cols: &[Column],
    source: Kind,
    target: Kind,
) -> Result<u64> {
    // Build the source SELECT, casting non-blob columns to text so any dialect
    // (dates, decimals, JSON, …) reads back as a String.
    let select_cols: Vec<String> = cols
        .iter()
        .map(|c| {
            let q = quote(&c.name, source);
            if c.cat == Cat::Blob {
                q
            } else {
                match source {
                    Kind::Mysql => format!("CAST({q} AS CHAR)"),
                    Kind::Postgres => format!("{q}::text"),
                    Kind::Sqlite => format!("CAST({q} AS TEXT)"),
                }
            }
        })
        .collect();
    let select = format!(
        "SELECT {} FROM {}",
        select_cols.join(", "),
        quote(table, source)
    );

    // Build the target INSERT. For Postgres, cast the text parameter to the
    // column type; MySQL/SQLite accept the implicit/affinity conversion.
    let col_list: Vec<String> = cols.iter().map(|c| quote(&c.name, target)).collect();
    let placeholders: Vec<String> = cols
        .iter()
        .map(|c| match target {
            Kind::Postgres if c.cat != Cat::Blob => {
                format!("?::{}", target_type(c.cat, target))
            }
            _ => "?".to_string(),
        })
        .collect();
    let insert = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote(table, target),
        col_list.join(", "),
        placeholders.join(", ")
    );

    let rows = sqlx::query(&select)
        .fetch_all(src)
        .await
        .map_err(|e| ServiceError::Init(format!("reading rows from {table}: {e}")))?;

    let mut tx = dst
        .begin()
        .await
        .map_err(|e| ServiceError::Init(e.to_string()))?;
    let mut count = 0u64;
    for row in &rows {
        let mut q = sqlx::query(&insert);
        for (i, c) in cols.iter().enumerate() {
            if c.cat == Cat::Blob {
                let v: Option<Vec<u8>> = row.try_get(i).ok().flatten();
                q = q.bind(v);
            } else {
                let v: Option<String> = read_text(row, i);
                q = q.bind(v);
            }
        }
        q.execute(&mut *tx)
            .await
            .map_err(|e| ServiceError::Init(format!("inserting into {table}: {e}")))?;
        count += 1;
    }
    tx.commit()
        .await
        .map_err(|e| ServiceError::Init(e.to_string()))?;
    Ok(count)
}

/// Read a column as a String, tolerating drivers that return it as bytes.
fn read_string(row: &AnyRow, i: usize) -> String {
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(i) {
        return s;
    }
    if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return String::from_utf8_lossy(&b).into_owned();
    }
    String::new()
}

/// Read a column as i64, tolerating i32 / bytes.
fn read_i64(row: &AnyRow, i: usize) -> i64 {
    if let Ok(v) = row.try_get::<i64, _>(i) {
        return v;
    }
    if let Ok(v) = row.try_get::<i32, _>(i) {
        return v as i64;
    }
    0
}

/// Read a column as text, tolerating drivers that hand back a native scalar.
fn read_text(row: &AnyRow, i: usize) -> Option<String> {
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return v;
    }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return v.map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return v.map(|n| n.to_string());
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return v.map(|n| n.to_string());
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
        return v.map(|b| if b { "1".into() } else { "0".into() });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite_spec(path: &std::path::Path) -> DbConnSpec {
        DbConnSpec {
            kind: "sqlite".into(),
            path: path.display().to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn sqlite_roundtrip_copies_schema_and_data() {
        sqlx::any::install_default_drivers();
        let dir = std::env::temp_dir().join(format!("grove-convert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.db");
        let dst_path = dir.join("dst.db");
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&dst_path);

        let src_url = format!("sqlite://{}?mode=rwc", src_path.display());
        let pool = AnyPoolOptions::new().connect(&src_url).await.unwrap();
        sqlx::query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER, note TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (id, name, age, note) VALUES (1, 'Alice', 30, 'hi')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (id, name, age, note) VALUES (2, 'Bob', NULL, 'x')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let msg = convert(&sqlite_spec(&src_path), &sqlite_spec(&dst_path), |_| {})
            .await
            .unwrap();
        assert!(msg.contains("1 table"), "{msg}");
        assert!(msg.contains("2 row"), "{msg}");

        let dpool = AnyPoolOptions::new()
            .connect(&format!("sqlite://{}?mode=ro", dst_path.display()))
            .await
            .unwrap();
        let rows = sqlx::query("SELECT id, name, age, note FROM users ORDER BY id")
            .fetch_all(&dpool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].try_get::<i64, _>(0).unwrap(), 1);
        assert_eq!(rows[0].try_get::<String, _>(1).unwrap(), "Alice");
        assert_eq!(rows[0].try_get::<i64, _>(2).unwrap(), 30);
        assert_eq!(rows[1].try_get::<Option<i64>, _>(2).unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Requires a throwaway MySQL: GROVE_TEST_MYSQL_PORT=13306 cargo test \
    //   -p grove-services mysql_sqlite_roundtrip -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn mysql_sqlite_roundtrip() {
        let port: u16 = std::env::var("GROVE_TEST_MYSQL_PORT")
            .expect("set GROVE_TEST_MYSQL_PORT")
            .parse()
            .unwrap();
        sqlx::any::install_default_drivers();

        let server = format!("mysql://root@127.0.0.1:{port}");
        let admin = AnyPoolOptions::new().connect(&server).await.unwrap();
        for db in ["grovesrc", "grovedst"] {
            sqlx::query(&format!("DROP DATABASE IF EXISTS {db}"))
                .execute(&admin)
                .await
                .unwrap();
            sqlx::query(&format!("CREATE DATABASE {db}"))
                .execute(&admin)
                .await
                .unwrap();
        }
        admin.close().await;

        let sp = AnyPoolOptions::new()
            .connect(&format!("mysql://root@127.0.0.1:{port}/grovesrc"))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE products (id INT PRIMARY KEY, name VARCHAR(100), \
             price DECIMAL(10,2), created_at DATETIME, active TINYINT(1))",
        )
        .execute(&sp)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO products VALUES \
             (1,'Widget',9.99,'2024-01-02 03:04:05',1),\
             (2,'Gadget',NULL,'2024-02-03 00:00:00',0)",
        )
        .execute(&sp)
        .await
        .unwrap();
        sp.close().await;

        let dir = std::env::temp_dir().join(format!("grove-mysqlite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sqlite_path = dir.join("out.db");
        let _ = std::fs::remove_file(&sqlite_path);

        let mysql_src = DbConnSpec {
            kind: "mysql".into(),
            host: "127.0.0.1".into(),
            port,
            user: "root".into(),
            database: "grovesrc".into(),
            ..Default::default()
        };

        // MySQL -> SQLite
        let msg = convert(&mysql_src, &sqlite_spec(&sqlite_path), |_| {})
            .await
            .unwrap();
        assert!(msg.contains("1 table") && msg.contains("2 row"), "{msg}");

        let vp = AnyPoolOptions::new()
            .connect(&format!("sqlite://{}?mode=ro", sqlite_path.display()))
            .await
            .unwrap();
        let rows = sqlx::query("SELECT id, name, created_at FROM products ORDER BY id")
            .fetch_all(&vp)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].try_get::<String, _>(1).unwrap(), "Widget");
        assert_eq!(
            rows[0].try_get::<String, _>(2).unwrap(),
            "2024-01-02 03:04:05"
        );
        vp.close().await;

        // SQLite -> MySQL
        let mysql_dst = DbConnSpec {
            database: "grovedst".into(),
            ..mysql_src.clone()
        };
        let msg2 = convert(&sqlite_spec(&sqlite_path), &mysql_dst, |_| {})
            .await
            .unwrap();
        assert!(msg2.contains("1 table") && msg2.contains("2 row"), "{msg2}");

        let dp = AnyPoolOptions::new()
            .connect(&format!("mysql://root@127.0.0.1:{port}/grovedst"))
            .await
            .unwrap();
        let cnt = sqlx::query("SELECT COUNT(*) FROM products")
            .fetch_one(&dp)
            .await
            .unwrap();
        assert_eq!(cnt.try_get::<i64, _>(0).unwrap(), 2);
        let name = sqlx::query("SELECT name FROM products WHERE id = 1")
            .fetch_one(&dp)
            .await
            .unwrap();
        assert_eq!(read_string(&name, 0), "Widget");
        dp.close().await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}

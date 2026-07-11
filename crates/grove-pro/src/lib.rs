//! Open-source **stub** for Grove Pro feature entry points.
//!
//! Grove is free and open source. A small set of *Pro* features (the database
//! schema inspector and inline row editing) are commercial: their real
//! implementation lives in a separate, proprietary crate that is injected into
//! official release binaries at build time. This in-tree stub keeps the public
//! workspace building freely and, when a build does **not** include the
//! proprietary crate, simply returns a friendly "not included" error.
//!
//! Note this is only the thin data-operation layer. Everything that makes Grove
//! useful — serving sites, HTTPS, PHP/Node, databases, mail, tunnels, the
//! free read-only database browser — is fully open and never routes through
//! here. See <https://elyracode.com/grove>.

use e_db::{ColumnInfo, DbConfig, ForeignKey, IndexInfo};

/// Primary-key / value pairs used to target a single row.
pub type Pk<'a> = &'a [(String, Option<String>)];

const NOT_INCLUDED: &str =
    "This build of Grove does not include Pro features (see elyracode.com/grove).";

/// Column metadata for a table (schema inspector) — Pro.
pub fn columns(_cfg: &DbConfig, _table: &str) -> Result<Vec<ColumnInfo>, String> {
    Err(NOT_INCLUDED.into())
}

/// Index metadata for a table (schema inspector) — Pro.
pub fn indexes(_cfg: &DbConfig, _table: &str) -> Result<Vec<IndexInfo>, String> {
    Err(NOT_INCLUDED.into())
}

/// Foreign keys for the database (schema inspector) — Pro.
pub fn foreign_keys(_cfg: &DbConfig) -> Result<Vec<ForeignKey>, String> {
    Err(NOT_INCLUDED.into())
}

/// `CREATE TABLE` DDL for a table (schema inspector) — Pro.
pub fn table_ddl(_cfg: &DbConfig, _table: &str) -> Result<String, String> {
    Err(NOT_INCLUDED.into())
}

/// Update a single cell, keyed by primary key — Pro.
pub fn update_cell(
    _cfg: &DbConfig,
    _table: &str,
    _column: &str,
    _value: Option<&str>,
    _pk: Pk,
) -> Result<u64, String> {
    Err(NOT_INCLUDED.into())
}

/// Delete a single row, keyed by primary key — Pro.
pub fn delete_row(_cfg: &DbConfig, _table: &str, _pk: Pk) -> Result<u64, String> {
    Err(NOT_INCLUDED.into())
}

/// Insert a row from column/value pairs — Pro.
pub fn insert_row(_cfg: &DbConfig, _table: &str, _values: Pk) -> Result<u64, String> {
    Err(NOT_INCLUDED.into())
}

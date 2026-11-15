use super::error::DbError;
use dioxus::logger::tracing;
use rusqlite::Connection;
use std::time::Duration;

pub(super) const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SqliteAutoVacuumMode {
    #[default]
    None,
    Full,
    Incremental,
}

impl SqliteAutoVacuumMode {
    fn from_pragma_value(value: i64) -> Result<Self, DbError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Full),
            2 => Ok(Self::Incremental),
            other => Err(DbError::new(format!(
                "Unexpected SQLite auto_vacuum mode value: {other}"
            ))),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }

    pub(crate) fn pragma_value(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Full => 1,
            Self::Incremental => 2,
        }
    }

    pub(crate) fn supports_incremental_vacuum(self) -> bool {
        matches!(self, Self::Incremental)
    }
}

pub(super) fn configure_connection(conn: &Connection, context: &'static str, enable_wal: bool) {
    if let Err(err) = conn.pragma_update(None, "foreign_keys", "ON") {
        tracing::warn!(database = context, sqlite_code = ?err.sqlite_error_code(), "failed to enable foreign keys");
    }

    if let Err(err) = conn.busy_timeout(SQLITE_BUSY_TIMEOUT) {
        tracing::warn!(database = context, sqlite_code = ?err.sqlite_error_code(), "failed to set busy_timeout");
    }

    if enable_wal {
        if let Err(err) = conn.pragma_update(None, "journal_mode", "WAL") {
            tracing::warn!(database = context, sqlite_code = ?err.sqlite_error_code(), "failed to enable WAL mode");
        } else {
            tracing::debug!(database = context, "WAL mode enabled");
        }
    }
}

pub(super) fn log_migration_result(
    database: &'static str,
    last_migration: Option<refinery::Migration>,
    applied_count: usize,
) {
    let schema_version = last_migration.map(|migration| migration.version());
    if applied_count == 0 {
        tracing::debug!(database, schema_version, "no migrations applied");
    } else {
        tracing::info!(
            database,
            schema_version,
            applied_count,
            "migrations applied"
        );
    }
}

pub(super) fn load_auto_vacuum_mode(
    conn: &Connection,
    context: &'static str,
) -> Result<SqliteAutoVacuumMode, DbError> {
    let mode: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .map_err(|err| DbError::from_rusqlite_error(context, err))?;
    SqliteAutoVacuumMode::from_pragma_value(mode)
        .map_err(|err| DbError::new(format!("{context}: {err}")))
}

pub(super) fn load_freelist_count(
    conn: &Connection,
    context: &'static str,
) -> Result<u32, DbError> {
    load_sqlite_pragma_u32(conn, "PRAGMA freelist_count", context)
}

pub(super) fn load_page_count(conn: &Connection, context: &'static str) -> Result<u32, DbError> {
    load_sqlite_pragma_u32(conn, "PRAGMA page_count", context)
}

pub(super) fn run_incremental_vacuum(
    conn: &Connection,
    pages: u32,
    context: &'static str,
) -> Result<(), DbError> {
    if pages == 0 {
        return Ok(());
    }

    conn.execute_batch(&format!("PRAGMA incremental_vacuum({pages})"))
        .map_err(|err| DbError::from_rusqlite_error(context, err))?;
    Ok(())
}

fn load_sqlite_pragma_u32(
    conn: &Connection,
    pragma_sql: &'static str,
    context: &'static str,
) -> Result<u32, DbError> {
    let value: i64 = conn
        .query_row(pragma_sql, [], |row| row.get(0))
        .map_err(|err| DbError::from_rusqlite_error(context, err))?;
    u32::try_from(value).map_err(|_| DbError::new(format!("{context}: value out of range")))
}

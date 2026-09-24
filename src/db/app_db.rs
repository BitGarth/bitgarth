//! App database - authentication, sessions, and other app-wide data
//!
//! Uses a thread-local singleton pattern with lazy initialization.
//! Migrations run automatically on first access.

use super::error::{AppDbStage, DbError};
use super::sqlite_config::configure_connection;
use crate::project_paths::{app_database_path_from_project_dir, get_project_dir};
use dioxus::logger::tracing;
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, path::Path, path::PathBuf};

#[cfg(all(test, feature = "db-tests"))]
#[path = "app_db_tests.rs"]
mod tests;

fn migrations_runner() -> Result<refinery::Runner, DbError> {
    const MIGRATIONS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/app_migrations.rs"));

    let migrations = MIGRATIONS
        .iter()
        .map(|(name, sql)| {
            refinery::Migration::unapplied(name, sql)
                .map_err(|e| DbError::from_app_refinery_error(AppDbStage::MigrationDefinition, e))
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    Ok(refinery::Runner::new(&migrations))
}

/// Flag to indicate whether we're in test mode (use in-memory database)
static TEST_MODE: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static TEST_DB_MUTEX: Mutex<()> = Mutex::new(());

/// Enable test mode - uses in-memory SQLite database
/// Must be called before any database operations
#[cfg(test)]
pub(crate) fn enable_test_mode() {
    TEST_MODE.store(true, Ordering::SeqCst);
}

/// Check if we're in test mode
fn is_test_mode() -> bool {
    TEST_MODE.load(Ordering::SeqCst)
}

fn test_db_guard() -> Result<Option<MutexGuard<'static, ()>>, DbError> {
    #[cfg(test)]
    {
        if is_test_mode() {
            return TEST_DB_MUTEX
                .lock()
                .map(Some)
                .map_err(|e| DbError::new(format!("Failed to lock test db mutex: {}", e)));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum AppDbConnectionKey {
    Production,
    TestInMemory,
}

enum AppDbAccessMode {
    Cached(AppDbConnectionKey),
    RuntimeContextFile(PathBuf),
}

fn ensure_parent_dir_exists(path: &Path) -> Result<(), DbError> {
    let Some(parent) = path.parent() else {
        return Err(DbError::new(format!(
            "App db path has no parent directory: {}",
            path.display()
        )));
    };

    fs::create_dir_all(parent).map_err(|err| DbError::from_app_io_error(AppDbStage::Path, err))
}

fn runtime_context_app_db_path() -> Result<Option<PathBuf>, DbError> {
    #[cfg(feature = "server")]
    if let Some(runtime_context) = crate::runtime_context::current_runtime_context() {
        let db_path = app_database_path_from_project_dir(runtime_context.project_dir());
        ensure_parent_dir_exists(&db_path)?;
        return Ok(Some(db_path));
    }

    Ok(None)
}

fn current_access_mode() -> Result<AppDbAccessMode, DbError> {
    if is_test_mode() {
        if let Some(db_path) = runtime_context_app_db_path()? {
            return Ok(AppDbAccessMode::RuntimeContextFile(db_path));
        }
        return Ok(AppDbAccessMode::Cached(AppDbConnectionKey::TestInMemory));
    }

    Ok(AppDbAccessMode::Cached(AppDbConnectionKey::Production))
}

fn initialize_connection_for_key(
    key: &AppDbConnectionKey,
) -> Result<rusqlite::Connection, DbError> {
    match key {
        AppDbConnectionKey::Production => {
            let root = get_project_dir()?;
            let db_path = app_database_path_from_project_dir(&root);
            initialize_file_connection(&db_path)
        }
        AppDbConnectionKey::TestInMemory => {
            tracing::info!("app db: opening in-memory database (test mode)");
            let mut conn = rusqlite::Connection::open_in_memory()
                .map_err(|e| DbError::from_app_sqlite_error(AppDbStage::Open, e))?;
            configure_and_migrate_connection(&mut conn)?;
            Ok(conn)
        }
    }
}

fn initialize_file_connection(db_path: &Path) -> Result<rusqlite::Connection, DbError> {
    ensure_parent_dir_exists(db_path)?;
    tracing::info!("app db: opening database at {}", db_path.display());

    let mut conn = rusqlite::Connection::open(db_path)
        .map_err(|e| DbError::from_app_sqlite_error(AppDbStage::Open, e))?;

    configure_and_migrate_connection(&mut conn)?;
    Ok(conn)
}

fn configure_and_migrate_connection(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    configure_connection(conn, "app db", !is_test_mode());

    let runner = migrations_runner()?;
    validate_migration_history(conn)
        .map_err(|e| DbError::from_app_sqlite_error(AppDbStage::Migration, e))?;
    let report = runner
        .run(conn)
        .map_err(|e| DbError::from_app_refinery_error(AppDbStage::Migration, e))?;

    let applied_count = report.applied_migrations().len();
    match runner
        .get_last_applied_migration(conn)
        .map_err(|e| DbError::from_app_refinery_error(AppDbStage::SchemaQuery, e))?
    {
        Some(migration) => {
            tracing::info!(
                "app db: migrations completed — schema version V{}__{}, applied {} new migration(s)",
                migration.version(),
                migration.name(),
                applied_count,
            );
        }
        None => {
            tracing::info!("app db: migrations completed — no migrations applied (empty schema)");
        }
    }

    Ok(())
}

fn validate_migration_history(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    // Refinery's locked SQLite driver unwraps these two parsers. Use the same
    // time parser (already re-exported by cookie) before handing it stored rows.
    use cookie::time::{OffsetDateTime, format_description::well_known::Rfc3339};
    use rusqlite::{Error::FromSqlConversionFailure, types::Type};

    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'refinery_schema_history' COLLATE NOCASE)",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let mut statement = conn.prepare("SELECT applied_on, checksum FROM refinery_schema_history")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let applied_on: String = row.get(0)?;
        OffsetDateTime::parse(&applied_on, &Rfc3339)
            .map_err(|error| FromSqlConversionFailure(0, Type::Text, Box::new(error)))?;
        let checksum: String = row.get(1)?;
        checksum
            .parse::<u64>()
            .map_err(|error| FromSqlConversionFailure(1, Type::Text, Box::new(error)))?;
    }
    Ok(())
}

type ThreadConnectionState = HashMap<AppDbConnectionKey, RefCell<rusqlite::Connection>>;

fn cached_connection(
    state: &mut ThreadConnectionState,
    key: AppDbConnectionKey,
    initialize: impl FnOnce(&AppDbConnectionKey) -> Result<rusqlite::Connection, DbError>,
) -> Result<&RefCell<rusqlite::Connection>, DbError> {
    use std::collections::hash_map::Entry;

    match state.entry(key) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => {
            let connection = initialize(entry.key())?;
            Ok(entry.insert(RefCell::new(connection)))
        }
    }
}

thread_local! {
    static DB_CELL: RefCell<ThreadConnectionState> = RefCell::new(HashMap::new());
}

/// Reset the database connection (for tests only)
/// This allows each test to start with a fresh in-memory database
#[cfg(test)]
pub(crate) fn reset_test_db() {
    DB_CELL.with(|cell| {
        cell.borrow_mut().remove(&AppDbConnectionKey::TestInMemory);
    });
}

/// Execute a function with a reference to the app database connection.
/// Returns an error if the database failed to initialize or if the callback fails.
pub(crate) fn with_app_db<F, T, E>(f: F) -> Result<T, E>
where
    F: FnOnce(&rusqlite::Connection) -> Result<T, E>,
    E: From<DbError>,
{
    match current_access_mode().map_err(E::from)? {
        AppDbAccessMode::RuntimeContextFile(db_path) => {
            let conn = initialize_file_connection(&db_path).map_err(E::from)?;
            f(&conn)
        }
        AppDbAccessMode::Cached(key) => {
            let _test_guard = test_db_guard().map_err(E::from)?;
            DB_CELL.with(|cell| {
                let mut cell_ref = cell.borrow_mut();
                let conn = cached_connection(&mut cell_ref, key, initialize_connection_for_key)
                    .map_err(E::from)?;
                let conn_ref = conn.borrow();
                f(&conn_ref)
            })
        }
    }
}

/// Initialize and validate the app database before starting services.
#[cfg(not(bitgarth_db_unit_only))]
pub(crate) fn initialize_app_db() -> Result<(), DbError> {
    with_app_db(|_| Ok(()))
}

/// Execute a function with a mutable reference to the app database connection.
/// This is needed for operations that require mutable access, such as transactions.
/// Returns an error if the database failed to initialize or if the callback fails.
pub(crate) fn with_app_db_mut<F, T, E>(f: F) -> Result<T, E>
where
    F: FnOnce(&mut rusqlite::Connection) -> Result<T, E>,
    E: From<DbError>,
{
    match current_access_mode().map_err(E::from)? {
        AppDbAccessMode::RuntimeContextFile(db_path) => {
            let mut conn = initialize_file_connection(&db_path).map_err(E::from)?;
            f(&mut conn)
        }
        AppDbAccessMode::Cached(key) => {
            let _test_guard = test_db_guard().map_err(E::from)?;
            DB_CELL.with(|cell| {
                let mut cell_ref = cell.borrow_mut();
                let conn = cached_connection(&mut cell_ref, key, initialize_connection_for_key)
                    .map_err(E::from)?;
                let mut conn_mut = conn.borrow_mut();
                f(&mut conn_mut)
            })
        }
    }
}

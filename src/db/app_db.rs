//! App database - authentication, sessions, and other app-wide data
//!
//! Uses a thread-local singleton pattern with lazy initialization.
//! Migrations run automatically on first access.

use super::error::DbError;
use super::sqlite_config::configure_connection;
use crate::project_paths::{app_database_path_from_project_dir, get_app_database_path};
use dioxus::logger::tracing;
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, path::Path, path::PathBuf};

fn migrations_runner() -> Result<refinery::Runner, DbError> {
    const MIGRATIONS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/app_migrations.rs"));

    let migrations = MIGRATIONS
        .iter()
        .map(|(name, sql)| {
            refinery::Migration::unapplied(name, sql)
                .map_err(|e| DbError::new(format!("Invalid migration {name}: {e}")))
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

    fs::create_dir_all(parent).map_err(|err| {
        DbError::new(format!(
            "Failed to create app db parent directory {}: {err}",
            parent.display()
        ))
    })
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
        AppDbConnectionKey::Production => initialize_file_connection(&get_app_database_path()?),
        AppDbConnectionKey::TestInMemory => {
            tracing::info!("app db: opening in-memory database (test mode)");
            let mut conn = rusqlite::Connection::open_in_memory()
                .map_err(|e| DbError::new(format!("Failed to open in-memory database: {}", e)))?;
            configure_and_migrate_connection(&mut conn)?;
            Ok(conn)
        }
    }
}

fn initialize_file_connection(db_path: &Path) -> Result<rusqlite::Connection, DbError> {
    ensure_parent_dir_exists(db_path)?;
    tracing::info!("app db: opening database at {:?}", db_path);

    let mut conn = rusqlite::Connection::open(db_path)
        .map_err(|e| DbError::new(format!("Failed to open database at {:?}: {}", db_path, e)))?;

    configure_and_migrate_connection(&mut conn)?;
    Ok(conn)
}

fn configure_and_migrate_connection(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    configure_connection(conn, "app db", !is_test_mode());

    let runner = migrations_runner()?;
    let report = runner
        .run(conn)
        .map_err(|e| DbError::new(format!("Failed to run app migrations: {}", e)))?;

    let applied_count = report.applied_migrations().len();
    match runner
        .get_last_applied_migration(conn)
        .map_err(|e| DbError::new(format!("Failed to query app schema version: {}", e)))?
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

type ConnectionCell = Result<RefCell<rusqlite::Connection>, DbError>;
type ThreadConnectionState = HashMap<AppDbConnectionKey, ConnectionCell>;

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
                let result = cell_ref
                    .entry(key.clone())
                    .or_insert_with(|| initialize_connection_for_key(&key).map(RefCell::new));
                match result {
                    Ok(conn) => {
                        let conn_ref = conn.borrow();
                        f(&conn_ref)
                    }
                    Err(e) => Err(E::from(e.clone())),
                }
            })
        }
    }
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
                let result = cell_ref
                    .entry(key.clone())
                    .or_insert_with(|| initialize_connection_for_key(&key).map(RefCell::new));
                match result {
                    Ok(conn) => {
                        let mut conn_mut = conn.borrow_mut();
                        f(&mut conn_mut)
                    }
                    Err(e) => Err(E::from(e.clone())),
                }
            })
        }
    }
}

//! User database - per-user settings and data
//!
//! Each user has their own SQLite database at `{project_dir}/users/{user_id}/data/u{user_id}.db`.
//! User databases are initialized after successful login and closed on logout.

use super::encryption::{Dek, SqlcipherCompatibility, UserDbOpenMode};
use super::error::DbError;
use super::sqlite_config::{SqliteAutoVacuumMode, configure_connection, load_auto_vacuum_mode};
use crate::models::UserId;
use crate::project_paths::{get_user_database_path, user_database_path_from_project_dir};
use chrono::Utc;
use dioxus::logger::tracing;
use once_cell::sync::{Lazy, OnceCell};
use rusqlite::OpenFlags;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard, RwLock, atomic::AtomicBool, atomic::AtomicUsize, atomic::Ordering,
};

pub(super) fn migrations_runner() -> Result<refinery::Runner, DbError> {
    const MIGRATIONS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/user_migrations.rs"));

    let migrations = MIGRATIONS
        .iter()
        .map(|(name, sql)| {
            refinery::Migration::unapplied(name, sql)
                .map_err(|e| DbError::new(format!("Invalid migration {name}: {e}")))
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    Ok(refinery::Runner::new(&migrations))
}

static TEST_MODE: AtomicBool = AtomicBool::new(false);
const READ_CONNECTION_POOL_SIZE: usize = 4;

struct UserConnections {
    reads: Vec<Mutex<rusqlite::Connection>>,
    next_read: AtomicUsize,
    write: Mutex<rusqlite::Connection>,
}

#[derive(Clone)]
struct InitializedUserConnection {
    connections: Arc<UserConnections>,
    open_mode: UserDbOpenMode,
}

struct UserConnectionEntry {
    state: OnceCell<InitializedUserConnection>,
}

impl UserConnectionEntry {
    fn get(&self) -> Option<&InitializedUserConnection> {
        self.state.get()
    }

    fn initialize(
        &self,
        scoped_key: ScopedUserDbKey,
        open_mode: UserDbOpenMode,
    ) -> Result<&InitializedUserConnection, DbError> {
        self.state.get_or_try_init(|| {
            let connections = open_user_connections(scoped_key, &open_mode)?;
            Ok(InitializedUserConnection {
                connections: Arc::new(connections),
                open_mode,
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum UserDbScopeId {
    Global,
    #[cfg(feature = "server")]
    Runtime(crate::runtime_context::RuntimeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScopedUserDbKey {
    scope_id: UserDbScopeId,
    user_id: UserId,
}

impl ScopedUserDbKey {
    fn new(user_id: UserId) -> Self {
        Self {
            scope_id: current_user_db_scope_id(),
            user_id,
        }
    }
}

fn current_user_db_scope_id() -> UserDbScopeId {
    #[cfg(feature = "server")]
    if let Some(runtime_context) = crate::runtime_context::current_runtime_context() {
        return UserDbScopeId::Runtime(runtime_context.runtime_id());
    }

    UserDbScopeId::Global
}

static USER_CONNECTIONS: Lazy<RwLock<HashMap<ScopedUserDbKey, Arc<UserConnectionEntry>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserDbLockState {
    pub(crate) read_locks: usize,
    pub(crate) write_locks: usize,
}

#[cfg(any(test, debug_assertions))]
impl UserDbLockState {
    fn is_unlocked(self) -> bool {
        self.read_locks == 0 && self.write_locks == 0
    }
}

#[cfg(any(test, debug_assertions))]
static USER_DB_LOCK_STATES: Lazy<Mutex<HashMap<ScopedUserDbKey, UserDbLockState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[cfg(any(test, debug_assertions))]
#[derive(Clone, Copy)]
enum UserDbLockKind {
    Read,
    Write,
}

#[cfg(any(test, debug_assertions))]
struct UserDbLockTracker {
    key: ScopedUserDbKey,
    kind: UserDbLockKind,
}

#[cfg(any(test, debug_assertions))]
impl UserDbLockTracker {
    fn acquire(key: ScopedUserDbKey, kind: UserDbLockKind) -> Self {
        let mut guard = match USER_DB_LOCK_STATES.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let state = guard.entry(key).or_default();
        match kind {
            UserDbLockKind::Read => {
                state.read_locks = state.read_locks.saturating_add(1);
            }
            UserDbLockKind::Write => {
                state.write_locks = state.write_locks.saturating_add(1);
            }
        }
        Self { key, kind }
    }
}

#[cfg(any(test, debug_assertions))]
impl Drop for UserDbLockTracker {
    fn drop(&mut self) {
        let mut guard = match USER_DB_LOCK_STATES.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(state) = guard.get_mut(&self.key) else {
            return;
        };
        match self.kind {
            UserDbLockKind::Read => {
                state.read_locks = state.read_locks.saturating_sub(1);
            }
            UserDbLockKind::Write => {
                state.write_locks = state.write_locks.saturating_sub(1);
            }
        }
        if state.is_unlocked() {
            guard.remove(&self.key);
        }
    }
}

#[cfg(any(test, debug_assertions))]
fn current_user_db_lock_state(user_id: UserId) -> UserDbLockState {
    let key = ScopedUserDbKey::new(user_id);
    let guard = match USER_DB_LOCK_STATES.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.get(&key).copied().unwrap_or_default()
}

#[cfg(any(test, debug_assertions))]
#[track_caller]
pub(crate) fn debug_assert_user_db_unlocked(user_id: UserId, boundary: &'static str) {
    let state = current_user_db_lock_state(user_id);
    debug_assert!(
        state.is_unlocked(),
        "user db lock held across {boundary} for user {user_id}: read_locks={}, write_locks={}",
        state.read_locks,
        state.write_locks,
    );
}

#[cfg(not(any(test, debug_assertions)))]
pub(crate) fn debug_assert_user_db_unlocked(_user_id: UserId, _boundary: &'static str) {}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn user_db_lock_state_for_test(user_id: UserId) -> UserDbLockState {
    current_user_db_lock_state(user_id)
}

#[cfg(test)]
pub(crate) fn enable_test_mode() {
    TEST_MODE.store(true, Ordering::SeqCst);
}

fn is_test_mode() -> bool {
    TEST_MODE.load(Ordering::SeqCst)
}

fn incompatible_user_schema_error(detail: impl Into<String>) -> DbError {
    DbError::new(format!(
        "User database schema is incompatible: {}. Recreate the user database.",
        detail.into()
    ))
}

const REQUIRED_USER_TABLES: &[&str] = &[
    "account_integration_sync_state",
    "digital_asset_account_hd_keys",
    "account_transaction_ledger",
    "source_connections",
    "sync_runs",
    "request_attempts",
    "raw_observation_sets",
    "raw_parse_attempts",
    "raw_mempool_transaction_versions",
    "raw_mempool_transaction_observations",
    "raw_etherscan_normal_transaction_versions",
    "raw_etherscan_internal_transaction_versions",
    "raw_etherscan_normal_transaction_observations",
    "raw_etherscan_internal_transaction_observations",
    "hd_account_chain_sync_state",
    "user_price_overrides",
    "api_keys",
    "manual_asset_accounts",
    "manual_asset_balance_assertions",
];

const REQUIRED_USER_COLUMNS: &[(&str, &str)] = &[
    (
        "digital_asset_account_hd_keys",
        "normalized_extended_pubkey",
    ),
    ("sync_runs", "source_connection_id"),
    ("request_attempts", "request_query_json"),
    ("raw_parse_attempts", "raw_object_key_json"),
    ("raw_mempool_transaction_versions", "source_connection_id"),
    (
        "raw_mempool_transaction_observations",
        "raw_observation_set_id",
    ),
    (
        "raw_etherscan_normal_transaction_versions",
        "source_connection_id",
    ),
    (
        "raw_etherscan_internal_transaction_versions",
        "source_connection_id",
    ),
    (
        "raw_etherscan_normal_transaction_observations",
        "raw_observation_set_id",
    ),
    (
        "raw_etherscan_internal_transaction_observations",
        "raw_observation_set_id",
    ),
    ("manual_asset_accounts", "asset_id"),
    ("manual_asset_accounts", "network_id"),
    ("manual_asset_accounts", "decimal_precision"),
    ("manual_asset_accounts", "unit_code"),
    ("manual_asset_accounts", "symbol"),
    ("manual_asset_accounts", "asset_name"),
    ("manual_asset_accounts", "network_name"),
    ("manual_asset_accounts", "coingecko_id"),
    ("manual_asset_accounts", "asset_source"),
    ("manual_asset_accounts", "precision_source"),
    ("manual_asset_accounts", "coingecko_platform_id"),
    ("manual_asset_accounts", "provider_platform_asset_ref"),
    ("manual_asset_balance_assertions", "entered_balance_text"),
];

const REQUIRED_USER_INDEXES: &[&str] = &[
    "idx_daa_hd_normalized_scheme",
    "idx_account_tx_ledger_account_status",
    "idx_account_tx_ledger_pending_page",
    "idx_account_tx_ledger_confirmed_page",
    "idx_account_tx_ledger_chain_tx_id",
    "idx_tx_inputs_address",
    "idx_source_connections_current_address",
    "idx_source_connections_status_integration",
    "idx_sync_runs_scope_started",
    "idx_sync_runs_status_started",
    "idx_request_attempts_run_attempted",
    "idx_raw_observation_sets_run_observed",
    "idx_raw_observation_sets_source_observed",
    "idx_raw_parse_attempts_run_attempted",
    "idx_raw_mempool_tx_versions_txid_created",
    "idx_raw_mempool_tx_versions_txid_hash",
    "idx_raw_mempool_tx_versions_supersedes",
    "idx_raw_mempool_tx_obs_run_observed",
    "idx_raw_etherscan_normal_versions_identity_hash",
    "idx_raw_etherscan_normal_versions_supersedes",
    "idx_raw_etherscan_internal_versions_identity_hash",
    "idx_raw_etherscan_internal_versions_supersedes",
    "idx_raw_etherscan_normal_observations_run_observed",
    "idx_raw_etherscan_normal_observations_request_order",
    "idx_raw_etherscan_internal_observations_run_observed",
    "idx_raw_etherscan_internal_observations_request_order",
    "idx_account_integration_sync_state_updated_at",
    "idx_hd_account_chain_sync_state_account_change",
    "idx_hd_account_chain_sync_state_updated_at",
    "idx_user_price_overrides_lookup",
    "idx_maa_wallet",
    "idx_maa_asset_instance",
    "idx_maa_label_key",
    "idx_maba_account_asserted_on",
];

struct UserSchemaCatalog {
    tables: HashSet<String>,
    indexes: HashSet<String>,
}

fn load_user_schema_catalog(conn: &rusqlite::Connection) -> Result<UserSchemaCatalog, DbError> {
    let mut stmt = conn
        .prepare("SELECT type, name FROM sqlite_master WHERE type IN ('table', 'index')")
        .map_err(|err| DbError::new(format!("Failed to inspect user schema catalog: {err}")))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| DbError::new(format!("Failed to query user schema catalog: {err}")))?;

    let mut tables = HashSet::new();
    let mut indexes = HashSet::new();
    for row in rows {
        let (object_type, object_name) =
            row.map_err(|err| DbError::new(format!("Failed to read user schema catalog: {err}")))?;
        match object_type.as_str() {
            "table" => {
                tables.insert(object_name);
            }
            "index" => {
                indexes.insert(object_name);
            }
            _ => {}
        }
    }

    Ok(UserSchemaCatalog { tables, indexes })
}

fn load_table_columns(
    conn: &rusqlite::Connection,
    table_name: &str,
) -> Result<HashSet<String>, DbError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .map_err(|e| {
            DbError::new(format!(
                "Failed to inspect table schema for {table_name}: {e}"
            ))
        })?;

    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| {
            DbError::new(format!(
                "Failed to query table columns for {table_name}: {e}"
            ))
        })?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| {
            DbError::new(format!(
                "Failed to read table columns for {table_name}: {e}"
            ))
        })?;

    Ok(columns)
}

fn ensure_column_exists(
    conn: &rusqlite::Connection,
    table_name: &str,
    column_name: &str,
) -> Result<(), DbError> {
    let columns = load_table_columns(conn, table_name)?;
    if columns.contains(column_name) {
        return Ok(());
    }

    Err(incompatible_user_schema_error(format!(
        "missing required column {table_name}.{column_name}"
    )))
}

fn ensure_index_exists(catalog: &UserSchemaCatalog, index_name: &str) -> Result<(), DbError> {
    if catalog.indexes.contains(index_name) {
        return Ok(());
    }

    Err(incompatible_user_schema_error(format!(
        "missing required index {index_name}"
    )))
}

fn ensure_table_exists(catalog: &UserSchemaCatalog, table_name: &str) -> Result<(), DbError> {
    if catalog.tables.contains(table_name) {
        return Ok(());
    }

    Err(incompatible_user_schema_error(format!(
        "missing required table {table_name}"
    )))
}

fn ensure_required_user_schema(conn: &rusqlite::Connection) -> Result<(), DbError> {
    let catalog = load_user_schema_catalog(conn)?;

    for table_name in REQUIRED_USER_TABLES {
        ensure_table_exists(&catalog, table_name)?;
    }
    for (table_name, column_name) in REQUIRED_USER_COLUMNS {
        ensure_column_exists(conn, table_name, column_name)?;
    }
    for index_name in REQUIRED_USER_INDEXES {
        ensure_index_exists(&catalog, index_name)?;
    }

    Ok(())
}

fn scoped_user_db_key(user_id: UserId) -> ScopedUserDbKey {
    ScopedUserDbKey::new(user_id)
}

fn user_db_scope_token(scope_id: UserDbScopeId) -> String {
    match scope_id {
        UserDbScopeId::Global => "global".to_string(),
        #[cfg(feature = "server")]
        UserDbScopeId::Runtime(runtime_id) => runtime_id.to_string(),
    }
}

fn in_memory_user_db_uri(key: ScopedUserDbKey) -> String {
    format!(
        "file:bg-user-{}-{}?mode=memory&cache=shared",
        user_db_scope_token(key.scope_id),
        key.user_id
    )
}

fn ensure_user_db_parent_dir_exists(db_path: &Path) -> Result<(), DbError> {
    let Some(parent) = db_path.parent() else {
        return Err(DbError::new(format!(
            "User db path has no parent directory: {}",
            db_path.display()
        )));
    };

    std::fs::create_dir_all(parent).map_err(|err| {
        DbError::new(format!(
            "Failed to create user db parent directory {}: {err}",
            parent.display()
        ))
    })
}

fn runtime_context_user_db_path(user_id: UserId) -> Result<Option<PathBuf>, DbError> {
    #[cfg(feature = "server")]
    if let Some(runtime_context) = crate::runtime_context::current_runtime_context() {
        let db_path = user_database_path_from_project_dir(runtime_context.project_dir(), user_id);
        ensure_user_db_parent_dir_exists(&db_path)?;
        return Ok(Some(db_path));
    }

    Ok(None)
}

fn apply_pragma_key(conn: &rusqlite::Connection, dek: &Dek) -> Result<(), DbError> {
    let key = format!("x'{}'", dek.as_hex());
    conn.execute_batch(&format!("PRAGMA key = \"{}\"", key))
        .map_err(|err| DbError::from_rusqlite_error("Failed to set PRAGMA key", err))?;
    Ok(())
}

fn apply_cipher_compatibility(
    conn: &rusqlite::Connection,
    compatibility: &SqlcipherCompatibility,
) -> Result<(), DbError> {
    conn.pragma_update(
        None,
        "cipher_compatibility",
        compatibility.as_u32().to_string(),
    )
    .map_err(|err| {
        DbError::from_rusqlite_error(
            format!(
                "Failed to set SQLCipher compatibility to {}",
                compatibility.as_u32()
            ),
            err,
        )
    })?;
    Ok(())
}

fn apply_encrypted_db_pragmas(
    conn: &rusqlite::Connection,
    dek: &Dek,
    compatibility: &SqlcipherCompatibility,
) -> Result<(), DbError> {
    apply_pragma_key(conn, dek)?;
    apply_cipher_compatibility(conn, compatibility)?;
    Ok(())
}

fn database_has_schema_objects(conn: &rusqlite::Connection) -> Result<bool, DbError> {
    let object_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'index', 'view', 'trigger')",
            [],
            |row| row.get(0),
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to inspect user database schema objects", err)
        })?;
    Ok(object_count > 0)
}

fn prepare_new_user_db_for_incremental_vacuum(
    conn: &rusqlite::Connection,
    user_id: UserId,
) -> Result<(), DbError> {
    if database_has_schema_objects(conn)? {
        return Ok(());
    }

    conn.pragma_update(
        None,
        "auto_vacuum",
        SqliteAutoVacuumMode::Incremental.pragma_value(),
    )
    .map_err(|err| {
        DbError::from_rusqlite_error(
            "Failed to enable incremental auto_vacuum for new user database",
            err,
        )
    })?;

    let mode = load_auto_vacuum_mode(
        conn,
        "Failed to verify incremental auto_vacuum mode for new user database",
    )?;
    if mode != SqliteAutoVacuumMode::Incremental {
        return Err(DbError::new(format!(
            "Failed to enable incremental auto_vacuum for new user database {user_id}: expected incremental mode, found {}",
            mode.as_str()
        )));
    }

    tracing::info!(
        user_id = %user_id,
        auto_vacuum_mode = mode.as_str(),
        "user db: enabled incremental auto_vacuum for new database"
    );

    Ok(())
}

fn open_user_write_connection(
    scoped_key: ScopedUserDbKey,
    dek: Option<&Dek>,
    compatibility: Option<&SqlcipherCompatibility>,
) -> Result<rusqlite::Connection, DbError> {
    if is_test_mode() {
        if let Some(db_path) = runtime_context_user_db_path(scoped_key.user_id)? {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let conn = rusqlite::Connection::open_with_flags(&db_path, flags).map_err(|err| {
                DbError::from_rusqlite_error(
                    format!(
                        "Failed to open runtime-scoped user write database at {}",
                        db_path.display()
                    ),
                    err,
                )
            })?;
            if let Some((dek, compatibility)) = dek.zip(compatibility) {
                apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
            }
            return Ok(conn);
        }

        let uri = in_memory_user_db_uri(scoped_key);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI;
        let conn = rusqlite::Connection::open_with_flags(&uri, flags).map_err(|err| {
            DbError::from_rusqlite_error("Failed to open in-memory user write database", err)
        })?;
        if let Some((dek, compatibility)) = dek.zip(compatibility) {
            apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
        }
        return Ok(conn);
    }

    let db_path = get_user_database_path(scoped_key.user_id)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let conn = rusqlite::Connection::open_with_flags(&db_path, flags).map_err(|err| {
        DbError::from_rusqlite_error(
            format!(
                "Failed to open user write database at {}",
                db_path.display()
            ),
            err,
        )
    })?;

    if let Some((dek, compatibility)) = dek.zip(compatibility) {
        apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
    }

    Ok(conn)
}

fn open_user_read_connection(
    scoped_key: ScopedUserDbKey,
    dek: Option<&Dek>,
    compatibility: Option<&SqlcipherCompatibility>,
) -> Result<rusqlite::Connection, DbError> {
    if is_test_mode() {
        if let Some(db_path) = runtime_context_user_db_path(scoped_key.user_id)? {
            let conn =
                rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|err| {
                    DbError::from_rusqlite_error(
                        format!(
                            "Failed to open runtime-scoped user read database at {}",
                            db_path.display()
                        ),
                        err,
                    )
                })?;
            if let Some((dek, compatibility)) = dek.zip(compatibility) {
                apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
            }
            return Ok(conn);
        }

        let uri = in_memory_user_db_uri(scoped_key);
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        let conn = rusqlite::Connection::open_with_flags(&uri, flags).map_err(|err| {
            DbError::from_rusqlite_error("Failed to open in-memory user read database", err)
        })?;
        if let Some((dek, compatibility)) = dek.zip(compatibility) {
            apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
        }
        return Ok(conn);
    }

    let db_path = get_user_database_path(scoped_key.user_id)?;
    let conn = rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| {
            DbError::from_rusqlite_error(
                format!("Failed to open user read database at {}", db_path.display()),
                err,
            )
        })?;

    if let Some((dek, compatibility)) = dek.zip(compatibility) {
        apply_encrypted_db_pragmas(&conn, dek, compatibility)?;
    }

    Ok(conn)
}

fn lookup_user_connection_entry(
    scoped_key: ScopedUserDbKey,
) -> Result<Option<Arc<UserConnectionEntry>>, DbError> {
    let guard = USER_CONNECTIONS
        .read()
        .map_err(|err| DbError::new(format!("Failed to acquire user db read lock: {err}")))?;
    Ok(guard.get(&scoped_key).cloned())
}

fn user_connection_entry(scoped_key: ScopedUserDbKey) -> Result<Arc<UserConnectionEntry>, DbError> {
    if let Some(entry) = lookup_user_connection_entry(scoped_key)? {
        return Ok(entry);
    }

    let mut guard = USER_CONNECTIONS
        .write()
        .map_err(|err| DbError::new(format!("Failed to acquire user db write lock: {err}")))?;
    Ok(guard
        .entry(scoped_key)
        .or_insert_with(|| {
            Arc::new(UserConnectionEntry {
                state: OnceCell::new(),
            })
        })
        .clone())
}

fn initialized_user_connections(
    user_id: UserId,
) -> Result<Arc<InitializedUserConnection>, DbError> {
    let scoped_key = scoped_user_db_key(user_id);
    let entry = lookup_user_connection_entry(scoped_key)?.ok_or_else(|| {
        DbError::new(format!(
            "User database not initialized for user {}",
            user_id
        ))
    })?;
    entry
        .get()
        .map(|initialized| Arc::new(initialized.clone()))
        .ok_or_else(|| {
            DbError::new(format!(
                "User database not initialized for user {}",
                user_id
            ))
        })
}

fn is_compatible_with(existing: &UserDbOpenMode, requested: &UserDbOpenMode) -> bool {
    match (existing, requested) {
        (
            UserDbOpenMode::Encrypted {
                dek: existing_dek,
                sqlcipher_compatibility: existing_compatibility,
                ..
            },
            UserDbOpenMode::Encrypted {
                dek: requested_dek,
                sqlcipher_compatibility: requested_compatibility,
                ..
            },
        ) => {
            existing_dek.as_hex() == requested_dek.as_hex()
                && existing_compatibility == requested_compatibility
        }
        #[cfg(feature = "dev-config")]
        (UserDbOpenMode::UnencryptedDev, UserDbOpenMode::UnencryptedDev) => true,
        #[cfg(all(test, feature = "db-tests"))]
        (UserDbOpenMode::PlaintextTest, UserDbOpenMode::PlaintextTest) => true,
        #[cfg(any(feature = "dev-config", all(test, feature = "db-tests")))]
        _ => false,
    }
}

fn open_user_connections(
    scoped_key: ScopedUserDbKey,
    open_mode: &UserDbOpenMode,
) -> Result<UserConnections, DbError> {
    let user_id = scoped_key.user_id;
    let (dek, encrypted_authority, sqlcipher_compatibility) = match open_mode {
        UserDbOpenMode::Encrypted {
            dek,
            authority,
            sqlcipher_compatibility,
        } => (
            Some(dek.clone()),
            Some(*authority),
            Some(sqlcipher_compatibility.clone()),
        ),
        #[cfg(feature = "dev-config")]
        UserDbOpenMode::UnencryptedDev => (None, None, None),
        #[cfg(all(test, feature = "db-tests"))]
        UserDbOpenMode::PlaintextTest => (None, None, None),
    };

    if is_test_mode() {
        tracing::info!(
            user_id = %user_id,
            read_handles = READ_CONNECTION_POOL_SIZE,
            encrypted_authority = ?encrypted_authority,
            "user db: opening shared in-memory read/write databases (test mode)"
        );
    } else {
        let db_path = get_user_database_path(user_id)?;
        tracing::info!(
            user_id = %user_id,
            path = ?db_path,
            read_handles = READ_CONNECTION_POOL_SIZE,
            encrypted_authority = ?encrypted_authority,
            "user db: opening read/write databases"
        );
    }

    let mut write_conn =
        open_user_write_connection(scoped_key, dek.as_ref(), sqlcipher_compatibility.as_ref())?;
    prepare_new_user_db_for_incremental_vacuum(&write_conn, user_id)?;
    configure_connection(
        &write_conn,
        &format!("user db write handle for user {user_id}"),
        !is_test_mode(),
    );

    let runner = migrations_runner()?;
    let report = runner
        .run(&mut write_conn)
        .map_err(|e| DbError::new(format!("Failed to run user migrations: {}", e)))?;

    let applied_count = report.applied_migrations().len();
    match runner
        .get_last_applied_migration(&mut write_conn)
        .map_err(|e| DbError::new(format!("Failed to query user schema version: {}", e)))?
    {
        Some(migration) => {
            tracing::info!(
                "user db: migrations completed for user {} — schema version V{}__{}, applied {} new migration(s)",
                user_id,
                migration.version(),
                migration.name(),
                applied_count,
            );
        }
        None => {
            tracing::info!(
                "user db: migrations completed for user {} — no migrations applied (empty schema)",
                user_id,
            );
        }
    }
    ensure_required_user_schema(&write_conn)?;
    crate::db::run_pending_user_data_repairs_conn(&mut write_conn, user_id, Utc::now())?;
    let repaired_legacy_mempool_heads =
        crate::db::raw_ingestion::repair_legacy_mempool_head_rebuild_contract(&mut write_conn)?;
    if repaired_legacy_mempool_heads > 0 {
        tracing::info!(
            user_id = %user_id,
            repaired_legacy_mempool_heads,
            "user db: repaired legacy mempool current-head lineage before opening read handles"
        );
    }

    let capability_ids = crate::db::capability_ids_for_user(user_id)?;
    super::paired_client_names::remove_orphan_paired_client_names_conn(
        &mut write_conn,
        &capability_ids,
    )?;

    let mut read_conns = Vec::with_capacity(READ_CONNECTION_POOL_SIZE);
    for read_index in 0..READ_CONNECTION_POOL_SIZE {
        let read_conn =
            open_user_read_connection(scoped_key, dek.as_ref(), sqlcipher_compatibility.as_ref())?;
        configure_connection(
            &read_conn,
            &format!("user db read handle {read_index} for user {user_id}"),
            false,
        );
        if let Err(err) = read_conn.pragma_update(None, "query_only", "ON") {
            tracing::warn!(
                "user db read handle {read_index} for user {user_id}: failed to enable query_only: {err}"
            );
        }
        read_conns.push(Mutex::new(read_conn));
    }

    Ok(UserConnections {
        reads: read_conns,
        next_read: AtomicUsize::new(0),
        write: Mutex::new(write_conn),
    })
}

fn lock_user_read_connection(
    user_connections: &UserConnections,
) -> Result<MutexGuard<'_, rusqlite::Connection>, DbError> {
    if user_connections.reads.is_empty() {
        return Err(DbError::new("User read connection pool is empty"));
    }

    let start_index =
        user_connections.next_read.fetch_add(1, Ordering::Relaxed) % user_connections.reads.len();

    for offset in 0..user_connections.reads.len() {
        let index = (start_index + offset) % user_connections.reads.len();
        if let Ok(conn) = user_connections.reads[index].try_lock() {
            return Ok(conn);
        }
    }

    user_connections.reads[start_index]
        .lock()
        .map_err(|err| DbError::new(format!("Failed to lock user read connection: {err}")))
}

pub(crate) fn initialize_user_db(
    user_id: UserId,
    open_mode: UserDbOpenMode,
) -> Result<(), DbError> {
    let scoped_key = scoped_user_db_key(user_id);

    if let Some(entry) = lookup_user_connection_entry(scoped_key)?
        && let Some(initialized) = entry.get()
    {
        if is_compatible_with(&initialized.open_mode, &open_mode) {
            tracing::debug!(
                user_id = %user_id,
                "user db: already initialized with compatible mode"
            );
            return Ok(());
        } else {
            return Err(DbError::new(format!(
                "User database already initialized with incompatible mode for user {}",
                user_id
            )));
        }
    }

    user_connection_entry(scoped_key)?.initialize(scoped_key, open_mode)?;
    Ok(())
}

pub(crate) fn close_user_db(user_id: UserId) -> Result<(), DbError> {
    let scoped_key = scoped_user_db_key(user_id);
    let mut guard = USER_CONNECTIONS
        .write()
        .map_err(|err| DbError::new(format!("Failed to acquire user db write lock: {err}")))?;

    if guard.remove(&scoped_key).is_some() {
        tracing::info!("user db: closed connection for user {}", user_id);
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn close_user_dbs_for_current_runtime() -> Result<(), DbError> {
    let scope_id = current_user_db_scope_id();
    let mut guard = USER_CONNECTIONS
        .write()
        .map_err(|err| DbError::new(format!("Failed to acquire user db write lock: {err}")))?;
    guard.retain(|scoped_key, _| scoped_key.scope_id != scope_id);

    #[cfg(any(test, debug_assertions))]
    {
        let mut lock_guard = match USER_DB_LOCK_STATES.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        lock_guard.retain(|scoped_key, _| scoped_key.scope_id != scope_id);
    }

    Ok(())
}

pub(crate) fn list_open_user_db_users() -> Result<Vec<UserId>, DbError> {
    let scope_id = current_user_db_scope_id();
    let guard = USER_CONNECTIONS
        .read()
        .map_err(|err| DbError::new(format!("Failed to acquire user db read lock: {err}")))?;

    Ok(guard
        .iter()
        .filter_map(|(scoped_key, entry)| {
            (scoped_key.scope_id == scope_id)
                .then_some(scoped_key.user_id)
                .filter(|_| entry.get().is_some())
        })
        .collect())
}

pub(crate) fn get_user_db_dek(user_id: &UserId) -> Result<Option<Dek>, DbError> {
    let scoped_key = scoped_user_db_key(*user_id);
    let entry = lookup_user_connection_entry(scoped_key)?.ok_or_else(|| {
        DbError::new(format!(
            "User database not initialized for user {}",
            user_id
        ))
    })?;

    if let Some(initialized) = entry.get() {
        match &initialized.open_mode {
            UserDbOpenMode::Encrypted { dek, .. } => return Ok(Some(dek.clone())),
            #[cfg(feature = "dev-config")]
            UserDbOpenMode::UnencryptedDev => return Ok(None),
            #[cfg(all(test, feature = "db-tests"))]
            UserDbOpenMode::PlaintextTest => return Ok(None),
        }
    }

    Err(DbError::new(format!(
        "User database not initialized for user {}",
        user_id
    )))
}

pub(crate) fn with_user_db<F, T, E>(user_id: UserId, f: F) -> Result<T, E>
where
    F: FnOnce(&rusqlite::Connection) -> Result<T, E>,
    E: From<DbError>,
{
    let initialized = initialized_user_connections(user_id).map_err(E::from)?;

    let conn = lock_user_read_connection(&initialized.connections).map_err(E::from)?;
    #[cfg(any(test, debug_assertions))]
    let tracker = UserDbLockTracker::acquire(scoped_user_db_key(user_id), UserDbLockKind::Read);

    let tx = conn.unchecked_transaction().map_err(|err| {
        E::from(DbError::from_rusqlite_error(
            "Failed to start user database read transaction",
            err,
        ))
    })?;
    let result = f(&tx);
    drop(tx);
    drop(conn);
    #[cfg(any(test, debug_assertions))]
    drop(tracker);
    result
}

pub(crate) fn with_user_db_mut<F, T, E>(user_id: UserId, f: F) -> Result<T, E>
where
    F: FnOnce(&mut rusqlite::Connection) -> Result<T, E>,
    E: From<DbError>,
{
    let initialized = initialized_user_connections(user_id).map_err(E::from)?;

    let mut conn = initialized
        .connections
        .write
        .lock()
        .map_err(|err| DbError::new(format!("Failed to lock user write connection: {err}")))?;
    #[cfg(any(test, debug_assertions))]
    let tracker = UserDbLockTracker::acquire(scoped_user_db_key(user_id), UserDbLockKind::Write);

    let result = f(&mut conn);
    drop(conn);
    #[cfg(any(test, debug_assertions))]
    drop(tracker);
    result
}

#[cfg(all(test, feature = "db-tests"))]
static TEMPLATE_USER_DB: std::sync::LazyLock<PathBuf> =
    std::sync::LazyLock::new(initialize_template_user_db);

#[cfg(all(test, feature = "db-tests"))]
fn initialize_template_user_db() -> PathBuf {
    let template_dir =
        std::env::temp_dir().join(format!("bitgarth_test_template_{}", std::process::id()));
    std::fs::create_dir_all(&template_dir).expect("template user db dir should create");
    let template_path = template_dir.join("user_db_template.sqlite");

    let mut conn =
        rusqlite::Connection::open(&template_path).expect("template user db should open");
    prepare_new_user_db_for_incremental_vacuum(&conn, UserId::new())
        .expect("template auto_vacuum should configure");
    configure_connection(&conn, "template user db", false);
    let runner = migrations_runner().expect("template migrations runner should create");
    runner
        .run(&mut conn)
        .expect("template migrations should apply");
    drop(conn);

    template_path
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn initialize_user_db_for_test(user_id: UserId) -> Result<(), DbError> {
    if let Some(db_path) = runtime_context_user_db_path(user_id)? {
        ensure_user_db_parent_dir_exists(&db_path)?;
        std::fs::copy(&*TEMPLATE_USER_DB, &db_path)
            .map_err(|e| DbError::new(format!("Failed to copy template user db: {e}")))?;
    }
    initialize_user_db(user_id, UserDbOpenMode::PlaintextTest)
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn initialize_user_db_for_test_with_auto_vacuum_mode(
    user_id: UserId,
    auto_vacuum_mode: SqliteAutoVacuumMode,
) -> Result<(), DbError> {
    let scoped_key = scoped_user_db_key(user_id);
    let db_path = runtime_context_user_db_path(user_id)?.ok_or_else(|| {
        DbError::new("Test user DB auto_vacuum helper requires an active runtime context")
    })?;
    ensure_user_db_parent_dir_exists(&db_path)?;

    let mut conn = rusqlite::Connection::open(&db_path).map_err(|err| {
        DbError::from_rusqlite_error(
            format!(
                "Failed to open test user database for seeded auto_vacuum mode at {}",
                db_path.display()
            ),
            err,
        )
    })?;
    configure_connection(
        &conn,
        &format!("seeded test user db write handle for user {user_id}"),
        false,
    );
    conn.pragma_update(None, "auto_vacuum", auto_vacuum_mode.pragma_value())
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to set seeded test user database auto_vacuum mode",
                err,
            )
        })?;
    migrations_runner()?
        .run(&mut conn)
        .map_err(|err| DbError::new(format!("Failed to run seeded user migrations: {err}")))?;
    ensure_required_user_schema(&conn)?;
    drop(conn);

    let mut guard = USER_CONNECTIONS
        .write()
        .map_err(|err| DbError::new(format!("Failed to acquire user db write lock: {err}")))?;
    guard.remove(&scoped_key);
    drop(guard);

    initialize_user_db(user_id, UserDbOpenMode::PlaintextTest)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::client_capabilities::{
        CapabilityId, ClientCapabilityRecord, ClientKeyVerifier, ClientPermission,
    };
    use crate::db::encryption::{
        DbEnvelope, UnlockAuthority, current_sqlcipher_compatibility, write_envelope,
    };
    use crate::db::settings::{load_settings, save_currency};
    use crate::db::user_data_repairs::{
        BITCOIN_HISTORY_FULL_RESYNC_REPAIR, UserDataRepairStatus, load_user_data_repair_status_conn,
    };
    use crate::models::CurrencyCode;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    fn spawn_with_current_runtime_context<F, T>(f: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        #[cfg(feature = "server")]
        let runtime_context = crate::runtime_context::current_runtime_context();

        std::thread::spawn(move || {
            #[cfg(feature = "server")]
            let _runtime_context_guard =
                runtime_context.map(crate::runtime_context::push_default_runtime_context);

            f()
        })
    }

    #[test]
    fn test_user_db_lifecycle() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");

        let user_id = UserId::new();

        let result: Result<(), DbError> = with_user_db(user_id, |_| Ok(()));
        assert!(result.is_err());

        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<i64, DbError> = with_user_db(user_id, |conn| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
                .map_err(|e| DbError::new(e.to_string()))?;
            Ok(count)
        });
        assert_eq!(result.unwrap(), 0);

        close_user_db(user_id).expect("Should close");

        let result: Result<(), DbError> = with_user_db(user_id, |_| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_users() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");

        let user1 = UserId::new();
        let user2 = UserId::new();

        initialize_user_db_for_test(user1).expect("Should initialize user 1");
        initialize_user_db_for_test(user2).expect("Should initialize user 2");

        let result1: Result<(), DbError> = with_user_db(user1, |_| Ok(()));
        let result2: Result<(), DbError> = with_user_db(user2, |_| Ok(()));
        assert!(result1.is_ok());
        assert!(result2.is_ok());

        close_user_db(user1).expect("Should close user 1");
        let result1: Result<(), DbError> = with_user_db(user1, |_| Ok(()));
        let result2: Result<(), DbError> = with_user_db(user2, |_| Ok(()));
        assert!(result1.is_err());
        assert!(result2.is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn reopening_keeps_names_for_inactive_durable_capabilities() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        crate::db::ensure_test_app_user(user_id);
        let (envelope, dek) =
            DbEnvelope::new_encrypted("SecurePass123").expect("envelope should create");
        write_envelope(user_id, &envelope).expect("envelope should persist");
        let open_mode = UserDbOpenMode::Encrypted {
            dek,
            authority: UnlockAuthority::PasswordLogin,
            sqlcipher_compatibility: envelope.sqlcipher_compatibility().unwrap(),
        };
        initialize_user_db(user_id, open_mode.clone()).expect("user database should initialize");
        let now = Utc::now();
        let revoked_id = CapabilityId::from_bytes([83_u8; 32]);
        let expired_id = CapabilityId::from_bytes([84_u8; 32]);
        for (capability_id, expires_at) in [
            (revoked_id, None),
            (expired_id, Some(now - chrono::Duration::minutes(1))),
        ] {
            crate::db::insert_active_client_capability(&ClientCapabilityRecord {
                capability_id,
                user_id,
                key_verifier: ClientKeyVerifier::from_raw_key(capability_id.as_bytes()),
                wrapped_dek: Some(vec![1, 2, 3]),
                wrap_nonce: Some(vec![4_u8; 12]),
                permission: ClientPermission::BalancesRead,
                created_at: now - chrono::Duration::minutes(2),
                expires_at,
                last_used_at: None,
                revoked_at: None,
            })
            .expect("capability should insert");
            crate::db::insert_paired_client_name(user_id, capability_id, "durable client")
                .expect("client name should insert");
        }
        crate::db::revoke_client_capability(user_id, revoked_id, now)
            .expect("capability should revoke");

        let reopened = open_user_connections(scoped_user_db_key(user_id), &open_mode)
            .expect("user database should reopen");
        drop(reopened);

        let names = crate::db::list_paired_client_names(user_id)
            .expect("paired-client names should list after reopen");
        assert!(names.contains_key(&revoked_id));
        assert!(names.contains_key(&expired_id));
    }

    #[cfg(all(feature = "server", feature = "dev-config"))]
    #[test]
    fn reopening_unencrypted_dev_removes_a_staged_name_without_a_durable_capability() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        crate::db::setup_unencrypted_dev_test_user(user_id);
        let orphan_id = CapabilityId::from_bytes([85_u8; 32]);
        crate::db::insert_paired_client_name(user_id, orphan_id, "staged orphan")
            .expect("orphan name should insert");

        let reopened =
            open_user_connections(scoped_user_db_key(user_id), &UserDbOpenMode::UnencryptedDev)
                .expect("unencrypted development database should reopen");
        drop(reopened);

        assert_eq!(
            crate::db::load_paired_client_name(user_id, orphan_id)
                .expect("orphan name lookup should succeed"),
            None
        );
    }

    #[test]
    fn test_required_fk_cascade_indexes_exist() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            let catalog = load_user_schema_catalog(conn)?;
            ensure_index_exists(&catalog, "idx_account_tx_ledger_chain_tx_id")?;
            Ok(())
        });

        assert!(result.is_ok());
    }

    #[test]
    fn test_account_integration_sync_state_schema_exists() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            let catalog = load_user_schema_catalog(conn)?;
            ensure_table_exists(&catalog, "account_integration_sync_state")?;
            ensure_index_exists(&catalog, "idx_account_integration_sync_state_updated_at")?;
            Ok(())
        });

        assert!(result.is_ok());
    }

    #[test]
    fn test_manual_asset_assertions_schema_includes_entered_balance_text() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            ensure_column_exists(
                conn,
                "manual_asset_balance_assertions",
                "entered_balance_text",
            )
        });

        assert!(result.is_ok());
    }

    #[test]
    fn current_schema_omits_legacy_manual_asset_storage() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            let catalog = load_user_schema_catalog(conn)?;
            for table in [
                "custom_asset_accounts",
                "custom_asset_balance_assertions",
                "custom_user_price_overrides",
            ] {
                assert!(!catalog.tables.contains(table), "unexpected table {table}");
            }
            for index in [
                "idx_caa_wallet",
                "idx_caa_unit_code",
                "idx_caa_label_key",
                "idx_caba_account_asserted_on",
                "idx_custom_user_price_overrides_lookup",
            ] {
                assert!(!catalog.indexes.contains(index), "unexpected index {index}");
            }
            Ok(())
        });

        assert!(result.is_ok());
    }

    #[test]
    fn initializing_v45_database_removes_legacy_manual_asset_rows() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        let db_path = runtime_context_user_db_path(user_id)
            .expect("runtime user db path should resolve")
            .expect("test runtime should use a file-backed user db");
        let mut conn = rusqlite::Connection::open(&db_path).expect("V45 fixture should open");
        migrations_runner()
            .expect("migration runner")
            .set_target(refinery::Target::Version(45))
            .run(&mut conn)
            .expect("migrations through V45 should apply");
        let now = "2026-07-14T12:00:00Z";
        conn.execute_batch(&format!(
            "INSERT INTO wallets
                 (id, label, label_key, identity_source, created_at, updated_at)
             VALUES ('legacy-wallet', 'Legacy Wallet', 'legacy wallet', 'user_provided', '{now}', '{now}');
             INSERT INTO custom_asset_accounts
                 (id, wallet_id, label, label_key, unit_code, display_scale, created_at, updated_at)
             VALUES ('legacy-account', 'legacy-wallet', 'Old Token', 'old token', 'OLD', 8, '{now}', '{now}');
             INSERT INTO custom_asset_balance_assertions
                 (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
                  entered_balance_text, note, created_at, updated_at)
             VALUES ('legacy-assertion', 'legacy-account', '2026-01-01', 0, 1,
                     '0.00000001', NULL, '{now}', '{now}');
             INSERT INTO custom_user_price_overrides
                 (id, subject_type, subject_id, quote_currency, price_time_utc, price,
                  source_note, created_at, updated_at)
             VALUES ('legacy-price', 'custom_unit_code', 'OLD', 'USD', '{now}', '1.25',
                     NULL, '{now}', '{now}');"
        ))
        .expect("V45 legacy rows should seed");
        drop(conn);

        initialize_user_db(user_id, UserDbOpenMode::PlaintextTest)
            .expect("V45 user database should initialize through V46");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            let catalog = load_user_schema_catalog(conn)?;
            assert!(!catalog.tables.contains("custom_asset_accounts"));
            assert!(!catalog.tables.contains("custom_asset_balance_assertions"));
            assert!(!catalog.tables.contains("custom_user_price_overrides"));
            let wallet_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))
                .map_err(|err| DbError::new(format!("wallet count failed: {err}")))?;
            assert_eq!(wallet_count, 1);
            Ok(())
        });
        result.expect("migrated user database should remain readable");
    }

    #[test]
    fn migrated_user_schema_accepts_manual_asset_snapshot_columns() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open db");
        migrations_runner()
            .expect("migration runner")
            .run(&mut conn)
            .expect("migrations apply");

        ensure_required_user_schema(&conn).expect("required schema should pass");

        let columns = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(manual_asset_accounts)")
                .expect("table info");
            stmt.query_map([], |row| row.get::<_, String>(1))
                .expect("query columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("columns")
        };
        assert!(!columns.contains(&"namespace_type".to_string()));
        assert!(!columns.contains(&"namespace_ref".to_string()));
        assert!(columns.contains(&"decimal_precision".to_string()));
        assert!(columns.contains(&"unit_code".to_string()));
        assert!(columns.contains(&"symbol".to_string()));
        assert!(columns.contains(&"asset_name".to_string()));
        assert!(columns.contains(&"network_name".to_string()));
        assert!(columns.contains(&"coingecko_id".to_string()));
    }

    #[test]
    fn test_phase3_raw_sync_index_review_keeps_supported_indexes_and_drops_unused_ones() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let result: Result<(), DbError> = with_user_db(user_id, |conn| {
            let catalog = load_user_schema_catalog(conn)?;

            for retained_index in [
                "idx_request_attempts_run_attempted",
                "idx_raw_parse_attempts_run_attempted",
                "idx_raw_mempool_tx_versions_txid_hash",
                "idx_raw_mempool_tx_versions_supersedes",
                "idx_raw_mempool_tx_obs_address_observed",
                "idx_raw_mempool_tx_obs_version_observed",
                "idx_raw_mempool_tx_obs_run_observed",
                "idx_raw_etherscan_normal_versions_identity_hash",
                "idx_raw_etherscan_normal_versions_supersedes",
                "idx_raw_etherscan_internal_versions_identity_hash",
                "idx_raw_etherscan_internal_versions_supersedes",
                "idx_raw_etherscan_normal_observations_request_order",
                "idx_raw_etherscan_internal_observations_request_order",
            ] {
                ensure_index_exists(&catalog, retained_index)?;
            }

            for removed_index in [
                "idx_request_attempts_scope_attempted",
                "idx_request_attempts_status_attempted",
                "idx_raw_parse_attempts_object_attempted",
                "idx_raw_parse_attempts_version_attempted",
                "idx_raw_etherscan_normal_versions_first_observed",
                "idx_raw_etherscan_internal_versions_first_observed",
                "idx_raw_etherscan_normal_observations_version_observed",
                "idx_raw_etherscan_internal_observations_version_observed",
            ] {
                assert!(
                    !catalog.indexes.contains(removed_index),
                    "expected removed index {removed_index} to be absent after migrations"
                );
            }

            Ok(())
        });

        assert!(result.is_ok());
    }

    #[test]
    fn test_phase3_drop_unused_raw_sync_indexes_migration_removes_targeted_indexes() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory user db should open");

        for migration_sql in [
            include_str!("../../migrations/user/V0__user_schema.sql"),
            include_str!("../../migrations/user/V7__raw_ingestion_sync_framework.sql"),
            include_str!("../../migrations/user/V8__mempool_raw_transaction_tables.sql"),
            include_str!("../../migrations/user/V9__etherscan_raw_transaction_tables.sql"),
        ] {
            conn.execute_batch(migration_sql)
                .expect("pre-phase3 user migrations should apply");
        }

        let catalog_before =
            load_user_schema_catalog(&conn).expect("catalog before V16 should load");
        for removed_index in [
            "idx_request_attempts_scope_attempted",
            "idx_request_attempts_status_attempted",
            "idx_raw_parse_attempts_object_attempted",
            "idx_raw_parse_attempts_version_attempted",
            "idx_raw_mempool_tx_obs_address_observed",
            "idx_raw_mempool_tx_obs_version_observed",
            "idx_raw_etherscan_normal_versions_first_observed",
            "idx_raw_etherscan_internal_versions_first_observed",
            "idx_raw_etherscan_normal_observations_version_observed",
            "idx_raw_etherscan_internal_observations_version_observed",
        ] {
            assert!(
                catalog_before.indexes.contains(removed_index),
                "expected {removed_index} to exist before V16 runs"
            );
        }

        conn.execute_batch(include_str!(
            "../../migrations/user/V16__drop_unused_raw_sync_indexes.sql"
        ))
        .expect("V16 should apply cleanly");

        let catalog_after = load_user_schema_catalog(&conn).expect("catalog after V16 should load");
        for retained_index in [
            "idx_request_attempts_run_attempted",
            "idx_raw_parse_attempts_run_attempted",
            "idx_raw_mempool_tx_versions_txid_created",
            "idx_raw_mempool_tx_obs_run_observed",
            "idx_raw_etherscan_normal_versions_identity_hash",
            "idx_raw_etherscan_internal_versions_identity_hash",
            "idx_raw_etherscan_normal_observations_request_order",
            "idx_raw_etherscan_internal_observations_request_order",
        ] {
            assert!(
                catalog_after.indexes.contains(retained_index),
                "expected retained index {retained_index} to survive V16"
            );
        }

        for removed_index in [
            "idx_request_attempts_scope_attempted",
            "idx_request_attempts_status_attempted",
            "idx_raw_parse_attempts_object_attempted",
            "idx_raw_parse_attempts_version_attempted",
            "idx_raw_mempool_tx_obs_address_observed",
            "idx_raw_mempool_tx_obs_version_observed",
            "idx_raw_etherscan_normal_versions_first_observed",
            "idx_raw_etherscan_internal_versions_first_observed",
            "idx_raw_etherscan_normal_observations_version_observed",
            "idx_raw_etherscan_internal_observations_version_observed",
        ] {
            assert!(
                !catalog_after.indexes.contains(removed_index),
                "expected {removed_index} to be dropped by V16"
            );
        }
    }

    fn apply_user_v0_schema(conn: &rusqlite::Connection) {
        conn.execute_batch(include_str!("../../migrations/user/V0__user_schema.sql"))
            .expect("V0 user schema should apply");
    }

    fn insert_legacy_etherscan_key(conn: &rusqlite::Connection, value: &str) {
        conn.execute(
            "INSERT INTO settings (settings_id, etherscan_api_key, updated_at)
             VALUES ('settings', ?1, '2026-05-24T00:00:00Z')",
            [value],
        )
        .expect("legacy setting insert should work");
    }

    fn table_column_exists(
        conn: &rusqlite::Connection,
        table_name: &str,
        column_name: &str,
    ) -> bool {
        let sql = format!(
            "SELECT EXISTS (
                SELECT 1
                FROM pragma_table_info('{table_name}')
                WHERE name = ?1
             )"
        );
        conn.query_row(&sql, [column_name], |row| row.get(0))
            .expect("column catalog query should work")
    }

    #[test]
    fn v37_migrates_existing_etherscan_api_key_to_api_keys() {
        let conn = rusqlite::Connection::open_in_memory().expect("open test db");
        apply_user_v0_schema(&conn);
        insert_legacy_etherscan_key(&conn, " LEGACY_ETHERSCAN_KEY ");

        conn.execute_batch(include_str!("../../migrations/user/V37__api_keys.sql"))
            .expect("V37 migration should apply");

        let stored: String = conn
            .query_row(
                "SELECT api_key FROM api_keys WHERE provider = 'etherscan'",
                [],
                |row| row.get(0),
            )
            .expect("migrated api key should exist");
        assert_eq!(stored, "LEGACY_ETHERSCAN_KEY");
    }

    #[test]
    fn v38_removes_legacy_etherscan_api_key_column_after_copy() {
        let conn = rusqlite::Connection::open_in_memory().expect("open test db");
        apply_user_v0_schema(&conn);
        insert_legacy_etherscan_key(&conn, " LEGACY_ETHERSCAN_KEY ");

        conn.execute_batch(include_str!("../../migrations/user/V37__api_keys.sql"))
            .expect("V37 migration should apply");
        conn.execute_batch(include_str!(
            "../../migrations/user/V38__drop_legacy_etherscan_api_key.sql"
        ))
        .expect("V38 migration should apply");

        let stored: String = conn
            .query_row(
                "SELECT api_key FROM api_keys WHERE provider = 'etherscan'",
                [],
                |row| row.get(0),
            )
            .expect("migrated api key should survive legacy column drop");
        assert_eq!(stored, "LEGACY_ETHERSCAN_KEY");
        assert!(
            !table_column_exists(&conn, "settings", "etherscan_api_key"),
            "V38 should remove the legacy Etherscan API key column"
        );
    }

    #[test]
    fn v37_ignores_blank_legacy_etherscan_api_key() {
        let conn = rusqlite::Connection::open_in_memory().expect("open test db");
        apply_user_v0_schema(&conn);
        insert_legacy_etherscan_key(&conn, "   ");

        conn.execute_batch(include_str!("../../migrations/user/V37__api_keys.sql"))
            .expect("V37 migration should apply");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_keys", [], |row| row.get(0))
            .expect("api key count should load");
        assert_eq!(count, 0);
    }

    fn apply_user_migrations_through_v20(conn: &rusqlite::Connection) {
        for migration_sql in [
            include_str!("../../migrations/user/V0__user_schema.sql"),
            include_str!("../../migrations/user/V1__account_transaction_ledger_paging_indexes.sql"),
            include_str!("../../migrations/user/V2__transaction_inputs_address_index.sql"),
            include_str!("../../migrations/user/V3__mempool_backfill_cursor.sql"),
            include_str!("../../migrations/user/V4__mempool_expected_tx_count.sql"),
            include_str!("../../migrations/user/V5__fk_cascade_supporting_indexes.sql"),
            include_str!("../../migrations/user/V6__drop_chain_prune_triggers.sql"),
            include_str!("../../migrations/user/V7__raw_ingestion_sync_framework.sql"),
            include_str!("../../migrations/user/V8__mempool_raw_transaction_tables.sql"),
            include_str!("../../migrations/user/V9__etherscan_raw_transaction_tables.sql"),
            include_str!("../../migrations/user/V10__etherscan_backfill_cursor.sql"),
            include_str!("../../migrations/user/V11__hd_account_chain_sync_state.sql"),
            include_str!("../../migrations/user/V12__account_integration_sync_state.sql"),
            include_str!("../../migrations/user/V13__custom_asset_accounts.sql"),
            include_str!("../../migrations/user/V14__custom_asset_entered_balance_text.sql"),
            include_str!("../../migrations/user/V15__raw_sync_history_retention_setting.sql"),
            include_str!("../../migrations/user/V16__drop_unused_raw_sync_indexes.sql"),
            include_str!("../../migrations/user/V17__mempool_head_rebuild_contract.sql"),
            include_str!("../../migrations/user/V18__etherscan_head_rebuild_contract.sql"),
            include_str!("../../migrations/user/V19__api_confirmed_balance.sql"),
            include_str!(
                "../../migrations/user/V20__rename_resulting_balance_to_closing_balance.sql"
            ),
        ] {
            conn.execute_batch(migration_sql)
                .expect("pre-V21 user migrations should apply");
        }
    }

    fn apply_user_migrations_through_v33(conn: &rusqlite::Connection) {
        apply_user_migrations_through_v20(conn);
        for migration_sql in [
            include_str!("../../migrations/user/V21__standardize_utxo_amount_hi_lo_storage.sql"),
            include_str!("../../migrations/user/V22__payment_state.sql"),
            include_str!("../../migrations/user/V23__pending_premium_transfers.sql"),
            include_str!("../../migrations/user/V24__payment_order_history.sql"),
            include_str!("../../migrations/user/V25__payment_order_basic_tier.sql"),
            include_str!("../../migrations/user/V26__payment_entitlement_cache.sql"),
            include_str!("../../migrations/user/V27__account_sync_slots.sql"),
            include_str!("../../migrations/user/V28__etherscan_recent_first_cursor.sql"),
            include_str!("../../migrations/user/V29__etherscan_history_status.sql"),
            include_str!("../../migrations/user/V30__transaction_sync_failure_count.sql"),
            include_str!("../../migrations/user/V31__drop_raw_sync_history_retention_setting.sql"),
            include_str!("../../migrations/user/V32__payment_refresh_sync_warning_status.sql"),
            include_str!("../../migrations/user/V33__payment_recovery_failed_status.sql"),
        ] {
            conn.execute_batch(migration_sql)
                .expect("pre-V34 user migrations should apply");
        }
    }

    #[test]
    fn test_v34_skips_malformed_legacy_token_backfill_rows() {
        for (case_name, token_id, active_token) in [
            ("short token id", "short", "legacy-token"),
            ("empty active token", "01JQABCDEF000000000000000F", ""),
        ] {
            let conn =
                rusqlite::Connection::open_in_memory().expect("in-memory user db should open");
            apply_user_migrations_through_v33(&conn);

            conn.execute(
                "INSERT INTO payment_subject \
                 (id, entitlement_holder_id, management_secret, active_token, token_id, \
                  subscription_subject_id, subscription_valid_until, token_expires_at, \
                  token_issued_at, last_refresh_at, last_refresh_status, updated_at, \
                  entitlement_tier, capability_set_id, capabilities_json, \
                  last_capability_refresh_at, last_successful_capability_refresh_at) \
                 VALUES ('premium', ?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?7, \
                         'premium', NULL, NULL, ?7, ?7)",
                rusqlite::params![
                    "01JQABCDEF000000000000000E",
                    active_token,
                    token_id,
                    "01JQABCDEF000000000000000G",
                    "2027-04-16T12:00:00Z",
                    "2026-04-23T12:00:00Z",
                    "2026-04-16T12:00:00Z",
                ],
            )
            .unwrap_or_else(|err| {
                panic!("{case_name}: legacy payment subject should insert: {err}")
            });

            conn.execute_batch(include_str!(
                "../../migrations/user/V34__payment_token_history.sql"
            ))
            .unwrap_or_else(|err| panic!("{case_name}: V34 should skip malformed row: {err}"));

            let history_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM payment_token_history", [], |row| {
                    row.get(0)
                })
                .unwrap_or_else(|err| panic!("{case_name}: history count should load: {err}"));
            assert_eq!(history_count, 0, "{case_name}");

            let pointer: Option<String> = conn
                .query_row(
                    "SELECT active_token_history_id FROM payment_subject WHERE id = 'premium'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or_else(|err| panic!("{case_name}: active pointer should load: {err}"));
            assert!(pointer.is_none(), "{case_name}");
        }
    }

    #[test]
    fn test_v21_standardizes_utxo_amount_storage() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory user db should open");
        apply_user_migrations_through_v20(&conn);

        conn.execute(
            "INSERT INTO wallets
             (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "wallet-1",
                "Wallet 1",
                "wallet-1",
                Option::<String>::None,
                "user_provided",
                Option::<String>::None,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("wallet fixture should insert");
        conn.execute(
            "INSERT INTO digital_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "account-1",
                "wallet-1",
                "Account 1",
                "account-1",
                "bitcoin",
                "mainnet",
                "single_address",
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("account fixture should insert");
        conn.execute(
            "INSERT INTO digital_asset_addresses
             (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                "addr-1",
                "account-1",
                "bitcoin",
                "mainnet",
                "bc1qexample",
                "bc1qexample",
                "native_segwit",
                Option::<i64>::None,
                Option::<i64>::None,
                "user_provided",
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("address fixture should insert");
        conn.execute(
            "INSERT INTO chain_transactions
             (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "chain-tx-1",
                "bitcoin",
                "mainnet",
                "tx-hash-1",
                "confirmed",
                Option::<i64>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<i64>::None,
                Option::<i64>::None,
                Option::<i64>::None,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("chain transaction fixture should insert");

        conn.execute(
            "INSERT INTO transaction_inputs
             (id, tx_id, input_index, prev_tx_hash, prev_output_index, address_id, value_amount, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "input-1",
                "chain-tx-1",
                0_i64,
                "prev-tx-1",
                0_i64,
                "addr-1",
                Option::<i64>::None,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("legacy input row should insert");
        conn.execute(
            "INSERT INTO transaction_outputs
             (id, tx_id, output_index, address_id, raw_address, script_pubkey_hex, value_amount, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "output-1",
                "chain-tx-1",
                0_i64,
                "addr-1",
                Option::<String>::None,
                "0014deadbeef",
                42_i64,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("legacy output row should insert");
        conn.execute(
            "INSERT INTO utxos
             (id, asset_id, network, tx_hash, output_index, address_id, value_amount, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "utxo-1",
                "bitcoin",
                "mainnet",
                "tx-hash-1",
                0_i64,
                "addr-1",
                43_i64,
                "confirmed",
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        )
        .expect("legacy utxo row should insert");

        conn.execute_batch(include_str!(
            "../../migrations/user/V21__standardize_utxo_amount_hi_lo_storage.sql"
        ))
        .expect("V21 should apply cleanly");

        let input_pair: (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT value_amount_hi, value_amount_lo
                 FROM transaction_inputs
                 WHERE id = 'input-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated input row should load");
        assert_eq!(input_pair, (None, None));

        let output_pair: (i64, i64) = conn
            .query_row(
                "SELECT value_amount_hi, value_amount_lo
                 FROM transaction_outputs
                 WHERE id = 'output-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated output row should load");
        assert_eq!(output_pair, (0, 42));

        let utxo_pair: (i64, i64) = conn
            .query_row(
                "SELECT value_amount_hi, value_amount_lo
                 FROM utxos
                 WHERE id = 'utxo-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated utxo row should load");
        assert_eq!(utxo_pair, (0, 43));

        let catalog = load_user_schema_catalog(&conn).expect("catalog after V21 should load");
        for index_name in [
            "idx_tx_inputs_prev",
            "idx_tx_inputs_address",
            "idx_tx_outputs_address",
            "idx_tx_outputs_raw_address",
            "idx_utxos_address_unspent_live",
            "idx_utxos_spent",
        ] {
            assert!(
                catalog.indexes.contains(index_name),
                "expected index {index_name} to exist after V21"
            );
        }

        let partial_pair_insert = conn.execute(
            "INSERT INTO transaction_inputs
             (id, tx_id, input_index, prev_tx_hash, prev_output_index, address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "input-2",
                "chain-tx-1",
                1_i64,
                "prev-tx-2",
                1_i64,
                "addr-1",
                0_i64,
                Option::<i64>::None,
                "2026-04-10T10:00:00Z",
                "2026-04-10T10:00:00Z",
            ],
        );
        assert!(
            partial_pair_insert.is_err(),
            "partial nullable split pairs should violate the V21 CHECK constraint"
        );
    }

    #[test]
    fn test_read_closure_keeps_one_snapshot_across_queries() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");
        with_user_db_mut(user_id, |conn| {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|err| DbError::new(err.to_string()))
        })
        .expect("snapshot test should use WAL like production");

        let (first_read_tx, first_read_rx) = mpsc::channel::<()>();
        let (write_completed_tx, write_completed_rx) = mpsc::channel::<()>();
        let read_user_id = user_id;
        let reader = spawn_with_current_runtime_context(move || {
            let result: Result<(i64, i64), DbError> = with_user_db(read_user_id, |conn| {
                let wallet_count = conn
                    .query_row("SELECT COUNT(*) FROM wallets", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|err| DbError::new(err.to_string()))?;
                first_read_tx
                    .send(())
                    .expect("first read should be reported");
                write_completed_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("writer should commit before the second read");
                let account_count = conn
                    .query_row("SELECT COUNT(*) FROM digital_asset_accounts", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|err| DbError::new(err.to_string()))?;
                Ok((wallet_count, account_count))
            });
            result
        });

        first_read_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reader should complete its first query");

        let wallet_id = crate::wallets::WalletId::new();
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        with_user_db_mut(user_id, |conn| {
            let tx = conn
                .transaction()
                .map_err(|err| DbError::new(err.to_string()))?;
            tx.execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, 'Snapshot Wallet', 'snapshot wallet', NULL, 'user_provided', NULL, ?2, ?2)",
                rusqlite::params![wallet_id.to_string(), "2026-07-26T00:00:00Z"],
            )
            .map_err(|err| DbError::new(err.to_string()))?;
            tx.execute(
                "INSERT INTO digital_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
                 VALUES (?1, ?2, 'Bitcoin Account 1', 'bitcoin account 1', 'bitcoin', 'mainnet', 'single_address', ?3, ?3)",
                rusqlite::params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    "2026-07-26T00:00:00Z"
                ],
            )
            .map_err(|err| DbError::new(err.to_string()))?;
            tx.commit().map_err(|err| DbError::new(err.to_string()))
        })
        .expect("writer should commit wallet and account together");
        write_completed_tx
            .send(())
            .expect("writer completion should be reported");

        let counts = reader
            .join()
            .expect("reader thread should join")
            .expect("reader closure should succeed");
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn test_read_connection_not_blocked_by_write_connection_mutex() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let (write_started_tx, write_started_rx) = mpsc::channel::<()>();
        let (release_write_tx, release_write_rx) = mpsc::channel::<()>();
        let (read_entered_tx, read_entered_rx) = mpsc::channel::<()>();

        let writer = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db_mut(user_id, |_| {
                write_started_tx
                    .send(())
                    .expect("writer should notify that lock is held");
                release_write_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("writer should be released by test");
                Ok(())
            });
            result
        });

        write_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer should start");

        let read_user_id = user_id;
        let reader = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db(read_user_id, |_| {
                read_entered_tx
                    .send(())
                    .expect("reader should report entry while writer still holds write lock");
                Ok(())
            });
            result
        });

        let lock_state = user_db_lock_state_for_test(user_id);
        assert_eq!(lock_state.read_locks, 0);
        assert_eq!(lock_state.write_locks, 1);
        read_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reader should not wait on the write connection mutex");

        release_write_tx
            .send(())
            .expect("test should release writer lock");

        reader
            .join()
            .expect("reader thread should join")
            .expect("reader closure should succeed");
        writer
            .join()
            .expect("writer thread should join")
            .expect("writer closure should succeed");

        assert!(user_db_lock_state_for_test(user_id).is_unlocked());
    }

    #[test]
    fn test_same_user_reads_can_use_different_read_handles() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let (read_started_tx, read_started_rx) = mpsc::channel::<()>();
        let (release_read_tx, release_read_rx) = mpsc::channel::<()>();
        let (second_read_entered_tx, second_read_entered_rx) = mpsc::channel::<()>();

        let holding_reader = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db(user_id, |_| {
                read_started_tx
                    .send(())
                    .expect("reader should notify that one read handle is held");
                release_read_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("reader should be released by test");
                Ok(())
            });
            result
        });

        read_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("holding reader should start");

        let lock_state = user_db_lock_state_for_test(user_id);
        assert_eq!(lock_state.read_locks, 1);
        assert_eq!(lock_state.write_locks, 0);

        let second_read_user_id = user_id;
        let second_reader = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db(second_read_user_id, |_| {
                second_read_entered_tx
                    .send(())
                    .expect("second reader should report entry while first read handle is held");
                Ok(())
            });
            result
        });

        second_read_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second reader should use a different read handle instead of waiting");

        release_read_tx
            .send(())
            .expect("test should release the holding reader");

        second_reader
            .join()
            .expect("second reader thread should join")
            .expect("second reader closure should succeed");
        holding_reader
            .join()
            .expect("holding reader thread should join")
            .expect("holding reader closure should succeed");

        assert!(user_db_lock_state_for_test(user_id).is_unlocked());
    }

    #[test]
    fn test_initialized_user_admission_not_blocked_by_other_user_db_operation() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let active_user_id = UserId::new();
        let admitted_user_id = UserId::new();
        initialize_user_db_for_test(active_user_id).expect("active user db should initialize");
        initialize_user_db_for_test(admitted_user_id).expect("admitted user db should initialize");

        let (operation_started_tx, operation_started_rx) = mpsc::channel::<()>();
        let (release_operation_tx, release_operation_rx) = mpsc::channel::<()>();
        let (admission_completed_tx, admission_completed_rx) =
            mpsc::channel::<Result<(), DbError>>();

        let active_reader = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db(active_user_id, |_| {
                operation_started_tx
                    .send(())
                    .expect("reader should notify when the user db operation is active");
                release_operation_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("reader should be released by the test");
                Ok(())
            });
            result
        });

        operation_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reader should start");

        let admission_thread = spawn_with_current_runtime_context(move || {
            let result = initialize_user_db_for_test(admitted_user_id);
            admission_completed_tx
                .send(result)
                .expect("admission result should be reported");
        });

        admission_completed_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("initialized-user admission should not wait on another user's db operation")
            .expect("initialized user admission should succeed");

        release_operation_tx
            .send(())
            .expect("test should release active db operation");

        admission_thread
            .join()
            .expect("admission thread should join");
        active_reader
            .join()
            .expect("reader thread should join")
            .expect("reader closure should succeed");
    }

    #[test]
    fn test_read_handle_is_read_only_and_rejects_writes() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let write_attempt = with_user_db(user_id, |conn| {
            conn.execute(
                "INSERT INTO settings (settings_id, updated_at) VALUES (?1, ?2)",
                rusqlite::params!["settings", "2026-03-07T00:00:00Z"],
            )
            .map(|_| ())
            .map_err(|err| {
                DbError::from_rusqlite_error("Read handle unexpectedly accepted a write", err)
            })
        });

        let write_error = write_attempt.expect_err("read handle should reject writes");
        assert!(
            write_error
                .to_string()
                .to_ascii_lowercase()
                .contains("readonly"),
            "read handle should reject writes structurally"
        );
    }

    #[test]
    fn test_same_user_id_is_isolated_across_parallel_test_runtimes() {
        let user_id = UserId::new();
        let start_barrier = Arc::new(Barrier::new(2));

        let dark_writer = std::thread::spawn({
            let start_barrier = Arc::clone(&start_barrier);
            move || {
                let _runtime =
                    crate::db::acquire_test_runtime().expect("eur runtime should initialize");
                initialize_user_db_for_test(user_id).expect("eur runtime should initialize user");
                start_barrier.wait();
                let eur = CurrencyCode::from_code("EUR").expect("EUR should parse");
                save_currency(user_id, eur).expect("eur runtime should persist currency");
                let settings = load_settings(user_id).expect("eur runtime should load settings");
                settings
                    .currency
                    .expect("eur runtime currency should be present")
            }
        });

        let light_writer = std::thread::spawn({
            let start_barrier = Arc::clone(&start_barrier);
            move || {
                let _runtime =
                    crate::db::acquire_test_runtime().expect("usd runtime should initialize");
                initialize_user_db_for_test(user_id).expect("usd runtime should initialize user");
                start_barrier.wait();
                let usd = CurrencyCode::from_code("USD").expect("USD should parse");
                save_currency(user_id, usd).expect("usd runtime should persist currency");
                let settings = load_settings(user_id).expect("usd runtime should load settings");
                settings
                    .currency
                    .expect("usd runtime currency should be present")
            }
        });

        let eur_currency = dark_writer.join().expect("eur runtime thread should join");
        let usd_currency = light_writer.join().expect("usd runtime thread should join");

        assert_eq!(eur_currency.code(), "EUR");
        assert_eq!(usd_currency.code(), "USD");
    }

    #[test]
    fn test_settings_writes_wait_for_write_handle_and_use_write_path() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("Should initialize");

        let (write_started_tx, write_started_rx) = mpsc::channel::<()>();
        let (release_write_tx, release_write_rx) = mpsc::channel::<()>();
        let (save_completed_tx, save_completed_rx) = mpsc::channel::<Result<(), DbError>>();

        let writer = spawn_with_current_runtime_context(move || {
            let result: Result<(), DbError> = with_user_db_mut(user_id, |_| {
                write_started_tx
                    .send(())
                    .expect("writer should notify that the write handle is held");
                release_write_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("writer should be released by the test");
                Ok(())
            });
            result
        });

        write_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer should start");

        let saver = spawn_with_current_runtime_context(move || {
            let eur = CurrencyCode::from_code("EUR").expect("EUR should parse");
            let result = save_currency(user_id, eur);
            save_completed_tx
                .send(result)
                .expect("save result should be reported");
        });

        let early_completion = save_completed_rx.recv_timeout(Duration::from_millis(250));
        assert!(
            early_completion.is_err(),
            "settings write should wait for the write handle instead of bypassing it"
        );

        release_write_tx
            .send(())
            .expect("test should release the write handle");

        let save_result = save_completed_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("settings write should complete after the write handle is released");
        save_result.expect("settings write should succeed through the write path");

        saver.join().expect("saver thread should join");
        writer
            .join()
            .expect("writer thread should join")
            .expect("writer closure should succeed");

        let saved_settings = load_settings(user_id).expect("settings should be readable");
        assert_eq!(
            saved_settings.currency.map(|c| c.code().to_string()),
            Some("EUR".to_string())
        );
    }

    #[test]
    fn test_new_user_db_enables_incremental_auto_vacuum() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");

        let auto_vacuum_mode = with_user_db(user_id, |conn| {
            load_auto_vacuum_mode(
                conn,
                "Failed to load auto_vacuum mode for new test user database",
            )
        })
        .expect("auto_vacuum mode should load");

        assert_eq!(auto_vacuum_mode, SqliteAutoVacuumMode::Incremental);
    }

    #[test]
    fn test_existing_user_db_auto_vacuum_mode_is_preserved() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test_with_auto_vacuum_mode(user_id, SqliteAutoVacuumMode::None)
            .expect("seeded user db should initialize");

        let auto_vacuum_mode = with_user_db(user_id, |conn| {
            load_auto_vacuum_mode(
                conn,
                "Failed to load auto_vacuum mode for seeded test user database",
            )
        })
        .expect("auto_vacuum mode should load");

        assert_eq!(auto_vacuum_mode, SqliteAutoVacuumMode::None);
    }

    #[test]
    fn v48_requires_legacy_etherscan_history_checkpoints_to_be_reverified() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory user db should open");
        conn.execute_batch(
            "CREATE TABLE transaction_sync_state (
                id TEXT PRIMARY KEY,
                etherscan_history_status TEXT
             );
             INSERT INTO transaction_sync_state (id, etherscan_history_status)
             VALUES ('continuous', 'continuous');",
        )
        .expect("legacy history checkpoints should seed");

        conn.execute_batch(include_str!(
            "../../migrations/user/V48__etherscan_history_checkpoint_version.sql"
        ))
        .expect("V48 should apply");

        let checkpoint = conn
            .query_row(
                "SELECT etherscan_history_status, etherscan_history_checkpoint_version
                 FROM transaction_sync_state
                 WHERE id = 'continuous'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                    ))
                },
            )
            .expect("checkpoint should load");
        assert_eq!(checkpoint, (Some("continuous".to_string()), None));
    }

    #[test]
    fn v49_bitcoin_history_invalidates_only_bitcoin_derived_state() {
        let mut conn =
            rusqlite::Connection::open_in_memory().expect("in-memory user db should open");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("foreign keys should enable");
        migrations_runner()
            .expect("migration runner")
            .set_target(refinery::Target::Version(48))
            .run(&mut conn)
            .expect("migrations through V48 should apply");

        let now = "2026-07-24T12:00:00Z";
        conn.execute_batch(&format!(
            "INSERT INTO wallets
                 (id, label, label_key, identity_source, created_at, updated_at)
             VALUES
                 ('wallet', 'Wallet', 'wallet', 'user_provided', '{now}', '{now}'),
                 ('manual-wallet', 'Manual', 'manual', 'user_provided', '{now}', '{now}');

             INSERT INTO digital_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
             VALUES
                 ('btc-single', 'wallet', 'Bitcoin', 'bitcoin', 'bitcoin', 'mainnet', 'single_address', '{now}', '{now}'),
                 ('btc-hd', 'wallet', 'Bitcoin HD', 'bitcoin hd', 'bitcoin', 'mainnet', 'hd_pubkey', '{now}', '{now}'),
                 ('eth-single', 'wallet', 'Ethereum', 'ethereum', 'ethereum', 'mainnet', 'single_address', '{now}', '{now}');

             INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme,
                  derivation_change, derivation_index, source_type, created_at, updated_at)
             VALUES
                 ('btc-address', 'btc-single', 'bitcoin', 'mainnet', 'btc-address', 'btc-address',
                  'native_segwit', NULL, NULL, 'user_provided', '{now}', '{now}'),
                 ('btc-hd-address', 'btc-hd', 'bitcoin', 'mainnet', 'btc-hd-address', 'btc-hd-address',
                  'native_segwit', 0, 0, 'derived', '{now}', '{now}'),
                 ('eth-address', 'eth-single', 'ethereum', 'mainnet', 'eth-address', 'eth-address',
                  'standard', NULL, NULL, 'user_provided', '{now}', '{now}');

             INSERT INTO source_connections
                 (id, integration, network, source_kind, normalized_source_key, status,
                  current_digital_asset_address_id, created_at, updated_at, activated_at, deactivated_at)
             VALUES
                 ('btc-source', 'mempool', 'mainnet', 'wallet_address_api_watch', 'btc-address',
                  'active', 'btc-address', '{now}', '{now}', '{now}', NULL),
                 ('eth-source', 'etherscan', 'mainnet', 'wallet_address_api_watch', 'eth-address',
                  'active', 'eth-address', '{now}', '{now}', '{now}', NULL);

             INSERT INTO sync_runs
                 (id, integration, scope_kind, scope_address_id, source_connection_id, asset_id,
                  network, trigger_kind, status, started_at, completed_at, summary_json, created_at, updated_at)
             VALUES
                 ('btc-run', 'mempool', 'address', 'btc-address', 'btc-source', 'bitcoin',
                  'mainnet', 'backfill', 'completed_success', '{now}', '{now}', NULL, '{now}', '{now}');

             INSERT INTO transaction_sync_state
                 (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result,
                  last_error, last_tip_height, new_tx_count, updated_tx_count,
                  mempool_backfill_cursor_txid, mempool_expected_tx_count,
                  api_confirmed_balance_hi, api_confirmed_balance_lo, created_at, updated_at)
             VALUES
                 ('btc-state', 'address', 'btc-address', 'btc-run', '{now}', '{now}', 'success',
                  NULL, 800000, 1, 0,
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 7,
                  3, 4, '{now}', '{now}'),
                 ('eth-state', 'address', 'eth-address', 'eth-run', '{now}', '{now}', 'success',
                  NULL, 19000000, 1, 0,
                  'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 8,
                  5, 6, '{now}', '{now}');

             INSERT INTO account_sync_state
                 (id, account_id, last_scanned_height, last_scanned_time, gap_limit,
                  last_derived_external_index, last_derived_internal_index, created_at, updated_at)
             VALUES
                 ('btc-hd-sync', 'btc-hd', 800000, '{now}', 20, 5, 4, '{now}', '{now}'),
                 ('eth-sync', 'eth-single', 19000000, '{now}', 0, NULL, NULL, '{now}', '{now}');

             INSERT INTO hd_account_chain_sync_state
                 (id, account_id, derivation_change, frontier_phase, next_index_to_scan,
                  consecutive_unused, active_rescan_from_index, created_at, updated_at)
             VALUES
                 ('btc-external', 'btc-hd', 0, 'derived_addresses', 6, 1, NULL, '{now}', '{now}'),
                 ('btc-internal', 'btc-hd', 1, 'active_rescan', 5, 2, 3, '{now}', '{now}');

             INSERT INTO chain_transactions
                 (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time,
                  fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
             VALUES
                 ('btc-tx', 'bitcoin', 'mainnet',
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                  'confirmed', 799999, 'btc-block', '{now}', 1, 0, NULL, '{now}', '{now}'),
                 ('eth-tx', 'ethereum', 'mainnet',
                  '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                  'confirmed', 18999999, 'eth-block', '{now}', 1, 0, 1, '{now}', '{now}');

             INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status,
                  occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type,
                  from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo,
                  fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo,
                  created_at, updated_at)
             VALUES
                 ('btc-ledger', 'btc-single', 'btc-tx', 'bitcoin', 'mainnet',
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                  'confirmed', '{now}', '{now}', 799999, NULL, NULL, 'receive', '[]', '[]',
                  0, 1, 0, 1, 11, 12, '{now}', '{now}'),
                 ('eth-ledger', 'eth-single', 'eth-tx', 'ethereum', 'mainnet',
                  '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                  'confirmed', '{now}', '{now}', 18999999, 1, 0, 'receive', '[]', '[]',
                  0, 1, 0, 1, 13, 14, '{now}', '{now}');

             INSERT INTO raw_mempool_transaction_versions
                 (id, source_connection_id, network, txid, payload_hash_sha256_hex, payload_bytes,
                  first_observed_at, supersedes_raw_version_id, created_at)
             VALUES
                 ('raw-btc', 'btc-source', 'mainnet',
                  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                  'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                  x'7b7d', '{now}', NULL, '{now}');

             INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
             VALUES
                 ('manual-account', 'manual-wallet', 'Solana', 'solana', 'solana',
                  'solana-mainnet', 9, 'SOL', NULL, 'Solana', 'Solana', 'solana',
                  'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, '{now}', '{now}');"
        ))
        .expect("V48 fixture should seed");

        let raw_version_count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM raw_mempool_transaction_versions",
                [],
                |row| row.get(0),
            )
            .expect("raw version count should load");

        conn.execute_batch(include_str!(
            "../../migrations/user/V49__bitcoin_history_coverage.sql"
        ))
        .expect("V49 should apply");

        let btc_address_state = conn
            .query_row(
                "SELECT mempool_backfill_cursor_txid, mempool_expected_tx_count,
                        api_confirmed_balance_hi, api_confirmed_balance_lo,
                        mempool_history_complete_tx_count, mempool_history_complete_height,
                        mempool_history_scan_start_run_id
                 FROM transaction_sync_state
                 WHERE id = 'btc-state'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .expect("bitcoin sync state should load");
        assert_eq!(
            btc_address_state,
            (None, None, Some(3), Some(4), None, None, None)
        );

        let single_account_sync_state_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM account_sync_state WHERE account_id = 'btc-single'",
                [],
                |row| row.get(0),
            )
            .expect("single-address account sync state count should load");
        assert_eq!(single_account_sync_state_count, 0);

        let hd_state = conn
            .query_row(
                "SELECT last_scanned_height, last_scanned_time, mempool_history_next_address_id
                 FROM account_sync_state WHERE account_id = 'btc-hd'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .expect("HD state should load");
        assert_eq!(hd_state, (None, None, None));

        let hd_frontiers = conn
            .prepare(
                "SELECT derivation_change, frontier_phase, next_index_to_scan, consecutive_unused
                 FROM hd_account_chain_sync_state
                 WHERE account_id = 'btc-hd'
                 ORDER BY derivation_change",
            )
            .expect("HD frontier query should prepare")
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .expect("HD frontiers should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("HD frontiers should load");
        assert_eq!(
            hd_frontiers,
            vec![
                (0, "existing_addresses".to_string(), 0, 0),
                (1, "existing_addresses".to_string(), 0, 0),
            ]
        );

        let repair_status =
            load_user_data_repair_status_conn(&conn, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)
                .expect("repair status should load");
        assert_eq!(repair_status, Some(UserDataRepairStatus::Pending));

        let bitcoin_closing_balance = conn
            .query_row(
                "SELECT closing_balance_hi, closing_balance_lo
                 FROM account_transaction_ledger WHERE id = 'btc-ledger'",
                [],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .expect("bitcoin closing balance should load");
        assert_eq!(bitcoin_closing_balance, (None, None));

        let expected_ethereum_balance = (Some(13_i64), Some(14_i64));
        let ethereum_closing_balance = conn
            .query_row(
                "SELECT closing_balance_hi, closing_balance_lo
                 FROM account_transaction_ledger WHERE id = 'eth-ledger'",
                [],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .expect("ethereum closing balance should load");
        assert_eq!(ethereum_closing_balance, expected_ethereum_balance);

        let expected_provider_balance = (Some(3_i64), Some(4_i64));
        let provider_balance = conn
            .query_row(
                "SELECT api_confirmed_balance_hi, api_confirmed_balance_lo
                 FROM transaction_sync_state WHERE id = 'btc-state'",
                [],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .expect("provider balance should load");
        assert_eq!(provider_balance, expected_provider_balance);

        let raw_version_count_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM raw_mempool_transaction_versions",
                [],
                |row| row.get(0),
            )
            .expect("raw version count should reload");
        assert_eq!(raw_version_count_after, raw_version_count_before);

        let manual_asset_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
                row.get(0)
            })
            .expect("manual asset count should load");
        assert_eq!(manual_asset_count, 1);

        for invalid_update in [
            "UPDATE transaction_sync_state
             SET mempool_history_complete_tx_count = 1
             WHERE id = 'btc-state'",
            "UPDATE transaction_sync_state
             SET mempool_history_complete_tx_count = -1,
                 mempool_history_complete_height = 1
             WHERE id = 'btc-state'",
            "UPDATE transaction_sync_state
             SET mempool_history_complete_tx_count = 1,
                 mempool_history_complete_height = -1
             WHERE id = 'btc-state'",
        ] {
            assert!(
                conn.execute_batch(invalid_update).is_err(),
                "invalid proof shape should be rejected"
            );
        }
    }

    #[test]
    fn encrypted_v48_database_opens_at_v50_without_losing_financial_data() {
        let runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        let db_path =
            user_database_path_from_project_dir(runtime.runtime_context().project_dir(), user_id);
        ensure_user_db_parent_dir_exists(&db_path).expect("user db directory should exist");
        let fixture_path = db_path.with_extension("v48-fixture");
        let (envelope, dek) = DbEnvelope::new_encrypted("representative-v48-password")
            .expect("envelope should create");
        let compatibility = envelope
            .sqlcipher_compatibility()
            .expect("encrypted envelope should expose compatibility");

        {
            let mut conn =
                rusqlite::Connection::open(&fixture_path).expect("V48 fixture should open");
            apply_encrypted_db_pragmas(&conn, &dek, &compatibility)
                .expect("V48 fixture should be encrypted");
            conn.pragma_update(None, "foreign_keys", "ON")
                .expect("foreign keys should enable");
            migrations_runner()
                .expect("migration runner")
                .set_target(refinery::Target::Version(48))
                .run(&mut conn)
                .expect("migrations through V48 should apply");

            let now = "2026-07-24T12:00:00Z";
            conn.execute_batch(&format!(
                "INSERT INTO wallets
                     (id, label, label_key, identity_source, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000001', 'Wallet', 'wallet', 'user_provided', '{now}', '{now}'),
                     ('01J00000000000000000000002', 'Manual', 'manual', 'user_provided', '{now}', '{now}');

                 INSERT INTO digital_asset_accounts
                     (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000003', '01J00000000000000000000001', 'Bitcoin', 'bitcoin', 'bitcoin', 'mainnet',
                      'single_address', '{now}', '{now}'),
                     ('01J00000000000000000000004', '01J00000000000000000000001', 'Ethereum', 'ethereum', 'ethereum', 'mainnet',
                      'single_address', '{now}', '{now}');

                 INSERT INTO digital_asset_addresses
                     (id, account_id, asset_id, network, address, address_normalized, address_scheme,
                      derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000005', '01J00000000000000000000003', 'bitcoin', 'mainnet', 'btc-address',
                      'btc-address', 'native_segwit', NULL, NULL, 'user_provided', '{now}', '{now}'),
                     ('01J00000000000000000000006', '01J00000000000000000000004', 'ethereum', 'mainnet', 'eth-address',
                      'eth-address', 'standard', NULL, NULL, 'user_provided', '{now}', '{now}');

                 INSERT INTO source_connections
                     (id, integration, network, source_kind, normalized_source_key, status,
                      current_digital_asset_address_id, created_at, updated_at, activated_at, deactivated_at)
                 VALUES
                     ('01J00000000000000000000007', 'mempool', 'mainnet', 'wallet_address_api_watch', 'btc-address',
                      'active', '01J00000000000000000000005', '{now}', '{now}', '{now}', NULL);

                 INSERT INTO sync_runs
                     (id, integration, scope_kind, scope_address_id, source_connection_id, asset_id,
                      network, trigger_kind, status, started_at, completed_at, summary_json, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000008', 'mempool', 'address', '01J00000000000000000000005', '01J00000000000000000000007', 'bitcoin',
                      'mainnet', 'backfill', 'completed_success', '{now}', '{now}', NULL, '{now}', '{now}');

                 INSERT INTO transaction_sync_state
                     (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result,
                      last_error, last_tip_height, new_tx_count, updated_tx_count,
                      api_confirmed_balance_hi, api_confirmed_balance_lo, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000009', 'address', '01J00000000000000000000005', '01J00000000000000000000008', '{now}', '{now}', 'success',
                      NULL, 800000, 1, 0, 3, 4, '{now}', '{now}');

                 INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time,
                      fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000010', 'bitcoin', 'mainnet',
                      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                      'confirmed', 799999, 'btc-block', '{now}', 1, 0, NULL, '{now}', '{now}'),
                     ('01J00000000000000000000011', 'ethereum', 'mainnet',
                      '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                      'confirmed', 18999999, 'eth-block', '{now}', 1, 0, 1, '{now}', '{now}');

                 INSERT INTO account_transaction_ledger
                     (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status,
                      occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type,
                      from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo,
                      fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo,
                      created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000012', '01J00000000000000000000003', '01J00000000000000000000010', 'bitcoin', 'mainnet',
                      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                      'confirmed', '{now}', '{now}', 799999, NULL, NULL, 'receive', '[]', '[]',
                      0, 1, 0, 1, 11, 12, '{now}', '{now}'),
                     ('01J00000000000000000000013', '01J00000000000000000000004', '01J00000000000000000000011', 'ethereum', 'mainnet',
                      '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                      'confirmed', '{now}', '{now}', 18999999, 1, 0, 'receive', '[]', '[]',
                      0, 1, 0, 1, 13, 14, '{now}', '{now}');

                 INSERT INTO manual_asset_accounts
                     (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                      unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                      precision_source, coingecko_platform_id, provider_platform_asset_ref,
                      created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000014', '01J00000000000000000000002', 'Solana', 'solana', 'solana',
                      'solana-mainnet', 9, 'SOL', NULL, 'Solana', 'Solana', 'solana',
                      'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, '{now}', '{now}');

                 INSERT INTO manual_asset_balance_assertions
                     (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
                      entered_balance_text, note, created_at, updated_at)
                 VALUES
                     ('01J00000000000000000000015', '01J00000000000000000000014', '2026-07-24', 0, 42,
                      '42', 'preserve me', '{now}', '{now}');"
            ))
            .expect("representative V48 data should seed");
        }

        std::fs::copy(&fixture_path, &db_path).expect("encrypted V48 fixture should copy");

        let canonical_before = (
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            799_999_i64,
        );
        let provider_balances_before = (Some(3_i64), Some(4_i64));
        let manual_assertions_before = (42_i64, Some("preserve me".to_string()));
        let ethereum_rows_before = (1_i64, 0_i64, 1_i64);

        let started_at = Instant::now();
        initialize_user_db(
            user_id,
            UserDbOpenMode::Encrypted {
                dek,
                authority: UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility: compatibility,
            },
        )
        .expect("encrypted V48 fixture should open through V50");
        let open_duration = started_at.elapsed();

        let (
            schema_version,
            repair_status,
            canonical_after,
            provider_balances_after,
            manual_assertions_after,
            ethereum_rows_after,
        ) = with_user_db_mut(user_id, |conn| -> Result<_, DbError> {
            let schema_version = migrations_runner()?
                .get_last_applied_migration(conn)
                .map_err(|err| DbError::new(format!("schema version should load: {err}")))?
                .map(|migration| migration.version())
                .unwrap_or_default();
            let canonical_after = conn
                .query_row(
                    "SELECT tx_hash, block_height FROM chain_transactions
                     WHERE id = '01J00000000000000000000010'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(|err| DbError::from_rusqlite_error("canonical row should load", err))?;
            let provider_balances_after = conn
                .query_row(
                    "SELECT api_confirmed_balance_hi, api_confirmed_balance_lo
                     FROM transaction_sync_state
                     WHERE id = '01J00000000000000000000009'",
                    [],
                    |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .map_err(|err| DbError::from_rusqlite_error("provider balance should load", err))?;
            let manual_assertions_after = conn
                .query_row(
                    "SELECT balance_amount_lo, note
                     FROM manual_asset_balance_assertions
                     WHERE id = '01J00000000000000000000015'",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .map_err(|err| DbError::from_rusqlite_error("manual assertion should load", err))?;
            let ethereum_rows_after = conn
                .query_row(
                    "SELECT COUNT(*), fee_amount_hi, fee_amount_lo
                     FROM chain_transactions
                     WHERE id = '01J00000000000000000000011'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map_err(|err| DbError::from_rusqlite_error("Ethereum row should load", err))?;
            let repair_status =
                load_user_data_repair_status_conn(conn, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)?;
            Ok((
                schema_version,
                repair_status,
                canonical_after,
                provider_balances_after,
                manual_assertions_after,
                ethereum_rows_after,
            ))
        })
        .expect("migrated financial data should load");

        eprintln!(
            "representative encrypted V48→V50 database open: {} ms",
            open_duration.as_millis()
        );
        assert_eq!(schema_version, 50);
        assert_eq!(repair_status, Some(UserDataRepairStatus::Pending));
        assert_eq!(canonical_before, canonical_after);
        assert_eq!(provider_balances_before, provider_balances_after);
        assert_eq!(manual_assertions_before, manual_assertions_after);
        assert_eq!(ethereum_rows_before, ethereum_rows_after);
        assert!(
            std::fs::metadata(db_path)
                .expect("database should exist")
                .len()
                > 0
        );
        assert_eq!(
            current_sqlcipher_compatibility()
                .expect("SQLCipher compatibility should load")
                .as_u32(),
            envelope
                .sqlcipher_compatibility()
                .expect("fixture compatibility should remain available")
                .as_u32()
        );
    }

    #[test]
    fn test_prepare_new_file_backed_user_db_enables_incremental_auto_vacuum_before_wal() {
        let db_path =
            std::env::temp_dir().join(format!("bitgarth-user-db-{}.sqlite3", UserId::new()));
        let _ = std::fs::remove_file(&db_path);
        let conn = rusqlite::Connection::open(&db_path).expect("file-backed user db should open");
        let user_id = UserId::new();

        prepare_new_user_db_for_incremental_vacuum(&conn, user_id)
            .expect("fresh file-backed user db should enable incremental auto_vacuum");
        configure_connection(&conn, "test file-backed user db", true);

        let auto_vacuum_mode = load_auto_vacuum_mode(
            &conn,
            "Failed to load auto_vacuum mode for fresh file-backed user database",
        )
        .expect("auto_vacuum mode should load");

        assert_eq!(auto_vacuum_mode, SqliteAutoVacuumMode::Incremental);

        drop(conn);
        let _ = std::fs::remove_file(&db_path);
    }
}

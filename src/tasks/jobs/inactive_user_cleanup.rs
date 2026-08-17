use crate::auth::{lifecycle, session};
use crate::db::{DbError, close_user_db, with_db, with_db_mut};
use crate::models::UserId;
use crate::project_paths::{get_project_dir, user_dir_from_project_dir};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dioxus::logger::tracing;
use rusqlite::params;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

const INACTIVE_DAYS: i64 = 180;
const PAID_GRACE_DAYS: i64 = 365;
const DEFAULT_BATCH_LIMIT: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InactiveUserCleanupInterval(Duration);

impl InactiveUserCleanupInterval {
    const fn from_hours(hours: u64) -> Self {
        Self(Duration::from_secs(hours * 60 * 60))
    }

    pub(crate) const fn as_duration(self) -> Duration {
        self.0
    }
}

pub(crate) const INACTIVE_USER_CLEANUP_INTERVAL: InactiveUserCleanupInterval =
    InactiveUserCleanupInterval::from_hours(24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InactiveUserCleanupParams {
    pub(crate) batch_limit: u32,
}

impl Default for InactiveUserCleanupParams {
    fn default() -> Self {
        Self {
            batch_limit: DEFAULT_BATCH_LIMIT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InactiveUserCleanupSummary {
    pub(crate) deleted_users: u64,
    pub(crate) skipped_after_recheck: u64,
}

#[derive(Debug)]
pub(crate) enum InactiveUserCleanupError {
    Db(DbError),
    Auth(session::AuthError),
    Lifecycle(lifecycle::UserLifecycleLockError),
    Io { source: io::Error },
}

impl std::fmt::Display for InactiveUserCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(err) => write!(f, "{err}"),
            Self::Auth(err) => write!(f, "{err}"),
            Self::Lifecycle(err) => write!(f, "{err}"),
            Self::Io { source } => write!(f, "failed to delete user directory: {source}"),
        }
    }
}

impl std::error::Error for InactiveUserCleanupError {}

impl From<DbError> for InactiveUserCleanupError {
    fn from(err: DbError) -> Self {
        Self::Db(err)
    }
}

impl From<session::AuthError> for InactiveUserCleanupError {
    fn from(err: session::AuthError) -> Self {
        Self::Auth(err)
    }
}

impl From<lifecycle::UserLifecycleLockError> for InactiveUserCleanupError {
    fn from(err: lifecycle::UserLifecycleLockError) -> Self {
        Self::Lifecycle(err)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeletionCandidate {
    user_id: UserId,
    activity_at: DateTime<Utc>,
}

pub(crate) fn run(
    params: InactiveUserCleanupParams,
) -> Result<InactiveUserCleanupSummary, InactiveUserCleanupError> {
    run_at(params, Utc::now())
}

fn parse_user_id(raw: String, column: usize) -> Result<UserId, rusqlite::Error> {
    UserId::from_str(&raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })
}

fn is_deletion_candidate_now(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<Option<DeletionCandidate>, DbError> {
    let inactive_cutoff = (now - ChronoDuration::days(INACTIVE_DAYS)).to_rfc3339();
    let paid_grace_cutoff = (now - ChronoDuration::days(PAID_GRACE_DAYS)).to_rfc3339();
    let user_id_raw = user_id.to_string();

    with_db(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT u.user_id, COALESCE(u.last_login_at, u.created_at) AS activity_at \
                 FROM users u \
                 WHERE u.user_id = ?1 \
                   AND COALESCE(u.last_login_at, u.created_at) < ?2 \
                   AND NOT EXISTS ( \
                       SELECT 1 \
                       FROM app_entitlement_snapshots s \
                       WHERE s.user_id = u.user_id \
                         AND s.entitlement_tier != 'free' \
                         AND (s.subscription_valid_until IS NULL OR s.subscription_valid_until >= ?3) \
                   )",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare inactive user recheck query", err)
            })?;

        match stmt.query_row(
            params![user_id_raw, inactive_cutoff, paid_grace_cutoff],
            |row| {
                Ok(DeletionCandidate {
                    user_id: parse_user_id(row.get::<_, String>(0)?, 0)?,
                    activity_at: parse_utc(row.get::<_, String>(1)?, 1)?,
                })
            },
        ) {
            Ok(candidate) => Ok(Some(candidate)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(DbError::from_rusqlite_error(
                "Failed to recheck inactive user candidate",
                err,
            )),
        }
    })
}

fn remove_dir_if_exists(path: &Path) -> Result<(), InactiveUserCleanupError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(InactiveUserCleanupError::Io { source: err }),
    }
}

fn remove_user_directory(
    project_dir: &Path,
    user_id: UserId,
) -> Result<(), InactiveUserCleanupError> {
    let user_dir = user_dir_from_project_dir(project_dir, user_id);
    remove_dir_if_exists(&user_dir)
}

fn delete_app_db_rows(user_id: UserId) -> Result<(), DbError> {
    with_db_mut(|conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::from_rusqlite_error("Failed to start inactive user delete transaction", err)
        })?;
        let user_id = user_id.to_string();

        tx.execute(
            "DELETE FROM app_entitlement_snapshots WHERE user_id = ?1",
            [&user_id],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to delete entitlement snapshots", err)
        })?;
        tx.execute(
            "DELETE FROM password_credentials \
             WHERE credential_id IN ( \
                 SELECT credential_id FROM auth_credentials WHERE user_id = ?1 \
             )",
            [&user_id],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to delete password credentials", err)
        })?;
        tx.execute(
            "DELETE FROM auth_credentials WHERE user_id = ?1",
            [&user_id],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to delete auth credentials", err))?;
        tx.execute(
            "DELETE FROM app_user_preferences WHERE user_id = ?1",
            [&user_id],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to delete app user preferences", err)
        })?;
        tx.execute(
            "DELETE FROM legal_acceptances WHERE user_id = ?1",
            [&user_id],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to delete legal acceptances", err))?;
        tx.execute("DELETE FROM sessions WHERE user_id = ?1", [&user_id])
            .map_err(|err| DbError::from_rusqlite_error("Failed to delete sessions", err))?;
        tx.execute("DELETE FROM users WHERE user_id = ?1", [&user_id])
            .map_err(|err| DbError::from_rusqlite_error("Failed to delete user", err))?;

        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error("Failed to commit inactive user delete transaction", err)
        })?;
        Ok(())
    })
}

fn user_has_live_session(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<bool, InactiveUserCleanupError> {
    Ok(session::list_users_with_unexpired_sessions_at(now)?
        .into_iter()
        .any(|live_user_id| live_user_id == user_id))
}

fn delete_candidate_user(
    project_dir: &Path,
    candidate: DeletionCandidate,
    now: DateTime<Utc>,
) -> Result<Option<DeletionCandidate>, InactiveUserCleanupError> {
    let guard = lifecycle::acquire_user_lifecycle_lock(candidate.user_id)?;
    let Some(rechecked) = is_deletion_candidate_now(candidate.user_id, now)? else {
        return Ok(None);
    };
    if user_has_live_session(candidate.user_id, now)? {
        return Ok(None);
    }

    guard.clear_browser_sessions()?;
    close_user_db(candidate.user_id)?;
    remove_user_directory(project_dir, candidate.user_id)?;
    delete_app_db_rows(candidate.user_id)?;
    Ok(Some(rechecked))
}

fn run_at(
    params: InactiveUserCleanupParams,
    now: DateTime<Utc>,
) -> Result<InactiveUserCleanupSummary, InactiveUserCleanupError> {
    let project_dir = get_project_dir()?;
    let candidates = list_deletion_candidates(now, params.batch_limit)?;

    let mut summary = InactiveUserCleanupSummary::default();
    for candidate in candidates {
        match delete_candidate_user(&project_dir, candidate, now)? {
            Some(deleted) => {
                summary.deleted_users += 1;
                tracing::info!(
                    user_id = %deleted.user_id,
                    activity_at = %deleted.activity_at,
                    "inactive user cleanup: deleted inactive hosted user"
                );
            }
            None => {
                summary.skipped_after_recheck += 1;
                tracing::debug!(
                    user_id = %candidate.user_id,
                    activity_at = %candidate.activity_at,
                    "inactive user cleanup: skipped candidate after locked recheck"
                );
            }
        }
    }

    tracing::info!(
        deleted_users = summary.deleted_users,
        skipped_after_recheck = summary.skipped_after_recheck,
        "inactive user cleanup: completed"
    );
    Ok(summary)
}

fn parse_utc(raw: String, column: usize) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
}

fn list_deletion_candidates(
    now: DateTime<Utc>,
    batch_limit: u32,
) -> Result<Vec<DeletionCandidate>, DbError> {
    let inactive_cutoff = (now - ChronoDuration::days(INACTIVE_DAYS)).to_rfc3339();
    let paid_grace_cutoff = (now - ChronoDuration::days(PAID_GRACE_DAYS)).to_rfc3339();
    let limit = i64::from(batch_limit);

    with_db(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT u.user_id, COALESCE(u.last_login_at, u.created_at) AS activity_at \
                 FROM users u \
                 WHERE COALESCE(u.last_login_at, u.created_at) < ?1 \
                   AND NOT EXISTS ( \
                       SELECT 1 \
                       FROM app_entitlement_snapshots s \
                       WHERE s.user_id = u.user_id \
                         AND s.entitlement_tier != 'free' \
                         AND (s.subscription_valid_until IS NULL OR s.subscription_valid_until >= ?2) \
                   ) \
                 ORDER BY activity_at ASC, u.user_id ASC \
                 LIMIT ?3",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare inactive user candidate query", err)
            })?;

        let rows = stmt
            .query_map(params![inactive_cutoff, paid_grace_cutoff, limit], |row| {
                Ok(DeletionCandidate {
                    user_id: parse_user_id(row.get::<_, String>(0)?, 0)?,
                    activity_at: parse_utc(row.get::<_, String>(1)?, 1)?,
                })
            })
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query inactive user candidates", err)
            })?;

        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(row.map_err(|err| {
                DbError::from_rusqlite_error("Failed to collect inactive user candidates", err)
            })?);
        }
        Ok(candidates)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::client_capabilities::{
        CapabilityId, ClientCapabilityRecord, ClientKeyVerifier, ClientPermission,
    };
    use crate::db::{acquire_test_runtime, unique_user_id, with_db, with_db_mut};
    use chrono::TimeZone;
    use rusqlite::params;
    use std::sync::{Arc, Barrier};

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0)
            .single()
            .expect("valid fixed timestamp")
    }

    fn insert_user(
        user_id: UserId,
        created_at: DateTime<Utc>,
        last_login_at: Option<DateTime<Utc>>,
    ) {
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO users (user_id, username, created_at, updated_at, last_login_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    user_id.to_string(),
                    format!("test_user_{user_id}"),
                    created_at.to_rfc3339(),
                    created_at.to_rfc3339(),
                    last_login_at.map(|value| value.to_rfc3339()),
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("insert user fixture failed", err))?;
            Ok::<_, DbError>(())
        })
        .expect("user fixture should insert");
    }

    fn insert_paid_snapshot(
        user_id: UserId,
        tier: &str,
        subscription_valid_until: Option<DateTime<Utc>>,
    ) {
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO app_entitlement_snapshots \
                 (snapshot_id, user_id, recorded_at, source, entitlement_holder_id, \
                  subscription_subject_id, token_id, entitlement_tier, subscription_valid_until) \
                 VALUES (?1, ?2, ?3, 'payment_poll', ?4, ?5, NULL, ?6, ?7)",
                params![
                    ulid::Ulid::new().to_string(),
                    user_id.to_string(),
                    fixed_now().to_rfc3339(),
                    ulid::Ulid::new().to_string(),
                    ulid::Ulid::new().to_string(),
                    tier,
                    subscription_valid_until.map(|value| value.to_rfc3339()),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("insert entitlement snapshot fixture failed", err)
            })?;
            Ok::<_, DbError>(())
        })
        .expect("snapshot fixture should insert");
    }

    fn candidate_ids(now: DateTime<Utc>) -> Vec<UserId> {
        list_deletion_candidates(now, 100)
            .expect("candidate query should succeed")
            .into_iter()
            .map(|candidate| candidate.user_id)
            .collect()
    }

    fn user_exists(user_id: UserId) -> bool {
        with_db(|conn| {
            let exists: i64 = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE user_id = ?1)",
                    [user_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|err| DbError::from_rusqlite_error("user exists query failed", err))?;
            Ok::<_, DbError>(exists != 0)
        })
        .expect("user query should succeed")
    }

    fn row_count(table: &str, user_id: UserId) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?1");
        with_db(|conn| {
            conn.query_row(&sql, [user_id.to_string()], |row| row.get(0))
                .map_err(|err| DbError::from_rusqlite_error("count query failed", err))
        })
        .expect("count query should succeed")
    }

    fn password_credential_count(credential_id: &str) -> i64 {
        with_db(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM password_credentials WHERE credential_id = ?1",
                [credential_id],
                |row| row.get(0),
            )
            .map_err(|err| DbError::from_rusqlite_error("password credential count failed", err))
        })
        .expect("password credential count should succeed")
    }

    fn insert_password_credential(user_id: UserId, now: DateTime<Utc>) -> String {
        let credential_id = ulid::Ulid::new().to_string();
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO auth_credentials \
                 (credential_id, user_id, auth_method, is_primary, created_at) \
                 VALUES (?1, ?2, 'password', 1, ?3)",
                params![credential_id, user_id.to_string(), now.to_rfc3339()],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("insert auth credential fixture failed", err)
            })?;
            conn.execute(
                "INSERT INTO password_credentials (credential_id, password_hash, salt) \
                 VALUES (?1, ?2, ?3)",
                params![credential_id, "password_hash", "salt"],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("insert password credential fixture failed", err)
            })?;
            Ok::<_, DbError>(())
        })
        .expect("password credential should insert");
        credential_id
    }

    fn insert_app_user_preferences(user_id: UserId, now: DateTime<Utc>) {
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO app_user_preferences \
                 (user_id, price_fetching_enabled, created_at, updated_at) \
                 VALUES (?1, 1, ?2, ?2)",
                params![user_id.to_string(), now.to_rfc3339()],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("insert app user preferences fixture failed", err)
            })?;
            Ok::<_, DbError>(())
        })
        .expect("app user preferences should insert");
    }

    fn disable_app_db_foreign_keys() {
        with_db_mut(|conn| {
            conn.pragma_update(None, "foreign_keys", "OFF")
                .map_err(|err| DbError::from_rusqlite_error("disable foreign keys failed", err))?;
            Ok::<_, DbError>(())
        })
        .expect("foreign keys should disable");
    }

    fn insert_expired_session(user_id: UserId, now: DateTime<Utc>) {
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO sessions \
                 (session_id, user_id, token_hash, created_at, expires_at, last_activity_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    ulid::Ulid::new().to_string(),
                    user_id.to_string(),
                    format!("token_hash_{user_id}"),
                    (now - ChronoDuration::days(200)).to_rfc3339(),
                    (now - ChronoDuration::days(199)).to_rfc3339(),
                    (now - ChronoDuration::days(199)).to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("insert session fixture failed", err))?;
            Ok::<_, DbError>(())
        })
        .expect("expired session should insert");
    }

    fn insert_legal_acceptance(user_id: UserId, now: DateTime<Utc>) {
        with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO legal_acceptances \
                 (legal_acceptance_id, user_id, document_kind, document_version, accepted_at, source, created_at) \
                 VALUES (?1, ?2, 'terms', '2026-06-25', ?3, 'registration', ?3)",
                params![
                    ulid::Ulid::new().to_string(),
                    user_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("insert legal acceptance fixture failed", err)
            })?;
            Ok::<_, DbError>(())
        })
        .expect("legal acceptance should insert");
    }

    #[test]
    fn candidate_selection_applies_activity_and_paid_grace_rules() {
        let _guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();

        let inactive_free = unique_user_id();
        insert_user(
            inactive_free,
            now - ChronoDuration::days(220),
            Some(now - ChronoDuration::days(181)),
        );

        let created_fallback = unique_user_id();
        insert_user(created_fallback, now - ChronoDuration::days(181), None);

        let recent_login = unique_user_id();
        insert_user(
            recent_login,
            now - ChronoDuration::days(400),
            Some(now - ChronoDuration::days(10)),
        );

        let active_paid = unique_user_id();
        insert_user(active_paid, now - ChronoDuration::days(220), None);
        insert_paid_snapshot(active_paid, "premium", Some(now + ChronoDuration::days(30)));

        let lapsed_inside_grace = unique_user_id();
        insert_user(lapsed_inside_grace, now - ChronoDuration::days(220), None);
        insert_paid_snapshot(
            lapsed_inside_grace,
            "basic",
            Some(now - ChronoDuration::days(100)),
        );

        let lapsed_after_grace = unique_user_id();
        insert_user(lapsed_after_grace, now - ChronoDuration::days(500), None);
        insert_paid_snapshot(
            lapsed_after_grace,
            "premium",
            Some(now - ChronoDuration::days(400)),
        );

        let unknown_paid_expiry = unique_user_id();
        insert_user(unknown_paid_expiry, now - ChronoDuration::days(500), None);
        insert_paid_snapshot(unknown_paid_expiry, "premium", None);

        let ids = candidate_ids(now);
        assert!(ids.contains(&inactive_free));
        assert!(ids.contains(&created_fallback));
        assert!(ids.contains(&lapsed_after_grace));
        assert!(!ids.contains(&recent_login));
        assert!(!ids.contains(&active_paid));
        assert!(!ids.contains(&lapsed_inside_grace));
        assert!(!ids.contains(&unknown_paid_expiry));
    }

    #[test]
    fn candidate_selection_respects_batch_limit_oldest_first() {
        let _guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let oldest = unique_user_id();
        let newest = unique_user_id();

        insert_user(oldest, now - ChronoDuration::days(300), None);
        insert_user(newest, now - ChronoDuration::days(250), None);

        let ids = list_deletion_candidates(now, 1)
            .expect("candidate query should succeed")
            .into_iter()
            .map(|candidate| candidate.user_id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![oldest]);
    }

    #[test]
    fn run_deletes_user_directory_and_app_rows_for_candidate() {
        let guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        insert_user(user_id, now - ChronoDuration::days(220), None);
        crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
        let credential_id = insert_password_credential(user_id, now);
        insert_app_user_preferences(user_id, now);
        insert_expired_session(user_id, now);
        insert_legal_acceptance(user_id, now);
        insert_paid_snapshot(user_id, "free", Some(now + ChronoDuration::days(30)));

        let project_dir = guard.runtime_context().project_dir().to_path_buf();
        let user_dir = user_dir_from_project_dir(&project_dir, user_id);
        let extra_file = user_dir.join("extra-hosted-user-file.txt");
        std::fs::write(&extra_file, b"extra").expect("extra file fixture");

        let summary =
            run_at(InactiveUserCleanupParams::default(), now).expect("cleanup should succeed");

        assert_eq!(summary.deleted_users, 1);
        assert_eq!(summary.skipped_after_recheck, 0);
        assert!(!user_dir.exists());
        assert!(!user_exists(user_id));
        assert_eq!(row_count("auth_credentials", user_id), 0);
        assert_eq!(password_credential_count(&credential_id), 0);
        assert_eq!(row_count("app_user_preferences", user_id), 0);
        assert_eq!(row_count("sessions", user_id), 0);
        assert_eq!(row_count("legal_acceptances", user_id), 0);
        assert_eq!(row_count("app_entitlement_snapshots", user_id), 0);
    }

    #[test]
    fn delete_app_db_rows_removes_auth_rows_and_preferences_without_fk_cascades() {
        let guard = acquire_test_runtime().expect("test runtime");
        drop(guard);
        let now = fixed_now();
        let user_id = unique_user_id();
        insert_user(user_id, now - ChronoDuration::days(220), None);
        let credential_id = insert_password_credential(user_id, now);
        insert_app_user_preferences(user_id, now);
        disable_app_db_foreign_keys();

        delete_app_db_rows(user_id).expect("app db rows should delete");

        assert!(!user_exists(user_id));
        assert_eq!(row_count("auth_credentials", user_id), 0);
        assert_eq!(password_credential_count(&credential_id), 0);
        assert_eq!(row_count("app_user_preferences", user_id), 0);
    }

    #[test]
    fn run_skips_candidate_with_live_session() {
        let _guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        insert_user(user_id, now - ChronoDuration::days(220), None);
        crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
        crate::auth::session::create_session_with_duration(
            user_id,
            crate::models::DEFAULT_SESSION_DURATION_MINUTES,
            &crate::db::encryption::SessionCreationContext::PlaintextTest,
        )
        .expect("live session should create");

        let summary =
            run_at(InactiveUserCleanupParams::default(), now).expect("cleanup should succeed");

        assert_eq!(summary.deleted_users, 0);
        assert_eq!(summary.skipped_after_recheck, 1);
        assert!(user_exists(user_id));
    }

    #[test]
    fn run_completes_partial_delete_when_user_directory_is_missing() {
        let _guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        insert_user(user_id, now - ChronoDuration::days(220), None);
        insert_legal_acceptance(user_id, now);

        let summary =
            run_at(InactiveUserCleanupParams::default(), now).expect("cleanup should succeed");

        assert_eq!(summary.deleted_users, 1);
        assert!(!user_exists(user_id));
        assert_eq!(row_count("legal_acceptances", user_id), 0);
    }

    #[test]
    fn run_resolves_project_dir_without_runtime_context() {
        let guard = acquire_test_runtime().expect("test runtime");
        let project_dir = guard.runtime_context().project_dir().to_path_buf();
        let now = fixed_now();

        // Hosted scheduler threads carry no runtime context, so the project dir
        // must resolve through the same path chain every other job uses.
        let result = std::thread::spawn(move || {
            let _project_dir_guard = crate::project_paths::push_project_dir_override(project_dir)
                .expect("project dir override");
            run_at(InactiveUserCleanupParams::default(), now)
        })
        .join()
        .expect("cleanup thread should not panic");

        result.expect("cleanup should run without a runtime context");
    }

    #[test]
    fn delete_candidate_skips_candidate_that_becomes_active_before_locked_recheck() {
        let guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        let old_activity_at = now - ChronoDuration::days(220);
        insert_user(user_id, old_activity_at, None);
        let stale_candidate = DeletionCandidate {
            user_id,
            activity_at: old_activity_at,
        };

        with_db_mut(|conn| {
            conn.execute(
                "UPDATE users SET last_login_at = ?1 WHERE user_id = ?2",
                params![
                    (now - ChronoDuration::days(1)).to_rfc3339(),
                    user_id.to_string(),
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("update activity fixture failed", err))?;
            Ok::<_, DbError>(())
        })
        .expect("activity update should succeed");

        let project_dir = guard.runtime_context().project_dir().to_path_buf();
        let deleted = delete_candidate_user(&project_dir, stale_candidate, now)
            .expect("cleanup should succeed");

        assert_eq!(deleted, None);
        assert!(user_exists(user_id));
    }

    #[test]
    fn client_key_activity_prevents_inactive_user_deletion() {
        let _guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        insert_user(user_id, now - ChronoDuration::days(220), None);
        let capability_id = CapabilityId::from_bytes([50_u8; 32]);
        crate::db::insert_active_client_capability(&ClientCapabilityRecord {
            capability_id,
            user_id,
            key_verifier: ClientKeyVerifier::from_raw_key(&[50_u8; 32]),
            wrapped_dek: Some(vec![1_u8; 48]),
            wrap_nonce: Some(vec![2_u8; 12]),
            permission: ClientPermission::BalancesRead,
            created_at: now - ChronoDuration::days(1),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        })
        .expect("capability fixture should insert");
        crate::db::record_client_capability_activity(capability_id, user_id, now)
            .expect("Client Key activity should update");

        let summary =
            run_at(InactiveUserCleanupParams::default(), now).expect("cleanup should succeed");

        assert_eq!(summary.deleted_users, 0);
        assert!(user_exists(user_id));
    }

    #[test]
    fn inactive_deletion_waits_for_client_key_request() {
        let guard = acquire_test_runtime().expect("test runtime");
        let now = fixed_now();
        let user_id = unique_user_id();
        let activity_at = now - ChronoDuration::days(220);
        insert_user(user_id, activity_at, None);
        crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
        let project_dir = guard.runtime_context().project_dir().to_path_buf();
        let user_dir = user_dir_from_project_dir(&project_dir, user_id);
        let candidate = DeletionCandidate {
            user_id,
            activity_at,
        };
        let request_acquired = Arc::new(Barrier::new(2));
        let release_request = Arc::new(Barrier::new(2));
        let request_context = guard.runtime_context();
        let worker_acquired = Arc::clone(&request_acquired);
        let worker_release = Arc::clone(&release_request);
        let request_worker = std::thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(request_context);
            let lease = lifecycle::acquire_client_key_request(
                user_id,
                CapabilityId::from_bytes([51_u8; 32]),
            )
            .expect("request registry should lock")
            .expect("Client Key request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(lease);
        });
        request_acquired.wait();

        let cleanup_context = guard.runtime_context();
        let cleanup_worker = std::thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(cleanup_context);
            delete_candidate_user(&project_dir, candidate, now)
        });
        lifecycle::wait_until_user_exclusive_for_test(user_id)
            .expect("cleanup should block new requests before waiting");
        assert!(user_exists(user_id));
        assert!(user_dir.exists());

        release_request.wait();
        request_worker
            .join()
            .expect("Client Key request should finish");
        let deleted = cleanup_worker
            .join()
            .expect("cleanup thread should finish")
            .expect("cleanup should succeed");
        assert_eq!(deleted, Some(candidate));
        assert!(!user_exists(user_id));
        assert!(!user_dir.exists());
    }
}

use crate::auth::lifecycle;
use crate::db::encryption::{SessionCreationContext, SessionWrapper};
use crate::db::{DbInitError, with_db_mut};
#[cfg(all(test, feature = "db-tests"))]
use crate::models::DEFAULT_SESSION_DURATION_MINUTES;
use crate::models::{Session, SessionId, SessionToken, TokenHash, UserId, parse_datetime};
use base64::{Engine, engine::general_purpose};
use chrono::{DateTime, Duration, Utc};
use dioxus::logger::tracing;
use rand::{Rng, thread_rng};
use std::str::FromStr;

const DEFAULT_IDLE_TIMEOUT_MINUTES: i64 = 60;
const DEFAULT_ABSOLUTE_TIMEOUT_MINUTES: i64 = 1440;
const ENV_IDLE_TIMEOUT: &str = "BITGARTH_SESSION_IDLE_TIMEOUT_MINUTES";
const ENV_ABSOLUTE_TIMEOUT: &str = "BITGARTH_SESSION_ABSOLUTE_TIMEOUT_MINUTES";

#[derive(Debug, Clone)]
pub(crate) struct SessionTimeoutPolicy {
    pub idle_timeout: Duration,
    pub absolute_timeout: Duration,
}

impl SessionTimeoutPolicy {
    pub(crate) fn resolve() -> Self {
        let idle_timeout = parse_env_minutes(ENV_IDLE_TIMEOUT, DEFAULT_IDLE_TIMEOUT_MINUTES);
        let absolute_timeout =
            parse_env_minutes(ENV_ABSOLUTE_TIMEOUT, DEFAULT_ABSOLUTE_TIMEOUT_MINUTES);
        Self {
            idle_timeout,
            absolute_timeout,
        }
    }
}

fn parse_env_minutes(var: &str, default_minutes: i64) -> Duration {
    parse_minutes_from(std::env::var(var).ok().as_deref(), default_minutes)
}

fn parse_minutes_from(val: Option<&str>, default_minutes: i64) -> Duration {
    match val {
        Some(raw) => match raw.parse::<i64>() {
            Ok(m) if m > 0 => Duration::minutes(m),
            Ok(_) => {
                tracing::warn!("session timeout env var must be a positive integer, using default");
                Duration::minutes(default_minutes)
            }
            Err(_) => {
                tracing::warn!("session timeout env var is not a valid integer, using default");
                Duration::minutes(default_minutes)
            }
        },
        None => Duration::minutes(default_minutes),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod policy_tests {
    use super::*;

    #[test]
    fn parse_minutes_from_uses_default_when_unset() {
        let dur = parse_minutes_from(None, 60);
        assert_eq!(dur, Duration::minutes(60));
    }

    #[test]
    fn parse_minutes_from_accepts_positive_integer() {
        let dur = parse_minutes_from(Some("120"), 60);
        assert_eq!(dur, Duration::minutes(120));
    }

    #[test]
    fn parse_minutes_from_rejects_zero() {
        let dur = parse_minutes_from(Some("0"), 60);
        assert_eq!(dur, Duration::minutes(60));
    }

    #[test]
    fn parse_minutes_from_rejects_negative() {
        let dur = parse_minutes_from(Some("-5"), 60);
        assert_eq!(dur, Duration::minutes(60));
    }

    #[test]
    fn parse_minutes_from_rejects_non_integer() {
        let dur = parse_minutes_from(Some("abc"), 60);
        assert_eq!(dur, Duration::minutes(60));
    }
}

#[derive(Debug)]
pub(crate) enum AuthError {
    Database(rusqlite::Error),
    DbInit(DbInitError),
    DateTimeParse(String),
    Encryption(String),
    Lifecycle(lifecycle::UserLifecycleLockError),
}

impl From<rusqlite::Error> for AuthError {
    fn from(e: rusqlite::Error) -> Self {
        AuthError::Database(e)
    }
}

impl From<DbInitError> for AuthError {
    fn from(e: DbInitError) -> Self {
        AuthError::DbInit(e)
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Database(e) => write!(f, "Database error: {}", e),
            AuthError::DbInit(e) => write!(f, "Database initialization error: {}", e),
            AuthError::DateTimeParse(e) => write!(f, "DateTime parse error: {}", e),
            AuthError::Encryption(e) => write!(f, "Encryption error: {}", e),
            AuthError::Lifecycle(e) => write!(f, "Lifecycle error: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<lifecycle::UserLifecycleLockError> for AuthError {
    fn from(error: lifecycle::UserLifecycleLockError) -> Self {
        Self::Lifecycle(error)
    }
}

pub(crate) const SESSION_COOKIE_NAME: &str = "bitgarth_session";

pub(crate) fn generate_session_token() -> SessionToken {
    let mut token = [0u8; 32];
    thread_rng().fill(&mut token);
    SessionToken::from_raw(general_purpose::STANDARD.encode(token))
}

fn hash_token(token: &SessionToken) -> TokenHash {
    TokenHash::from_token(token)
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLookupResult {
    pub session_id: SessionId,
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub wrapped_dek_nonce: Option<String>,
    pub wrapped_dek_ciphertext: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionInvalidationSummary {
    pub deleted_sessions: u64,
    pub closed_user_db: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExpiredSessionCleanupSummary {
    pub removed_sessions: u64,
    pub closed_user_db_connections: u64,
}

pub(crate) fn create_session_with_duration(
    user_id: UserId,
    duration_minutes: u32,
    context: &SessionCreationContext,
) -> Result<Session, AuthError> {
    tracing::debug!(
        user_id = %user_id,
        duration_minutes,
        "auth session: creating session with explicit duration"
    );
    let token = generate_session_token();
    let session_id = SessionId::new();
    let now = Utc::now();
    let expires_at = now + Duration::minutes(i64::from(duration_minutes));
    let token_hash = hash_token(&token);

    let (wrapped_nonce, wrapped_ciphertext) = match context {
        SessionCreationContext::Encrypted { dek, server_secret } => {
            let wrapper = SessionWrapper::wrap(
                dek,
                server_secret.as_bytes(),
                token.as_str(),
                session_id,
                user_id,
            )
            .map_err(|e| AuthError::Encryption(e.to_string()))?;
            (
                Some(wrapper.nonce_base64()),
                Some(wrapper.ciphertext_base64()),
            )
        }
        #[cfg(all(test, feature = "db-tests"))]
        SessionCreationContext::PlaintextTest => (None, None),
        #[cfg(feature = "dev-config")]
        SessionCreationContext::UnencryptedDev => (None, None),
    };

    let session = with_db_mut(|conn| -> Result<Session, AuthError> {
        conn.execute(
            "INSERT INTO sessions (session_id, user_id, token_hash, created_at, expires_at, last_activity_at, wrapped_dek_nonce, wrapped_dek_ciphertext) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                session_id.to_string(),
                user_id.to_string(),
                token_hash.as_str(),
                now.to_rfc3339(),
                expires_at.to_rfc3339(),
                now.to_rfc3339(),
                wrapped_nonce,
                wrapped_ciphertext,
            ],
        )?;

        let session = Session {
            session_id,
            user_id,
            token,
            created_at: now,
            expires_at,
        };

        tracing::debug!(
            user_id = %session.user_id,
            session_id = %session.session_id,
            expires_at = %session.expires_at,
            "auth session: session persisted"
        );

        Ok(session)
    })?;
    lifecycle::pin_browser_session(user_id, session_id)?;
    Ok(session)
}

fn invalidate_session_with_conn(
    conn: &mut rusqlite::Connection,
    session_id: SessionId,
) -> Result<u64, AuthError> {
    let deleted_sessions = conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        [session_id.to_string()],
    )? as u64;
    Ok(deleted_sessions)
}

pub(crate) fn invalidate_session(
    session_id: SessionId,
    user_id: UserId,
) -> Result<SessionInvalidationSummary, AuthError> {
    let deleted_sessions = with_db_mut(|conn| invalidate_session_with_conn(conn, session_id))?;
    if deleted_sessions == 0 {
        return Ok(SessionInvalidationSummary::default());
    }
    let closed_user_db = lifecycle::unpin_browser_session(user_id, session_id)?;
    Ok(SessionInvalidationSummary {
        deleted_sessions,
        closed_user_db,
    })
}

pub(crate) fn get_session_by_token(
    token: &SessionToken,
) -> Result<Option<SessionLookupResult>, AuthError> {
    enum LookupOutcome {
        Active(SessionLookupResult),
        Invalidated {
            session_id: SessionId,
            user_id: UserId,
        },
        Missing,
    }

    let now = Utc::now();
    let token_hash = hash_token(token);
    let policy = SessionTimeoutPolicy::resolve();

    let outcome = with_db_mut(|conn| -> Result<LookupOutcome, AuthError> {
        let result = {
            let mut stmt = conn.prepare(
                "SELECT session_id, user_id, created_at, expires_at, last_activity_at, wrapped_dek_nonce, wrapped_dek_ciphertext \
                 FROM sessions \
                 WHERE token_hash = ?1",
            )?;

            stmt.query_row([token_hash.as_str()], |row| {
                let session_id_str: String = row.get(0)?;
                let user_id_str: String = row.get(1)?;
                let created_at_str: String = row.get(2)?;
                let expires_at_str: String = row.get(3)?;
                let last_activity_at_str: Option<String> = row.get(4)?;
                let wrapped_dek_nonce: Option<String> = row.get(5)?;
                let wrapped_dek_ciphertext: Option<String> = row.get(6)?;

                let session_id = SessionId::from_str(&session_id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let user_id = UserId::from_str(&user_id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let created_at = parse_datetime(&created_at_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let expires_at = parse_datetime(&expires_at_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let last_activity_at = match last_activity_at_str {
                    Some(s) => parse_datetime(&s).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    None => created_at,
                };

                Ok(SessionLookupResult {
                    session_id,
                    user_id,
                    created_at,
                    expires_at,
                    last_activity_at,
                    wrapped_dek_nonce,
                    wrapped_dek_ciphertext,
                })
            })
        };

        match result {
            Ok(session) => {
                if session.expires_at <= now {
                    tracing::debug!(
                        user_id = %session.user_id,
                        session_id = %session.session_id,
                        expires_at = %session.expires_at,
                        "auth session: session expired (absolute), deleting"
                    );
                    let deleted_sessions = invalidate_session_with_conn(conn, session.session_id)?;
                    tracing::debug!(
                        user_id = %session.user_id,
                        session_id = %session.session_id,
                        deleted_sessions,
                        "auth session: expired session invalidated"
                    );
                    return Ok(LookupOutcome::Invalidated {
                        session_id: session.session_id,
                        user_id: session.user_id,
                    });
                }

                if session.last_activity_at + policy.idle_timeout <= now {
                    tracing::debug!(
                        user_id = %session.user_id,
                        session_id = %session.session_id,
                        last_activity_at = %session.last_activity_at,
                        "auth session: session expired (idle), deleting"
                    );
                    let deleted_sessions = invalidate_session_with_conn(conn, session.session_id)?;
                    tracing::debug!(
                        user_id = %session.user_id,
                        session_id = %session.session_id,
                        deleted_sessions,
                        "auth session: idle-expired session invalidated"
                    );
                    return Ok(LookupOutcome::Invalidated {
                        session_id: session.session_id,
                        user_id: session.user_id,
                    });
                }

                conn.execute(
                    "UPDATE sessions SET last_activity_at = ?1 WHERE session_id = ?2",
                    rusqlite::params![now.to_rfc3339(), session.session_id.to_string()],
                )?;

                tracing::debug!(
                    user_id = %session.user_id,
                    session_id = %session.session_id,
                    expires_at = %session.expires_at,
                    "auth session: session valid"
                );
                Ok(LookupOutcome::Active(session))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                tracing::debug!("auth session: session token not found");
                Ok(LookupOutcome::Missing)
            }
            Err(e) => Err(AuthError::Database(e)),
        }
    })?;

    match outcome {
        LookupOutcome::Active(session) => Ok(Some(session)),
        LookupOutcome::Invalidated {
            session_id,
            user_id,
        } => {
            lifecycle::unpin_browser_session(user_id, session_id)?;
            Ok(None)
        }
        LookupOutcome::Missing => Ok(None),
    }
}

pub(crate) fn session_exists(session_id: SessionId, user_id: UserId) -> Result<bool, AuthError> {
    with_db_mut(|conn| {
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sessions WHERE session_id = ?1 AND user_id = ?2
             )",
            rusqlite::params![session_id.to_string(), user_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    })
}

pub(crate) fn delete_session(token: &SessionToken) -> Result<(), AuthError> {
    let token_hash = hash_token(token);
    tracing::debug!(
        token_hash = %token_hash,
        "auth session: deleting session by token hash"
    );
    let invalidated = with_db_mut(|conn| {
        let result = conn.query_row(
            "SELECT session_id, user_id FROM sessions WHERE token_hash = ?1",
            [token_hash.as_str()],
            |row| {
                let session_id_str: String = row.get(0)?;
                let user_id_str: String = row.get(1)?;
                let session_id = SessionId::from_str(&session_id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let user_id = UserId::from_str(&user_id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok((session_id, user_id))
            },
        );

        match result {
            Ok((session_id, user_id)) => {
                let deleted_sessions = invalidate_session_with_conn(conn, session_id)?;
                Ok(Some((session_id, user_id, deleted_sessions)))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AuthError::Database(e)),
        }
    })?;

    if let Some((session_id, user_id, deleted_sessions)) = invalidated {
        let closed_user_db = if deleted_sessions > 0 {
            lifecycle::unpin_browser_session(user_id, session_id)?
        } else {
            false
        };
        tracing::debug!(
            session_id = %session_id,
            user_id = %user_id,
            deleted_sessions,
            closed_user_db,
            "auth session: session invalidated by token"
        );
    }
    Ok(())
}

fn parse_user_id_column(user_id_str: String, column: usize) -> Result<UserId, rusqlite::Error> {
    UserId::from_str(&user_id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn delete_expired_sessions_before(
    now: chrono::DateTime<Utc>,
) -> Result<Vec<(SessionId, UserId)>, AuthError> {
    let policy = SessionTimeoutPolicy::resolve();
    let now_rfc3339 = now.to_rfc3339();
    let idle_cutoff = (now - policy.idle_timeout).to_rfc3339();
    with_db_mut(|conn| {
        let transaction = conn.transaction()?;
        let expired_sessions = {
            let mut stmt = transaction.prepare(
                "SELECT session_id, user_id \
                 FROM sessions \
                 WHERE expires_at <= ?1 \
                    OR COALESCE(last_activity_at, created_at) <= ?2",
            )?;
            stmt.query_map(rusqlite::params![now_rfc3339, idle_cutoff], |row| {
                let session_id =
                    SessionId::from_str(&row.get::<_, String>(0)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                let user_id = parse_user_id_column(row.get::<_, String>(1)?, 1)?;
                Ok((session_id, user_id))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        transaction.execute(
            "DELETE FROM sessions \
             WHERE expires_at <= ?1 \
                OR COALESCE(last_activity_at, created_at) <= ?2",
            rusqlite::params![now_rfc3339, idle_cutoff],
        )?;
        transaction.commit()?;
        Ok(expired_sessions)
    })
}

pub(crate) fn list_users_with_unexpired_sessions_at(
    now: chrono::DateTime<Utc>,
) -> Result<Vec<UserId>, AuthError> {
    let policy = SessionTimeoutPolicy::resolve();
    let now_rfc3339 = now.to_rfc3339();
    let idle_cutoff = (now - policy.idle_timeout).to_rfc3339();
    with_db_mut(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT user_id \
             FROM sessions \
             WHERE expires_at > ?1 \
               AND COALESCE(last_activity_at, created_at) > ?2",
        )?;

        let users = stmt
            .query_map(rusqlite::params![now_rfc3339, idle_cutoff], |row| {
                let user_id_str: String = row.get(0)?;
                parse_user_id_column(user_id_str, 0)
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(users)
    })
}

pub(crate) fn cleanup_expired_sessions_before(
    now: chrono::DateTime<Utc>,
) -> Result<ExpiredSessionCleanupSummary, AuthError> {
    let expired_sessions = delete_expired_sessions_before(now)?;
    let removed_sessions = expired_sessions.len() as u64;

    let mut closed_user_db_connections = 0_u64;
    for (session_id, user_id) in expired_sessions {
        if lifecycle::unpin_browser_session(user_id, session_id)? {
            closed_user_db_connections += 1;
        }
    }

    tracing::debug!(
        removed_sessions,
        closed_user_db_connections,
        "auth session: cleaned up expired sessions"
    );

    Ok(ExpiredSessionCleanupSummary {
        removed_sessions,
        closed_user_db_connections,
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::client_capabilities::CapabilityId;
    use crate::db::{
        acquire_test_runtime, list_open_user_db_users, setup_test_user, unique_user_id, with_db_mut,
    };
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn set_last_activity_at(token: &SessionToken, last_activity_at: DateTime<Utc>) {
        let token_hash = hash_token(token);
        with_db_mut(|conn| {
            conn.execute(
                "UPDATE sessions SET last_activity_at = ?1 WHERE token_hash = ?2",
                rusqlite::params![last_activity_at.to_rfc3339(), token_hash.as_str()],
            )?;
            Ok::<_, AuthError>(())
        })
        .expect("last activity update should succeed");
    }

    #[test]
    fn expired_session_closes_user_db_when_it_was_last_session() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let token =
            create_session_with_duration(user_id, 0, &SessionCreationContext::PlaintextTest)
                .expect("session should be created")
                .token;

        assert!(
            list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );

        let result = get_session_by_token(&token).expect("lookup should succeed");
        assert!(result.is_none(), "expired session should not validate");
        assert!(
            !list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );
    }

    #[test]
    fn expired_session_keeps_user_db_open_when_another_session_exists() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let expired_token =
            create_session_with_duration(user_id, 0, &SessionCreationContext::PlaintextTest)
                .expect("expired session should be created")
                .token;

        let live_token = create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("live session should be created")
        .token;

        let expired_result = get_session_by_token(&expired_token).expect("lookup should succeed");
        assert!(
            expired_result.is_none(),
            "expired session should not validate"
        );
        assert!(
            list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );

        let live_result = get_session_by_token(&live_token).expect("live lookup should succeed");
        assert!(live_result.is_some(), "live session should remain valid");
    }

    #[test]
    fn idle_expired_session_is_not_listed_as_unexpired() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let token = create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("session should be created")
        .token;
        let now = Utc::now();
        set_last_activity_at(&token, now - Duration::days(365));

        let users = list_users_with_unexpired_sessions_at(now).expect("list should succeed");

        assert!(
            !users.contains(&user_id),
            "idle-expired session should not count as unexpired"
        );
    }

    #[test]
    fn cleanup_removes_idle_expired_sessions_and_closes_user_db() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let token = create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("session should be created")
        .token;
        let now = Utc::now();
        set_last_activity_at(&token, now - Duration::days(365));

        assert!(
            list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );

        let summary = cleanup_expired_sessions_before(now).expect("cleanup should succeed");

        assert_eq!(summary.removed_sessions, 1);
        assert_eq!(summary.closed_user_db_connections, 1);
        assert!(
            !list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );
    }

    #[test]
    fn delete_session_closes_user_db_when_it_was_last_session() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let token = create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("session should be created")
        .token;

        delete_session(&token).expect("delete should succeed");

        assert!(
            !list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );
    }

    #[test]
    fn logout_defers_close_until_client_key_request_finishes() {
        let runtime = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let token = create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("session should be created")
        .token;
        let acquired = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_context = runtime.runtime_context();
        let worker_acquired = Arc::clone(&acquired);
        let worker_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(worker_context);
            let lease = lifecycle::acquire_client_key_request(
                user_id,
                CapabilityId::from_bytes([41_u8; 32]),
            )
            .expect("request registry should lock")
            .expect("Client Key request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(lease);
        });

        acquired.wait();
        delete_session(&token).expect("logout session delete should succeed");
        assert!(
            list_open_user_db_users()
                .expect("open user databases should list")
                .contains(&user_id),
            "logout must not close the database under an active Client Key request"
        );
        release.wait();
        worker.join().expect("Client Key request should finish");
        assert!(
            !list_open_user_db_users()
                .expect("open user databases should list after request")
                .contains(&user_id)
        );
    }

    #[test]
    fn session_expiry_defers_close_until_client_key_request_finishes() {
        let runtime = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let token =
            create_session_with_duration(user_id, 0, &SessionCreationContext::PlaintextTest)
                .expect("expired session should be created")
                .token;
        let acquired = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_context = runtime.runtime_context();
        let worker_acquired = Arc::clone(&acquired);
        let worker_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(worker_context);
            let lease = lifecycle::acquire_client_key_request(
                user_id,
                CapabilityId::from_bytes([42_u8; 32]),
            )
            .expect("request registry should lock")
            .expect("Client Key request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(lease);
        });

        acquired.wait();
        assert!(
            get_session_by_token(&token)
                .expect("expired session lookup should succeed")
                .is_none()
        );
        assert!(
            list_open_user_db_users()
                .expect("open user databases should list")
                .contains(&user_id),
            "expiry must not close the database under an active Client Key request"
        );
        release.wait();
        worker.join().expect("Client Key request should finish");
        assert!(
            !list_open_user_db_users()
                .expect("open user databases should list after request")
                .contains(&user_id)
        );
    }
}

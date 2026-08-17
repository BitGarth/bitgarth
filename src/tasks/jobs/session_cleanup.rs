use crate::auth::session;
use chrono::Utc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionCleanupInterval(Duration);

impl SessionCleanupInterval {
    const fn from_minutes(minutes: u64) -> Self {
        Self(Duration::from_secs(minutes * 60))
    }

    pub(crate) const fn as_duration(self) -> Duration {
        self.0
    }
}

pub(crate) const SESSION_CLEANUP_INTERVAL: SessionCleanupInterval =
    SessionCleanupInterval::from_minutes(30);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionCleanupParams;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionCleanupSummary {
    pub(crate) removed_sessions: u64,
    pub(crate) closed_user_db_connections: u64,
    pub(crate) expired_client_capabilities: u64,
}

#[derive(Debug)]
pub(crate) enum SessionCleanupError {
    Auth(session::AuthError),
    Database(crate::db::DbError),
    Capability(crate::auth::lifecycle::ClientCapabilityShutdownError),
}

impl std::fmt::Display for SessionCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionCleanupError::Auth(err) => write!(f, "{err}"),
            SessionCleanupError::Database(err) => write!(f, "{err}"),
            SessionCleanupError::Capability(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SessionCleanupError {}

impl From<session::AuthError> for SessionCleanupError {
    fn from(err: session::AuthError) -> Self {
        SessionCleanupError::Auth(err)
    }
}

impl From<crate::db::DbError> for SessionCleanupError {
    fn from(err: crate::db::DbError) -> Self {
        Self::Database(err)
    }
}

impl From<crate::auth::lifecycle::ClientCapabilityShutdownError> for SessionCleanupError {
    fn from(err: crate::auth::lifecycle::ClientCapabilityShutdownError) -> Self {
        Self::Capability(err)
    }
}

pub(crate) fn run(
    _params: SessionCleanupParams,
) -> Result<SessionCleanupSummary, SessionCleanupError> {
    let now = Utc::now();
    let expired_capabilities = crate::db::list_expired_client_capabilities(now)?;
    let mut expired_client_capabilities = 0_u64;
    for capability in expired_capabilities {
        if crate::auth::lifecycle::shutdown_expired_client_capability(
            capability.user_id,
            capability.capability_id,
            now,
        )? {
            expired_client_capabilities = expired_client_capabilities.saturating_add(1);
        }
    }
    let summary = session::cleanup_expired_sessions_before(now)?;

    Ok(SessionCleanupSummary {
        removed_sessions: summary.removed_sessions,
        closed_user_db_connections: summary.closed_user_db_connections,
        expired_client_capabilities,
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::client_capabilities::{
        CapabilityId, ClientCapabilityRecord, ClientKeyVerifier, ClientPermission,
    };
    use crate::db::encryption::SessionCreationContext;
    use crate::db::{
        acquire_test_runtime, list_open_user_db_users, setup_test_user, unique_user_id,
    };
    use crate::models::DEFAULT_SESSION_DURATION_MINUTES;
    use std::sync::{Arc, Barrier};

    #[test]
    fn cleanup_expired_sessions_closes_only_users_without_live_sessions() {
        let _guard = acquire_test_runtime().expect("should acquire test runtime");
        let expired_only_user = unique_user_id();
        let mixed_user = unique_user_id();

        setup_test_user(expired_only_user);
        setup_test_user(mixed_user);

        session::create_session_with_duration(
            expired_only_user,
            0,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("expired-only session should be created");

        session::create_session_with_duration(
            mixed_user,
            0,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("mixed expired session should be created");
        session::create_session_with_duration(
            mixed_user,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("mixed live session should be created");

        // NOTE: assert only on per-user outcomes scoped to this test's unique users.
        // `removed_sessions` / `closed_user_db_connections` are app-wide counts that other
        // tests running in parallel can inflate (expired sessions cleaned globally), so exact
        // counts would be flaky.
        run(SessionCleanupParams).expect("cleanup should succeed");

        let open_users = list_open_user_db_users().expect("should list open user dbs");
        assert!(!open_users.contains(&expired_only_user));
        assert!(open_users.contains(&mixed_user));
    }

    #[test]
    fn cleanup_defers_close_until_client_key_request_finishes() {
        let runtime = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        session::create_session_with_duration(user_id, 0, &SessionCreationContext::PlaintextTest)
            .expect("expired session should be created");
        let request_acquired = Arc::new(Barrier::new(2));
        let release_request = Arc::new(Barrier::new(2));
        let request_context = runtime.runtime_context();
        let worker_acquired = Arc::clone(&request_acquired);
        let worker_release = Arc::clone(&release_request);
        let worker = std::thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(request_context);
            let lease = crate::auth::lifecycle::acquire_client_key_request(
                user_id,
                CapabilityId::from_bytes([61_u8; 32]),
            )
            .expect("request registry should lock")
            .expect("Client Key request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(lease);
        });

        request_acquired.wait();
        run(SessionCleanupParams).expect("session cleanup should succeed");
        assert!(
            list_open_user_db_users()
                .expect("open user databases should list")
                .contains(&user_id)
        );
        release_request.wait();
        worker.join().expect("Client Key request should finish");
        assert!(
            !list_open_user_db_users()
                .expect("open user databases should list after request")
                .contains(&user_id)
        );
    }

    #[test]
    fn cleanup_blocks_expired_capability_and_waits_for_its_request() {
        let runtime = acquire_test_runtime().expect("should acquire test runtime");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = Utc::now();
        let capability_id = CapabilityId::from_bytes([62_u8; 32]);
        crate::db::insert_active_client_capability(&ClientCapabilityRecord {
            capability_id,
            user_id,
            key_verifier: ClientKeyVerifier::from_raw_key(&[62_u8; 32]),
            wrapped_dek: Some(vec![1_u8; 48]),
            wrap_nonce: Some(vec![2_u8; 12]),
            permission: ClientPermission::BalancesRead,
            created_at: now - chrono::Duration::minutes(2),
            expires_at: Some(now - chrono::Duration::minutes(1)),
            last_used_at: None,
            revoked_at: None,
        })
        .expect("expired capability fixture should insert");

        let request_acquired = Arc::new(Barrier::new(2));
        let release_request = Arc::new(Barrier::new(2));
        let request_context = runtime.runtime_context();
        let cleanup_context = request_context.clone();
        let worker_acquired = Arc::clone(&request_acquired);
        let worker_release = Arc::clone(&release_request);
        let request_worker = std::thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(request_context);
            let lease = crate::auth::lifecycle::acquire_client_key_request(user_id, capability_id)
                .expect("request registry should lock")
                .expect("Client Key request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(lease);
        });
        request_acquired.wait();

        let cleanup_worker = std::thread::spawn(move || {
            let _context = crate::runtime_context::push_default_runtime_context(cleanup_context);
            run(SessionCleanupParams)
        });
        crate::auth::lifecycle::wait_until_capability_blocked_for_test(user_id, capability_id)
            .expect("cleanup should block capability");
        release_request.wait();
        request_worker.join().expect("request should finish");
        let summary = cleanup_worker
            .join()
            .expect("cleanup thread should finish")
            .expect("cleanup should succeed");
        assert_eq!(summary.expired_client_capabilities, 1);
        let record = crate::db::load_client_capability(capability_id)
            .expect("expired capability should load")
            .expect("expired capability should remain for audit");
        assert!(record.wrapped_dek.is_none());
        assert!(record.wrap_nonce.is_none());
    }
}

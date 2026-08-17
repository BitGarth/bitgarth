#[cfg(feature = "server")]
use super::session_token::lookup_session_token;
#[cfg(feature = "server")]
use crate::auth::lifecycle::{self, UserRequestLease};
#[cfg(feature = "server")]
use crate::auth::session;
#[cfg(feature = "server")]
use crate::db::encryption::{
    SessionWrapper, UnlockAuthority, UserDbOpenMode, read_envelope, resolve_server_master_secret,
};
#[cfg(feature = "server")]
use crate::models::{Session, SessionId, SessionToken, UserId};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use dioxus::logger::tracing;

#[cfg(feature = "server")]
const INVALID_OR_EXPIRED_SESSION: &str = "Invalid or expired session";

#[cfg(feature = "server")]
pub(crate) struct InitializedSession {
    pub(crate) session: Session,
    _request_lease: UserRequestLease,
}

#[cfg(feature = "server")]
pub(crate) fn require_session_token<E>(
    caller_name: &str,
    cookies: &CookieJar,
    unauthorized_error: impl FnOnce(String) -> E,
) -> Result<SessionToken, E> {
    lookup_session_token(caller_name, cookies)
        .ok_or_else(|| unauthorized_error(INVALID_OR_EXPIRED_SESSION.to_string()))
}

#[cfg(feature = "server")]
fn invalidate_failed_session_best_effort(
    context: &'static str,
    session_id: SessionId,
    user_id: UserId,
) {
    if let Err(error) = session::invalidate_session(session_id, user_id) {
        tracing::error!(
            session_id = %session_id,
            user_id = %user_id,
            error = %error,
            "{context}: failed to invalidate unrecoverable session"
        );
    }
}

#[cfg(feature = "server")]
pub(crate) fn require_initialized_session<E>(
    context: &'static str,
    session_token: &SessionToken,
    unauthorized_error: impl Fn(String) -> E,
    internal_error: impl Fn(String) -> E,
) -> Result<InitializedSession, E> {
    let lookup_result = match session::get_session_by_token(session_token)
        .map_err(|err| internal_error(err.to_string()))?
    {
        Some(lookup_result) => lookup_result,
        None => {
            tracing::debug!("{context}: session validation failed (invalid or expired)");
            return Err(unauthorized_error(INVALID_OR_EXPIRED_SESSION.to_string()));
        }
    };

    tracing::debug!(
        user_id = %lookup_result.user_id,
        session_id = %lookup_result.session_id,
        expires_at = %lookup_result.expires_at,
        context,
        "session validated"
    );

    let request_lease = lifecycle::acquire_session_request(lookup_result.user_id)
        .map_err(|error| internal_error(error.to_string()))?
        .ok_or_else(|| unauthorized_error(INVALID_OR_EXPIRED_SESSION.to_string()))?;

    let open_mode = match (
        &lookup_result.wrapped_dek_nonce,
        &lookup_result.wrapped_dek_ciphertext,
    ) {
        (Some(nonce), Some(ciphertext)) => {
            let envelope = read_envelope(lookup_result.user_id).map_err(|err| {
                internal_error(format!(
                    "Failed to read user envelope for session restore: {err}"
                ))
            })?;
            let sqlcipher_compatibility = envelope.sqlcipher_compatibility().ok_or_else(|| {
                internal_error(
                    "Missing encrypted envelope SQLCipher compatibility for session restore"
                        .to_string(),
                )
            })?;
            let server_secret = resolve_server_master_secret()
                .map_err(|e| internal_error(format!("Failed to resolve server secret: {e}")))?;

            let wrapper = SessionWrapper::from_base64(nonce, ciphertext)
                .map_err(|e| internal_error(format!("Failed to parse session wrapper: {e}")))?;

            let dek = wrapper
                .unwrap(
                    server_secret.as_bytes(),
                    session_token.as_str(),
                    lookup_result.session_id,
                    lookup_result.user_id,
                )
                .map_err(|e| {
                    tracing::warn!(
                        user_id = %lookup_result.user_id,
                        session_id = %lookup_result.session_id,
                        error = %e,
                        "{context}: session wrapper unwrap failed, invalidating session"
                    );
                    invalidate_failed_session_best_effort(
                        context,
                        lookup_result.session_id,
                        lookup_result.user_id,
                    );
                    unauthorized_error(INVALID_OR_EXPIRED_SESSION.to_string())
                })?;

            UserDbOpenMode::Encrypted {
                dek,
                authority: UnlockAuthority::SessionRestore {
                    session_id: lookup_result.session_id,
                },
                sqlcipher_compatibility,
            }
        }
        (None, None) => {
            #[cfg(all(test, feature = "db-tests"))]
            {
                UserDbOpenMode::PlaintextTest
            }
            #[cfg(all(not(test), feature = "dev-config"))]
            {
                UserDbOpenMode::UnencryptedDev
            }
            #[cfg(all(not(test), not(feature = "dev-config")))]
            {
                tracing::error!(
                    user_id = %lookup_result.user_id,
                    session_id = %lookup_result.session_id,
                    "{context}: session row has NULL wrapper fields in production build"
                );
                return Err(internal_error(
                    "Session row has NULL wrapper fields in production build".to_string(),
                ));
            }
            #[cfg(all(test, not(feature = "db-tests")))]
            {
                tracing::error!(
                    user_id = %lookup_result.user_id,
                    session_id = %lookup_result.session_id,
                    "{context}: session row has NULL wrapper fields without db-tests feature"
                );
                return Err(internal_error(
                    "Session row has NULL wrapper fields without db-tests feature".to_string(),
                ));
            }
        }
        _ => {
            tracing::error!(
                user_id = %lookup_result.user_id,
                session_id = %lookup_result.session_id,
                "{context}: session row has inconsistent wrapper fields"
            );
            return Err(internal_error(
                "Session row has inconsistent wrapper fields".to_string(),
            ));
        }
    };

    crate::db::initialize_user_db(lookup_result.user_id, open_mode)
        .map_err(|err| internal_error(err.to_string()))?;

    lifecycle::pin_browser_session(lookup_result.user_id, lookup_result.session_id)
        .map_err(|error| internal_error(error.to_string()))?;
    let session_still_exists =
        session::session_exists(lookup_result.session_id, lookup_result.user_id)
            .map_err(|error| internal_error(error.to_string()))?;
    if !session_still_exists {
        lifecycle::unpin_browser_session(lookup_result.user_id, lookup_result.session_id)
            .map_err(|error| internal_error(error.to_string()))?;
        return Err(unauthorized_error(INVALID_OR_EXPIRED_SESSION.to_string()));
    }

    Ok(InitializedSession {
        session: Session {
            session_id: lookup_result.session_id,
            user_id: lookup_result.user_id,
            token: session_token.clone(),
            created_at: lookup_result.created_at,
            expires_at: lookup_result.expires_at,
        },
        _request_lease: request_lease,
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::*;

    #[test]
    fn require_session_token_returns_error_when_missing() {
        #[cfg(feature = "desktop")]
        crate::desktop_session::clear();

        let cookies = CookieJar::new();
        let result = require_session_token("test", &cookies, |msg| msg);
        assert_eq!(result.err(), Some("Invalid or expired session".to_string()));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::encryption::{
        DbEnvelope, Dek, ServerMasterSecret, SessionCreationContext, UnlockAuthority,
        UserDbOpenMode, current_sqlcipher_compatibility, generate_server_master_secret_for_test,
        replace_cached_server_master_secret_for_test, write_envelope,
    };
    use crate::models::DEFAULT_SESSION_DURATION_MINUTES;

    struct CachedSecretGuard {
        previous: Option<ServerMasterSecret>,
    }

    impl CachedSecretGuard {
        fn replace(secret: Option<ServerMasterSecret>) -> Self {
            let previous = replace_cached_server_master_secret_for_test(secret);
            Self { previous }
        }
    }

    impl Drop for CachedSecretGuard {
        fn drop(&mut self) {
            let previous = self.previous.take();
            replace_cached_server_master_secret_for_test(previous);
        }
    }

    #[test]
    fn require_initialized_session_returns_unauthorized_for_unknown_token() {
        db::enable_test_mode();
        db::reset_test_db();
        db::enable_user_test_mode();

        let token = SessionToken::from_raw("missing-token".to_string());
        let result = require_initialized_session("test", &token, |msg| msg, |msg| msg);
        assert_eq!(result.err(), Some("Invalid or expired session".to_string()));
    }

    #[test]
    fn require_initialized_session_returns_unauthorized_on_unwrap_failure() {
        let _guard = db::acquire_test_runtime().expect("should acquire test runtime");
        let user_id = db::unique_user_id();
        let dek = Dek::generate();
        let sqlcipher_compatibility =
            current_sqlcipher_compatibility().expect("should detect sqlcipher compatibility");
        db::ensure_test_app_user(user_id);
        write_envelope(
            user_id,
            &DbEnvelope::Encrypted {
                sqlcipher_version: sqlcipher_compatibility.clone(),
                wrapped_keys: Vec::new(),
            },
        )
        .expect("envelope should write");

        db::initialize_user_db(
            user_id,
            UserDbOpenMode::Encrypted {
                dek: dek.clone(),
                authority: UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility,
            },
        )
        .expect("user db should initialize");

        let session_secret = generate_server_master_secret_for_test();
        let wrong_secret = generate_server_master_secret_for_test();
        let session = session::create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::Encrypted {
                dek,
                server_secret: session_secret,
            },
        )
        .expect("encrypted session should be created");

        let _secret_guard = CachedSecretGuard::replace(Some(wrong_secret));
        let result = require_initialized_session("test", &session.token, |msg| msg, |msg| msg);

        assert_eq!(result.err(), Some("Invalid or expired session".to_string()));
        let lookup = session::get_session_by_token(&session.token).expect("lookup should succeed");
        assert!(
            lookup.is_none(),
            "failed unwrap should invalidate the session"
        );
        assert!(
            !db::list_open_user_db_users()
                .expect("should list open user dbs")
                .contains(&user_id)
        );
    }

    #[test]
    fn initialized_session_holds_request_lease_until_drop() {
        let _guard = db::acquire_test_runtime().expect("should acquire test runtime");
        let user_id = db::unique_user_id();
        db::setup_test_user(user_id);
        let session = session::create_session_with_duration(
            user_id,
            DEFAULT_SESSION_DURATION_MINUTES,
            &SessionCreationContext::PlaintextTest,
        )
        .expect("session should be created");
        let initialized = require_initialized_session("test", &session.token, |msg| msg, |msg| msg)
            .expect("session should initialize");

        session::delete_session(&session.token).expect("session should invalidate");
        assert!(
            db::list_open_user_db_users()
                .expect("open user databases should list")
                .contains(&user_id),
            "request lease must keep the database open after its browser pin is removed"
        );

        drop(initialized);
        assert!(
            !db::list_open_user_db_users()
                .expect("open user databases should list after request")
                .contains(&user_id)
        );
    }
}

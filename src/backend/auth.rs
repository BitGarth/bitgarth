use crate::legal::LegalAcknowledgement;
#[cfg(all(feature = "server", feature = "desktop"))]
use crate::models::AuthEntryBannerKind;
#[cfg(feature = "server")]
use crate::models::FieldErrors;
use crate::models::{AuthEntryDecision, AuthResponse, RawPlaintextPassword, RawUsername};
#[cfg(feature = "server")]
use crate::models::{
    AuthEntryMode, CredentialId, Session, User, UserId, ValidatedPlaintextPassword,
    ValidatedUsername,
};
#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;

use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::ProxyHeaderTrust;
#[cfg(feature = "server")]
use super::session_context::{require_initialized_session, require_session_token};
#[cfg(feature = "server")]
use super::session_token::lookup_session_token;
#[cfg(feature = "server")]
use crate::auth::{lifecycle, password, session};
#[cfg(feature = "server")]
use crate::db::{
    encryption::{
        DbEnvelope, SessionCreationContext, UnlockAuthority, UserDbOpenMode, read_envelope,
        resolve_server_master_secret, user_envelope_path, write_envelope,
    },
    get_user_db_dek, initialize_user_db, load_settings, with_db,
};
#[cfg(feature = "server")]
use crate::models::parse_datetime;
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::{DateTime, Utc};
#[cfg(feature = "server")]
use cookie::time::Duration as CookieDuration;
#[cfg(feature = "server")]
use cookie::{Cookie, SameSite};
#[cfg(feature = "server")]
use dioxus::fullstack::http::header::SET_COOKIE;
#[cfg(feature = "server")]
use dioxus::fullstack::http::{HeaderMap, Uri};
#[cfg(feature = "server")]
use dioxus::fullstack::{FullstackContext, HeaderValue};

pub(crate) type AuthError = ApiErrorEnvelope;

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> AuthError {
    AuthError::unauthorized(message)
}

#[cfg(feature = "server")]
fn validation_error(errors: FieldErrors) -> AuthError {
    AuthError::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn username_conflict_error() -> AuthError {
    let mut errors = FieldErrors::new();
    errors.add("username", "Username already exists".to_string());
    AuthError::conflict("Username already exists", errors)
}

#[cfg(feature = "server")]
const LOGIN_ENVELOPE_MISSING_SUPPORT_CODE: &str = "AUTH-LOGIN-ENVELOPE-MISSING";
#[cfg(feature = "server")]
const LOGIN_ENVELOPE_READ_SUPPORT_CODE: &str = "AUTH-LOGIN-ENVELOPE-READ";
#[cfg(feature = "server")]
const LOGIN_ENVELOPE_UNLOCK_SUPPORT_CODE: &str = "AUTH-LOGIN-ENVELOPE-UNLOCK";

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> AuthError {
    tracing::error!(
        context,
        error = %detail,
        "auth: internal failure"
    );
    AuthError::internal()
}

#[cfg(feature = "server")]
fn login_support_error(
    user_id: UserId,
    summary: &str,
    technical_detail: impl Into<String>,
    support_code: &str,
) -> AuthError {
    let envelope_path = user_envelope_path(user_id)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unavailable".to_string());
    let technical_detail = technical_detail.into();
    let message = format!(
        "{summary} User ID: {user_id}. Envelope file: {envelope_path}. Technical detail: {technical_detail}. Support code: {support_code}."
    );
    AuthError::conflict(message, FieldErrors::new())
}

#[cfg(feature = "server")]
fn login_envelope_read_error(user_id: UserId, detail: impl Into<String>) -> AuthError {
    let detail = detail.into();
    let summary = if detail.starts_with("Envelope file not found at ") {
        "Your account cannot be opened because its encrypted database envelope file is missing. BitGarth verified your password, but the JSON envelope required to unlock your private database was not found."
    } else {
        "Your account cannot be opened because its encrypted database envelope is unreadable. BitGarth verified your password, but the JSON envelope required to unlock your private database could not be read."
    };
    let support_code = if detail.starts_with("Envelope file not found at ") {
        LOGIN_ENVELOPE_MISSING_SUPPORT_CODE
    } else {
        LOGIN_ENVELOPE_READ_SUPPORT_CODE
    };
    login_support_error(user_id, summary, detail, support_code)
}

#[cfg(feature = "server")]
fn login_envelope_unlock_error(user_id: UserId, detail: impl Into<String>) -> AuthError {
    login_support_error(
        user_id,
        "Your account cannot be opened because its encrypted database envelope could not unlock the database key. BitGarth verified your password, but the stored envelope wrapper could not decrypt the key needed to open your private database.",
        detail,
        LOGIN_ENVELOPE_UNLOCK_SUPPORT_CODE,
    )
}

// ============ Cookie Helpers ============

#[cfg(feature = "server")]
const COOKIE_SECURE_POLICY_ENV: &str = "BITGARTH_COOKIE_SECURE_POLICY";

#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CookieSecurePolicy {
    #[default]
    Auto,
    Always,
    Never,
}

#[cfg(feature = "server")]
impl CookieSecurePolicy {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    fn from_env() -> Self {
        let default = Self::default();
        let raw = match std::env::var(COOKIE_SECURE_POLICY_ENV) {
            Ok(raw) => raw,
            Err(_) => return default,
        };

        match Self::parse(&raw) {
            Some(policy) => policy,
            None => {
                tracing::warn!(
                    env_var = COOKIE_SECURE_POLICY_ENV,
                    value = %raw,
                    fallback = ?default,
                    "auth: invalid cookie secure policy, using default"
                );
                default
            }
        }
    }
}

#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestScheme {
    Http,
    Https,
}

#[cfg(feature = "server")]
impl RequestScheme {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            _ => None,
        }
    }
}

#[cfg(feature = "server")]
fn request_scheme_from_uri(uri: &Uri) -> Option<RequestScheme> {
    uri.scheme_str().and_then(RequestScheme::parse)
}

#[cfg(feature = "server")]
fn request_scheme_from_forwarded_header(value: &str) -> Option<RequestScheme> {
    for forwarded_element in value.split(',') {
        for attribute in forwarded_element.split(';') {
            let mut key_value = attribute.trim().splitn(2, '=');
            let key = match key_value.next() {
                Some(key) => key.trim(),
                None => continue,
            };

            if !key.eq_ignore_ascii_case("proto") {
                continue;
            }

            let proto = match key_value.next() {
                Some(proto) => proto.trim().trim_matches('"'),
                None => continue,
            };

            if let Some(scheme) = RequestScheme::parse(proto) {
                return Some(scheme);
            }
        }
    }

    None
}

#[cfg(feature = "server")]
fn request_scheme_from_x_forwarded_proto_header(value: &str) -> Option<RequestScheme> {
    value
        .split(',')
        .find_map(|segment| RequestScheme::parse(segment.trim()))
}

#[cfg(feature = "server")]
fn request_scheme_from_headers(
    headers: &HeaderMap,
    proxy_header_trust: ProxyHeaderTrust,
) -> Option<RequestScheme> {
    if !proxy_header_trust.allows_forwarded_proto() {
        return None;
    }

    if proxy_header_trust.allows_forwarded_for() {
        let forwarded = headers
            .get("forwarded")
            .and_then(|value| value.to_str().ok())
            .and_then(request_scheme_from_forwarded_header);
        if forwarded.is_some() {
            return forwarded;
        }
    }

    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(request_scheme_from_x_forwarded_proto_header)
}

#[cfg(feature = "server")]
fn resolve_request_scheme(
    ctx: &FullstackContext,
    proxy_header_trust: ProxyHeaderTrust,
) -> Option<RequestScheme> {
    let parts = ctx.parts_mut();
    request_scheme_from_uri(&parts.uri)
        .or_else(|| request_scheme_from_headers(&parts.headers, proxy_header_trust))
}

#[cfg(feature = "server")]
fn should_use_secure_cookie(ctx: &FullstackContext) -> bool {
    let policy = CookieSecurePolicy::from_env();
    let proxy_header_trust = ProxyHeaderTrust::from_env();
    let request_scheme = resolve_request_scheme(ctx, proxy_header_trust);

    let use_secure = match policy {
        CookieSecurePolicy::Always => true,
        CookieSecurePolicy::Never => false,
        CookieSecurePolicy::Auto => matches!(request_scheme, Some(RequestScheme::Https)),
    };

    tracing::debug!(
        policy = ?policy,
        proxy_header_trust = ?proxy_header_trust,
        request_scheme = ?request_scheme,
        secure = use_secure,
        "auth: resolved session cookie security"
    );

    use_secure
}

#[cfg(feature = "server")]
fn set_session_cookie(session: &Session) -> Result<(), AuthError> {
    use chrono::Utc;

    let Some(ctx) = FullstackContext::current() else {
        #[cfg(feature = "desktop")]
        {
            crate::desktop_session::set(session.token.clone());
            tracing::debug!(
                user_id = %session.user_id,
                session_id = %session.session_id,
                "auth: stored desktop session token"
            );
        }
        return Ok(());
    };

    // Calculate cookie max_age from session expiry
    let duration_seconds = (session.expires_at - Utc::now()).num_seconds();
    let max_age = CookieDuration::seconds(duration_seconds);
    let use_secure = should_use_secure_cookie(&ctx);

    tracing::debug!(
        user_id = %session.user_id,
        session_id = %session.session_id,
        max_age_seconds = duration_seconds,
        secure = use_secure,
        "auth: setting session cookie"
    );

    let cookie = Cookie::build((
        session::SESSION_COOKIE_NAME,
        session.token.as_str().to_string(),
    ))
    .path("/")
    .http_only(true)
    .same_site(SameSite::Lax)
    .secure(use_secure)
    .max_age(max_age)
    .build();

    let header_value = HeaderValue::from_str(&cookie.to_string())
        .map_err(|e| internal_error("set_session_cookie", e))?;
    ctx.add_response_header(SET_COOKIE, header_value);
    Ok(())
}

#[cfg(feature = "server")]
fn clear_session_cookie() -> Result<(), AuthError> {
    let Some(ctx) = FullstackContext::current() else {
        #[cfg(feature = "desktop")]
        {
            crate::desktop_session::clear();
            tracing::debug!("auth: cleared desktop session token");
        }
        return Ok(());
    };

    tracing::debug!("auth: clearing session cookie");

    let cookie = Cookie::build((session::SESSION_COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(should_use_secure_cookie(&ctx))
        .max_age(CookieDuration::seconds(0))
        .build();

    let header_value = HeaderValue::from_str(&cookie.to_string())
        .map_err(|e| internal_error("clear_session_cookie", e))?;
    ctx.add_response_header(SET_COOKIE, header_value);
    Ok(())
}

#[cfg(feature = "server")]
fn get_user_by_username(username: &ValidatedUsername) -> Option<User> {
    use crate::auth::session::AuthError as SessionAuthError;
    use std::str::FromStr;

    let result: Result<Option<User>, SessionAuthError> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT user_id, username, created_at, updated_at FROM users WHERE username = ?1",
        )?;
        let result = stmt.query_row([username.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        });

        match result {
            Ok((user_id_str, username, created_at_str, updated_at_str)) => {
                let user_id = UserId::from_str(&user_id_str).map_err(|e| {
                    SessionAuthError::DateTimeParse(format!("Invalid user_id ULID: {}", e))
                })?;
                let created_at = parse_datetime(&created_at_str)
                    .map_err(|e| SessionAuthError::DateTimeParse(e.to_string()))?;
                let updated_at = parse_datetime(&updated_at_str)
                    .map_err(|e| SessionAuthError::DateTimeParse(e.to_string()))?;
                Ok(Some(User {
                    user_id,
                    username,
                    created_at,
                    updated_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(SessionAuthError::Database(e)),
        }
    });
    let user = result.ok().flatten();
    tracing::debug!(
        username = %username.as_str(),
        found = user.is_some(),
        "auth: lookup user by username"
    );
    user
}

#[cfg(feature = "server")]
fn get_user_by_id(user_id: &UserId) -> Option<User> {
    use crate::auth::session::AuthError as SessionAuthError;
    use std::str::FromStr;

    let result: Result<Option<User>, SessionAuthError> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT user_id, username, created_at, updated_at FROM users WHERE user_id = ?1",
        )?;
        let result = stmt.query_row([user_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        });

        match result {
            Ok((user_id_str, username, created_at_str, updated_at_str)) => {
                let user_id = UserId::from_str(&user_id_str).map_err(|e| {
                    SessionAuthError::DateTimeParse(format!("Invalid user_id ULID: {}", e))
                })?;
                let created_at = parse_datetime(&created_at_str)
                    .map_err(|e| SessionAuthError::DateTimeParse(e.to_string()))?;
                let updated_at = parse_datetime(&updated_at_str)
                    .map_err(|e| SessionAuthError::DateTimeParse(e.to_string()))?;
                Ok(Some(User {
                    user_id,
                    username,
                    created_at,
                    updated_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(SessionAuthError::Database(e)),
        }
    });

    let user = result.ok().flatten();
    tracing::debug!(
        user_id = %user_id,
        found = user.is_some(),
        "auth: lookup user by id"
    );
    user
}

#[cfg(feature = "server")]
fn get_password_hash_for_user(user_id: UserId) -> Option<String> {
    use crate::auth::session::AuthError as SessionAuthError;

    let result: Result<Option<String>, SessionAuthError> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pc.password_hash
             FROM auth_credentials ac
             JOIN password_credentials pc ON ac.credential_id = pc.credential_id
             WHERE ac.user_id = ?1 AND ac.auth_method = 'password'",
        )?;
        let result = stmt.query_row([user_id.to_string()], |row| row.get::<_, String>(0));
        match result {
            Ok(hash) => Ok(Some(hash)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(SessionAuthError::Database(e)),
        }
    });
    let hash = result.ok().flatten();
    tracing::debug!(
        user_id = %user_id,
        found = hash.is_some(),
        "auth: lookup password hash"
    );
    hash
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct RawRegisterCredentials {
    username: RawUsername,
    password: RawPlaintextPassword,
    legal_acknowledgement: Option<LegalAcknowledgement>,
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct ValidatedRegisterCredentials {
    username: ValidatedUsername,
    password: ValidatedPlaintextPassword,
    legal_acknowledgement: crate::legal::ValidatedLegalAcknowledgement,
}

#[cfg(feature = "server")]
impl RawRegisterCredentials {
    fn try_into_validated(self) -> Result<ValidatedRegisterCredentials, FieldErrors> {
        let mut errors = FieldErrors::new();

        let username = match self.username.validate() {
            Ok(value) => Some(value),
            Err(err) => {
                errors.add("username", err.to_string());
                None
            }
        };

        let password = match self.password.validate_all() {
            Ok(value) => Some(value),
            Err(validation_errors) => {
                for err in validation_errors {
                    errors.add("password", err.to_string());
                }
                None
            }
        };

        let legal_acknowledgement =
            match crate::legal::validate_registration_acknowledgement(self.legal_acknowledgement) {
                Ok(value) => Some(value),
                Err(validation_errors) => {
                    for (field, messages) in validation_errors.0 {
                        for message in messages {
                            errors.add(&field, message);
                        }
                    }
                    None
                }
            };

        if !errors.is_empty() {
            return Err(errors);
        }

        match (username, password, legal_acknowledgement) {
            (Some(username), Some(password), Some(legal_acknowledgement)) => {
                Ok(ValidatedRegisterCredentials {
                    username,
                    password,
                    legal_acknowledgement,
                })
            }
            _ => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add("username", "missing validated credentials".to_string());
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct RawLoginCredentials {
    username: RawUsername,
    password: RawPlaintextPassword,
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct ValidatedLoginCredentials {
    username: ValidatedUsername,
    password: RawPlaintextPassword,
}

#[cfg(feature = "server")]
impl RawLoginCredentials {
    fn try_into_validated(self) -> Result<ValidatedLoginCredentials, FieldErrors> {
        let username = self.username.validate().map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add("username", err.to_string());
            errors
        })?;

        Ok(ValidatedLoginCredentials {
            username,
            password: self.password,
        })
    }
}

#[cfg(feature = "server")]
fn create_user_with_password(
    username: &ValidatedUsername,
    password: &ValidatedPlaintextPassword,
    legal_acknowledgement: &crate::legal::ValidatedLegalAcknowledgement,
) -> Result<User, AuthError> {
    use crate::auth::session::AuthError as SessionAuthError;
    use crate::db::with_db_mut;
    use chrono::Utc;

    // Hash the password
    let (password_hash, salt) =
        password::hash_password(password).map_err(|e| internal_error("hash_password", e))?;

    let result: Result<User, SessionAuthError> = with_db_mut(|conn| {
        let tx = conn.transaction()?;
        let user_id = UserId::new();
        let credential_id = CredentialId::new();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Insert user
        tx.execute(
            "INSERT INTO users (user_id, username, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![user_id.to_string(), username.as_str(), now_str, now_str],
        )?;

        // Insert auth credential
        tx.execute(
            "INSERT INTO auth_credentials (credential_id, user_id, auth_method, is_primary, created_at) VALUES (?1, ?2, 'password', 1, ?3)",
            rusqlite::params![credential_id.to_string(), user_id.to_string(), now_str],
        )?;

        // Insert password credential - convert to strings for database storage
        tx.execute(
            "INSERT INTO password_credentials (credential_id, password_hash, salt) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                credential_id.to_string(),
                password_hash.to_string(),
                salt.to_string()
            ],
        )?;

        crate::db::legal_acceptances::insert_registration_acceptances(
            &tx,
            user_id,
            legal_acknowledgement,
            &now_str,
        )?;

        tx.commit()?;

        let user = User {
            user_id,
            username: username.as_str().to_string(),
            created_at: now,
            updated_at: now,
        };

        tracing::debug!(
            user_id = %user.user_id,
            username = %user.username,
            "auth: created user with password credential"
        );

        Ok(user)
    });

    result.map_err(|error| {
        if let SessionAuthError::Database(rusqlite::Error::SqliteFailure(details, message)) = &error
            && details.code == rusqlite::ErrorCode::ConstraintViolation
            && message
                .as_ref()
                .is_some_and(|value| value.contains("users.username"))
        {
            return username_conflict_error();
        }

        internal_error("create_user_with_password", error)
    })
}

#[post("/_app/auth/register")]
pub(crate) async fn register(
    username: RawUsername,
    password: RawPlaintextPassword,
    legal_acknowledgement: Option<LegalAcknowledgement>,
) -> Result<AuthResponse, AuthError> {
    let raw_username = username.as_str().to_string();
    tracing::debug!(
        username = %raw_username,
        "auth: register attempt"
    );
    let validated = RawRegisterCredentials {
        username,
        password,
        legal_acknowledgement,
    }
    .try_into_validated()
    .map_err(|errors| {
        tracing::debug!(
            username = %raw_username,
            error_fields = errors.0.len(),
            "auth: register validation failed"
        );
        validation_error(errors)
    })?;

    let user = create_user_with_password(
        &validated.username,
        &validated.password,
        &validated.legal_acknowledgement,
    )?;
    let _lifecycle_guard = lifecycle::acquire_user_lifecycle_lock(user.user_id)
        .map_err(|e| internal_error("acquire_user_lifecycle_lock", e))?;

    #[cfg(feature = "dev-config")]
    let use_unencrypted_dev = crate::db::encryption::should_use_unencrypted_dev();
    #[cfg(not(feature = "dev-config"))]
    let use_unencrypted_dev = false;

    let (open_mode, session_context) = if use_unencrypted_dev {
        #[cfg(feature = "dev-config")]
        {
            write_envelope(user.user_id, &DbEnvelope::unencrypted_dev())
                .map_err(|e| internal_error("write_envelope", e))?;
            (
                UserDbOpenMode::UnencryptedDev,
                SessionCreationContext::UnencryptedDev,
            )
        }
        #[cfg(not(feature = "dev-config"))]
        {
            unreachable!("use_unencrypted_dev is always false without dev-config")
        }
    } else {
        let (envelope, dek) = DbEnvelope::new_encrypted(validated.password.as_str())
            .map_err(|e| internal_error("new_encrypted", e))?;
        let sqlcipher_compatibility = envelope
            .sqlcipher_compatibility()
            .ok_or_else(|| internal_error("sqlcipher_compatibility", "missing encrypted mode"))?;
        write_envelope(user.user_id, &envelope).map_err(|e| internal_error("write_envelope", e))?;
        let server_secret = resolve_server_master_secret()
            .map_err(|e| internal_error("resolve_server_master_secret", e))?;
        (
            UserDbOpenMode::Encrypted {
                dek: dek.clone(),
                authority: UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility,
            },
            SessionCreationContext::Encrypted { dek, server_secret },
        )
    };

    initialize_user_db(user.user_id, open_mode)
        .map_err(|e| internal_error("initialize_user_db", e))?;

    let mut settings =
        load_settings(user.user_id).map_err(|e| internal_error("load_settings", e))?;

    let duration_minutes = session::SessionTimeoutPolicy::resolve()
        .absolute_timeout
        .num_minutes()
        .try_into()
        .unwrap_or(1440);

    let sess =
        session::create_session_with_duration(user.user_id, duration_minutes, &session_context)
            .map_err(|e| internal_error("create_session", e))?;

    settings.etherscan_api_key = None;

    tracing::debug!(
        user_id = %user.user_id,
        "auth: user db initialized and settings loaded"
    );

    set_session_cookie(&sess)?;

    tracing::debug!(
        user_id = %user.user_id,
        session_id = %sess.session_id,
        expires_at = %sess.expires_at,
        "auth: register completed with session"
    );

    Ok(AuthResponse { user, settings })
}

#[post("/_app/auth/login")]
pub(crate) async fn login(
    username: RawUsername,
    password: RawPlaintextPassword,
) -> Result<AuthResponse, AuthError> {
    let raw_username = username.as_str().to_string();
    tracing::debug!(
        username = %raw_username,
        "auth: login attempt"
    );

    let validated = RawLoginCredentials { username, password }
        .try_into_validated()
        .map_err(|errors| {
            let message = errors
                .first("username")
                .cloned()
                .unwrap_or_else(|| errors.to_string());
            tracing::debug!(
                username = %raw_username,
                error = %message,
                "auth: login username validation failed"
            );
            validation_error(errors)
        })?;

    // Look up user by username
    let user = get_user_by_username(&validated.username).ok_or_else(|| {
        tracing::debug!(
            username = %validated.username.as_str(),
            "auth: login failed, user not found"
        );
        AuthError::unauthorized("Invalid username or password")
    })?;

    let _lifecycle_guard = lifecycle::acquire_user_lifecycle_lock(user.user_id)
        .map_err(|e| internal_error("acquire_user_lifecycle_lock", e))?;

    // Get password hash and verify
    let password_hash = get_password_hash_for_user(user.user_id).ok_or_else(|| {
        tracing::debug!(
            user_id = %user.user_id,
            "auth: login failed, missing password hash"
        );
        AuthError::unauthorized("Invalid username or password")
    })?;

    let is_valid = password::verify_password(&validated.password, &password_hash)
        .ok()
        .unwrap_or(false);

    if !is_valid {
        tracing::debug!(
            user_id = %user.user_id,
            "auth: login failed, invalid password"
        );
        return Err(AuthError::unauthorized("Invalid username or password"));
    }

    tracing::debug!(
        user_id = %user.user_id,
        "auth: login password verified"
    );

    let envelope = read_envelope(user.user_id).map_err(|e| {
        tracing::error!(
            user_id = %user.user_id,
            error = %e,
            "auth: login blocked by envelope read failure"
        );
        login_envelope_read_error(user.user_id, e.to_string())
    })?;

    let (open_mode, session_context) = match &envelope {
        DbEnvelope::Encrypted { .. } => {
            let dek = envelope
                .unwrap_with_password(validated.password.as_str())
                .map_err(|e| {
                    tracing::error!(
                        user_id = %user.user_id,
                        error = %e,
                        "auth: login blocked by envelope unlock failure"
                    );
                    login_envelope_unlock_error(user.user_id, e.to_string())
                })?;
            let sqlcipher_compatibility = envelope.sqlcipher_compatibility().ok_or_else(|| {
                internal_error(
                    "sqlcipher_compatibility",
                    "missing encrypted envelope metadata",
                )
            })?;
            let server_secret = resolve_server_master_secret()
                .map_err(|e| internal_error("resolve_server_master_secret", e))?;
            (
                UserDbOpenMode::Encrypted {
                    dek: dek.clone(),
                    authority: UnlockAuthority::PasswordLogin,
                    sqlcipher_compatibility,
                },
                SessionCreationContext::Encrypted { dek, server_secret },
            )
        }
        #[cfg(feature = "dev-config")]
        DbEnvelope::UnencryptedDev => (
            UserDbOpenMode::UnencryptedDev,
            SessionCreationContext::UnencryptedDev,
        ),
    };

    initialize_user_db(user.user_id, open_mode)
        .map_err(|e| internal_error("initialize_user_db", e))?;

    let mut settings =
        load_settings(user.user_id).map_err(|e| internal_error("load_settings", e))?;

    let duration_minutes = session::SessionTimeoutPolicy::resolve()
        .absolute_timeout
        .num_minutes()
        .try_into()
        .unwrap_or(1440);

    let sess =
        session::create_session_with_duration(user.user_id, duration_minutes, &session_context)
            .map_err(|e| internal_error("create_session", e))?;

    settings.etherscan_api_key = None;

    tracing::debug!(
        user_id = %user.user_id,
        "auth: user db initialized and settings loaded"
    );

    if let Err(error) = record_last_login(user.user_id, Utc::now()) {
        if let Err(delete_error) = session::delete_session(&sess.token) {
            tracing::warn!(
                user_id = %user.user_id,
                session_id = %sess.session_id,
                error = %delete_error,
                "auth: failed to remove session after last_login_at write failure"
            );
        }
        return Err(error);
    }
    set_session_cookie(&sess)?;
    crate::backend::payments::refresh_entitlements_after_login_in_background(user.user_id);
    let _ = crate::tasks::enqueue_price_history_reconciliation(
        user.user_id,
        crate::tasks::PriceHistoryReconciliationReason::Login,
    )
    .await;

    tracing::debug!(
        user_id = %user.user_id,
        session_id = %sess.session_id,
        expires_at = %sess.expires_at,
        "auth: login completed with session"
    );

    Ok(AuthResponse { user, settings })
}

#[post("/_app/auth/logout", cookies: CookieJar)]
pub(crate) async fn logout() -> Result<(), AuthError> {
    let session_token = lookup_session_token("logout", &cookies);

    // Get user_id from session before deleting
    let user_id = session_token
        .as_ref()
        .and_then(|token| session::get_session_by_token(token).ok())
        .flatten()
        .map(|s| s.user_id);

    tracing::debug!(
        user_id = ?user_id,
        has_session_token = session_token.is_some(),
        "auth: logout requested"
    );

    // Delete the session if present
    if let Some(token) = session_token {
        session::delete_session(&token).map_err(|e| internal_error("delete_session", e))?;
        tracing::debug!(
            user_id = ?user_id,
            "auth: session deleted during logout"
        );
    }

    clear_session_cookie()?;

    Ok(())
}

#[cfg(all(feature = "server", feature = "desktop"))]
fn decide_entry(has_session_token: bool, has_users: bool) -> AuthEntryMode {
    if has_session_token || has_users {
        AuthEntryMode::Login
    } else {
        AuthEntryMode::Register
    }
}

#[cfg(all(feature = "server", feature = "desktop"))]
fn has_any_user() -> Result<bool, session::AuthError> {
    with_db(|conn| {
        let exists: i64 = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM users LIMIT 1)", [], |row| {
                row.get(0)
            })
            .map_err(session::AuthError::Database)?;
        Ok(exists == 1)
    })
}

#[get("/_app/auth/entry", cookies: CookieJar)]
pub(crate) async fn auth_entry() -> Result<AuthEntryDecision, AuthError> {
    let has_session_token = lookup_session_token("auth_entry", &cookies).is_some();

    if has_session_token {
        tracing::debug!(
            has_session_token,
            "auth: entry decision -> login (session token present)"
        );
        return Ok(AuthEntryDecision {
            mode: AuthEntryMode::Login,
            banner: None,
        });
    }

    #[cfg(feature = "desktop")]
    {
        match has_any_user() {
            Ok(has_users) => {
                let mode = decide_entry(has_session_token, has_users);
                tracing::debug!(
                    has_users,
                    mode = ?mode,
                    "auth: entry decision (desktop user check)"
                );
                Ok(AuthEntryDecision { mode, banner: None })
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "auth: entry decision failed to query users"
                );
                Ok(AuthEntryDecision {
                    mode: AuthEntryMode::Register,
                    banner: Some(AuthEntryBannerKind::DatabaseUnavailable),
                })
            }
        }
    }

    #[cfg(not(feature = "desktop"))]
    {
        tracing::debug!("auth: entry decision -> register (web default)");
        Ok(AuthEntryDecision {
            mode: AuthEntryMode::Register,
            banner: None,
        })
    }
}

#[get("/_app/auth/me", cookies: CookieJar)]
pub(crate) async fn me() -> Result<AuthResponse, AuthError> {
    tracing::debug!("auth: session restore (me) requested");
    let session_token = require_session_token("me", &cookies, unauthorized_error)?;
    let initialized_session =
        require_initialized_session("auth", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;

    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        session_id = %initialized_session.session.session_id,
        "auth: session restore validated session"
    );

    let mut settings = load_settings(user_id).map_err(|e| internal_error("load_settings", e))?;
    settings.etherscan_api_key = None;

    tracing::debug!(
        user_id = %user_id,
        "auth: session restore loaded settings"
    );

    let user = get_user_by_id(&user_id).ok_or_else(|| {
        tracing::debug!(
            user_id = %user_id,
            "auth: session restore failed, user not found"
        );
        AuthError::unauthorized("Invalid or expired session")
    })?;

    tracing::debug!(
        user_id = %user.user_id,
        "auth: session restore completed"
    );

    Ok(AuthResponse { user, settings })
}

#[post("/_app/auth/change-password", cookies: CookieJar)]
pub(crate) async fn change_password(
    old_password: RawPlaintextPassword,
    new_password: RawPlaintextPassword,
) -> Result<(), AuthError> {
    let session_token = require_session_token("change_password", &cookies, unauthorized_error)?;
    let initialized_session =
        require_initialized_session("auth", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;

    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        "auth: change password requested"
    );

    let new_validated = new_password.validate_all().map_err(|errors| {
        let mut field_errors = FieldErrors::new();
        for err in errors {
            field_errors.add("new_password", err.to_string());
        }
        validation_error(field_errors)
    })?;

    let password_hash = get_password_hash_for_user(user_id)
        .ok_or_else(|| internal_error("change_password", "missing password hash"))?;

    let is_valid = password::verify_password(&old_password, &password_hash)
        .ok()
        .unwrap_or(false);

    if !is_valid {
        tracing::debug!(
            user_id = %user_id,
            "auth: change password failed, invalid old password"
        );
        let mut errors = FieldErrors::new();
        errors.add("old_password", "Incorrect password".to_string());
        return Err(AuthError::unauthorized_with_errors(
            "Incorrect password",
            errors,
        ));
    }

    let envelope = read_envelope(user_id).map_err(|e| internal_error("read_envelope", e))?;

    match &envelope {
        DbEnvelope::Encrypted { .. } => {
            let dek = get_user_db_dek(&user_id)
                .map_err(|e| internal_error("get_user_db_dek", e))?
                .ok_or_else(|| {
                    internal_error("change_password", "DEK not found for encrypted user")
                })?;

            let mut envelope = envelope;
            let new_wrap_id = envelope
                .add_password_wrapper(&dek, new_validated.as_str())
                .map_err(|e| internal_error("add_password_wrapper", e))?;

            write_envelope(user_id, &envelope).map_err(|e| internal_error("write_envelope", e))?;

            let (new_hash, new_salt) = password::hash_password(&new_validated)
                .map_err(|e| internal_error("hash_password", e))?;

            update_password_hash(user_id, new_hash, new_salt)
                .map_err(|e| internal_error("update_password_hash", e))?;

            envelope.compact_password_wrappers(new_wrap_id.as_str());
            if let Err(e) = write_envelope(user_id, &envelope) {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "auth: change password envelope compaction failed (non-fatal)"
                );
            }

            tracing::debug!(
                user_id = %user_id,
                "auth: change password completed for encrypted user"
            );
        }
        #[cfg(feature = "dev-config")]
        DbEnvelope::UnencryptedDev => {
            let (new_hash, new_salt) = password::hash_password(&new_validated)
                .map_err(|e| internal_error("hash_password", e))?;

            update_password_hash(user_id, new_hash, new_salt)
                .map_err(|e| internal_error("update_password_hash", e))?;

            tracing::debug!(
                user_id = %user_id,
                "auth: change password completed for unencrypted dev user"
            );
        }
    }

    Ok(())
}

#[cfg(feature = "server")]
fn update_password_hash(
    user_id: UserId,
    new_hash: password::PasswordHashString,
    new_salt: password::SaltString,
) -> Result<(), AuthError> {
    use crate::auth::session::AuthError as SessionAuthError;
    use crate::db::with_db_mut;

    let result: Result<(), SessionAuthError> = with_db_mut(|conn| {
        conn.execute(
            "UPDATE password_credentials SET password_hash = ?1, salt = ?2 \
             WHERE credential_id = (\
                 SELECT credential_id FROM auth_credentials \
                 WHERE user_id = ?3 AND auth_method = 'password'\
             )",
            rusqlite::params![
                new_hash.to_string(),
                new_salt.to_string(),
                user_id.to_string()
            ],
        )?;
        Ok(())
    });

    result.map_err(|e| internal_error("update_password_hash", e))
}

#[cfg(feature = "server")]
fn record_last_login(user_id: UserId, now: DateTime<Utc>) -> Result<(), AuthError> {
    use crate::auth::session::AuthError as SessionAuthError;
    use crate::db::with_db_mut;

    let now = now.to_rfc3339();
    let result: Result<(), SessionAuthError> = with_db_mut(|conn| {
        conn.execute(
            "UPDATE users SET last_login_at = ?1 WHERE user_id = ?2",
            rusqlite::params![now, user_id.to_string()],
        )?;
        Ok(())
    });

    result.map_err(|error| internal_error("record_last_login", error))
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    #[cfg(feature = "db-tests")]
    use crate::db;
    #[cfg(feature = "db-tests")]
    use ulid::Ulid;

    /// Helper to set up a fresh in-memory database for each test
    #[cfg(feature = "db-tests")]
    fn setup_test_db() -> db::TestRuntimeGuard {
        db::acquire_test_runtime().expect("Failed to initialize test runtime")
    }

    #[cfg(feature = "db-tests")]
    fn unique_username(prefix: &str) -> String {
        format!("{prefix}_{}", Ulid::new())
    }

    #[cfg(feature = "db-tests")]
    fn current_legal_acknowledgement() -> Option<LegalAcknowledgement> {
        Some(crate::legal::current_registration_acknowledgement())
    }

    #[cfg(feature = "db-tests")]
    fn last_login_at_for_user(user_id: UserId) -> Option<String> {
        let result: Result<Option<String>, session::AuthError> = with_db(|conn| {
            let mut stmt = conn.prepare("SELECT last_login_at FROM users WHERE user_id = ?1")?;
            match stmt.query_row([user_id.to_string()], |row| row.get::<_, Option<String>>(0)) {
                Ok(value) => Ok(value),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(error) => Err(session::AuthError::Database(error)),
            }
        });

        result.expect("last_login_at query should succeed")
    }

    #[cfg(feature = "db-tests")]
    fn session_count_for_user(user_id: UserId) -> i64 {
        let result: Result<i64, session::AuthError> = with_db(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .map_err(session::AuthError::Database)
        });

        result.expect("session count query should succeed")
    }

    #[cfg(feature = "db-tests")]
    fn assert_field_error_contains(error: &AuthError, field: &str, expected_fragment: &str) {
        let message = error
            .first_field_error(field)
            .unwrap_or_else(|| panic!("missing field error for {field}"));
        assert!(
            message.contains(expected_fragment),
            "expected {field} error to contain '{expected_fragment}', got '{message}'",
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn raw_register_credentials_try_into_validated_collects_field_errors() {
        let credentials = RawRegisterCredentials {
            username: RawUsername::new(String::new()),
            password: RawPlaintextPassword::new("short".to_string()),
            legal_acknowledgement: None,
        };

        let errors = credentials
            .try_into_validated()
            .expect_err("invalid credentials should fail");
        assert!(errors.get("username").is_some());
        assert!(errors.get("password").is_some());
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn raw_register_credentials_try_into_validated_requires_legal_acknowledgement() {
        let credentials = RawRegisterCredentials {
            username: RawUsername::new("legaluser".to_string()),
            password: RawPlaintextPassword::new("SecurePass123".to_string()),
            legal_acknowledgement: None,
        };

        let errors = credentials
            .try_into_validated()
            .expect_err("missing legal acknowledgement should fail");

        assert!(errors.get("legal_acknowledgement").is_some());
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn raw_login_credentials_try_into_validated_rejects_invalid_username() {
        let credentials = RawLoginCredentials {
            username: RawUsername::new(String::new()),
            password: RawPlaintextPassword::new("SecurePass123".to_string()),
        };

        let errors = credentials
            .try_into_validated()
            .expect_err("empty username should fail");
        assert!(errors.get("username").is_some());
    }

    // ============ Cookie Policy Tests ============

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_cookie_secure_policy_parse_values() {
        assert_eq!(
            CookieSecurePolicy::parse("auto"),
            Some(CookieSecurePolicy::Auto)
        );
        assert_eq!(
            CookieSecurePolicy::parse("ALWAYS"),
            Some(CookieSecurePolicy::Always)
        );
        assert_eq!(
            CookieSecurePolicy::parse(" never "),
            Some(CookieSecurePolicy::Never)
        );
        assert_eq!(CookieSecurePolicy::parse("unknown"), None);
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_proxy_header_trust_parse_values() {
        assert_eq!(
            ProxyHeaderTrust::parse("true"),
            Some(ProxyHeaderTrust::Trusted)
        );
        assert_eq!(
            ProxyHeaderTrust::parse("0"),
            Some(ProxyHeaderTrust::Untrusted)
        );
        assert_eq!(
            ProxyHeaderTrust::parse("proto"),
            Some(ProxyHeaderTrust::ForwardedProtoOnly)
        );
        assert_eq!(ProxyHeaderTrust::parse("maybe"), None);
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_request_scheme_from_forwarded_header_prefers_proto() {
        let value = r#"for=192.0.2.1;proto=https;by=203.0.113.1"#;
        assert_eq!(
            request_scheme_from_forwarded_header(value),
            Some(RequestScheme::Https)
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_request_scheme_from_x_forwarded_proto_takes_first_value() {
        let value = "https,http";
        assert_eq!(
            request_scheme_from_x_forwarded_proto_header(value),
            Some(RequestScheme::Https)
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_request_scheme_from_headers_requires_trust() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=192.0.2.1;proto=http"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        assert_eq!(
            request_scheme_from_headers(&headers, ProxyHeaderTrust::Untrusted),
            None
        );
        assert_eq!(
            request_scheme_from_headers(&headers, ProxyHeaderTrust::Trusted),
            Some(RequestScheme::Http)
        );
        assert_eq!(
            request_scheme_from_headers(&headers, ProxyHeaderTrust::ForwardedProtoOnly),
            Some(RequestScheme::Https)
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn test_request_scheme_from_uri_detects_https() {
        let uri = "https://bitgarth.local/_app/auth/login"
            .parse::<Uri>()
            .expect("valid URI");
        assert_eq!(request_scheme_from_uri(&uri), Some(RequestScheme::Https));
    }

    // ============ Entry Decision Tests ============

    #[cfg(all(not(bitgarth_db_unit_only), feature = "desktop"))]
    #[test]
    fn test_decide_entry_prefers_login_with_session_token() {
        assert_eq!(decide_entry(true, false), AuthEntryMode::Login);
        assert_eq!(decide_entry(true, true), AuthEntryMode::Login);
    }

    #[cfg(all(not(bitgarth_db_unit_only), feature = "desktop"))]
    #[test]
    fn test_decide_entry_register_without_users_or_token() {
        assert_eq!(decide_entry(false, false), AuthEntryMode::Register);
    }

    #[cfg(all(feature = "db-tests", feature = "desktop"))]
    #[test]
    fn test_has_any_user_detects_empty_db() {
        let _guard = setup_test_db();
        assert!(matches!(has_any_user(), Ok(false)));
    }

    #[cfg(all(feature = "db-tests", feature = "desktop"))]
    #[test]
    fn test_has_any_user_detects_existing_user() {
        let _guard = setup_test_db();
        let user_id = UserId::new();
        let now = "2024-01-01T00:00:00Z".to_string();
        let insert_result: Result<(), session::AuthError> = with_db(|conn| {
            conn.execute(
                "INSERT INTO users (user_id, username, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    user_id.to_string(),
                    "entry_test_user",
                    now,
                    now
                ],
            )
            .map_err(session::AuthError::Database)?;
            Ok(())
        });
        assert!(insert_result.is_ok());
        assert!(matches!(has_any_user(), Ok(true)));
    }

    // ============ Register Tests ============

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_creates_user_and_returns_session() {
        let _guard = setup_test_db();

        let username = unique_username("testuser");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());

        let result = register(
            RawUsername::new(username.clone()),
            password,
            current_legal_acknowledgement(),
        )
        .await;

        assert!(result.is_ok(), "Registration should succeed");
        let response = result.unwrap();
        assert_eq!(response.user.username, username);
        assert!(!response.user.user_id.to_string().is_empty());

        let acceptance_count: i64 = with_db(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM legal_acceptances WHERE user_id = ?1",
                [response.user.user_id.to_string()],
                |row| row.get(0),
            )
            .map_err(session::AuthError::Database)
        })
        .expect("acceptance count query should succeed");
        assert_eq!(acceptance_count, 2);
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_with_weak_password_fails() {
        let _guard = setup_test_db();

        let username = unique_username("testuser");
        let password = RawPlaintextPassword::new("weak".to_string());

        let result = register(
            RawUsername::new(username),
            password,
            current_legal_acknowledgement(),
        )
        .await;

        assert!(
            result.is_err(),
            "Registration with weak password should fail"
        );
        let err = result.unwrap_err();
        assert!(err.is_validation());
        assert_field_error_contains(&err, "password", "8 characters");
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_with_empty_username_returns_field_error() {
        let _guard = setup_test_db();

        let result = register(
            RawUsername::new(String::new()),
            RawPlaintextPassword::new("SecurePass123".to_string()),
            current_legal_acknowledgement(),
        )
        .await;

        match result {
            Err(err) if err.is_validation() => {
                assert!(err.first_field_error("username").is_some());
            }
            other => panic!("Expected validation error, got {:?}", other),
        }
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_password_missing_uppercase_fails() {
        let _guard = setup_test_db();

        let username = unique_username("testuser");
        let password = RawPlaintextPassword::new("securepass123".to_string());

        let result = register(
            RawUsername::new(username),
            password,
            current_legal_acknowledgement(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_validation());
        assert_field_error_contains(&err, "password", "uppercase");
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_password_missing_lowercase_fails() {
        let _guard = setup_test_db();

        let username = unique_username("testuser");
        let password = RawPlaintextPassword::new("SECUREPASS123".to_string());

        let result = register(
            RawUsername::new(username),
            password,
            current_legal_acknowledgement(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_validation());
        assert_field_error_contains(&err, "password", "lowercase");
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_password_missing_number_fails() {
        let _guard = setup_test_db();

        let username = unique_username("testuser");
        let password = RawPlaintextPassword::new("SecurePassword".to_string());

        let result = register(
            RawUsername::new(username),
            password,
            current_legal_acknowledgement(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_validation());
        assert_field_error_contains(&err, "password", "number");
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_duplicate_username_fails() {
        let _guard = setup_test_db();

        let username1 = unique_username("duplicateuser");
        let password1 = RawPlaintextPassword::new("SecurePass123".to_string());
        register(
            RawUsername::new(username1.clone()),
            password1,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();

        let username2 = RawUsername::new(username1);
        let password2 = RawPlaintextPassword::new("AnotherPass123".to_string());
        let result = register(username2, password2, current_legal_acknowledgement()).await;

        assert!(result.is_err(), "Duplicate username should fail");
    }

    // ============ Login Tests ============

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_with_valid_credentials_succeeds() {
        let _guard = setup_test_db();

        // Register first
        let username = unique_username("loginuser");
        let username_value = username.clone();
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        register(
            RawUsername::new(username.clone()),
            password,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();

        // Then login
        let username = RawUsername::new(username);
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let result = login(username, password).await;

        assert!(result.is_ok(), "Login should succeed");
        let response = result.unwrap();
        assert_eq!(response.user.username, username_value);
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_successful_login_sets_last_login_at() {
        let _guard = setup_test_db();

        let username = unique_username("lastlogin");
        let register_response = register(
            RawUsername::new(username.clone()),
            RawPlaintextPassword::new("SecurePass123".to_string()),
            current_legal_acknowledgement(),
        )
        .await
        .expect("register should succeed");

        assert_eq!(last_login_at_for_user(register_response.user.user_id), None);

        let before_login = chrono::Utc::now();
        let login_response = login(
            RawUsername::new(username),
            RawPlaintextPassword::new("SecurePass123".to_string()),
        )
        .await
        .expect("login should succeed");
        let after_login = chrono::Utc::now();

        let raw_last_login = last_login_at_for_user(login_response.user.user_id)
            .expect("successful login should record last_login_at");
        let last_login_at =
            parse_datetime(&raw_last_login).expect("last_login_at should be RFC3339");

        assert!(
            last_login_at >= before_login && last_login_at <= after_login,
            "last_login_at should fall within the login call window"
        );
        assert_eq!(
            login_response.user.updated_at, register_response.user.updated_at,
            "login must not mutate users.updated_at"
        );
        let persisted_user = get_user_by_id(&login_response.user.user_id)
            .expect("login user should still exist after successful login");
        assert_eq!(
            persisted_user.updated_at, register_response.user.updated_at,
            "login must not persist a users.updated_at mutation"
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_failed_login_leaves_last_login_unchanged() {
        let _guard = setup_test_db();

        let username = unique_username("lastloginfail");
        let register_response = register(
            RawUsername::new(username.clone()),
            RawPlaintextPassword::new("SecurePass123".to_string()),
            current_legal_acknowledgement(),
        )
        .await
        .expect("register should succeed");

        login(
            RawUsername::new(username.clone()),
            RawPlaintextPassword::new("SecurePass123".to_string()),
        )
        .await
        .expect("initial login should succeed");
        let last_login_after_success = last_login_at_for_user(register_response.user.user_id)
            .expect("successful login should record last_login_at");

        let result = login(
            RawUsername::new(username),
            RawPlaintextPassword::new("WrongPass123".to_string()),
        )
        .await;

        assert!(result.is_err(), "wrong password should fail login");
        assert_eq!(
            last_login_at_for_user(register_response.user.user_id),
            Some(last_login_after_success),
            "failed login must not update last_login_at"
        );
    }

    #[cfg(feature = "db-tests")]
    #[test]
    fn test_record_last_login_write_failure_is_fatal() {
        let _guard = setup_test_db();

        let drop_result: Result<(), session::AuthError> = crate::db::with_db_mut(|conn| {
            conn.execute("DROP TABLE users", [])?;
            Ok(())
        });
        drop_result.expect("test should be able to remove users table");

        let result = record_last_login(UserId::new(), chrono::Utc::now());

        assert!(result.is_err(), "last_login_at write failure must fail");
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_last_login_write_failure_removes_created_session() {
        let _guard = setup_test_db();

        let username = unique_username("lastloginfatal");
        let register_response = register(
            RawUsername::new(username.clone()),
            RawPlaintextPassword::new("SecurePass123".to_string()),
            current_legal_acknowledgement(),
        )
        .await
        .expect("register should succeed");
        let user_id = register_response.user.user_id;
        let sessions_before_login = session_count_for_user(user_id);

        let trigger_result: Result<(), session::AuthError> = crate::db::with_db_mut(|conn| {
            conn.execute(
                "CREATE TRIGGER fail_last_login_update \
                 BEFORE UPDATE OF last_login_at ON users \
                 BEGIN \
                     SELECT RAISE(ABORT, 'forced last_login_at failure'); \
                 END",
                [],
            )?;
            Ok(())
        });
        trigger_result.expect("trigger fixture should install");

        let result = login(
            RawUsername::new(username),
            RawPlaintextPassword::new("SecurePass123".to_string()),
        )
        .await;

        assert!(
            result.is_err(),
            "last_login_at write failure must fail login"
        );
        assert_eq!(
            session_count_for_user(user_id),
            sessions_before_login,
            "failed login must not leave the newly created session row"
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_with_wrong_password_fails() {
        let _guard = setup_test_db();

        // Register first
        let username = unique_username("wrongpassuser");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        register(
            RawUsername::new(username.clone()),
            password,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();

        // Then login with wrong password
        let username = RawUsername::new(username);
        let password = RawPlaintextPassword::new("WrongPass123".to_string());
        let result = login(username, password).await;

        assert!(result.is_err(), "Login with wrong password should fail");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid username or password"));
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_with_nonexistent_user_fails() {
        let _guard = setup_test_db();

        let username = unique_username("nonexistent");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let result = login(RawUsername::new(username), password).await;

        assert!(result.is_err(), "Login with nonexistent user should fail");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid username or password"));
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_with_empty_username_returns_validation_error() {
        let _guard = setup_test_db();

        let result = login(
            RawUsername::new(String::new()),
            RawPlaintextPassword::new("Whatever123".to_string()),
        )
        .await;

        match result {
            Err(err) if err.is_validation() => {
                assert!(err.first_field_error("username").is_some());
            }
            other => panic!("Expected validation error, got {:?}", other),
        }
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_returns_same_user_each_time() {
        let _guard = setup_test_db();

        // Register
        let username = unique_username("multilogin");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        register(
            RawUsername::new(username.clone()),
            password,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();

        // Login twice
        let username1 = RawUsername::new(username.clone());
        let password1 = RawPlaintextPassword::new("SecurePass123".to_string());
        let response1 = login(username1, password1).await.unwrap();

        let username2 = RawUsername::new(username);
        let password2 = RawPlaintextPassword::new("SecurePass123".to_string());
        let response2 = login(username2, password2).await.unwrap();

        assert_eq!(
            response1.user.user_id, response2.user.user_id,
            "Each login should return the same user"
        );
    }

    // ============ User Data Tests ============

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_returns_valid_timestamps() {
        let _guard = setup_test_db();

        let username = unique_username("timestampuser");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let before_register = chrono::Utc::now();

        let response = register(
            RawUsername::new(username),
            password,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();
        let after_register = chrono::Utc::now();

        assert!(
            response.user.created_at >= before_register
                && response.user.created_at <= after_register,
            "created_at should fall within the registration call window"
        );
        assert!(
            response.user.updated_at >= before_register
                && response.user.updated_at <= after_register,
            "updated_at should fall within the registration call window"
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_register_leaves_last_login_null() {
        let _guard = setup_test_db();

        let username = unique_username("registerlastlogin");
        let response = register(
            RawUsername::new(username),
            RawPlaintextPassword::new("SecurePass123".to_string()),
            current_legal_acknowledgement(),
        )
        .await
        .expect("register should succeed");

        assert_eq!(
            last_login_at_for_user(response.user.user_id),
            None,
            "registration creates a session but must not record last_login_at"
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_returns_same_user_data_as_register() {
        let _guard = setup_test_db();

        let username = unique_username("sameuser");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let register_response = register(
            RawUsername::new(username.clone()),
            password,
            current_legal_acknowledgement(),
        )
        .await
        .unwrap();

        let username = RawUsername::new(username);
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let login_response = login(username, password).await.unwrap();

        assert_eq!(register_response.user.user_id, login_response.user.user_id);
        assert_eq!(
            register_response.user.username,
            login_response.user.username
        );
        assert_eq!(
            register_response.user.created_at,
            login_response.user.created_at
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_login_missing_envelope_returns_support_message() {
        let _guard = setup_test_db();

        let username = unique_username("missingenv");
        let password = RawPlaintextPassword::new("SecurePass123".to_string());

        let register_response = register(
            RawUsername::new(username.clone()),
            password.clone(),
            current_legal_acknowledgement(),
        )
        .await
        .expect("register should succeed");
        let envelope_path =
            crate::db::encryption::user_envelope_path(register_response.user.user_id)
                .expect("should resolve envelope path");
        std::fs::remove_file(&envelope_path).expect("should remove envelope file");

        let error = login(RawUsername::new(username), password)
            .await
            .expect_err("login should fail when envelope is missing");

        assert!(error.is_conflict(), "expected support-style conflict error");
        assert!(
            error
                .message
                .contains("encrypted database envelope file is missing")
        );
        assert!(
            error
                .message
                .contains(&register_response.user.user_id.to_string())
        );
        assert!(error.message.contains(&envelope_path.display().to_string()));
        assert!(error.message.contains(LOGIN_ENVELOPE_MISSING_SUPPORT_CODE));
    }
}

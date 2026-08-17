use dioxus::prelude::*;

#[cfg(feature = "server")]
use crate::db::{
    load_update_state, save_successful_update_check,
    set_update_check_enabled as db_set_update_check_enabled,
};
#[cfg(feature = "server")]
use crate::models::SessionToken;
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::{Duration, Utc};
#[cfg(feature = "server")]
use dioxus::logger::tracing;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateStatus {
    pub(crate) available: bool,
    pub(crate) latest: Option<String>,
    pub(crate) current: String,
    pub(crate) channel: String,
    pub(crate) release_url: Option<String>,
    pub(crate) update_check_enabled: bool,
    pub(crate) last_checked_at: Option<String>,
}

pub(crate) type UpdatesError = super::ApiErrorEnvelope;

#[cfg(feature = "server")]
fn parse_semver(raw: &str) -> Option<semver::Version> {
    let version = raw.strip_prefix('v').unwrap_or(raw);
    semver::Version::parse(version)
        .ok()
        .filter(|version| version.pre.is_empty())
}

#[cfg(feature = "server")]
fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

#[cfg(feature = "server")]
fn status_from_state(state: crate::db::AppUpdateState) -> UpdateStatus {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let available = state
        .latest_seen
        .as_deref()
        .is_some_and(|latest| is_newer_version(latest, &current));

    UpdateStatus {
        available,
        latest: state.latest_seen,
        current,
        channel: crate::channel::channel().as_header_value().to_string(),
        release_url: state.release_url,
        update_check_enabled: state.update_check_enabled,
        last_checked_at: state.last_checked_at.map(|dt| dt.to_rfc3339()),
    }
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> UpdatesError {
    tracing::error!(
        context,
        error = %detail,
        "updates: internal failure"
    );
    UpdatesError::internal()
}

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> UpdatesError {
    UpdatesError::unauthorized(message)
}

#[cfg(feature = "server")]
fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, UpdatesError> {
    super::session_token::lookup_session_token("updates", cookies)
        .ok_or_else(|| unauthorized_error("Invalid or expired session".to_string()))
}

#[cfg(feature = "server")]
fn session_user_id(cookies: &CookieJar) -> Result<crate::models::UserId, UpdatesError> {
    let session_token = session_token_from_cookie(cookies)?;
    crate::auth::session::get_session_by_token(&session_token)
        .map_err(|err| internal_error("get_session_by_token", err))?
        .map(|session| session.user_id)
        .ok_or_else(|| unauthorized_error("Invalid or expired session".to_string()))
}

#[get("/_app/updates/status", cookies: CookieJar)]
pub(crate) async fn update_status() -> Result<UpdateStatus, UpdatesError> {
    let _user_id = session_user_id(&cookies)?;
    let state = load_update_state().map_err(|err| internal_error("load_update_state", err))?;
    Ok(status_from_state(state))
}

#[post("/_app/updates/refresh", cookies: CookieJar)]
pub(crate) async fn refresh_update_status(force: bool) -> Result<UpdateStatus, UpdatesError> {
    let user_id = session_user_id(&cookies)?;
    let state = load_update_state().map_err(|err| internal_error("load_update_state", err))?;
    // The toggle only governs *automatic* checks. An explicit "Check now"
    // (force) is an intentional manual check and must run regardless — some
    // users keep automatic checks off and only check by hand.
    if !force && !state.update_check_enabled {
        return Ok(status_from_state(state));
    }

    if !force
        && state
            .last_checked_at
            .is_some_and(|checked_at| Utc::now() - checked_at < Duration::hours(24))
    {
        return Ok(status_from_state(state));
    }

    let client = crate::payments::client::BitGarthCentralClient::new(user_id);
    match client {
        Ok(client) => match client.latest_app_version().await {
            Ok(response) => {
                let crate::payments::client::LatestAppVersionResponse {
                    latest,
                    image: _image,
                    release_url,
                    published_at,
                } = response;
                if parse_semver(&latest).is_some() {
                    save_successful_update_check(
                        &latest,
                        &release_url,
                        published_at.as_deref(),
                        Utc::now(),
                    )
                    .map_err(|err| internal_error("save_successful_update_check", err))?;
                }
            }
            Err(err) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %err,
                    "updates: latest version refresh failed"
                );
            }
        },
        Err(err) => {
            tracing::warn!(
                user_id = %user_id,
                error = %err,
                "updates: Central client creation failed"
            );
        }
    }

    let state = load_update_state().map_err(|err| internal_error("reload_update_state", err))?;
    Ok(status_from_state(state))
}

#[post("/_app/updates/checks-enabled", cookies: CookieJar)]
pub(crate) async fn set_update_check_enabled(enabled: bool) -> Result<UpdateStatus, UpdatesError> {
    let _user_id = session_user_id(&cookies)?;
    db_set_update_check_enabled(enabled, Utc::now())
        .map_err(|err| internal_error("set_update_check_enabled", err))?;
    let state = load_update_state().map_err(|err| internal_error("load_update_state", err))?;
    Ok(status_from_state(state))
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_only_reports_newer_stable_versions() {
        assert!(is_newer_version("0.1.5", "0.1.4"));
        assert!(!is_newer_version("0.1.4", "0.1.4"));
        assert!(!is_newer_version("0.1.3", "0.1.4"));
        assert!(is_newer_version("v0.1.5", "0.1.4"));
        assert!(!is_newer_version("0.1.5-alpha.1", "0.1.4"));
        assert!(!is_newer_version("not-a-version", "0.1.4"));
    }
}

//! Session guard hook for detecting expired sessions in API responses.

use crate::backend::SettingsError;
use crate::{AuthState, AuthStatus, BannerMessage, BannerState};
use dioxus::logger::tracing;
use dioxus::prelude::*;

/// A guard that detects session expiry from API errors and updates app state accordingly.
///
/// When an Unauthorized error is detected:
/// 1. Shows an inline banner to the user
/// 2. Sets auth state to Unauthenticated (triggers redirect to login)
#[derive(Clone, Copy)]
pub(crate) struct SessionGuard {
    auth_state: AuthState,
    banner_state: BannerState,
}

impl SessionGuard {
    /// Check an API result for session expiry.
    ///
    /// If the result is an Unauthorized error, this will:
    /// - Set a banner message explaining the session expired
    /// - Update auth state to Unauthenticated
    ///
    /// This is a side-effecting function that modifies global state.
    pub(crate) fn check<T>(&mut self, result: Result<T, SettingsError>) {
        let user_id = authenticated_user_id(self.auth_state);

        match &result {
            Ok(_) => {
                tracing::debug!(
                    user_id = ?user_id,
                    "session guard: api call succeeded"
                );
            }
            Err(err) if err.is_unauthorized() => {
                handle_session_expired(self.auth_state, self.banner_state, "session guard");
            }
            Err(err) => {
                tracing::debug!(
                    user_id = ?user_id,
                    error = %err,
                    "session guard: api call failed with non-auth error"
                );
            }
        }
    }
}

fn authenticated_user_id(auth_state: AuthState) -> Option<crate::models::UserId> {
    match &*auth_state.read() {
        AuthStatus::Authenticated(auth) => Some(auth.user.user_id),
        _ => None,
    }
}

/// Marks the session expired after an unauthorized API response: signs the UI
/// out and, if the user was signed in, shows the session-expired banner.
pub(crate) fn handle_session_expired(
    mut auth_state: AuthState,
    mut banner_state: BannerState,
    context: &'static str,
) {
    let user_id = authenticated_user_id(auth_state);
    tracing::debug!(user_id = ?user_id, context, "ui: session expired");
    auth_state.set(AuthStatus::Unauthenticated);
    if user_id.is_some() {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

/// Hook that returns a SessionGuard for checking API results.
///
/// Usage:
/// ```rust
/// let guard = use_session_guard();
/// spawn(async move {
///     guard.check(save_language(new_locale).await);
/// });
/// ```
pub(crate) fn use_session_guard() -> SessionGuard {
    SessionGuard {
        auth_state: use_context::<AuthState>(),
        banner_state: use_context::<BannerState>(),
    }
}

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
        let (user_id, was_authenticated) = {
            let auth_snapshot = self.auth_state.read();
            match &*auth_snapshot {
                AuthStatus::Authenticated(auth) => (Some(auth.user.user_id), true),
                _ => (None, false),
            }
        };

        match &result {
            Ok(_) => {
                tracing::debug!(
                    user_id = ?user_id,
                    "session guard: api call succeeded"
                );
            }
            Err(err) if err.is_unauthorized() => {
                tracing::debug!(
                    user_id = ?user_id,
                    "session guard: unauthorized detected, marking session expired"
                );
                self.auth_state.set(AuthStatus::Unauthenticated);
                if was_authenticated {
                    self.banner_state.set(Some(BannerMessage::SessionExpired));
                }
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

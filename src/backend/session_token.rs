use crate::models::SessionToken;
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(all(feature = "server", feature = "desktop"))]
use dioxus::fullstack::FullstackContext;

/// Reads the session token from cookies and (on desktop) the in-memory session store.
/// This is a side-effect-free lookup that returns both the token and debug metadata.
#[cfg(feature = "server")]
pub(super) fn lookup_session_token(caller_name: &str, cookies: &CookieJar) -> Option<SessionToken> {
    let token_from_cookie = cookies
        .get(crate::auth::session::SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().trim().to_string())
        .filter(|value| !value.is_empty())
        .map(SessionToken::from_raw);

    let token_from_desktop = desktop_session_token();

    let token: Option<SessionToken> = token_from_cookie.clone().or(token_from_desktop.clone());

    tracing::debug!(
        cookie_present = token_from_cookie.is_some(),
        desktop_session_present = token_from_desktop.is_some(),
        caller = caller_name,
        "read session token"
    );

    token
}

#[cfg(feature = "server")]
fn desktop_session_token() -> Option<SessionToken> {
    #[cfg(feature = "desktop")]
    {
        if FullstackContext::current().is_some() {
            return None;
        }
        crate::desktop_session::get()
    }
    #[cfg(not(feature = "desktop"))]
    {
        None
    }
}

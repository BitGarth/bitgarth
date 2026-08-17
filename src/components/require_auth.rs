use crate::{AuthState, AuthStatus, Route};
use dioxus::prelude::*;

/// Layout wrapper that gates a subtree of routes behind a valid session.
///
/// Behavior:
/// - `Authenticated`: render the nested route via `Outlet::<Route>`.
/// - `Unauthenticated`: redirect to `/login` without rendering protected
///   content (no flash of authenticated UI).
/// - `Unknown` (still hydrating): render nothing. The root `App` resolves
///   auth via `use_server_future(me())` before this guard evaluates, so
///   under SSR the status is already `Authenticated` or `Unauthenticated`
///   when `RequireAuth` runs. The `Unknown` arm only matters during a
///   transient client-side refetch (e.g. after `restart()`), where
///   bouncing to `/login` would be wrong.
#[component]
pub fn RequireAuth() -> Element {
    let auth_state = use_context::<AuthState>();
    let navigator = use_navigator();

    match &*auth_state.read() {
        AuthStatus::Authenticated(_) => rsx! { Outlet::<Route> {} },
        AuthStatus::Unauthenticated => {
            navigator.replace(Route::Login);
            rsx! {}
        }
        AuthStatus::Unknown => rsx! {},
    }
}

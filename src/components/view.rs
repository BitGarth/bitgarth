use crate::{AuthEntryState, AuthState, AuthStatus, Route};
use dioxus::prelude::*;

#[component]
pub fn HomeView() -> Element {
    let auth_state = use_context::<AuthState>();
    let auth_entry = use_context::<AuthEntryState>();
    let navigator = use_navigator();

    if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
        {
            navigator.replace(Route::Wallets);
        }
        return rsx! {};
    }

    let mode = auth_entry.read().mode;
    rsx! {
        crate::components::AuthShell { mode, pairing_code: None }
    }
}

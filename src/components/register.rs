use crate::models::AuthEntryMode;
use crate::{AuthState, AuthStatus, Route};
use dioxus::prelude::*;

#[component]
pub fn Register() -> Element {
    let auth_state = use_context::<AuthState>();
    let navigator = use_navigator();

    if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
        {
            navigator.replace(Route::Wallets);
        }
        return rsx! {};
    }

    rsx! {
        crate::components::AuthShell { mode: AuthEntryMode::Register, pairing_code: None }
    }
}

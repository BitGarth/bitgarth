use crate::models::AuthEntryMode;
use crate::{AuthState, AuthStatus, Route};
use dioxus::prelude::*;

#[component]
pub fn Login() -> Element {
    let auth_state = use_context::<AuthState>();
    let navigator = use_navigator();

    if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
        navigator.replace(Route::Wallets);
        return rsx! {};
    }

    rsx! {
        crate::components::AuthShell { mode: AuthEntryMode::Login, pairing_code: None }
    }
}

#[component]
pub fn PairingLogin(code: String) -> Element {
    let auth_state = use_context::<AuthState>();
    let navigator = use_navigator();

    if matches!(&*auth_state.read(), AuthStatus::Authenticated(_)) {
        navigator.replace(Route::PairingApproval {
            code: Some(code.clone()),
        });
        return rsx! {};
    }

    rsx! {
        crate::components::AuthShell {
            key: "{code}",
            mode: AuthEntryMode::Login,
            pairing_code: Some(code),
        }
    }
}

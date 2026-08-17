use crate::models::AuthResponse;
#[cfg(feature = "web")]
use chrono::Utc;
#[cfg(feature = "web")]
use dioxus::logger::tracing;

#[cfg(feature = "web")]
const AUTH_STORAGE_KEY: &str = "bitgarth.auth";

#[cfg(feature = "web")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
}

#[cfg(feature = "web")]
pub fn load_auth() -> Option<AuthResponse> {
    let storage = storage()?;
    let raw = match storage.get_item(AUTH_STORAGE_KEY) {
        Ok(Some(value)) => value,
        Ok(None) => {
            tracing::debug!(
                "auth persistence: no stored auth state"
            );
            return None;
        }
        Err(err) => {
            tracing::warn!("auth persistence: failed to read localStorage: {:?}", err);
            return None;
        }
    };

    match serde_json::from_str::<AuthResponse>(&raw) {
        Ok(auth) => {
            tracing::debug!(
                user_id = %auth.user.user_id,
                "auth persistence: loaded auth state"
            );
            Some(auth)
        }
        Err(err) => {
            tracing::warn!("auth persistence: failed to parse auth state: {}", err);
            None
        }
    }
}

#[cfg(feature = "web")]
pub fn save_auth(auth: &AuthResponse) {
    let storage = match storage() {
        Some(storage) => storage,
        None => {
            tracing::debug!(
                user_id = %auth.user.user_id,
                username = %auth.user.username,
                "auth persistence: no localStorage available for save"
            );
            return;
        }
    };

    let payload = match serde_json::to_string(auth) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!("auth persistence: failed to serialize auth state: {}", err);
            return;
        }
    };

    if let Err(err) = storage.set_item(AUTH_STORAGE_KEY, &payload) {
        tracing::warn!("auth persistence: failed to write localStorage: {:?}", err);
    } else {
        tracing::debug!(
            user_id = %auth.user.user_id,
            username = %auth.user.username,
            "auth persistence: saved auth state"
        );
    }
}

#[cfg(feature = "web")]
pub fn clear_auth() {
    if let Some(storage) = storage() {
        if let Err(err) = storage.remove_item(AUTH_STORAGE_KEY) {
            tracing::warn!("auth persistence: failed to clear localStorage: {:?}", err);
        } else {
            tracing::debug!(
                "auth persistence: cleared auth state"
            );
        }
    } else {
        tracing::debug!(
            "auth persistence: no localStorage available for clear"
        );
    }
}

#[cfg(not(feature = "web"))]
pub fn load_auth() -> Option<AuthResponse> {
    None
}

#[cfg(not(feature = "web"))]
pub fn save_auth(_auth: &AuthResponse) {}

#[cfg(not(feature = "web"))]
pub fn clear_auth() {}

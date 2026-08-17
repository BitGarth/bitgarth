use crate::models::SessionToken;
use dioxus::logger::tracing;
use std::sync::{OnceLock, RwLock};

static SESSION_TOKEN: OnceLock<RwLock<Option<SessionToken>>> = OnceLock::new();

fn storage() -> &'static RwLock<Option<SessionToken>> {
    SESSION_TOKEN.get_or_init(|| RwLock::new(None))
}

pub(crate) fn set(token: SessionToken) {
    let mut guard = match storage().write() {
        Ok(guard) => guard,
        Err(err) => {
            tracing::warn!("desktop session: token lock poisoned while writing");
            err.into_inner()
        }
    };
    *guard = Some(token);
    tracing::debug!("desktop session: stored session token in memory");
}

pub(crate) fn clear() {
    let mut guard = match storage().write() {
        Ok(guard) => guard,
        Err(err) => {
            tracing::warn!("desktop session: token lock poisoned while clearing");
            err.into_inner()
        }
    };
    *guard = None;
    tracing::debug!("desktop session: cleared session token from memory");
}

pub(crate) fn get() -> Option<SessionToken> {
    match storage().read() {
        Ok(guard) => guard.clone(),
        Err(err) => {
            tracing::warn!("desktop session: token lock poisoned while reading");
            err.into_inner().clone()
        }
    }
}

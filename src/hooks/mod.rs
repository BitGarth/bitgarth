//! Application hooks for common patterns.

mod use_session_guard;

pub(crate) use use_session_guard::{handle_session_expired, use_session_guard};

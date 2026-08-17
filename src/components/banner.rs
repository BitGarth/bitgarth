use crate::{BannerMessage, BannerSeverity, BannerState};
use dioxus::prelude::*;

#[component]
pub fn Banner() -> Element {
    let banner_state = use_context::<BannerState>();
    let Some(message) = banner_state.read().clone() else {
        return rsx! {};
    };

    let is_session_expired = matches!(message, BannerMessage::SessionExpired);

    let (severity, text) = match message {
        BannerMessage::SessionExpired => (
            BannerSeverity::Warning,
            "Your session has expired. Please log in again.".to_string(),
        ),
        BannerMessage::DatabaseUnavailable => (
            BannerSeverity::Error,
            "The server is unable to connect to the database. Please wait a minute and try again."
                .to_string(),
        ),
        BannerMessage::Custom { severity, text } => (severity, text),
    };

    let class = match severity {
        BannerSeverity::Error => "banner banner-error",
        BannerSeverity::Warning => "banner banner-warning",
        BannerSeverity::Info => "banner banner-info",
    };

    rsx! {
        div { class: "{class}",
            "{text}"
            if is_session_expired {
                " Redirecting to login\u{2026}"
            }
        }
    }
}

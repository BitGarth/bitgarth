use crate::{AuthState, AuthStatus, Route};
use dioxus::prelude::*;

fn missing_path(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

#[component]
pub fn NotFound(segments: crate::route_params::UnmatchedSegments) -> Element {
    #[cfg(feature = "server")]
    dioxus::fullstack::FullstackContext::commit_http_status(
        dioxus::fullstack::StatusCode::NOT_FOUND,
        None,
    );

    let signed_in = matches!(
        *use_context::<AuthState>().read(),
        AuthStatus::Authenticated(_)
    );
    let path = missing_path(&segments.0);

    rsx! {
        div { class: "not-found-shell",
            div { class: "not-found-card",
                p { class: "not-found-label", "Error 404" }
                h1 { class: "not-found-title", "Page not found" }
                p { class: "not-found-subtitle", "The page you requested does not exist." }
                p { class: "not-found-path",
                    "Requested path "
                    code { "{path}" }
                }
                div { class: "not-found-actions",
                    if signed_in {
                        Link { class: "btn btn-primary", to: Route::Wallets, "Go to Wallets" }
                    } else {
                        Link { class: "btn btn-primary", to: Route::HomeView, "Go home" }
                        Link { class: "btn btn-secondary", to: Route::Login, "Open login" }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn missing_path_joins_segments() {
        let path = missing_path(&["settings".to_string(), "theme".to_string()]);
        assert_eq!(path, "/settings/theme");
    }

    #[test]
    fn missing_path_empty_segments() {
        assert_eq!(missing_path(&[]), "/");
    }
}

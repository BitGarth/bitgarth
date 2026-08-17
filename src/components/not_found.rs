use crate::Route;
use dioxus::prelude::*;

fn missing_path(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    #[cfg(feature = "server")]
    dioxus::fullstack::FullstackContext::commit_http_status(
        dioxus::fullstack::StatusCode::NOT_FOUND,
        None,
    );

    let navigator = use_navigator();
    let path = missing_path(&segments);

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
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            navigator.push(Route::HomeView);
                        },
                        "Go home"
                    }
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| {
                            navigator.push(Route::Login);
                        },
                        "Open login"
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

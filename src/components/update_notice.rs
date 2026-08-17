use dioxus::prelude::*;

/// Shared signal: `Some(server_build)` when the client has drifted from the
/// server, `None` otherwise. Provided by `App`, set by `BuildDriftWatcher`.
///
/// Newtype, not a bare `Signal<Option<String>>` alias: Dioxus keys context by
/// concrete type, and `InstanceNoticeState` is also `Signal<Option<String>>`.
/// A shared alias collides into one context slot, so the drift watcher would
/// null the instance-notice banner. The wrapper gives it a distinct `TypeId`.
#[derive(Clone, Copy)]
pub(crate) struct BuildDriftState(pub(crate) Signal<Option<String>>);

fn dismiss_update_notice(mut state: BuildDriftState) {
    state.0.set(None);
}

#[cfg(any(
    all(feature = "web", target_arch = "wasm32"),
    all(test, feature = "server", not(bitgarth_db_unit_only))
))]
fn apply_build_drift_result(mut state: BuildDriftState, loaded: &str, server: &str) {
    if crate::version::is_drifted(loaded, server) {
        state.0.set(Some(server.to_string()));
    } else if state.0.peek().is_some() {
        state.0.set(None);
    }
}

/// Quiet parchment notice strip shown when the loaded client build differs
/// from the running server build. Botanical Ledger `.instance-notice` family,
/// extended to be actionable (Reload) and dismissible.
#[component]
pub(crate) fn UpdateNotice() -> Element {
    let state = use_context::<BuildDriftState>();
    let server_build = state.0.read().clone();
    let Some(server_build) = server_build else {
        return rsx! {};
    };
    if !crate::version::is_drifted(crate::version::version(), &server_build) {
        return rsx! {};
    }

    let server_label = crate::version::format_build(&server_build);
    let client_label = crate::version::format_build(crate::version::version());

    rsx! {
        div {
            class: "update-notice",
            role: "status",
            "aria-live": "polite",
            "aria-label": "A new version is available",
            div { class: "update-notice-body",
                span { class: "update-notice-eyebrow", "Update" }
                p { class: "update-notice-message",
                    "Server is on "
                    code { class: "update-notice-build update-notice-build-server", "{server_label}" }
                    ". This tab is on "
                    code { class: "update-notice-build update-notice-build-client", "{client_label}" }
                    ". Reload to update."
                }
            }
            div { class: "update-notice-actions",
                button {
                    class: "btn",
                    r#type: "button",
                    onclick: move |_| {
                        let _ = dioxus::document::eval("window.location.reload();");
                    },
                    "Reload"
                }
                button {
                    class: "update-notice-dismiss",
                    r#type: "button",
                    "aria-label": "Dismiss",
                    onclick: move |_| dismiss_update_notice(state),
                    "\u{00d7}"
                }
            }
        }
    }
}

/// Web-only: polls the server build on window-focus and route-change and
/// updates `BuildDriftState`. Renders nothing.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
#[component]
pub(crate) fn BuildDriftWatcher() -> Element {
    let state = use_context::<BuildDriftState>();

    // Re-runs on every in-app navigation (NavBar provides route context).
    let route = use_route::<crate::Route>();

    // Check now (covers navigation) and whenever the tab regains focus.
    use_effect(use_reactive((&route,), move |_| {
        spawn(async move {
            update_build_drift(state).await;
        });
    }));

    // Focus / visibility listener: fires the same check when the user returns.
    use_effect(move || {
        spawn(async move {
            let mut listener = dioxus::document::eval(
                r#"
                const fire = () => dioxus.send("focus");
                window.addEventListener("focus", fire);
                document.addEventListener("visibilitychange", () => {
                    if (document.visibilityState === "visible") fire();
                });
                "#,
            );
            while listener.recv::<serde_json::Value>().await.is_ok() {
                update_build_drift(state).await;
            }
        });
    });

    rsx! {}
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
async fn update_build_drift(state: BuildDriftState) {
    if let Some(server) = fetch_server_build().await {
        apply_build_drift_result(state, crate::version::version(), &server);
    }
}

/// Fetch the server build with the browser cache bypassed. Returns `None` on
/// any transport error (the drift check stays silent on failure).
#[cfg(all(feature = "web", target_arch = "wasm32"))]
async fn fetch_server_build() -> Option<String> {
    let mut eval = dioxus::document::eval(
        r#"
        try {
            const r = await fetch("/api/v1/build", {
                cache: "no-store",
                credentials: "same-origin",
            });
            if (!r.ok) { dioxus.send(null); return; }
            dioxus.send(await r.text());
        } catch (e) {
            dioxus.send(null);
        }
        "#,
    );
    match eval.recv::<serde_json::Value>().await {
        Ok(serde_json::Value::String(build)) if !build.trim().is_empty() => {
            Some(build.trim().to_string())
        }
        _ => None,
    }
}

/// Non-browser targets never drift (server is in-process). No-op.
#[cfg(not(all(feature = "web", target_arch = "wasm32")))]
#[component]
pub(crate) fn BuildDriftWatcher() -> Element {
    rsx! {}
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::{apply_build_drift_result, dismiss_update_notice};
    use crate::components::{BuildDriftState, UpdateNotice};
    use dioxus::prelude::*;

    fn render(initial: Option<String>) -> String {
        #[component]
        fn Wrapper(initial: Option<String>) -> Element {
            let state = BuildDriftState(use_signal(|| initial.clone()));
            use_context_provider(|| state);
            rsx! { UpdateNotice {} }
        }
        let mut dom = VirtualDom::new_with_props(Wrapper, WrapperProps { initial });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn render_after_dismiss(initial: Option<String>) -> String {
        #[component]
        fn Wrapper(initial: Option<String>) -> Element {
            let state = BuildDriftState(use_signal(|| initial.clone()));
            dismiss_update_notice(state);
            use_context_provider(|| state);
            rsx! { UpdateNotice {} }
        }
        let mut dom = VirtualDom::new_with_props(Wrapper, WrapperProps { initial });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn render_after_build_result(
        initial: Option<String>,
        loaded: String,
        server: String,
    ) -> String {
        #[component]
        fn Wrapper(initial: Option<String>, loaded: String, server: String) -> Element {
            let state = BuildDriftState(use_signal(|| initial.clone()));
            apply_build_drift_result(state, &loaded, &server);
            let cleared = state.0.read().is_none();
            rsx! {
                if cleared {
                    span { "cleared" }
                } else {
                    span { "drifted" }
                }
            }
        }
        let mut dom = VirtualDom::new_with_props(
            Wrapper,
            WrapperProps {
                initial,
                loaded,
                server,
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn notice_absent_when_state_is_none() {
        let rendered = render(None);
        assert!(!rendered.contains("update-notice"));
    }

    #[test]
    fn notice_present_and_accessible_when_drifted() {
        let rendered = render(Some("0.1.6-def5678".to_string()));
        assert!(rendered.contains(r#"class="update-notice""#));
        assert!(rendered.contains(r#"role="status""#));
        assert!(rendered.contains(r#"aria-live="polite""#));
        assert!(rendered.contains(r#"aria-label="A new version is available""#));
    }

    #[test]
    fn notice_shows_server_build_and_reload_action() {
        let rendered = render(Some("0.1.6-def5678".to_string()));
        let client_label = crate::version::format_build(crate::version::version());
        assert!(rendered.contains("0.1.6 (def5678)"));
        assert!(rendered.contains(&client_label));
        assert!(rendered.contains("Reload"));
        assert!(rendered.contains(r#"aria-label="Dismiss""#));
        assert!(rendered.matches(r#"type="button""#).count() >= 2);
    }

    #[test]
    fn dismiss_helper_clears_signal() {
        let rendered = render_after_dismiss(Some("0.1.6-def5678".to_string()));
        assert!(!rendered.contains("update-notice"));
    }

    #[test]
    fn build_result_helper_clears_stale_drift_when_server_matches_loaded() {
        let loaded = crate::version::version().to_string();
        let rendered =
            render_after_build_result(Some("0.1.6-def5678".to_string()), loaded.clone(), loaded);
        assert!(rendered.contains("cleared"));
        assert!(!rendered.contains("drifted"));
    }
}

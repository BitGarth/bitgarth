use dioxus::document::eval;
use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToastLevel {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToastMessage {
    pub id: u64,
    pub level: ToastLevel,
    pub text: String,
}

/// Shared signal holding the active toast list. Provide via `use_context_provider` in App.
pub(crate) type ToastState = Signal<Vec<ToastMessage>>;

static NEXT_TOAST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Push a new toast into the shared state. Auto-dismisses after ~4 seconds.
pub(crate) fn push_toast(mut state: ToastState, level: ToastLevel, text: String) {
    let id = NEXT_TOAST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    state.write().push(ToastMessage { id, level, text });

    // Schedule auto-dismiss using JS setTimeout (same pattern as banner.rs)
    spawn(async move {
        let mut timer = eval(r#"setTimeout(() => { dioxus.send(null); }, 4000);"#);
        let _ = timer.recv::<serde_json::Value>().await;
        state.write().retain(|t| t.id != id);
    });
}

fn dismiss_toast(mut state: ToastState, id: u64) {
    state.write().retain(|t| t.id != id);
}

#[component]
pub(crate) fn ToastContainer() -> Element {
    let state = use_context::<ToastState>();
    let toasts = state.read();

    if toasts.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "toast-container", role: "status", "aria-live": "polite",
            for toast in toasts.iter() {
                {
                    let toast_class = match toast.level {
                        ToastLevel::Success => "toast toast-success",
                        ToastLevel::Error => "toast toast-error",
                        ToastLevel::Info => "toast toast-info",
                    };
                    let toast_id = toast.id;

                    rsx! {
                        div { class: "{toast_class}", key: "{toast_id}",
                            span { "{toast.text}" }
                            button {
                                class: "toast-dismiss",
                                r#type: "button",
                                "aria-label": "Dismiss",
                                onclick: move |_| dismiss_toast(state, toast_id),
                                "\u{00d7}"
                            }
                        }
                    }
                }
            }
        }
    }
}

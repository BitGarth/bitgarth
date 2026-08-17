use crate::Route;
use dioxus::prelude::*;
use wasm_bindgen::JsCast;

#[component]
pub fn CommandPalette() -> Element {
    let mut is_open = use_signal(|| false);
    let mut search_query = use_signal(String::new);
    let navigator = use_navigator();

    // Handle global keyboard shortcuts
    let _is_open_handle = is_open;
    use_effect(move || {
        let mut is_open_handle = _is_open_handle;
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
                move |event: web_sys::KeyboardEvent| {
                    if (event.meta_key() || event.ctrl_key()) && event.key() == "k" {
                        event.prevent_default();
                        is_open_handle.set(true);
                    } else if event.key() == "Escape" && is_open_handle() {
                        is_open_handle.set(false);
                    }
                },
            ) as Box<dyn FnMut(_)>);

            let _ = document
                .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());

            closure.forget(); // Leak the closure to keep it alive (for simplicity in this example)
        }
    });

    let items = vec![
        ("Wallets", Route::Wallets {}),
        ("Settings", Route::Settings { section: None }),
    ];

    let filtered_items: Vec<_> = items
        .into_iter()
        .filter(|(name, _)| name.to_lowercase().contains(&search_query().to_lowercase()))
        .collect();

    if !is_open() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-start justify-center pt-20 sm:pt-24",
            div {
                class: "fixed inset-0 bg-black/50 backdrop-blur-sm transition-opacity",
                onclick: move |_| is_open.set(false),
            }
            div {
                class: "relative w-full max-w-lg transform overflow-hidden rounded-xl bg-white dark:bg-gray-900 shadow-2xl ring-1 ring-black/5 transition-all",
                div {
                    class: "relative",
                    input {
                        class: "h-14 w-full border-0 bg-transparent pl-4 pr-4 text-gray-900 dark:text-gray-100 placeholder:text-gray-400 focus:ring-0 sm:text-sm",
                        placeholder: "Search pages...",
                        value: "{search_query}",
                        oninput: move |e| search_query.set(e.value()),
                        onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                    }
                }
                if filtered_items.is_empty() {
                    div {
                        class: "px-4 py-14 text-center text-sm sm:px-14",
                        p { class: "mt-4 text-gray-900 dark:text-gray-100", "No results found." }
                    }
                } else {
                    ul {
                        class: "max-h-80 scroll-py-2 divide-y divide-gray-100 dark:divide-gray-800 overflow-y-auto",
                        for (name, route) in filtered_items {
                            li {
                                button {
                                    class: "w-full cursor-default select-none px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-800 text-left text-sm text-gray-900 dark:text-gray-100",
                                    onclick: move |_| {
                                        is_open.set(false);
                                        search_query.set(String::new());
                                        navigator.push(route.clone());
                                    },
                                    "{name}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

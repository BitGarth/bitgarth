use dioxus::prelude::*;

use super::{EyeIcon, EyeOffIcon};

/// A password input with a show/hide eye toggle. The toggle is per-field
/// local state — it does not persist across page reloads or auto-hide on
/// blur. Browser autofill / password manager integration is preserved
/// because the underlying `<input>` keeps its `autocomplete` attribute.
#[component]
pub fn PasswordInput(
    id: String,
    value: Signal<String>,
    placeholder: String,
    autocomplete: &'static str,
    #[props(default)] has_error: bool,
    #[props(default)] disabled: bool,
    #[props(default)] autofocus: bool,
    #[props(default)] on_change: EventHandler<String>,
) -> Element {
    let mut visible = use_signal(|| false);
    let input_class = if has_error {
        "form-input input-error"
    } else {
        "form-input"
    };
    let input_type = if visible() { "text" } else { "password" };
    let toggle_label = if visible() {
        "Hide password"
    } else {
        "Show password"
    };

    rsx! {
        div { class: "password-input-row",
            input {
                class: input_class,
                r#type: input_type,
                id: "{id}",
                "data-testid": id,
                placeholder: "{placeholder}",
                autocomplete: autocomplete,
                value: "{value}",
                disabled: disabled,
                oninput: move |evt| {
                    let new_value = evt.value();
                    let mut value = value;
                    value.set(new_value.clone());
                    on_change.call(new_value);
                },
                onmounted: move |e| async move {
                    if autofocus {
                        let _ = e.set_focus(true).await;
                    }
                },
            }
            button {
                r#type: "button",
                class: "password-toggle-btn",
                tabindex: "-1",
                "aria-label": toggle_label,
                "aria-pressed": visible(),
                disabled: disabled,
                onclick: move |_| visible.set(!visible()),
                if visible() { EyeOffIcon {} } else { EyeIcon {} }
            }
        }
    }
}

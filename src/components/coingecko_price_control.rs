use dioxus::prelude::*;

use crate::backend::{save_currency, set_price_fetching_enabled};
use crate::hooks::use_session_guard;
use crate::models::CurrencyCode;
use crate::settings::{SettingsState, common_currencies};

use super::{ToastLevel, ToastState, push_toast};

/// Navbar control for CoinGecko price fetching. It shares SettingsState with
/// the Settings page so both surfaces stay in sync.
#[component]
pub(crate) fn CoinGeckoPriceControl() -> Element {
    let settings_state = use_context::<SettingsState>();
    let toast_state = use_context::<ToastState>();
    let guard = use_session_guard();
    let mut enabled = settings_state.price_fetching_enabled;
    let mut currency = settings_state.currency;
    let mut saving = use_signal(|| false);
    let mut currency_saving = use_signal(|| false);
    let currency_options = common_currencies();

    let toggle = move |_| {
        if saving() {
            return;
        }

        let next = !enabled();
        saving.set(true);

        spawn(async move {
            match set_price_fetching_enabled(next).await {
                Ok(saved) => enabled.set(saved),
                Err(err) if err.is_unauthorized() => {
                    let mut guard = guard;
                    guard.check::<bool>(Err(err));
                }
                Err(_) => {
                    push_toast(
                        toast_state,
                        ToastLevel::Error,
                        "Couldn't update CoinGecko prices - try again.".to_string(),
                    );
                }
            }
            saving.set(false);
        });
    };

    let on_currency = move |evt: Event<FormData>| {
        if currency_saving() {
            return;
        }

        let Some(new_currency) = CurrencyCode::from_code(&evt.value()) else {
            return;
        };

        currency_saving.set(true);

        spawn(async move {
            match save_currency(new_currency).await {
                Ok(()) => {
                    currency.set(new_currency);
                }
                Err(err) if err.is_unauthorized() => {
                    currency_saving.set(false);
                    let mut guard = guard;
                    guard.check::<()>(Err(err));
                }
                Err(_) => {
                    push_toast(
                        toast_state,
                        ToastLevel::Error,
                        "Couldn't change currency - try again.".to_string(),
                    );
                }
            }
            currency_saving.set(false);
        });
    };

    rsx! {
        div {
            class: "nav-price-control",
            "data-testid": "nav-price-control",
            title: "When enabled, BitGarth requests prices for your assets and selected currency.",
            label { class: "nav-price-switch",
                input {
                    "data-testid": "nav-price-fetching-toggle",
                    r#type: "checkbox",
                    checked: enabled(),
                    disabled: saving(),
                    onchange: toggle,
                    "aria-label": "Show CoinGecko prices",
                }
                span { class: "nav-price-glyph", "$" }
                span { class: "nav-price-label", "CoinGecko Prices" }
                span { class: "nav-price-track",
                    span { class: "nav-price-knob" }
                }
            }
            if enabled() {
                select {
                    class: "nav-price-currency",
                    "data-testid": "nav-price-currency-select",
                    "aria-label": "Select display currency",
                    disabled: currency_saving(),
                    value: "{currency.read().code()}",
                    onchange: on_currency,
                    for option_currency in currency_options.iter().copied() {
                        option {
                            value: "{option_currency.code()}",
                            selected: currency() == option_currency,
                            "{option_currency.code()}"
                        }
                    }
                }
            }
        }
    }
}

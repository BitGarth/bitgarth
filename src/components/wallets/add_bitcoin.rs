use super::helpers::{
    handle_session_expired, prevalidate_bitcoin_address_input,
    prevalidate_wallet_label_for_new_wallet,
};
use super::wallet_dropdown::{
    AccountNameField, WalletChoice, WalletDropdown, initial_wallet_dropdown_choice,
    wallet_options_for_dropdown,
};
use crate::backend::{WalletView, add_bitcoin_address, get_wallets};
use crate::components::form_helpers::{
    begin_submit, finish_submit, first_matching_field_error, is_form_field_error,
    primary_field_or_message,
};
use crate::components::{ToastLevel, ToastState, push_toast};
use crate::wallets::{AddBtcAddressRequest, Network, RawBtcAddress, RawLabel, WalletId};
use crate::{AuthState, BannerState};
use dioxus::prelude::*;

#[component]
pub(crate) fn AddBitcoinAddressFlow(
    default_wallet_id: Option<WalletId>,
    on_complete: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let toast_state = use_context::<ToastState>();
    let mut address_input = use_signal(String::new);
    let mut wallet_choice = use_signal(|| None::<WalletChoice>);
    let mut wallet_label_input = use_signal(String::new);
    let mut wallet_label_error = use_signal(|| None::<String>);
    let mut account_label_input = use_signal(String::new);
    let mut account_label_error = use_signal(|| None::<String>);
    let mut field_error = use_signal(|| None::<String>);
    let mut save_error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);

    // Load wallets for the picker
    let wallets_resource = use_resource(move || async move { get_wallets().await });

    let wallets: Vec<WalletView> = wallets_resource
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map_or_else(Vec::new, |r| r.wallets.clone());
    let wallet_options = wallet_options_for_dropdown(&wallets, default_wallet_id);
    let wallets_loading = wallets_resource.value().read().is_none();
    if !wallets_loading && wallet_choice.peek().is_none() {
        wallet_choice.set(Some(initial_wallet_dropdown_choice(
            default_wallet_id,
            None,
            wallet_options.len(),
        )));
    }

    let save = move |_| {
        if !begin_submit(saving) {
            return;
        }

        field_error.set(None);
        wallet_label_error.set(None);
        account_label_error.set(None);
        save_error.set(None);

        let address_raw = address_input().trim().to_string();
        if let Err(err) = prevalidate_bitcoin_address_input(&address_raw) {
            field_error.set(Some(err));
            finish_submit(saving);
            return;
        }

        let choice = match wallet_choice() {
            Some(choice) => choice,
            None => {
                field_error.set(Some("Wallets are still loading.".to_string()));
                finish_submit(saving);
                return;
            }
        };
        let wallet_id = match choice {
            WalletChoice::Existing(id) => Some(id),
            WalletChoice::Unselected => {
                field_error.set(Some(
                    "Select an existing wallet or create a new one".to_string(),
                ));
                finish_submit(saving);
                return;
            }
            WalletChoice::CreateNew => None,
        };

        let wallet_label_raw = wallet_label_input().trim().to_string();
        if wallet_id.is_none()
            && let Err(err) = prevalidate_wallet_label_for_new_wallet(&wallet_label_raw)
        {
            wallet_label_error.set(Some(err));
            finish_submit(saving);
            return;
        }

        let wallet_label = if wallet_id.is_none() {
            Some(RawLabel::new(wallet_label_raw))
        } else {
            None
        };

        let account_label_raw = account_label_input().trim().to_string();
        let account_label = if account_label_raw.is_empty() {
            None
        } else {
            Some(RawLabel::new(account_label_raw))
        };

        let request = AddBtcAddressRequest {
            address: RawBtcAddress::new(address_raw),
            network: Network::Mainnet,
            wallet_id,
            wallet_label,
            account_label,
        };

        spawn(async move {
            match add_bitcoin_address(request).await {
                Ok(response) => {
                    finish_submit(saving);
                    if let Some(notice) = response.account_limit_notice {
                        push_toast(toast_state, ToastLevel::Info, notice.message);
                    }
                    on_complete.call(());
                }
                Err(err) if err.is_unauthorized() => {
                    finish_submit(saving);
                    handle_session_expired(auth_state, banner_state, "add bitcoin address");
                }
                Err(err) if is_form_field_error(&err) => {
                    finish_submit(saving);
                    if let Some(message) = first_matching_field_error(&err, &["wallet_label"]) {
                        wallet_label_error.set(Some(message));
                    } else if let Some(message) =
                        first_matching_field_error(&err, &["account_label", "label"])
                    {
                        account_label_error.set(Some(message));
                    } else {
                        let message = primary_field_or_message(&err, &["address", "wallet_label"]);
                        field_error.set(Some(message));
                    }
                }
                Err(err) => {
                    finish_submit(saving);
                    save_error.set(Some(err.to_string()));
                }
            }
        });
    };

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "Add Bitcoin Address" }
                }
                div { class: "modal-body",
                    div { class: "flow-step",
                        p { class: "muted",
                            "Add a watch-only Bitcoin address to a new or existing wallet. "
                            "Useful for paper wallets or individual addresses."
                        }

                        label { class: "form-label", "Bitcoin address" }
                        input {
                            class: "xpub-input",
                            r#type: "text",
                            autocomplete: "off",
                            placeholder: "bc1q... / 1... / 3...",
                            value: "{address_input}",
                            oninput: move |e| address_input.set(e.value()),
                            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                        }

                        if wallets_loading {
                            label { class: "form-label", "Wallet" }
                            select {
                                class: "selector",
                                disabled: true,
                                option {
                                    value: super::CREATE_NEW_WALLET_OPTION_VALUE,
                                    "Loading wallets..."
                                }
                            }
                        } else if let Some(choice) = wallet_choice() {
                            WalletDropdown {
                                wallets: wallet_options.clone(),
                                choice,
                                default_wallet_id,
                                pinned_wallet: None,
                                new_wallet_label: wallet_label_input(),
                                wallet_label_error: wallet_label_error(),
                                on_choice_change: move |choice| {
                                    wallet_choice.set(Some(choice));
                                    field_error.set(None);
                                    wallet_label_error.set(None);
                                },
                                on_new_wallet_label_change: move |value| {
                                    wallet_label_input.set(value);
                                    wallet_label_error.set(None);
                                },
                            }
                        }

                        AccountNameField {
                            value: account_label_input(),
                            placeholder: "Bitcoin Account 1".to_string(),
                            error: account_label_error(),
                            on_input: move |value| {
                                account_label_input.set(value);
                                account_label_error.set(None);
                            },
                        }

                        if let Some(error) = field_error() {
                            div { class: "alert alert-error",
                                strong { "Validation error: " }
                                "{error}"
                            }
                        }

                        if let Some(error) = save_error() {
                            div { class: "alert alert-error",
                                strong { "Error: " }
                                "{error}"
                            }
                        }

                        div { class: "modal-actions",
                            button {
                                class: "btn btn-secondary",
                                disabled: saving(),
                                onclick: move |_| on_cancel.call(()),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: saving() || wallets_loading || wallet_choice().is_none(),
                                onclick: save,
                                if saving() { "Adding..." } else { "Add Address" }
                            }
                        }
                    }
                }
            }
        }
    }
}

use super::helpers::{
    XpubDefaultSchemeInput, address_scheme_label, handle_session_expired,
    prevalidate_wallet_label_for_new_wallet, prevalidate_xpub_input, select_default_xpub_scheme,
};
use super::wallet_dropdown::{
    AccountNameField, PinnedWallet, WalletChoice, WalletDropdown, initial_wallet_dropdown_choice,
    wallet_options_for_dropdown,
};
use crate::backend::{ValidateXpubResponse, WalletView, add_xpub, get_wallets, validate_xpub};
use crate::components::form_helpers::{
    first_matching_field_error, is_form_field_error, primary_field_or_message,
};
use crate::components::{ToastLevel, ToastState, push_toast};
use crate::wallets::{AddXpubRequest, AddressScheme, RawLabel, ValidateXpubRequest, WalletId};
use crate::{AuthState, BannerState};
use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub(super) enum AddXpubStep {
    Input,
    Validating,
    Results,
    Saving,
}

#[component]
pub(crate) fn AddXpubFlow(
    default_wallet_id: Option<WalletId>,
    on_complete: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut step = use_signal(|| AddXpubStep::Input);
    let mut xpub_input = use_signal(String::new);
    let validation_result = use_signal(|| None::<ValidateXpubResponse>);
    let mut validation_error = use_signal(|| None::<String>);
    let mut selected_scheme = use_signal(|| AddressScheme::Legacy);
    let mut wallet_choice = use_signal(|| None::<WalletChoice>);
    let mut wallet_label_input = use_signal(String::new);
    let mut wallet_label_error = use_signal(|| None::<String>);
    let mut account_label_input = use_signal(String::new);
    let mut account_label_error = use_signal(|| None::<String>);
    let mut field_error = use_signal(|| None::<String>);
    let mut save_error = use_signal(|| None::<String>);

    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let toast_state = use_context::<ToastState>();

    let wallets_resource = use_resource(move || async move { get_wallets().await });
    let wallets: Vec<WalletView> = wallets_resource
        .value()
        .read()
        .as_ref()
        .and_then(|response| response.as_ref().ok())
        .map_or_else(Vec::new, |response| response.wallets.clone());
    let wallets_loading = wallets_resource.value().read().is_none();

    if !wallets_loading && wallet_choice.peek().is_none() {
        wallet_choice.set(Some(initial_wallet_dropdown_choice(
            default_wallet_id,
            None,
            wallets.len(),
        )));
    }

    rsx! {
        div { class: "modal-overlay",
            onclick: move |_| on_cancel.call(()),
            div { class: "modal",
                onclick: move |e| e.stop_propagation(),
                div { class: "modal-header",
                    h3 { "Add Bitcoin Extended Public Key" }
                }
                div { class: "modal-body",
                    match step() {
                        AddXpubStep::Input => rsx! {
                            div { class: "flow-step",
                                p { "Paste your xpub, ypub, or zpub key" }
                                textarea {
                                    class: "xpub-input",
                                    autocomplete: "off",
                                    placeholder: "xpub..., ypub..., or zpub...",
                                    rows: 3,
                                    value: "{xpub_input}",
                                    oninput: move |e| xpub_input.set(e.value()),
                                    onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                }
                                if let Some(err) = validation_error() {
                                    div { class: "error-block",
                                        p { "{err}" }
                                    }
                                }
                                div { class: "modal-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        onclick: move |_| on_cancel.call(()),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-primary",
                                        disabled: xpub_input().trim().is_empty(),
                                        onclick: move |_| {
                                            let input = xpub_input().trim().to_string();
                                            let previous_validation_had_pinned_wallet =
                                                validation_result()
                                                    .as_ref()
                                                    .and_then(|response| {
                                                        response.existing_wallet.as_ref()
                                                    })
                                                    .is_some();
                                            let wallet_count = wallets.len();
                                            let mut step = step;
                                            let mut validation_result = validation_result;
                                            let mut validation_error = validation_error;
                                            let mut selected_scheme = selected_scheme;
                                            let mut wallet_choice = wallet_choice;
                                            let mut wallet_label_input = wallet_label_input;
                                            let mut wallet_label_error = wallet_label_error;
                                            let mut field_error = field_error;
                                            let mut save_error = save_error;
                                            let auth_state = auth_state;
                                            let banner_state = banner_state;
                                            if let Err(err) = prevalidate_xpub_input(&input) {
                                                validation_error.set(Some(err));
                                                step.set(AddXpubStep::Input);
                                                return;
                                            }
                                            step.set(AddXpubStep::Validating);
                                            validation_error.set(None);
                                            wallet_label_error.set(None);
                                            field_error.set(None);
                                            save_error.set(None);
                                            spawn(async move {
                                                let result = validate_xpub(ValidateXpubRequest {
                                                    extended_pubkey: input,
                                                }).await;
                                                match result {
                                                    Ok(response) => {
                                                        let scheme_inputs: Vec<XpubDefaultSchemeInput> =
                                                            response
                                                                .schemes
                                                                .iter()
                                                                .map(|scheme| XpubDefaultSchemeInput {
                                                                    address_scheme: scheme.address_scheme,
                                                                    has_activity: scheme.has_activity,
                                                                    already_linked: scheme.already_linked,
                                                                })
                                                                .collect();
                                                        let default_scheme = select_default_xpub_scheme(
                                                            response.suggested_scheme,
                                                            &scheme_inputs,
                                                        )
                                                            .unwrap_or(response.suggested_scheme);
                                                        selected_scheme.set(default_scheme);
                                                        if let Some(existing_wallet) =
                                                            response.existing_wallet.as_ref()
                                                        {
                                                            wallet_choice.set(Some(
                                                                WalletChoice::Existing(
                                                                    existing_wallet.wallet_id,
                                                                ),
                                                            ));
                                                            wallet_label_input.set(String::new());
                                                        } else if previous_validation_had_pinned_wallet
                                                        {
                                                            wallet_choice.set(if wallets_loading {
                                                                None
                                                            } else {
                                                                Some(
                                                                    initial_wallet_dropdown_choice(
                                                                        default_wallet_id,
                                                                        None,
                                                                        wallet_count,
                                                                    ),
                                                                )
                                                            });
                                                        }
                                                        validation_result.set(Some(response));
                                                        step.set(AddXpubStep::Results);
                                                    }
                                                    Err(err) => {
                                                        if err.is_unauthorized() {
                                                            handle_session_expired(auth_state, banner_state, "validate xpub");
                                                        }
                                                        validation_error.set(Some(err.to_string()));
                                                        step.set(AddXpubStep::Input);
                                                    }
                                                }
                                            });
                                        },
                                        "Next"
                                    }
                                }
                            }
                        },
                        AddXpubStep::Validating => rsx! {
                            div { class: "flow-step",
                                p { "Validating key..." }
                            }
                        },
                        AddXpubStep::Results => {
                            match validation_result() {
                                Some(result) => {
                                    let has_unlinked_scheme =
                                        result.schemes.iter().any(|scheme| !scheme.already_linked);
                                    let pinned_wallet = result.existing_wallet.as_ref().map(
                                        |existing_wallet| PinnedWallet {
                                            id: existing_wallet.wallet_id,
                                            label: existing_wallet.wallet_label.clone(),
                                            message: format!(
                                                "This key is already in wallet '{}'. New address types are added to the same wallet.",
                                                existing_wallet.wallet_label
                                            ),
                                        },
                                    );
                                    let wallet_options = wallet_options_for_dropdown(
                                        &wallets,
                                        if pinned_wallet.is_some() {
                                            None
                                        } else {
                                            default_wallet_id
                                        },
                                    );
                                    let selected_linked_scheme = result
                                        .schemes
                                        .iter()
                                        .find(|scheme| {
                                            scheme.address_scheme == selected_scheme()
                                                && scheme.already_linked
                                        })
                                        .map(|scheme| {
                                            (
                                                address_scheme_label(scheme.address_scheme)
                                                    .to_string(),
                                                scheme
                                                    .linked_wallet_label
                                                    .clone()
                                                    .unwrap_or_else(|| "Unknown wallet".to_string()),
                                                scheme
                                                    .linked_account_label
                                                    .clone()
                                                    .unwrap_or_else(|| "Unknown account".to_string()),
                                            )
                                        });

                                    rsx! {
                                        div { class: "flow-step",
                                            p { "Choose the address type for this key:" }

                                            if !has_unlinked_scheme {
                                                div { class: "alert alert-info",
                                                    p { "All supported address types for this key are already linked." }
                                                    p { "No additional account can be added for this key." }
                                                }
                                            } else if let Some((scheme_name, wallet_label, account_label)) = selected_linked_scheme.clone() {
                                                div { class: "alert alert-info",
                                                    p {
                                                        "Already linked: "
                                                        strong { "{scheme_name}" }
                                                        " is already linked to wallet "
                                                        strong { "'{wallet_label}'" }
                                                        " as account "
                                                        strong { "'{account_label}'" }
                                                        "."
                                                    }
                                                    p { "Choose an unlinked address type to continue." }
                                                }
                                            }

                                            div { class: "xpub-scheme-picker",
                                                for scheme_result in &result.schemes {
                                                    {
                                                        let scheme = scheme_result.address_scheme;
                                                        let display_name = address_scheme_label(scheme).to_string();
                                                        let note = scheme_result.scheme_note.clone();
                                                        let address = scheme_result.first_address.clone();
                                                        let has_activity = scheme_result.has_activity;
                                                        let activity_error = scheme_result.activity_check_error.clone();
                                                        let linked_wallet_label = scheme_result
                                                            .linked_wallet_label
                                                            .clone()
                                                            .unwrap_or_else(|| "Unknown wallet".to_string());
                                                        let linked_account_label = scheme_result
                                                            .linked_account_label
                                                            .clone()
                                                            .unwrap_or_else(|| "Unknown account".to_string());
                                                        let already_linked = scheme_result.already_linked;
                                                        rsx! {
                                                            label { class: "xpub-scheme-option",
                                                                class: if selected_scheme() == scheme { "selected" },
                                                                class: if already_linked { "linked" },
                                                                input {
                                                                    r#type: "radio",
                                                                    name: "address_scheme",
                                                                    checked: selected_scheme() == scheme,
                                                                    onchange: move |_| selected_scheme.set(scheme),
                                                                }
                                                                div { class: "xpub-scheme-details",
                                                                    div { class: "xpub-scheme-header",
                                                                        span { class: "xpub-scheme-name", "{display_name}" }
                                                                        span { class: "xpub-activity-indicator",
                                                                            match has_activity {
                                                                                Some(true) => rsx! {
                                                                                    span { class: "activity-yes", "\u{2713} Activity" }
                                                                                },
                                                                                Some(false) => rsx! {
                                                                                    span { class: "activity-no", "\u{2014} No activity" }
                                                                                },
                                                                                None => rsx! {
                                                                                    if let Some(_err) = &activity_error {
                                                                                        span { class: "activity-unknown", "? Check failed" }
                                                                                    } else {
                                                                                        span { class: "activity-unknown", "?" }
                                                                                    }
                                                                                },
                                                                            }
                                                                        }
                                                                    }
                                                                    p { class: "muted", "{note}" }
                                                                    code { class: "xpub-check-address", "{address}" }
                                                                    if already_linked {
                                                                        p { class: "muted",
                                                                            "Already linked to wallet "
                                                                            strong { "'{linked_wallet_label}'" }
                                                                            " as account "
                                                                            strong { "'{linked_account_label}'" }
                                                                            "."
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            if has_unlinked_scheme {
                                                p { class: "muted",
                                                    "A wallet typically represents a single seed phrase. "
                                                    "Multiple extended public keys can belong to the same wallet."
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
                                                        wallets: wallet_options,
                                                        choice,
                                                        default_wallet_id,
                                                        pinned_wallet: pinned_wallet.clone(),
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
                                            }

                                    div { class: "modal-actions",
                                        button {
                                            class: "btn btn-secondary",
                                            onclick: move |_| {
                                                validation_error.set(None);
                                                field_error.set(None);
                                                wallet_label_error.set(None);
                                                save_error.set(None);
                                                step.set(AddXpubStep::Input);
                                            },
                                            "Back"
                                        }
                                        if has_unlinked_scheme {
                                            button {
                                                class: "btn btn-primary",
                                                disabled: selected_linked_scheme.is_some()
                                                    || wallets_loading
                                                    || wallet_choice().is_none(),
                                                onclick: move |_| {
                                                    let input = xpub_input().trim().to_string();
                                                    let choice = match wallet_choice() {
                                                        Some(choice) => choice,
                                                        None => {
                                                            field_error.set(Some("Wallets are still loading.".to_string()));
                                                            return;
                                                        }
                                                    };
                                                    let label_text = wallet_label_input().trim().to_string();
                                                    let mut step = step;
                                                    let mut wallet_label_error = wallet_label_error;
                                                    let mut account_label_error = account_label_error;
                                                    let mut field_error = field_error;
                                                    let mut save_error = save_error;
                                                    let auth_state = auth_state;
                                                    let banner_state = banner_state;

                                                    if let Err(err) = prevalidate_xpub_input(&input) {
                                                        save_error.set(Some(err));
                                                        return;
                                                    }

                                                    field_error.set(None);
                                                    wallet_label_error.set(None);
                                                    account_label_error.set(None);
                                                    save_error.set(None);

                                                    let (wallet_id, wallet_label) = match choice {
                                                        WalletChoice::Unselected => {
                                                            field_error.set(Some(
                                                                "Select an existing wallet or create a new one"
                                                                    .to_string(),
                                                            ));
                                                            return;
                                                        }
                                                        WalletChoice::CreateNew => {
                                                            if let Err(err) =
                                                                prevalidate_wallet_label_for_new_wallet(&label_text)
                                                            {
                                                                wallet_label_error.set(Some(err));
                                                                return;
                                                            }
                                                            (None, Some(RawLabel::new(label_text)))
                                                        }
                                                        WalletChoice::Existing(wallet_id) => {
                                                            (Some(wallet_id), None)
                                                        }
                                                    };

                                                    let account_label_raw =
                                                        account_label_input().trim().to_string();
                                                    let account_label = if account_label_raw.is_empty() {
                                                        None
                                                    } else {
                                                        Some(RawLabel::new(account_label_raw))
                                                    };

                                                    let scheme = selected_scheme();
                                                    step.set(AddXpubStep::Saving);
                                                    spawn(async move {
                                                        let result = add_xpub(AddXpubRequest {
                                                            extended_pubkey: input,
                                                            address_scheme: scheme,
                                                            wallet_id,
                                                            wallet_label,
                                                            account_label,
                                                        }).await;
                                                        match result {
                                                            Ok(response) => {
                                                                if let Some(notice) = response.account_limit_notice {
                                                                    push_toast(
                                                                        toast_state,
                                                                        ToastLevel::Info,
                                                                        notice.message,
                                                                    );
                                                                }
                                                                on_complete.call(());
                                                            }
                                                            Err(err) if err.is_unauthorized() => {
                                                                handle_session_expired(
                                                                    auth_state,
                                                                    banner_state,
                                                                    "add xpub",
                                                                );
                                                                step.set(AddXpubStep::Results);
                                                            }
                                                            Err(err) if is_form_field_error(&err) => {
                                                                if let Some(message) =
                                                                    first_matching_field_error(&err, &["wallet_label"])
                                                                {
                                                                    wallet_label_error.set(Some(message));
                                                                } else if let Some(message) =
                                                                    first_matching_field_error(&err, &["account_label", "label"])
                                                                {
                                                                    account_label_error.set(Some(message));
                                                                } else {
                                                                    let message = primary_field_or_message(
                                                                        &err,
                                                                        &[
                                                                            "extended_pubkey",
                                                                            "address_scheme",
                                                                            "wallet_id",
                                                                        ],
                                                                    );
                                                                    field_error.set(Some(message));
                                                                }
                                                                step.set(AddXpubStep::Results);
                                                            }
                                                            Err(err) => {
                                                                save_error.set(Some(err.to_string()));
                                                                step.set(AddXpubStep::Results);
                                                            }
                                                        }
                                                    });
                                                },
                                                "Add"
                                            }
                                        } else {
                                            button {
                                                class: "btn btn-secondary",
                                                onclick: move |_| on_cancel.call(()),
                                                "Cancel"
                                            }
                                        }
                                    }
                                },
                            }
                        },
                        None => rsx! {
                            div { class: "flow-step",
                                p { "No validation data available." }
                            }
                        },
                            }
                        },
                        AddXpubStep::Saving => rsx! {
                            div { class: "flow-step",
                                p { "Saving..." }
                            }
                        },
                    }
                }
            }
        }
    }
}

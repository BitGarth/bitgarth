use super::helpers::{
    ADDRESS_SCHEME_CHOICES, AccountSelection, ExistingAccountAddressTypes, address_scheme_label,
    address_scheme_sort_key, available_schemes_for_account_with_selected,
    default_selection_for_available_schemes, display_account_number, parse_display_account_number,
};
use crate::wallets::{AccountIndex, AddressScheme, WALLET_LABEL_MAX_LENGTH};
use dioxus::prelude::*;

#[component]
pub(super) fn AccountSelector(
    existing_wallet_label: Option<String>,
    new_wallet_label: String,
    wallet_label_error: Option<String>,
    existing_accounts: Vec<AccountIndex>,
    existing_account_address_types: Vec<ExistingAccountAddressTypes>,
    on_new_wallet_label_change: EventHandler<String>,
    selected: Vec<AccountSelection>,
    on_change: EventHandler<Vec<AccountSelection>>,
    on_continue: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut input_value = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let existing_indices: Vec<String> = existing_accounts
        .iter()
        .map(|account| display_account_number(*account).to_string())
        .collect();
    let selected_for_add = selected.clone();
    let selection_rows: Vec<(usize, AccountSelection, Vec<AddressScheme>)> = selected
        .iter()
        .cloned()
        .enumerate()
        .map(|(row_index, selection)| {
            let selected_without_current: Vec<AccountSelection> = selected
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != row_index)
                .map(|(_, value)| value.clone())
                .collect();
            let mut available = available_schemes_for_account_with_selected(
                &existing_account_address_types,
                &selected_without_current,
                selection.account,
            );
            if !available.contains(&selection.address_scheme) {
                available.push(selection.address_scheme);
                available.sort_by_key(|scheme| address_scheme_sort_key(*scheme));
            }
            (row_index, selection, available)
        })
        .collect();
    let wallet_label_ready = existing_wallet_label.is_some() || !new_wallet_label.trim().is_empty();
    let can_continue = !selected.is_empty() && wallet_label_ready;

    rsx! {
        div { class: "account-selector",
            if let Some(label) = &existing_wallet_label {
                p { class: "muted", "Existing wallet detected: {label}" }
            } else {
                div { class: "xpub-label-input",
                    label { r#for: "trezor_wallet_label", "Wallet label" }
                    input {
                        r#type: "text",
                        id: "trezor_wallet_label",
                        autocomplete: "off",
                        placeholder: "e.g. Trezor Wallet",
                        maxlength: WALLET_LABEL_MAX_LENGTH as i64,
                        value: "{new_wallet_label}",
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        oninput: move |e| on_new_wallet_label_change.call(e.value()),
                    }
                }
                if let Some(err) = wallet_label_error {
                    p { class: "error-text", "{err}" }
                }
            }
            if !existing_indices.is_empty() {
                p { class: "muted",
                    "Already linked account numbers: {existing_indices.join(\", \")}."
                }
                p { class: "muted",
                    "You can select an existing account number again to link another address type."
                }
            }
            p { class: "muted", "For each row, choose one account number and one address type." }
            p { class: "muted", "Already linked address types are hidden and cannot be selected again." }

            div { class: "account-list",
                for (row_index, selection, available_schemes) in selection_rows {
                    AccountSelectionRow {
                        row_index,
                        selection,
                        available_schemes,
                        selected: selected.clone(),
                        on_change,
                    }
                }
            }
            div { class: "address-scheme-notes",
                for option in ADDRESS_SCHEME_CHOICES {
                    p { class: "muted",
                        "{option.label}: {option.note}"
                    }
                }
            }

            div { class: "account-add",
                input {
                    r#type: "number",
                    autocomplete: "off",
                    placeholder: "Account number",
                    value: "{input_value}",
                    oninput: move |e| {
                        input_value.set(e.value());
                        error.set(None);
                    }
                }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| {
                        let parsed = input_value().trim().parse::<u32>();
                        let value = match parsed {
                            Ok(value) => value,
                            Err(_) => {
                                error.set(Some("Please enter a valid account number (1 or higher).".to_string()));
                                return;
                            }
                        };

                        let account = match parse_display_account_number(value) {
                            Ok(account) => account,
                            Err(message) => {
                                error.set(Some(message));
                                return;
                            }
                        };

                        let available_schemes =
                            available_schemes_for_account_with_selected(
                                &existing_account_address_types,
                                &selected_for_add,
                                account,
                            );
                        if available_schemes.is_empty() {
                            error.set(Some(
                                "All supported address types are already linked for that account."
                                    .to_string(),
                            ));
                            return;
                        }

                        let Some(address_scheme) =
                            default_selection_for_available_schemes(&available_schemes)
                        else {
                            error.set(Some(
                                "No address types are available for that account.".to_string(),
                            ));
                            return;
                        };

                        let mut next = selected_for_add.clone();
                        next.push(AccountSelection {
                            account,
                            address_scheme,
                        });
                        next.sort_by(|left, right| {
                            left.account
                                .as_u32()
                                .cmp(&right.account.as_u32())
                                .then(
                                    address_scheme_sort_key(left.address_scheme)
                                        .cmp(&address_scheme_sort_key(right.address_scheme)),
                                )
                        });
                        on_change.call(next);
                        input_value.set(String::new());
                    },
                    "Add"
                }
            }

            if let Some(err) = error() {
                p { class: "error-text", "{err}" }
            }

            div { class: "modal-actions",
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
                button {
                    class: "btn btn-primary",
                    disabled: !can_continue,
                    onclick: move |_| on_continue.call(()),
                    "Continue"
                }
            }
        }
    }
}

#[component]
pub(super) fn AccountSelectionRow(
    row_index: usize,
    selection: AccountSelection,
    available_schemes: Vec<AddressScheme>,
    selected: Vec<AccountSelection>,
    on_change: EventHandler<Vec<AccountSelection>>,
) -> Element {
    let selected_for_remove = selected.clone();
    let selected_for_change = selected.clone();
    let available_for_change = available_schemes.clone();
    let available_for_render = available_schemes.clone();

    rsx! {
        div { class: "account-selection-row",
            div { class: "account-selection-header",
                span { class: "account-selection-label", "Account {display_account_number(selection.account)}" }
                button {
                    class: "pill-remove",
                    r#type: "button",
                    onclick: move |_| {
                        let mut next = selected_for_remove.clone();
                        if row_index < next.len() {
                            next.remove(row_index);
                        }
                        on_change.call(next);
                    },
                    "Remove"
                }
            }
            div { class: "account-selection-schemes",
                if available_schemes.is_empty() {
                    p { class: "muted", "All supported address types are already linked." }
                } else {
                    select {
                        class: "selector",
                        value: "{selection.address_scheme.as_str()}",
                        onchange: move |evt| {
                            let value = evt.value();
                            let Some(address_scheme) = AddressScheme::from_str(value.as_str()) else {
                                return;
                            };
                            if !available_for_change.contains(&address_scheme) {
                                return;
                            }
                            let mut next = selected_for_change.clone();
                            if row_index < next.len() {
                                next[row_index].address_scheme = address_scheme;
                            }
                            on_change.call(next);
                        },
                        for scheme in available_for_render {
                            option {
                                value: "{scheme.as_str()}",
                                selected: scheme == selection.address_scheme,
                                "{address_scheme_label(scheme)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

use crate::backend::WalletView;
use crate::wallets::{ACCOUNT_LABEL_MAX_LENGTH, WALLET_LABEL_MAX_LENGTH, WalletId};
use dioxus::prelude::*;

const UNSELECTED_WALLET_OPTION_VALUE: &str = "__unselected_wallet__";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WalletChoice {
    Unselected,
    Existing(WalletId),
    CreateNew,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WalletOption {
    pub(super) id: WalletId,
    pub(super) label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PinnedWallet {
    pub(super) id: WalletId,
    pub(super) label: String,
    pub(super) message: String,
}

pub(super) fn initial_wallet_dropdown_choice(
    default_wallet_id: Option<WalletId>,
    pinned_wallet: Option<&PinnedWallet>,
    wallet_count: usize,
) -> WalletChoice {
    match pinned_wallet {
        Some(pinned_wallet) => WalletChoice::Existing(pinned_wallet.id),
        None => match default_wallet_id {
            Some(default_wallet_id) => WalletChoice::Existing(default_wallet_id),
            None if wallet_count == 0 => WalletChoice::CreateNew,
            None => WalletChoice::Unselected,
        },
    }
}

pub(super) fn wallet_dropdown_is_disabled(
    default_wallet_id: Option<WalletId>,
    pinned_wallet: Option<&PinnedWallet>,
    wallet_count: usize,
) -> bool {
    pinned_wallet.is_some() || default_wallet_id.is_some() || wallet_count == 0
}

fn wallet_choice_select_value(choice: &WalletChoice) -> String {
    match choice {
        WalletChoice::Unselected => UNSELECTED_WALLET_OPTION_VALUE.to_string(),
        WalletChoice::Existing(id) => id.to_string(),
        WalletChoice::CreateNew => super::CREATE_NEW_WALLET_OPTION_VALUE.to_string(),
    }
}

fn parse_wallet_dropdown_choice(value: &str) -> Option<WalletChoice> {
    if value == UNSELECTED_WALLET_OPTION_VALUE {
        return Some(WalletChoice::Unselected);
    }
    if value == super::CREATE_NEW_WALLET_OPTION_VALUE {
        return Some(WalletChoice::CreateNew);
    }
    value.parse::<WalletId>().ok().map(WalletChoice::Existing)
}

fn display_wallet_option_label(wallet_id: WalletId, label: &str) -> String {
    if !label.is_empty() {
        return label.to_string();
    }

    let wallet_id = wallet_id.to_string();
    let truncated = if wallet_id.len() > 8 {
        format!("{}...", &wallet_id[..8])
    } else {
        wallet_id
    };
    format!("Unlabeled wallet ({truncated})")
}

pub(super) fn wallet_options_for_dropdown(
    wallets: &[WalletView],
    default_wallet_id: Option<WalletId>,
) -> Vec<WalletOption> {
    wallets
        .iter()
        .filter(|wallet| match default_wallet_id {
            Some(default_wallet_id) => wallet.id == default_wallet_id,
            None => true,
        })
        .map(|wallet| WalletOption {
            id: wallet.id,
            label: wallet.label.clone(),
        })
        .collect()
}

#[component]
pub(super) fn WalletDropdown(
    wallets: Vec<WalletOption>,
    choice: WalletChoice,
    default_wallet_id: Option<WalletId>,
    pinned_wallet: Option<PinnedWallet>,
    new_wallet_label: String,
    wallet_label_error: Option<String>,
    on_choice_change: EventHandler<WalletChoice>,
    on_new_wallet_label_change: EventHandler<String>,
) -> Element {
    let dropdown_disabled =
        wallet_dropdown_is_disabled(default_wallet_id, pinned_wallet.as_ref(), wallets.len());
    let mut existing_wallets = wallets;

    if let WalletChoice::Existing(selected_wallet_id) = choice.clone()
        && existing_wallets
            .iter()
            .all(|wallet| wallet.id != selected_wallet_id)
    {
        let label = pinned_wallet
            .as_ref()
            .filter(|wallet| wallet.id == selected_wallet_id)
            .map(|wallet| wallet.label.clone())
            .unwrap_or_default();
        existing_wallets.insert(
            0,
            WalletOption {
                id: selected_wallet_id,
                label,
            },
        );
    }

    rsx! {
        label { class: "form-label", "Wallet" }
        select {
            class: "selector",
            disabled: dropdown_disabled,
            value: wallet_choice_select_value(&choice),
            onchange: move |event| {
                if let Some(choice) = parse_wallet_dropdown_choice(&event.value()) {
                    on_choice_change.call(choice);
                }
            },
            if pinned_wallet.is_some() || default_wallet_id.is_some() {
                for wallet in existing_wallets {
                    option {
                        value: "{wallet.id}",
                        "{display_wallet_option_label(wallet.id, &wallet.label)}"
                    }
                }
            } else if existing_wallets.is_empty() {
                option {
                    value: super::CREATE_NEW_WALLET_OPTION_VALUE,
                    "Create a new wallet"
                }
            } else {
                option {
                    value: UNSELECTED_WALLET_OPTION_VALUE,
                    "Select an existing wallet"
                }
                for wallet in existing_wallets {
                    option {
                        value: "{wallet.id}",
                        "{display_wallet_option_label(wallet.id, &wallet.label)}"
                    }
                }
                option {
                    value: super::CREATE_NEW_WALLET_OPTION_VALUE,
                    "Create a new wallet"
                }
            }
        }

        if let Some(pinned_wallet) = pinned_wallet {
            p { class: "muted", "{pinned_wallet.message}" }
        }

        if choice == WalletChoice::CreateNew {
            label { class: "form-label", r#for: "wallet_label", "Wallet label" }
            input {
                class: "selector",
                r#type: "text",
                id: "wallet_label",
                maxlength: WALLET_LABEL_MAX_LENGTH,
                autocomplete: "off",
                placeholder: "My Wallet",
                value: "{new_wallet_label}",
                oninput: move |event| on_new_wallet_label_change.call(event.value()),
                onmounted: move |event| async move {
                    let _ = event.set_focus(true).await;
                },
            }
            if let Some(error) = wallet_label_error {
                p { class: "error-text", "{error}" }
            }
        }
    }
}

/// Optional account-name field shared by every "add account" modal. Placed
/// below the wallet selector. Left blank, the backend auto-names the account
/// ("Bitcoin Account 1", etc.); the `placeholder` shows that default.
#[component]
pub(super) fn AccountNameField(
    value: String,
    placeholder: String,
    error: Option<String>,
    on_input: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "form-label", r#for: "account_name", "Account name" }
        input {
            class: "selector",
            r#type: "text",
            id: "account_name",
            maxlength: ACCOUNT_LABEL_MAX_LENGTH,
            autocomplete: "off",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |event| on_input.call(event.value()),
        }
        p { class: "muted account-name-hint", "Leave blank to auto-name." }
        if let Some(error) = error {
            p { class: "error-text", "{error}" }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn initial_wallet_dropdown_choice_returns_unselected_when_wallets_exist_without_pin() {
        assert_eq!(
            initial_wallet_dropdown_choice(None, None, 2),
            WalletChoice::Unselected
        );
    }

    #[test]
    fn initial_wallet_dropdown_choice_returns_create_new_when_no_wallets_exist() {
        assert_eq!(
            initial_wallet_dropdown_choice(None, None, 0),
            WalletChoice::CreateNew
        );
    }

    #[test]
    fn initial_wallet_dropdown_choice_uses_default_wallet_when_present() {
        let default_wallet_id = WalletId::new();

        assert_eq!(
            initial_wallet_dropdown_choice(Some(default_wallet_id), None, 3),
            WalletChoice::Existing(default_wallet_id)
        );
    }

    #[test]
    fn initial_wallet_dropdown_choice_prefers_pinned_wallet_over_default_wallet() {
        let default_wallet_id = WalletId::new();
        let pinned_wallet_id = WalletId::new();
        let pinned_wallet = PinnedWallet {
            id: pinned_wallet_id,
            label: "Pinned Wallet".to_string(),
            message: "Pinned for grouping.".to_string(),
        };

        assert_eq!(
            initial_wallet_dropdown_choice(Some(default_wallet_id), Some(&pinned_wallet), 3),
            WalletChoice::Existing(pinned_wallet_id)
        );
    }

    #[test]
    fn wallet_dropdown_is_disabled_for_pinned_default_or_empty_states() {
        let default_wallet_id = WalletId::new();
        let pinned_wallet = PinnedWallet {
            id: WalletId::new(),
            label: "Pinned Wallet".to_string(),
            message: "Pinned for grouping.".to_string(),
        };

        assert!(wallet_dropdown_is_disabled(None, None, 0));
        assert!(wallet_dropdown_is_disabled(
            Some(default_wallet_id),
            None,
            2
        ));
        assert!(wallet_dropdown_is_disabled(None, Some(&pinned_wallet), 2));
        assert!(!wallet_dropdown_is_disabled(None, None, 2));
    }
}

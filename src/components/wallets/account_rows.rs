use super::super::ChevronRightIcon;
use super::super::{
    format_current_value_for_display, format_number_for_display, format_wallet_balance_parts,
};
use super::AccountSyncStatusPill;
use super::account_details::{AccountAddressesModal, ChangeWalletInline, LabelEditor};
use super::dialogs::{
    AddressSchemeDeleteConfirmDialog, DeleteAccountConfirmDialog, KebabMenu, KebabMenuItem,
};
use super::helpers::{
    AccountAddressesLoader, WalletMoveOption, account_row_subline, address_scheme_label,
    copy_to_clipboard, parse_label_for_editor,
};
use crate::backend::{
    AccountBalanceStateView, AccountReferenceKind, AccountView, CustomAccountView,
    ManualAssetAccountView, NativeAccountView,
};
use crate::backend::{delete_wallet_account, trigger_sync, update_account_label};
use crate::settings::SettingsState;
use crate::transactions::{
    EtherscanHistoryStatus, RawTransactionSyncScope, RawTransactionSyncTriggerRequest,
    RawTransactionSyncTriggerSource,
};
use crate::wallets::{ACCOUNT_LABEL_MAX_LENGTH, RawLabel, UpdateAccountLabelRequest, WalletId};
use crate::{AuthState, BannerState, Route};
use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
struct CustomAccountRowView {
    custom_view: CustomAccountView,
    account_state: Option<crate::backend::AccountStateView>,
}

#[component]
pub(super) fn WalletAccountRowSection(
    account_view: AccountView,
    show_change_wallet_action: bool,
    current_wallet_id: WalletId,
    current_wallet_has_accessors: bool,
    destination_wallets: Vec<WalletMoveOption>,
    on_action_error: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    match account_view {
        AccountView::Native(scheme_view) => rsx! {
            NativeAccountRowSection {
                scheme_view: (*scheme_view).clone(),
                show_change_wallet_action,
                current_wallet_id,
                current_wallet_has_accessors,
                destination_wallets,
                on_action_error,
                on_refresh,
            }
        },
        AccountView::Custom(custom_view) => rsx! {
            CustomAccountRowSection {
                custom_row: CustomAccountRowView {
                    custom_view,
                    account_state: None,
                },
                show_change_wallet_action,
                current_wallet_id,
                current_wallet_has_accessors,
                destination_wallets,
                on_action_error,
                on_refresh,
            }
        },
        AccountView::Manual(view) => {
            let custom_row = manual_account_as_custom_view(view);
            rsx! {
                CustomAccountRowSection {
                    custom_row,
                    show_change_wallet_action,
                    current_wallet_id,
                    current_wallet_has_accessors,
                    destination_wallets,
                    on_action_error,
                    on_refresh,
                }
            }
        }
    }
}

fn manual_account_as_custom_view(view: ManualAssetAccountView) -> CustomAccountRowView {
    CustomAccountRowView {
        account_state: Some(view.account_state),
        custom_view: CustomAccountView {
            account_id: view.account_id,
            label: view.label,
            unit_code: view.unit_code,
            decimal_precision: view.decimal_precision,
            symbol: view.symbol,
            balance_state: view.balance_state,
            current_value: view.current_value,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_views::ManualAssetInstanceIdView;
    use crate::backend::{AccountBalanceStateView, CurrentAssetValueView};
    use crate::models::CurrencyCode;
    use crate::wallets::WalletAccountId;

    fn unknown_balance() -> AccountBalanceStateView {
        AccountBalanceStateView::Unknown
    }

    #[test]
    fn manual_accounts_reuse_custom_row_rendering_data() {
        let manual = ManualAssetAccountView {
            account_id: WalletAccountId::new(),
            account_state: crate::backend::AccountStateView::Active,
            label: "ADA Account 1".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            unit_code: "ADA".to_string(),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano Mainnet".to_string(),
            decimal_precision: 6,
            symbol: None,
            balance_state: unknown_balance(),
            current_value: Some(CurrentAssetValueView {
                price: "2".to_string(),
                converted_value: "4".to_string(),
                currency: CurrencyCode::from_code("USD").unwrap(),
            }),
        };
        let manual_row = manual_account_as_custom_view(manual);

        assert_eq!(manual_row.custom_view.unit_code, "ADA");
        assert_eq!(manual_row.custom_view.label, "ADA Account 1");
        assert!(manual_row.custom_view.current_value.is_some());
    }

    #[test]
    fn manual_account_conversion_preserves_inactive_state() {
        let manual = ManualAssetAccountView {
            account_id: WalletAccountId::new(),
            account_state: crate::backend::AccountStateView::Inactive,
            label: "Inactive ADA".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            unit_code: "ADA".to_string(),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano Mainnet".to_string(),
            decimal_precision: 6,
            symbol: None,
            balance_state: unknown_balance(),
            current_value: None,
        };

        let manual_row = manual_account_as_custom_view(manual);

        assert_eq!(
            manual_row.account_state,
            Some(crate::backend::AccountStateView::Inactive)
        );
    }
}

#[component]
fn NativeAccountRowSection(
    scheme_view: NativeAccountView,
    show_change_wallet_action: bool,
    current_wallet_id: WalletId,
    current_wallet_has_accessors: bool,
    destination_wallets: Vec<WalletMoveOption>,
    on_action_error: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut editing_label = use_signal(|| false);
    let mut show_delete_confirm = use_signal(|| false);
    let mut show_change_wallet = use_signal(|| false);
    let mut show_addresses_modal = use_signal(|| false);
    let sync_slot_submitting = use_signal(|| false);
    let addresses_loading = use_signal(|| false);
    let addresses_error = use_signal(|| None::<String>);
    let addresses_page = use_signal(|| None::<crate::wallets::GetAccountAddressesResponse>);
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let settings_state = use_context::<SettingsState>();
    let navigator = use_navigator();

    let account_id = scheme_view.account_id;
    let native_account_id = scheme_view.native_account_id;
    let scheme = scheme_view.scheme;
    let scheme_label = address_scheme_label(scheme);
    let reference_kind = scheme_view.account_reference_kind;
    let reference_value = scheme_view.account_reference.clone();
    let asset = scheme_view.asset;
    let account_row_subline_text = use_memo(use_reactive(
        (&asset, &scheme, &reference_value),
        move |(asset, scheme, reference_value)| {
            account_row_subline(asset, scheme, &reference_value)
        },
    ));
    let is_balance_provisional = scheme_view.balance.balance_reliability.is_provisional();
    let number_format = (settings_state.number_format)();
    let price_fetching_enabled = (settings_state.price_fetching_enabled)();
    let (balance_number, balance_unit) =
        format_wallet_balance_parts(&scheme_view.balance, number_format);
    let current_value_display = scheme_view.balance.current_value.as_ref().map(|value| {
        format_current_value_for_display(&value.converted_value, value.currency, number_format)
    });
    let network = scheme_view.balance.context.network;
    let supports_address_modal =
        scheme_view.has_derived_addresses || reference_kind == AccountReferenceKind::SingleAddress;
    let _sync_slot = scheme_view.sync_slot.clone();
    let manual_sync = scheme_view.manual_sync.clone();
    let is_inactive = scheme_view.account_state == crate::backend::AccountStateView::Inactive;
    let sync_state = use_context::<super::sync_state::AccountSyncStateSignal>();
    let etherscan_history_status = sync_state
        .read()
        .get(&native_account_id)
        .and_then(|state| state.snapshot.etherscan_history_status);

    let title = scheme_view.label.clone();
    let current_account_label = scheme_view.label.clone();
    let addresses_loader = AccountAddressesLoader {
        account_id: native_account_id,
        address_scheme: scheme,
        auth_state,
        banner_state,
        addresses_loading,
        addresses_error,
        addresses_page,
    };

    let asset_badge_class = "account-asset-badge";
    let asset_badge_text = match asset {
        crate::wallets::SyncedAssetId::Bitcoin => "BTC",
        crate::wallets::SyncedAssetId::Ethereum => "ETH",
    };

    // Build kebab menu items
    let copy_label = if reference_kind == AccountReferenceKind::ExtendedPubkey {
        "Copy Xpub"
    } else {
        "Copy Address"
    };
    let copy_value = reference_value.clone();

    let mut kebab_items = vec![KebabMenuItem {
        label: "Rename".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| editing_label.set(true)),
        danger: false,
        disabled: false,
        title: None,
    }];
    if show_change_wallet_action {
        kebab_items.push(KebabMenuItem {
            label: "Change Wallet".to_string(),
            test_id: None,
            on_click: EventHandler::new(move |_| show_change_wallet.set(true)),
            danger: false,
            disabled: false,
            title: None,
        });
    }
    if supports_address_modal {
        kebab_items.push(KebabMenuItem {
            label: "View Addresses".to_string(),
            test_id: None,
            on_click: EventHandler::new(move |_| {
                show_addresses_modal.set(true);
                addresses_loader.request_page(1);
            }),
            danger: false,
            disabled: false,
            title: None,
        });
    }
    kebab_items.push(KebabMenuItem {
        label: "View Transactions".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| {
            navigator.push(Route::AccountTransactions {
                account_id,
                start: None,
                end: None,
            });
        }),
        danger: false,
        disabled: false,
        title: None,
    });
    if !copy_value.is_empty() {
        kebab_items.push(KebabMenuItem {
            label: copy_label.to_string(),
            test_id: None,
            on_click: EventHandler::new(move |_| {
                copy_to_clipboard(&copy_value);
            }),
            danger: false,
            disabled: false,
            title: None,
        });
    }
    kebab_items.push(KebabMenuItem {
        label: "Delete".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| show_delete_confirm.set(true)),
        danger: true,
        disabled: false,
        title: None,
    });

    rsx! {
        div { class: if is_inactive { "account-row account-row-inactive" } else { "account-row" },
            if editing_label() {
                LabelEditor {
                    current: parse_label_for_editor(&current_account_label, ACCOUNT_LABEL_MAX_LENGTH, "Account"),
                    max_len: ACCOUNT_LABEL_MAX_LENGTH,
                    on_save: move |label: RawLabel| {
                        spawn(async move {
                            let auth_state = auth_state;
                            let banner_state = banner_state;
                            let request = UpdateAccountLabelRequest { account_id, label };
                            if let Err(err) = update_account_label(request).await {
                                if err.is_unauthorized() {
                                    super::helpers::handle_session_expired(
                                        auth_state,
                                        banner_state,
                                        "account label update",
                                    );
                                }
                                on_action_error.call(err.to_string());
                            } else {
                                on_refresh.call(());
                            }
                        });
                        editing_label.set(false);
                    },
                    on_cancel: move |_| editing_label.set(false),
                }
            } else {
                div { class: "account-row-info",
                    span { class: "{asset_badge_class}", "{asset_badge_text}" }
                    div { class: "account-row-name-group",
                        div { class: "account-row-name-line",
                            Link {
                                class: "account-name-link",
                                to: Route::AccountTransactions {
                                    account_id,
                                    start: None,
                                    end: None,
                                },
                                span { class: "account-name", "{title}" }
                            }
                            AccountSyncStatusPill { account_id: native_account_id }
                            if etherscan_history_status == Some(EtherscanHistoryStatus::Gap) {
                                span {
                                    class: "account-history-gap-badge",
                                    "data-testid": "account-history-gap-badge",
                                    title: "This account has a transaction history gap. Upgrade to import the missing history.",
                                    "History gap"
                                }
                            }
                        }
                        if let Some(subline) = account_row_subline_text() {
                            span {
                                class: "account-row-subline",
                                "data-testid": "account-row-subline",
                                "{subline}"
                            }
                        }
                    }
                }
                div { class: "account-row-right",
                    {
                        let (is_syncing, has_retry) = {
                            let sync_map = sync_state.read();
                            let syncing = sync_map.get(&native_account_id).is_some_and(|s| s.is_any_integration_active());
                            let retry = sync_map.get(&native_account_id).is_some_and(|s| s.has_active_retry());
                            (syncing, retry)
                        };
                        let is_disabled = manual_sync.disabled_reason.is_some() || is_syncing || has_retry;
                        let tooltip: String = match (&manual_sync.disabled_reason, is_syncing, has_retry) {
                            (_, true, _) => "Syncing...".to_string(),
                            (_, _, true) => "Syncing again soon...".to_string(),
                            (Some(reason), _, _) => match reason {
                                crate::backend::ManualSyncDisabledReason::SyncUnavailableOnPlan => {
                                    "Sync unavailable on your current plan.".to_string()
                                }
                                crate::backend::ManualSyncDisabledReason::AccountInactive => {
                                    "Upgrade to activate this account.".to_string()
                                }
                            },
                            (None, _, _) => match (&manual_sync.mode, &manual_sync.slot_effect) {
                                (crate::backend::ManualSyncMode::BalanceRefresh, crate::backend::ManualSyncSlotEffect::WillSelectAvailableSlot) => {
                                    format!("Refresh balance. Uses 1 of {} available synced accounts.", manual_sync.slot_limit.saturating_sub(manual_sync.used_slots))
                                }
                                (crate::backend::ManualSyncMode::TransactionHistory, crate::backend::ManualSyncSlotEffect::WillSelectAvailableSlot) => {
                                    format!("Sync transactions. Uses 1 of {} available synced accounts.", manual_sync.slot_limit.saturating_sub(manual_sync.used_slots))
                                }
                                (crate::backend::ManualSyncMode::BalanceRefresh, _) => "Refresh balance".to_string(),
                                (crate::backend::ManualSyncMode::TransactionHistory, _) => "Sync transactions".to_string(),
                                (crate::backend::ManualSyncMode::Unavailable, _) => "Sync unavailable".to_string(),
                            },
                        };
                        rsx! {
                            button {
                                class: if is_syncing { "sync-icon-btn spinning" } else if is_disabled { "sync-icon-btn disabled" } else { "sync-icon-btn" },
                                disabled: is_disabled,
                                title: "{tooltip}",
                                "data-testid": "account-sync-icon",
                                onclick: move |_| {
                                    let mut sync_slot_submitting = sync_slot_submitting;
                                    if sync_slot_submitting() {
                                        return;
                                    }
                                    sync_slot_submitting.set(true);
                                    spawn(async move {
                                        let request = RawTransactionSyncTriggerRequest {
                                            source: RawTransactionSyncTriggerSource::manual(),
                                            scope: RawTransactionSyncScope::Account {
                                                account_id: native_account_id,
                                            },
                                        };
                                        match trigger_sync(request).await {
                                            Ok(_) => on_refresh.call(()),
                                            Err(err) => {
                                                if err.is_unauthorized() {
                                                    super::helpers::handle_session_expired(
                                                        auth_state,
                                                        banner_state,
                                                        "sync trigger",
                                                    );
                                                }
                                                on_action_error.call(err.to_string());
                                            }
                                        }
                                        sync_slot_submitting.set(false);
                                    });
                                },
                                super::super::RefreshIcon {}
                            }
                        }
                    }
                    div { class: "account-balance-group",
                        if is_balance_provisional {
                            span { class: "account-balance-provisional-label", "Provisional balance" }
                        }
                        div { class: "account-balance-amount-row",
                            span { class: "account-balance", "{balance_number}" }
                            if let Some(unit) = balance_unit.as_deref() {
                                span { class: "account-balance-unit", "{unit}" }
                            }
                        }
                        if let Some(value_display) = current_value_display.as_deref() {
                            span { class: "account-current-value", "{value_display}" }
                        } else if price_fetching_enabled
                            && scheme_view.account_state == crate::backend::AccountStateView::Active
                        {
                            span { class: "account-current-value is-missing", "no price" }
                        }
                    }
                    Link {
                        class: "account-navigate",
                        to: Route::AccountTransactions {
                            account_id,
                            start: None,
                            end: None,
                        },
                        ChevronRightIcon {}
                    }
                    KebabMenu {
                        aria_label: "Account actions".to_string(),
                        items: kebab_items,
                    }
                }
            }

            if show_change_wallet_action && show_change_wallet() {
                ChangeWalletInline {
                    account_id,
                    current_wallet_id,
                    current_wallet_has_accessors,
                    destination_wallets: destination_wallets.clone(),
                    on_refresh,
                    on_close: move |_| show_change_wallet.set(false),
                }
            }

            if show_addresses_modal() {
                AccountAddressesModal {
                    scheme_label: scheme_label.to_string(),
                    asset,
                    network,
                    addresses_page: addresses_page(),
                    loading: addresses_loading(),
                    error: addresses_error(),
                    on_close: move |_| show_addresses_modal.set(false),
                    on_prev_page: move |_| {
                        if let Some(page) = addresses_page()
                            && page.page > 1 {
                            addresses_loader.request_page(page.page - 1);
                        }
                    },
                    on_next_page: move |_| {
                        if let Some(page) = addresses_page()
                            && page.page.saturating_mul(page.page_size) < page.total {
                            addresses_loader.request_page(page.page + 1);
                        }
                    },
                    on_retry: move |_| {
                        let requested_page = addresses_page().map(|page| page.page).unwrap_or(1);
                        addresses_loader.request_page(requested_page);
                    },
                }
            }

            if show_delete_confirm() {
                AddressSchemeDeleteConfirmDialog {
                    scheme_label: scheme_label.to_string(),
                    on_confirm: move |_| {
                        spawn(async move {
                            let auth_state = auth_state;
                            let banner_state = banner_state;
                            let request = crate::wallets::DeleteAccountRequest {
                                account_id,
                            };
                            if let Err(err) = delete_wallet_account(request).await {
                                if err.is_unauthorized() {
                                    super::helpers::handle_session_expired(
                                        auth_state,
                                        banner_state,
                                        "account address type delete",
                                    );
                                }
                                on_action_error.call(err.to_string());
                            } else {
                                on_refresh.call(());
                            }
                        });
                        show_delete_confirm.set(false);
                    },
                    on_cancel: move |_| show_delete_confirm.set(false),
                }
            }
        }
    }
}

#[component]
fn CustomAccountRowSection(
    custom_row: CustomAccountRowView,
    show_change_wallet_action: bool,
    current_wallet_id: WalletId,
    current_wallet_has_accessors: bool,
    destination_wallets: Vec<WalletMoveOption>,
    on_action_error: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut editing_label = use_signal(|| false);
    let mut show_delete_confirm = use_signal(|| false);
    let mut show_change_wallet = use_signal(|| false);
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let settings_state = use_context::<SettingsState>();
    let navigator = use_navigator();

    let custom_view = custom_row.custom_view;
    let is_inactive = custom_row.account_state == Some(crate::backend::AccountStateView::Inactive);
    let account_id = custom_view.account_id;
    let title = custom_view.label.clone();
    let current_account_label = custom_view.label.clone();
    let number_format = (settings_state.number_format)();
    let (balance_number, balance_unit) = match &custom_view.balance_state {
        AccountBalanceStateView::Known { amount } => (
            format_number_for_display(&amount.formatted_value, number_format),
            Some(custom_view.unit_code.clone()),
        ),
        AccountBalanceStateView::Unknown => (format!("Unknown {}", custom_view.unit_code), None),
    };
    let current_value_display = custom_view.current_value.as_ref().map(|value| {
        format_current_value_for_display(&value.converted_value, value.currency, number_format)
    });

    let mut kebab_items = vec![KebabMenuItem {
        label: "Rename".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| editing_label.set(true)),
        danger: false,
        disabled: false,
        title: None,
    }];
    if show_change_wallet_action {
        kebab_items.push(KebabMenuItem {
            label: "Change Wallet".to_string(),
            test_id: None,
            on_click: EventHandler::new(move |_| show_change_wallet.set(true)),
            danger: false,
            disabled: false,
            title: None,
        });
    }
    kebab_items.push(KebabMenuItem {
        label: "View Transactions".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| {
            navigator.push(Route::AccountTransactions {
                account_id,
                start: None,
                end: None,
            });
        }),
        danger: false,
        disabled: false,
        title: None,
    });
    kebab_items.push(KebabMenuItem {
        label: "Delete".to_string(),
        test_id: None,
        on_click: EventHandler::new(move |_| show_delete_confirm.set(true)),
        danger: true,
        disabled: false,
        title: None,
    });

    rsx! {
        div { class: if is_inactive { "account-row account-row-inactive" } else { "account-row" },
            if editing_label() {
                LabelEditor {
                    current: parse_label_for_editor(&current_account_label, ACCOUNT_LABEL_MAX_LENGTH, "Account"),
                    max_len: ACCOUNT_LABEL_MAX_LENGTH,
                    on_save: move |label: RawLabel| {
                        spawn(async move {
                            let auth_state = auth_state;
                            let banner_state = banner_state;
                            let request = UpdateAccountLabelRequest { account_id, label };
                            if let Err(err) = update_account_label(request).await {
                                if err.is_unauthorized() {
                                    super::helpers::handle_session_expired(auth_state, banner_state, "account label update");
                                }
                                on_action_error.call(err.to_string());
                            } else {
                                on_refresh.call(());
                            }
                        });
                        editing_label.set(false);
                    },
                    on_cancel: move |_| editing_label.set(false),
                }
            } else {
                div { class: "account-row-info",
                    span { class: "account-asset-badge", "{custom_view.unit_code}" }
                    Link {
                        class: "account-name-link",
                        to: Route::AccountTransactions {
                            account_id,
                            start: None,
                            end: None,
                        },
                        span { class: "account-name", "{title}" }
                    }
                }
                div { class: "account-row-right",
                    div { class: "account-balance-group",
                        div { class: "account-balance-amount-row",
                            span { class: "account-balance", "{balance_number}" }
                            if let Some(unit) = balance_unit.as_deref() {
                                span { class: "account-balance-unit", "{unit}" }
                            }
                        }
                        if let Some(value_display) = current_value_display.as_deref() {
                            span { class: "account-current-value", "{value_display}" }
                        }
                    }
                    Link {
                        class: "account-navigate",
                        to: Route::AccountTransactions {
                            account_id,
                            start: None,
                            end: None,
                        },
                        ChevronRightIcon {}
                    }
                    KebabMenu {
                        aria_label: "Account actions".to_string(),
                        items: kebab_items,
                    }
                }
            }

            if show_change_wallet_action && show_change_wallet() {
                ChangeWalletInline {
                    account_id,
                    current_wallet_id,
                    current_wallet_has_accessors,
                    destination_wallets: destination_wallets.clone(),
                    on_refresh,
                    on_close: move |_| show_change_wallet.set(false),
                }
            }

            if show_delete_confirm() {
                DeleteAccountConfirmDialog {
                    account_label: title.clone(),
                    on_confirm: move |_| {
                        spawn(async move {
                            let auth_state = auth_state;
                            let banner_state = banner_state;
                            let request = crate::wallets::DeleteAccountRequest { account_id };
                            if let Err(err) = delete_wallet_account(request).await {
                                if err.is_unauthorized() {
                                    super::helpers::handle_session_expired(auth_state, banner_state, "account delete");
                                }
                                on_action_error.call(err.to_string());
                            } else {
                                on_refresh.call(());
                            }
                        });
                        show_delete_confirm.set(false);
                    },
                    on_cancel: move |_| show_delete_confirm.set(false),
                }
            }
        }
    }
}

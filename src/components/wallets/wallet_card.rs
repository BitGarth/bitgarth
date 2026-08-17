use super::account_details::LabelEditor;
use super::account_rows::WalletAccountRowSection;
use super::dialogs::{DeleteWalletConfirmDialog, KebabMenu, KebabMenuItem};
use super::helpers::{
    AccountSchemeRow, WalletMoveOption, build_account_scheme_rows, parse_label_for_editor,
};
use crate::backend::{AccountStateView, AccountView, WalletValueSummaryView, WalletView};
use crate::backend::{delete_wallet, update_wallet_label};
use crate::components::{ChevronRightIcon, format_current_value_for_display};
use crate::models::NumberFormat;
use crate::wallets::{
    DeleteAccountsChoice, DeleteWalletRequest, RawLabel, UpdateWalletLabelRequest,
    WALLET_LABEL_MAX_LENGTH,
};
use crate::{AuthState, BannerState, Route};
use dioxus::prelude::*;

#[component]
pub(super) fn WalletCard(
    wallet: WalletView,
    wallet_move_options: Vec<WalletMoveOption>,
    number_format: NumberFormat,
    collapsed: bool,
    on_toggle_collapsed: EventHandler<()>,
    on_action_error: EventHandler<String>,
    on_refresh: EventHandler<()>,
) -> Element {
    let mut editing_label = use_signal(|| false);
    let mut show_delete_wallet_confirm = use_signal(|| false);
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let navigator = use_navigator();

    let label = wallet.label.clone();
    let fingerprint_display = wallet.master_fingerprint.clone();
    let wallet_id_for_kebab = wallet.id;
    let collapse_label = if collapsed {
        "Expand wallet"
    } else {
        "Collapse wallet"
    };
    let value_summary = wallet
        .value_summary
        .as_ref()
        .map(|summary| wallet_value_summary_text(summary, number_format));

    let wallet_account_scheme_rows = build_account_scheme_rows(&wallet);
    let (active_account_rows, inactive_account_rows) =
        split_account_rows_by_state(wallet_account_scheme_rows);
    let destination_wallets: Vec<WalletMoveOption> = wallet_move_options
        .into_iter()
        .filter(|candidate| candidate.wallet_id != wallet.id)
        .collect();

    rsx! {
        div { class: "card wallet-card",
            div { class: "wallet-card-header",
                if editing_label() {
                    LabelEditor {
                        current: parse_label_for_editor(&wallet.label, WALLET_LABEL_MAX_LENGTH, "Wallet"),
                        max_len: WALLET_LABEL_MAX_LENGTH,
                        on_save: move |label: RawLabel| {
                            let wallet_id = wallet.id;
                            spawn(async move {
                                let auth_state = auth_state;
                                let banner_state = banner_state;
                                let request = UpdateWalletLabelRequest { wallet_id, label };
                                if let Err(err) = update_wallet_label(request).await {
                                    if err.is_unauthorized() {
                                        super::helpers::handle_session_expired(
                                            auth_state,
                                            banner_state,
                                            "wallet label update",
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
                    div { class: "wallet-heading-row",
                        button {
                            class: if collapsed { "wallet-collapse-btn collapsed" } else { "wallet-collapse-btn" },
                            type: "button",
                            title: "{collapse_label}",
                            "aria-label": "{collapse_label}",
                            onclick: move |_| on_toggle_collapsed.call(()),
                            span { class: "wallet-collapse-icon",
                                ChevronRightIcon {}
                            }
                        }
                        div { class: "wallet-title-stack",
                            div { class: "wallet-label-row",
                                h3 { class: "wallet-label",
                                    Link {
                                        class: "wallet-label-link",
                                        style: "color: inherit; text-decoration: none;",
                                        to: Route::WalletReport {
                                            wallet_id: wallet.id,
                                            start: None,
                                            end: None,
                                        },
                                        "{label}"
                                    }
                                }
                                if let Some(ref fp) = fingerprint_display {
                                    span { class: "wallet-fingerprint", "{fp}" }
                                }
                            }
                            if let Some(summary) = value_summary.as_deref() {
                                p { class: "wallet-value-summary", "{summary}" }
                            }
                        }
                    }
                    KebabMenu {
                        aria_label: "Wallet actions".to_string(),
                        items: vec![
                            KebabMenuItem {
                                label: "View Holdings Report".to_string(),
                                test_id: None,
                                on_click: EventHandler::new(move |_| {
                                    navigator.push(Route::WalletReport {
                                        wallet_id: wallet_id_for_kebab,
                                        start: None,
                                        end: None,
                                    });
                                }),
                                danger: false,
                                disabled: false,
                                title: None,
                            },
                            KebabMenuItem {
                                label: "Rename".to_string(),
                                test_id: None,
                                on_click: EventHandler::new(move |_| editing_label.set(true)),
                                danger: false,
                                disabled: false,
                                title: None,
                            },
                            KebabMenuItem {
                                label: "Delete".to_string(),
                                test_id: None,
                                on_click: EventHandler::new(move |_| show_delete_wallet_confirm.set(true)),
                                danger: true,
                                disabled: false,
                                title: None,
                            },
                        ],
                    }
                }
            }

            if !collapsed {
                div { class: "wallet-card-body",
                    if active_account_rows.is_empty() && inactive_account_rows.is_empty() {
                        p { class: "muted", "No linked accounts yet." }
                    } else {
                        if !active_account_rows.is_empty() {
                            div { class: "wallet-account-group wallet-account-group-active",
                                for row in active_account_rows {
                                    WalletAccountRowSection {
                                        account_view: row.scheme_view,
                                        show_change_wallet_action: row.show_change_wallet_action,
                                        current_wallet_id: wallet.id,
                                        current_wallet_has_accessors: wallet.has_accessors,
                                        destination_wallets: destination_wallets.clone(),
                                        on_action_error,
                                        on_refresh,
                                    }
                                }
                            }
                        }
                        if !inactive_account_rows.is_empty() {
                            div { class: "wallet-account-group wallet-account-group-inactive",
                                div { class: "wallet-account-group-heading",
                                    span { "Inactive accounts" }
                                }
                                for row in inactive_account_rows {
                                    WalletAccountRowSection {
                                        account_view: row.scheme_view,
                                        show_change_wallet_action: row.show_change_wallet_action,
                                        current_wallet_id: wallet.id,
                                        current_wallet_has_accessors: wallet.has_accessors,
                                        destination_wallets: destination_wallets.clone(),
                                        on_action_error,
                                        on_refresh,
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if show_delete_wallet_confirm() {
                DeleteWalletConfirmDialog {
                    wallet_label: label.clone(),
                    on_confirm: move |delete_accounts| {
                        let wallet_id = wallet.id;
                        spawn(async move {
                            let auth_state = auth_state;
                            let banner_state = banner_state;
                            let request = DeleteWalletRequest {
                                wallet_id,
                                delete_accounts: DeleteAccountsChoice::new(delete_accounts),
                            };
                            if let Err(err) = delete_wallet(request).await {
                                if err.is_unauthorized() {
                                    super::helpers::handle_session_expired(
                                        auth_state,
                                        banner_state,
                                        "wallet delete",
                                    );
                                }
                                on_action_error.call(err.to_string());
                            } else {
                                on_refresh.call(());
                            }
                        });
                        show_delete_wallet_confirm.set(false);
                    },
                    on_cancel: move |_| show_delete_wallet_confirm.set(false),
                }
            }
        }
    }
}

fn wallet_value_summary_text(
    summary: &WalletValueSummaryView,
    number_format: NumberFormat,
) -> String {
    let total =
        format_current_value_for_display(&summary.priced_total, summary.currency, number_format);
    if summary.priced_asset_count == summary.total_asset_count {
        format!("{total} total")
    } else {
        format!(
            "{total} total · {}/{} priced",
            summary.priced_asset_count, summary.total_asset_count
        )
    }
}

fn split_account_rows_by_state(
    rows: Vec<AccountSchemeRow>,
) -> (Vec<AccountSchemeRow>, Vec<AccountSchemeRow>) {
    rows.into_iter()
        .partition(|row| !account_view_is_inactive(&row.scheme_view))
}

fn account_view_is_inactive(view: &AccountView) -> bool {
    match view {
        AccountView::Native(native) => native.account_state == AccountStateView::Inactive,
        AccountView::Manual(manual) => manual.account_state == AccountStateView::Inactive,
        AccountView::Custom(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_views::ManualAssetInstanceIdView;
    use crate::backend::{AccountBalanceStateView, ManualAssetAccountView};
    use crate::wallets::WalletAccountId;

    fn manual_row(label: &str, account_state: AccountStateView) -> AccountSchemeRow {
        AccountSchemeRow {
            scheme_view: AccountView::Manual(ManualAssetAccountView {
                account_id: WalletAccountId::new(),
                account_state,
                label: label.to_string(),
                asset_instance_id: ManualAssetInstanceIdView {
                    asset_id: label.to_string(),
                    network_id: "manual-mainnet".to_string(),
                },
                unit_code: label.to_string(),
                asset_name: label.to_string(),
                network_name: "Manual".to_string(),
                decimal_precision: 6,
                symbol: None,
                balance_state: AccountBalanceStateView::Unknown,
                current_value: None,
            }),
            show_change_wallet_action: true,
        }
    }

    #[test]
    fn split_account_rows_keeps_inactive_rows_after_active_rows() {
        let (active_rows, inactive_rows) = split_account_rows_by_state(vec![
            manual_row("Inactive", AccountStateView::Inactive),
            manual_row("Active", AccountStateView::Active),
        ]);

        assert_eq!(active_rows.len(), 1);
        assert_eq!(inactive_rows.len(), 1);
        assert!(matches!(
            &active_rows[0].scheme_view,
            AccountView::Manual(view) if view.label == "Active"
        ));
        assert!(matches!(
            &inactive_rows[0].scheme_view,
            AccountView::Manual(view) if view.label == "Inactive"
        ));
    }
}

use super::add_bitcoin::AddBitcoinAddressFlow;
use super::add_ethereum::AddEthereumAddressFlow;
use super::add_manual_asset::{AddManualAssetFlow, route_for_added_manual_asset};
use super::add_xpub::AddXpubFlow;
use super::helpers::{build_wallet_move_options, handle_session_expired};
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
use super::link_trezor::{LinkFlowMode, LinkTrezorFlow};
#[cfg(test)]
use super::sync_bridge::{SYNC_BRIDGE_CLEANUP_SCRIPT, SYNC_BRIDGE_SCRIPT};
use super::sync_bridge::{SyncBridgeSignals, use_sync_event_bridge};
use super::sync_state::{
    AccountSyncNowSignal, AccountSyncStateSignal, GlobalSyncInProgressSignal, SyncRunCompletion,
    build_account_sync_state_map,
};
use super::wallet_card::WalletCard;
use crate::backend::{
    AccountView, WalletsValueSummaryView, get_account_sync_snapshots, get_settings, get_wallets,
};
use crate::components::{EtherscanApiKeyNotice, format_current_value_for_display};
use crate::settings::SettingsState;
use crate::transactions::AccountSyncSnapshot;
use crate::wallets::{SyncedAssetId, WalletId};
use crate::{AuthState, BannerState};
use chrono::Utc;
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub(super) fn WalletsSection(
    mut link_trezor_requested: Signal<bool>,
    mut show_add_xpub: Signal<bool>,
    mut show_add_bitcoin: Signal<bool>,
    mut show_add_ethereum: Signal<bool>,
    mut show_add_manual_asset: Signal<bool>,
) -> Element {
    let banner_state = use_context::<BannerState>();
    let auth_state = use_context::<AuthState>();
    let navigator = use_navigator();
    let account_sync_state = use_context::<AccountSyncStateSignal>();
    let account_sync_now = use_context::<AccountSyncNowSignal>();
    let global_sync_in_progress = use_context::<GlobalSyncInProgressSignal>();
    let settings_state = use_context::<SettingsState>();
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    let mut link_flow = use_signal(|| None::<LinkFlowMode>);
    let mut action_error = use_signal(|| None::<String>);
    let last_run_completion = use_signal(|| None::<SyncRunCompletion>);
    let mut collapsed_wallets = use_signal(HashSet::<WalletId>::new);

    // Acknowledge the parameter even when Trezor is not compiled in.
    #[cfg(not(any(target_arch = "wasm32", feature = "desktop")))]
    let _ = link_trezor_requested;

    let mut account_sync_snapshots_resource =
        use_server_future(move || async move { get_account_sync_snapshots().await })?;
    let account_sync_snapshots_value = account_sync_snapshots_resource.value();
    let price_fetching_signal = settings_state.price_fetching_enabled;
    let currency_signal = settings_state.currency;
    let mut wallets_resource = use_server_future(move || {
        // Wallet fiat values are computed server-side from these settings.
        // Reading both signals here makes Dioxus refetch when either changes.
        let _ = price_fetching_signal();
        let _ = currency_signal();
        async move { get_wallets().await }
    })?;
    let wallets_value = wallets_resource.value();
    let settings_resource = use_server_future(move || async move { get_settings().await })?;
    let has_etherscan_api_key = settings_resource
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .is_some_and(|s| s.has_etherscan_api_key);
    let number_format = (settings_state.number_format)();
    let mut refresh_generation = use_signal(|| 0_u32);
    let refresh_wallet_data = Callback::new(move |()| {
        let generation = (*refresh_generation.peek()).wrapping_add(1);
        refresh_generation.set(generation);
        spawn(async move {
            let wallets = get_wallets().await;
            let account_sync_snapshots = get_account_sync_snapshots().await;
            if *refresh_generation.peek() == generation {
                // Keep the ready resources mounted: restart() re-suspends this
                // component and can make Dioxus reclaim its subtree twice.
                wallets_resource.set(Some(wallets));
                account_sync_snapshots_resource.set(Some(account_sync_snapshots));
            }
        });
    });

    let wallet_list = wallets_value.read();
    use_effect(move || {
        let banner_state = banner_state;
        let auth_state = auth_state;
        let mut action_error = action_error;
        let value = wallets_value.read().clone();
        if let Some(Err(err)) = value {
            if err.is_unauthorized() {
                handle_session_expired(auth_state, banner_state, "wallets list");
            }
            action_error.set(Some(err.to_string()));
        }
    });

    use_effect(move || {
        let banner_state = banner_state;
        let auth_state = auth_state;
        let mut action_error = action_error;
        let mut account_sync_state = account_sync_state;
        let mut account_sync_now = account_sync_now;
        let mut global_sync_in_progress = global_sync_in_progress;
        let value = account_sync_snapshots_value.read().clone();
        if let Some(result) = value {
            match result {
                Ok(snapshots) => {
                    let has_running_sync = snapshots.iter().any(AccountSyncSnapshot::is_running);
                    let previous_states = account_sync_state.peek().clone();
                    account_sync_state
                        .set(build_account_sync_state_map(&previous_states, snapshots));
                    global_sync_in_progress.set(has_running_sync);
                    account_sync_now.set(Some(Utc::now()));
                }
                Err(err) => {
                    if err.is_unauthorized() {
                        handle_session_expired(auth_state, banner_state, "account sync snapshots");
                    }
                    action_error.set(Some(err.to_string()));
                }
            }
        }
    });

    use_sync_event_bridge(
        SyncBridgeSignals {
            account_sync_state,
            account_sync_now,
            global_sync_in_progress,
            last_run_completion,
            action_error,
        },
        auth_state,
        banner_state,
        refresh_wallet_data,
    );

    // Handle link trezor trigger from parent ActionsSection
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    use_effect(move || {
        if link_trezor_requested() {
            link_flow.set(Some(LinkFlowMode::NewWallet));
            link_trezor_requested.set(false);
        }
    });

    rsx! {
        section { class: "wallet-section", "aria-label": "Wallets",
            if let Some(error) = action_error() {
                div { class: "alert alert-error",
                    strong { "Error: " }
                    "{error}"
                }
            }

            match &*wallet_list {
                None => rsx! {
                    div { class: "card skeleton-card",
                        div { class: "card-body",
                            div { class: "skeleton-line skeleton-line-title" }
                            div { class: "skeleton-line skeleton-line-short" }
                            div { class: "skeleton-line skeleton-line-medium" }
                        }
                    }
                    div { class: "card skeleton-card",
                        div { class: "card-body",
                            div { class: "skeleton-line skeleton-line-title" }
                            div { class: "skeleton-line skeleton-line-short" }
                            div { class: "skeleton-line skeleton-line-medium" }
                        }
                    }
                },
                Some(Err(_)) => {
                    rsx! {
                        div { class: "card",
                            div { class: "card-body", "Failed to load wallets." }
                        }
                    }
                }
                Some(Ok(response)) => {
                    let wallets = &response.wallets;
                    let account_limit = &response.account_limit;
                    let wallet_move_options = build_wallet_move_options(wallets);
                    let wallet_ids: Vec<WalletId> = wallets.iter().map(|wallet| wallet.id).collect();
                    let collapsed_count = {
                        let collapsed = collapsed_wallets.read();
                        wallet_ids.iter().filter(|id| collapsed.contains(id)).count()
                    };
                    let all_collapsed = !wallet_ids.is_empty() && collapsed_count == wallet_ids.len();
                    let none_collapsed = collapsed_count == 0;
                    let wallet_ids_for_collapse = wallet_ids.clone();
                    let net_worth = response.value_summary.as_ref().map(|summary| {
                        (
                            format_current_value_for_display(
                                &summary.priced_total,
                                summary.currency,
                                number_format,
                            ),
                            net_worth_sub_text(summary),
                        )
                    });
                    let has_ethereum_account = wallets.iter().any(|wallet| {
                        wallet.accounts.iter().any(|account| {
                            matches!(account, AccountView::Native(n) if n.asset == SyncedAssetId::Ethereum)
                        })
                    });
                    if wallets.is_empty() {
                        rsx! {
                            div { class: "card empty-state",
                                div { class: "card-body",
                                    super::super::EmptyWalletIllustration {}
                                    p { class: "empty-state-heading", "data-testid": "wallets-empty-state-title", "No wallets linked yet." }
                                    p { class: "empty-state-body", "data-testid": "wallets-empty-state-body", "Use the + Add button above to get started." }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            if has_ethereum_account && !has_etherscan_api_key {
                                EtherscanApiKeyNotice {}
                            }
                            if let Some((total, sub)) = net_worth.as_ref() {
                                section { class: "wallet-value-overview", "aria-label": "Net worth",
                                    span { class: "wallet-value-overview-label", "Net worth" }
                                    strong { class: "wallet-value-overview-total", "{total}" }
                                    p { class: "wallet-value-overview-sub", "{sub}" }
                                }
                            }
                            div { class: "wallet-list-toolbar",
                                if account_limit.inactive_count > 0
                                    && let Some(upgrade_call_to_action) =
                                        account_limit.upgrade_call_to_action.as_deref()
                                {
                                    div { class: "account-limit-summary-group",
                                        a {
                                            class: "account-limit-upgrade-cta upgrade-link",
                                            href: "/payments",
                                            "{upgrade_call_to_action}"
                                        }
                                    }
                                }
                                div { class: "wallet-list-controls",
                                    button {
                                        class: "btn ghost wallet-list-control-btn",
                                        type: "button",
                                        disabled: all_collapsed,
                                        onclick: move |_| {
                                            collapsed_wallets.set(
                                                wallet_ids_for_collapse.iter().copied().collect::<HashSet<_>>(),
                                            );
                                        },
                                        "Collapse all"
                                    }
                                    button {
                                        class: "btn ghost wallet-list-control-btn",
                                        type: "button",
                                        disabled: none_collapsed,
                                        onclick: move |_| collapsed_wallets.with_mut(|ids| ids.clear()),
                                        "Expand all"
                                    }
                                }
                            }
                            div { class: "wallet-list",
                                for wallet in wallets.iter().cloned() {
                                    {
                                        let wallet_id = wallet.id;
                                        let is_collapsed = collapsed_wallets.read().contains(&wallet_id);
                                        rsx! {
                                            WalletCard {
                                                wallet: wallet.clone(),
                                                wallet_move_options: wallet_move_options.clone(),
                                                number_format,
                                                collapsed: is_collapsed,
                                                on_toggle_collapsed: move |_| {
                                                    collapsed_wallets.with_mut(|ids| {
                                                        if !ids.insert(wallet_id) {
                                                            ids.remove(&wallet_id);
                                                        }
                                                    });
                                                },
                                                on_action_error: move |err| action_error.set(Some(err)),
                                                on_refresh: move |_| refresh_wallet_data.call(()),
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            {
                #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
                {
                    if let Some(mode) = link_flow() {
                        rsx! {
                            LinkTrezorFlow {
                                mode,
                                on_complete: move |_| {
                                    link_flow.set(None);
                                    refresh_wallet_data.call(());
                                },
                                on_cancel: move |_| link_flow.set(None),
                            }
                        }
                    } else {
                        rsx! {}
                    }
                }
                #[cfg(not(any(target_arch = "wasm32", feature = "desktop")))]
                { rsx! {} }
            }

            if show_add_xpub() {
                AddXpubFlow {
                    default_wallet_id: None,
                    on_complete: move |_| {
                        show_add_xpub.set(false);
                        refresh_wallet_data.call(());
                    },
                    on_cancel: move |_| show_add_xpub.set(false),
                }
            }

            if show_add_bitcoin() {
                AddBitcoinAddressFlow {
                    default_wallet_id: None,
                    on_complete: move |_| {
                        show_add_bitcoin.set(false);
                        refresh_wallet_data.call(());
                    },
                    on_cancel: move |_| show_add_bitcoin.set(false),
                }
            }

            if show_add_ethereum() {
                AddEthereumAddressFlow {
                    default_wallet_id: None,
                    on_complete: move |_| {
                        show_add_ethereum.set(false);
                        refresh_wallet_data.call(());
                    },
                    on_cancel: move |_| show_add_ethereum.set(false),
                }
            }

            if show_add_manual_asset() {
                AddManualAssetFlow {
                    default_wallet_id: None,
                    on_complete: move |account_id| {
                        show_add_manual_asset.set(false);
                        refresh_wallet_data.call(());
                        navigator.push(route_for_added_manual_asset(account_id));
                    },
                    on_cancel: move |_| show_add_manual_asset.set(false),
                }
            }

        }
    }
}

/// Attribution line under the net-worth figure: wallet count, plus an honest
/// count of unpriced assets when the total is partial. No price source is named
/// here — the figure speaks for itself.
fn net_worth_sub_text(summary: &WalletsValueSummaryView) -> String {
    let wallets = if summary.total_wallet_count == 1 {
        "across 1 wallet".to_string()
    } else {
        format!("across {} wallets", summary.total_wallet_count)
    };
    if summary.priced_asset_count == summary.total_asset_count {
        wallets
    } else {
        let unpriced = summary
            .total_asset_count
            .saturating_sub(summary.priced_asset_count);
        let noun = if unpriced == 1 { "asset" } else { "assets" };
        format!("{wallets} · {unpriced} {noun} without a price")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_bridge_script_keeps_single_cleanable_browser_bridge() {
        assert!(SYNC_BRIDGE_SCRIPT.contains("previousBridge.close()"));
        assert!(SYNC_BRIDGE_SCRIPT.contains("source.close()"));
        assert!(SYNC_BRIDGE_SCRIPT.contains("clearInterval(intervalId)"));
        assert!(SYNC_BRIDGE_SCRIPT.contains("await stopPromise"));
        assert!(SYNC_BRIDGE_CLEANUP_SCRIPT.contains("bridge.close()"));
    }
}

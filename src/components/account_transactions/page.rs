use super::filters::TransactionStatusFilterRow;
use super::helpers::{
    AccountTransactionsLoader, ActiveFilters, ManualAssertionFormMode, ManualAssertionFormState,
    amount_context_for_custom_response, amount_context_for_response,
    build_transaction_filters_query, current_year_to_date_range, dispatch_date_range_filter_event,
    format_balance_date, format_balance_reliability_display, format_custom_balance_state_display,
    format_native_balance_state_display, handle_session_expired, history_pages, history_sort,
    local_today_in_timezone, manual_assertion_precision_helper_text, manual_sync_outcome_message,
    route_for_account_transactions, should_sync_initial_account_response,
};
use super::identity::{AccountIdentity, AccountIdentitySection};
use super::manual_assertions::{ManualAssertionEditorModal, ManualAssertionsSection};
use super::native_table::TransactionsTableSection;
use super::sync_control::SyncControlCard;

use super::super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterPolicy, DateRangeRouteParams, DateRangeSelection,
    initialize_date_range_filter, transition_date_range_filter,
};
use super::super::date_range_toolbar::DateRangeToolbar;
use super::super::form_helpers::{
    begin_submit, finish_submit, is_form_field_error, primary_field_or_message,
};
use super::super::wallets::{
    AccountAddressesLoader, AccountAddressesModal, AccountSyncStatusPill,
    AddressSchemeDeleteConfirmDialog, ChangeWalletInline, KebabMenu, KebabMenuItem, LabelEditor,
    SyncBridgeSignals, SyncRunCompletion, WalletMoveOption, address_scheme_label,
    build_account_sync_state_map, build_wallet_move_options, parse_label_for_editor,
    use_sync_event_bridge,
};
use crate::backend::{
    add_manual_asset_balance_assertion, delete_manual_asset_balance_assertion,
    delete_wallet_account, get_account_sync_snapshots, get_account_transactions, get_settings,
    get_wallets, trigger_sync, update_account_label, update_manual_asset_balance_assertion,
};
use crate::components::EtherscanApiKeyNotice;
use crate::report_dates::{dial_year_range, displayed_calendar_year};
use crate::timezone::format_timestamp;
use crate::transactions::{
    EtherscanHistoryStatus, RawTransactionSyncScope, RawTransactionSyncTriggerRequest,
    RawTransactionSyncTriggerSource,
};
use crate::wallets::requests::TransactionHistoryCoverageNoticeView;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AddManualAssetBalanceAssertionRequest, DeleteAccountRequest,
    DeleteManualAssetBalanceAssertionRequest, ManualAssetBalanceAssertionRowResponse, RawLabel,
    RawManualAssetAssertionNote, RawManualAssetBalance, ReportDateParam, SyncedAssetId,
    TransactionSortDirection, UpdateAccountLabelRequest, UpdateManualAssetBalanceAssertionRequest,
    WalletAccountHistoryResponse, WalletAccountId,
};
use crate::{AuthState, BannerState, Route};
use chrono::{DateTime, Datelike, Utc};
use dioxus::prelude::*;

const WALLETS_CSS: Asset = asset!("/assets/wallets.css");

#[component]
pub(crate) fn AccountTransactions(
    account_id: WalletAccountId,
    start: Option<String>,
    end: Option<String>,
) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let settings_state = use_context::<crate::settings::SettingsState>();
    let sync_state: crate::components::wallets::AccountSyncStateSignal =
        use_signal(std::collections::HashMap::new);
    use_context_provider(|| sync_state);
    let navigator = use_navigator();
    // Default a bare route to the current calendar year (year-to-date) so the
    // page opens on a year like the wallet report, not an empty custom range.
    let raw_route = DateRangeRouteParams::new(start, end);
    let current_route = if raw_route.is_empty() {
        current_year_to_date_range(local_today_in_timezone(
            Utc::now(),
            (settings_state.timezone)(),
        ))
        .map(|range| {
            let selection = DateRangeSelection::Range(range);
            DateRangeRouteParams::new(selection.start_query_value(), selection.end_query_value())
        })
        .unwrap_or(raw_route)
    } else {
        raw_route
    };

    // Provide sync-now context for the address modal sync status cells
    let mut account_sync_now: Signal<Option<DateTime<Utc>>> = use_signal(|| None);
    use_context_provider(|| account_sync_now);
    use_effect(move || {
        account_sync_now.set(Some(Utc::now()));
    });

    let loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut response = use_signal(|| None::<WalletAccountHistoryResponse>);
    let mut initial_synced_account_id = use_signal(|| None::<WalletAccountId>);

    // Signals for native account kebab actions
    let mut editing_label = use_signal(|| false);
    let mut show_delete_confirm = use_signal(|| false);
    let mut show_change_wallet = use_signal(|| false);
    let mut change_wallet_destinations = use_signal(|| None::<Vec<WalletMoveOption>>);
    let change_wallet_loading = use_signal(|| false);
    let mut show_addresses_modal = use_signal(|| false);
    let addresses_loading = use_signal(|| false);
    let addresses_error = use_signal(|| None::<String>);
    let addresses_page = use_signal(|| None::<crate::wallets::GetAccountAddressesResponse>);
    let sync_slot_submitting = use_signal(|| false);
    let global_sync_in_progress = use_signal(|| false);
    let last_sync_run_completion = use_signal(|| None::<SyncRunCompletion>);
    let bridge_error = use_signal(|| None::<String>);
    let mut manual_sync_pending_run =
        use_signal(|| None::<crate::transactions::TransactionSyncRunId>);
    let mut manual_sync_outcome = use_signal(|| None::<String>);

    let initial_filter =
        initialize_date_range_filter(DateRangeFilterPolicy::OptionalRange, current_route.clone());
    let initial_selection = initial_filter.state.selection();
    let initial_pending_route_selection = match initial_filter.effect {
        DateRangeFilterEffect::None => None,
        DateRangeFilterEffect::ReplaceRoute(selection) => Some(selection),
    };
    let mut filter_state = use_signal(|| initial_filter.state.clone());
    let mut pending_route_selection = use_signal(|| initial_pending_route_selection);
    let mut pending_transactions_reload = use_signal(|| None::<DateRangeSelection>);
    let mut filters = use_signal(ActiveFilters::default);
    let mut custom_form = use_signal(|| None::<ManualAssertionFormState>);
    let mut custom_form_field_error = use_signal(|| None::<String>);
    let mut custom_form_save_error = use_signal(|| None::<String>);
    let custom_form_submitting = use_signal(|| false);

    let loader = AccountTransactionsLoader {
        account_id,
        auth_state,
        banner_state,
        loading,
        error,
        response,
    };

    if filter_state.peek().route() != &current_route {
        let current_state = filter_state.peek().clone();
        let previous_route_selection = current_state.route().selection();
        let next_route_selection = current_route.selection();
        let outcome = transition_date_range_filter(
            DateRangeFilterPolicy::OptionalRange,
            &current_state,
            super::super::date_range_filter::DateRangeFilterEvent::RouteChanged(current_route),
        );

        if current_state != outcome.state {
            filter_state.set(outcome.state);
        }

        if let DateRangeFilterEffect::ReplaceRoute(selection) = outcome.effect
            && pending_route_selection.peek().as_ref() != Some(&selection)
        {
            pending_route_selection.set(Some(selection));
        }

        if previous_route_selection != next_route_selection
            && pending_transactions_reload.peek().as_ref() != Some(&next_route_selection)
        {
            pending_transactions_reload.set(Some(next_route_selection));
        }
    }

    use_effect(move || {
        if let Some(selection) = pending_route_selection() {
            navigator.replace(route_for_account_transactions(account_id, selection));
            pending_route_selection.set(None);
        }
    });

    use_effect(move || {
        if let Some(selection) = pending_transactions_reload() {
            if loading() {
                return;
            }

            let current_sort = response()
                .as_ref()
                .map(history_sort)
                .unwrap_or(TransactionSortDirection::Descending);
            loader.request(
                1,
                1,
                current_sort,
                &filters(),
                selection,
                (settings_state.timezone)(),
            );
            pending_transactions_reload.set(None);
        }
    });

    let initial_filters_value = build_transaction_filters_query(
        &ActiveFilters::default(),
        initial_selection,
        (settings_state.timezone)(),
    );
    let initial_resource = use_server_future(move || {
        let filters_value = initial_filters_value.clone();
        async move { get_account_transactions(account_id, Some(1), Some(1), None, filters_value).await }
    })?;
    let settings_resource = use_server_future(move || async move { get_settings().await })?;
    let has_etherscan_api_key = settings_resource
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .is_some_and(|s| s.has_etherscan_api_key);
    let mut account_sync_snapshots_resource =
        use_server_future(move || async move { get_account_sync_snapshots().await })?;
    let account_sync_snapshots_value = account_sync_snapshots_resource.value();

    // Populate the sync-state map from the static snapshot so the header status
    // mark and record read the same ladder as /wallets. Errors degrade silently
    // to "Not synced" — the page already surfaces transaction-loading errors.
    let mut sync_state_for_snapshots = sync_state;
    use_effect(move || {
        if let Some(Ok(snapshots)) = account_sync_snapshots_value.read().clone() {
            let previous = sync_state_for_snapshots.peek().clone();
            sync_state_for_snapshots.set(build_account_sync_state_map(&previous, snapshots));
        }
    });

    if should_sync_initial_account_response(account_id, *initial_synced_account_id.peek())
        && let Some(initial_result) = initial_resource()
    {
        initial_synced_account_id.set(Some(account_id));
        match initial_result {
            Ok(initial_response) => response.set(Some(initial_response)),
            Err(err) => {
                if err.is_unauthorized() {
                    handle_session_expired(auth_state, banner_state, "account transactions");
                }
                error.set(Some(err.to_string()));
            }
        }
    }

    let current_sort = response()
        .as_ref()
        .map(history_sort)
        .unwrap_or(TransactionSortDirection::Descending);
    let number_format = (settings_state.number_format)();
    let date_format = (settings_state.date_time_format)();
    let timezone = (settings_state.timezone)();
    let preset_today = local_today_in_timezone(Utc::now(), timezone);
    let this_year_preset = current_year_to_date_range(preset_today);
    let current_year = preset_today.year();
    let filter_state_snapshot = filter_state();
    let current_route_selection = filter_state_snapshot.route().selection();

    use_sync_event_bridge(
        SyncBridgeSignals {
            account_sync_state: sync_state,
            account_sync_now,
            global_sync_in_progress,
            last_run_completion: last_sync_run_completion,
            action_error: bridge_error,
        },
        auth_state,
        banner_state,
        Callback::new(move |()| {
            account_sync_snapshots_resource.restart();
            let (pending_page, confirmed_page) =
                response().as_ref().map(history_pages).unwrap_or((1, 1));
            let current_sort = response()
                .as_ref()
                .map(history_sort)
                .unwrap_or(TransactionSortDirection::Descending);
            loader.request(
                pending_page,
                confirmed_page,
                current_sort,
                &filters(),
                current_route_selection,
                timezone,
            );
        }),
    );

    use_effect(move || {
        let Some(completion) = last_sync_run_completion() else {
            return;
        };
        let Some(pending_run_id) = *manual_sync_pending_run.peek() else {
            return;
        };
        if completion.run_id != Some(pending_run_id) {
            return;
        }
        manual_sync_outcome.set(Some(manual_sync_outcome_message(
            &completion,
            timezone.into(),
        )));
        manual_sync_pending_run.set(None);
    });
    let active_range = match current_route_selection {
        DateRangeSelection::Range(range) => Some(range),
        DateRangeSelection::Empty => None,
    };
    let displayed_year =
        active_range.and_then(|range| displayed_calendar_year(range, this_year_preset));
    let year_label = displayed_year
        .map(|year| year.to_string())
        .unwrap_or_else(|| "Custom range".to_string());
    let disable_previous_year = displayed_year.is_none();
    let disable_next_year = displayed_year.is_none_or(|year| year >= current_year);
    let show_this_year = displayed_year != Some(current_year);
    let custom_range_open = displayed_year.is_none();
    let start_input_value = filter_state_snapshot.start_input_value().to_string();
    let end_input_value = filter_state_snapshot.end_input_value().to_string();
    let validation_message = filter_state_snapshot
        .validation_message()
        .map(str::to_string);
    rsx! {
        document::Stylesheet { href: WALLETS_CSS }

        div { class: "page-container transactions-page",
            // Page header with wallet/account heading, back link, and balance
            div { class: "page-header transactions-page-header",
                if let Some(data) = response() {
                    match data {
                        WalletAccountHistoryResponse::Native(data) => {
                            let show_etherscan_notice =
                                data.asset == SyncedAssetId::Ethereum && !has_etherscan_api_key;
                            let amount_context = amount_context_for_response(&data, number_format);
                            let opening_balance_display = format_native_balance_state_display(
                                &data.opening_balance_state,
                                data.asset,
                                &None,
                                &amount_context,
                                number_format,
                            );
                            let opening_balance_display = format_balance_reliability_display(
                                opening_balance_display,
                                &data.opening_balance_reliability,
                            );
                            let closing_balance_display = format_native_balance_state_display(
                                &data.closing_balance_state,
                                data.asset,
                                &None,
                                &amount_context,
                                number_format,
                            );
                            let closing_balance_display = format_balance_reliability_display(
                                closing_balance_display,
                                &data.closing_balance_reliability,
                            );
                            let opening_date_display = data
                                .opening_balance_date
                                .as_deref()
                                .map(|value| format_balance_date(value, date_format));
                            let closing_date_display = data
                                .closing_balance_date
                                .as_deref()
                                .map(|value| format_balance_date(value, date_format));
                            let current_balance_display = format_native_balance_state_display(
                                &data.current_balance_state,
                                data.asset,
                                &None,
                                &amount_context,
                                number_format,
                            );
                            let current_balance_checked_at_display = data
                                .current_balance_checked_at
                                .as_deref()
                                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                                .map(|value| {
                                    format_timestamp(
                                        &value.with_timezone(&Utc),
                                        timezone.into(),
                                        date_format,
                                    )
                                });
                            let current_balance_unavailable = matches!(
                                &*data.current_balance_state,
                                crate::backend::NativeBalanceStateView::Unknown
                            );

                            let scheme_label = address_scheme_label(data.address_scheme).to_string();
                            let native_account_id = data.native_account_id;
                            let address_scheme = data.address_scheme;
                            let current_wallet_id = data.wallet_id;
                            let current_account_label = data.account_label.clone().unwrap_or_default();
                            let asset = data.asset;
                            let network = data.network;
                            let _sync_slot = data.sync_slot.clone();
                            let manual_sync = (*data.manual_sync).clone();

                            let addresses_loader = AccountAddressesLoader {
                                account_id: native_account_id,
                                address_scheme,
                                auth_state,
                                banner_state,
                                addresses_loading,
                                addresses_error,
                                addresses_page,
                            };

                            let mut kebab_items = vec![KebabMenuItem {
                                label: "Rename".to_string(),
                                test_id: None,
                                on_click: EventHandler::new(move |_| editing_label.set(true)),
                                danger: false,
                                disabled: false,
                                title: None,
                            }];
                            kebab_items.push(KebabMenuItem {
                                label: "Change Wallet".to_string(),
                                test_id: None,
                                on_click: EventHandler::new(move |_| {
                                    show_change_wallet.set(true);
                                    if change_wallet_destinations.peek().is_none() {
                                        let mut change_wallet_loading = change_wallet_loading;
                                        change_wallet_loading.set(true);
                                        spawn(async move {
                                            match get_wallets().await {
                                                Ok(wallets) => {
                                                    let options = build_wallet_move_options(&wallets.wallets);
                                                    change_wallet_destinations.set(Some(options));
                                                }
                                                Err(err) => {
                                                    if err.is_unauthorized() {
                                                        handle_session_expired(auth_state, banner_state, "wallet move options");
                                                    }
                                                    dioxus::logger::tracing::warn!(error = %err, "Failed to load wallet move options");
                                                }
                                            }
                                            change_wallet_loading.set(false);
                                        });
                                    }
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
                                if show_etherscan_notice {
                                    EtherscanApiKeyNotice {}
                                }
                                div { class: "tx-header-top-row",
                                    div { class: "tx-header-title-group",
                                        h1 { class: "tx-header-heading",
                                            Link {
                                                class: "tx-header-wallet-label",
                                                to: Route::WalletReport {
                                                    wallet_id: data.wallet_id,
                                                    start: None,
                                                    end: None,
                                                },
                                                "{data.wallet_label}"
                                            }
                                            if let Some(label) = &data.account_label {
                                                span { class: "tx-header-separator", " / " }
                                                span { class: "tx-header-account-label", "{label}" }
                                            }
                                        }
                                        AccountSyncStatusPill { account_id: native_account_id }
                                    }
                                    div { class: "tx-header-actions",
                                        {
                                            let (is_syncing, has_retry) = {
                                                let sync_map = sync_state.read();
                                                let syncing = sync_map.get(&native_account_id).is_some_and(|s| s.is_any_integration_active())
                                                    || manual_sync_pending_run.read().is_some();
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
                                                                Ok(sync_response) => {
                                                                    manual_sync_pending_run.set(Some(sync_response.sync_run_id));
                                                                    manual_sync_outcome.set(None);
                                                                }
                                                                Err(err) => {
                                                                    if err.is_unauthorized() {
                                                                        handle_session_expired(auth_state, banner_state, "sync trigger");
                                                                    }
                                                                    error.set(Some(err.to_string()));
                                                                }
                                                            }
                                                            sync_slot_submitting.set(false);
                                                        });
                                                    },
                                                    crate::components::RefreshIcon {}
                                                }
                                                if let Some(outcome) = manual_sync_outcome() {
                                                    span {
                                                        class: "muted manual-sync-outcome",
                                                        "data-testid": "manual-sync-outcome",
                                                        "{outcome}"
                                                    }
                                                }
                                            }
                                        }
                                        KebabMenu {
                                            aria_label: "Account actions".to_string(),
                                            items: kebab_items,
                                        }
                                    }
                                }

                                if editing_label() {
                                    LabelEditor {
                                        current: parse_label_for_editor(&current_account_label, ACCOUNT_LABEL_MAX_LENGTH, "Account"),
                                        max_len: ACCOUNT_LABEL_MAX_LENGTH,
                                        on_save: move |label: RawLabel| {
                                            spawn(async move {
                                                let request = UpdateAccountLabelRequest { account_id, label };
                                                match update_account_label(request).await {
                                                    Ok(_) => {
                                                        let (pending_page, confirmed_page) = response()
                                                            .as_ref()
                                                            .map(history_pages)
                                                            .unwrap_or((1, 1));
                                                        loader.request(
                                                            pending_page,
                                                            confirmed_page,
                                                            current_sort,
                                                            &filters(),
                                                            current_route_selection,
                                                            timezone,
                                                        );
                                                    }
                                                    Err(err) => {
                                                        if err.is_unauthorized() {
                                                            handle_session_expired(auth_state, banner_state, "account label update");
                                                        }
                                                        error.set(Some(err.to_string()));
                                                    }
                                                }
                                            });
                                            editing_label.set(false);
                                        },
                                        on_cancel: move |_| editing_label.set(false),
                                    }
                                }

                                if show_change_wallet() {
                                    if change_wallet_loading() {
                                        p { class: "muted", "Loading wallets..." }
                                    } else if let Some(destinations) = change_wallet_destinations() {
                                        ChangeWalletInline {
                                            account_id,
                                            current_wallet_id,
                                            current_wallet_has_accessors: false,
                                            destination_wallets: destinations,
                                            on_refresh: move |_| {
                                                let (pending_page, confirmed_page) = response()
                                                    .as_ref()
                                                    .map(history_pages)
                                                    .unwrap_or((1, 1));
                                                loader.request(
                                                    pending_page,
                                                    confirmed_page,
                                                    current_sort,
                                                    &filters(),
                                                    current_route_selection,
                                                    timezone,
                                                );
                                            },
                                            on_close: move |_| show_change_wallet.set(false),
                                        }
                                    }
                                }

                                if show_addresses_modal() {
                                    AccountAddressesModal {
                                        scheme_label: scheme_label.clone(),
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
                                        scheme_label: scheme_label.clone(),
                                        on_confirm: move |_| {
                                            spawn(async move {
                                                let request = DeleteAccountRequest { account_id };
                                                match delete_wallet_account(request).await {
                                                    Ok(_) => {
                                                        navigator.replace(Route::Wallets);
                                                    }
                                                    Err(err) => {
                                                        if err.is_unauthorized() {
                                                            handle_session_expired(auth_state, banner_state, "account delete");
                                                        }
                                                        error.set(Some(err.to_string()));
                                                        show_delete_confirm.set(false);
                                                    }
                                                }
                                            });
                                        },
                                        on_cancel: move |_| show_delete_confirm.set(false),
                                    }
                                }
                                if data.is_free_tier {
                                    p { class: "tx-header-balance tx-header-current-balance",
                                        span { class: "tx-header-balance-label",
                                            if current_balance_unavailable {
                                                "Current balance (Awaiting first sync): "
                                            } else if let Some(date_display) = current_balance_checked_at_display {
                                                "Current balance as of {date_display}: "
                                            } else {
                                                "Current balance (check time unavailable): "
                                            }
                                        }
                                        span { class: "tx-header-balance-value", "{current_balance_display}" }
                                    }
                                } else {
                                    p { class: "tx-header-balance tx-header-opening-balance",
                                        if let Some(date_display) = opening_date_display {
                                            span { class: "tx-header-balance-label",
                                                "Opening balance on {date_display}: "
                                            }
                                        } else {
                                            span { class: "tx-header-balance-label",
                                                "Opening balance (Awaiting first sync): "
                                            }
                                        }
                                        span { class: "tx-header-balance-value",
                                            "{opening_balance_display}"
                                        }
                                        if matches!(
                                            data.opening_balance_state,
                                            crate::backend::NativeBalanceStateView::Unknown
                                        ) {
                                            span { class: "muted tx-header-balance-hint",
                                                " (history not synced yet)"
                                            }
                                        }
                                    }
                                    p { class: "tx-header-balance tx-header-closing-balance",
                                        if let Some(date_display) = closing_date_display {
                                            span { class: "tx-header-balance-label",
                                                "Closing balance on {date_display}: "
                                            }
                                        } else {
                                            span { class: "tx-header-balance-label",
                                                "Closing balance (Awaiting first sync): "
                                            }
                                        }
                                        span { class: "tx-header-balance-value",
                                            "{closing_balance_display}"
                                        }
                                    }
                                }
                            }
                        }
                        WalletAccountHistoryResponse::Custom(data) => {
                            let amount_context =
                                amount_context_for_custom_response(&data, number_format);
                            let opening_balance_display = format_custom_balance_state_display(
                                &data.opening_balance_state,
                                &data.unit_code,
                                &None,
                                &amount_context,
                                number_format,
                            );
                            let closing_balance_display = format_custom_balance_state_display(
                                &data.closing_balance_state,
                                &data.unit_code,
                                &None,
                                &amount_context,
                                number_format,
                            );
                            let opening_date_display = data
                                .opening_balance_date
                                .as_deref()
                                .map(|value| format_balance_date(value, date_format));
                            let closing_date_display = data
                                .closing_balance_date
                                .as_deref()
                                .map(|value| format_balance_date(value, date_format));

                            rsx! {
                                div { class: "tx-header-top-row",
                                    h1 { class: "tx-header-heading",
                                        Link {
                                            class: "tx-header-wallet-label",
                                            to: Route::WalletReport {
                                                wallet_id: data.wallet_id,
                                                start: None,
                                                end: None,
                                            },
                                            "{data.wallet_label}"
                                        }
                                        span { class: "tx-header-separator", " / " }
                                        span { class: "tx-header-account-label", "{data.account_label}" }
                                    }
                                }
                                p { class: "tx-header-balance tx-header-opening-balance",
                                    if let Some(date_display) = opening_date_display {
                                        span { class: "tx-header-balance-label",
                                            "Opening balance on {date_display}: "
                                        }
                                    } else {
                                        span { class: "tx-header-balance-label",
                                            "Opening balance (Awaiting first assertion): "
                                        }
                                    }
                                    span { class: "tx-header-balance-value",
                                        "{opening_balance_display}"
                                    }
                                }
                                p { class: "tx-header-balance tx-header-closing-balance",
                                    if let Some(date_display) = closing_date_display {
                                        span { class: "tx-header-balance-label",
                                            "Closing balance on {date_display}: "
                                        }
                                    } else {
                                        span { class: "tx-header-balance-label",
                                            "Closing balance (Awaiting first assertion): "
                                        }
                                    }
                                    span { class: "tx-header-balance-value",
                                        "{closing_balance_display}"
                                    }
                                }
                            }
                        }
                    }
                } else {
                    div { class: "tx-header-top-row",
                        h1 { class: "tx-header-heading", "Transactions" }
                    }
                }
            }

            {
                let identity_props = response.read().as_ref().map(|data| match data {
                    WalletAccountHistoryResponse::Native(data) => {
                        let identity_loader = AccountAddressesLoader {
                            account_id: data.native_account_id,
                            address_scheme: data.address_scheme,
                            auth_state,
                            banner_state,
                            addresses_loading,
                            addresses_error,
                            addresses_page,
                        };
                        (
                            AccountIdentity::Native {
                                asset: data.asset,
                                network: data.network,
                                reference_kind: data.account_reference_kind,
                                reference: data.account_reference.clone(),
                                address_scheme: data.address_scheme,
                            },
                            Some(identity_loader),
                        )
                    }
                    WalletAccountHistoryResponse::Custom(data) => (
                        AccountIdentity::Manual {
                            unit_code: data.unit_code.clone(),
                            symbol: data.symbol.clone(),
                            decimal_precision: data.decimal_precision,
                            asset_name: data.asset_name.clone(),
                            network_name: data.network_name.clone(),
                        },
                        None,
                    ),
                });

                if let Some((identity, identity_loader)) = identity_props {
                    rsx! {
                        AccountIdentitySection {
                            identity,
                            on_view_addresses: move |_| {
                                if let Some(identity_loader) = identity_loader {
                                    show_addresses_modal.set(true);
                                    identity_loader.request_page(1);
                                }
                            },
                        }
                    }
                } else {
                    rsx! {}
                }
            }

            if let Some(error_message) = error() {
                div { class: "error-block",
                    p { "{error_message}" }
                    button {
                        class: "btn btn-outline",
                        r#type: "button",
                        disabled: loading(),
                        onclick: move |_| {
                            let (pending_page, confirmed_page) = response()
                                .as_ref()
                                .map(history_pages)
                                .unwrap_or((1, 1));
                            loader.request(
                                pending_page,
                                confirmed_page,
                                current_sort,
                                &filters(),
                                current_route_selection,
                                timezone,
                            );
                        },
                        "Retry"
                    }
                }
            }

            if response().as_ref().is_some_and(|data| {
                matches!(
                    data,
                    WalletAccountHistoryResponse::Native(native)
                        if native.etherscan_history_status == Some(EtherscanHistoryStatus::Gap)
                )
            }) {
                div { class: "history-gap-notice", "data-testid": "history-gap-notice",
                    "This account has a transaction history gap. Upgrade to import the missing history."
                }
            }

            if let Some(notice) = response().as_ref().and_then(|data| match data {
                WalletAccountHistoryResponse::Native(native) => {
                    native.transaction_history_coverage_notice.clone()
                }
                WalletAccountHistoryResponse::Custom(_) => None,
            }) {
                div {
                    class: "history-gap-notice",
                    "data-testid": "transaction-history-coverage-notice",
                    match notice {
                        TransactionHistoryCoverageNoticeView::Free {
                            approximate_unsynced_count,
                        } => rsx! {
                            "This account has approximately {approximate_unsynced_count} unsynced transactions. "
                            Link { to: Route::Payments, "Upgrade " }
                            "to sync transaction history."
                        },
                        TransactionHistoryCoverageNoticeView::Paid {
                            approximate_unsynced_count,
                            confirmed_synced_count,
                            max_transactions_per_account,
                        } => rsx! {
                            "This account has approximately {approximate_unsynced_count} unsynced transactions and {confirmed_synced_count} synced transactions. The internal limit of transactions per account is {max_transactions_per_account} and we have not yet provided a way to sync more. "
                            a { href: "mailto:hello@bitgarth.app", "Send us an email " }
                            "to let us know this should be a priority."
                        },
                    }
                }
            }

            if loading() && response().is_none() {
                div { class: "card skeleton-card",
                    div { class: "card-body",
                        div { class: "skeleton-line skeleton-line-title" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-medium" }
                    }
                }
            }

            {
                let show_sync_control = response().as_ref().is_some_and(|data| {
                    matches!(data, WalletAccountHistoryResponse::Native(n) if n.sync_control_enabled)
                });
                if show_sync_control {
                    rsx! {
                        SyncControlCard {
                            account_id,
                            loading: loading(),
                            on_complete: move |_| {
                                let (pending_page, confirmed_page) = response()
                                    .as_ref()
                                    .map(history_pages)
                                    .unwrap_or((1, 1));
                                loader.request(
                                    pending_page,
                                    confirmed_page,
                                    current_sort,
                                    &filters(),
                                    current_route_selection,
                                    timezone,
                                );
                            },
                        }
                    }
                } else {
                    rsx! {}
                }
            }

            if matches!(response(), Some(WalletAccountHistoryResponse::Native(_))) {
                div { class: "card",
                    div { class: "card-body",
                        DateRangeToolbar {
                            start_input_id: "account-transactions-start".to_string(),
                            start_input_value,
                            end_input_id: "account-transactions-end".to_string(),
                            end_input_value,
                                                                                    validation_message,
                            disable_date_inputs: loading(),
                            year_label,
                            disable_previous_year,
                            disable_next_year,
                            show_this_year,
                            custom_range_open,
                            on_start_change: move |value| {
                                dispatch_date_range_filter_event(
                                    DateRangeFilterPolicy::OptionalRange,
                                    filter_state,
                                    pending_route_selection,
                                    super::super::date_range_filter::DateRangeFilterEvent::StartEdited(value),
                                );
                            },
                            on_end_change: move |value| {
                                dispatch_date_range_filter_event(
                                    DateRangeFilterPolicy::OptionalRange,
                                    filter_state,
                                    pending_route_selection,
                                    super::super::date_range_filter::DateRangeFilterEvent::EndEdited(value),
                                );
                            },
                            on_this_year: move |_| {
                                if let Some(range) = this_year_preset {
                                    dispatch_date_range_filter_event(
                                        DateRangeFilterPolicy::OptionalRange,
                                        filter_state,
                                        pending_route_selection,
                                        super::super::date_range_filter::DateRangeFilterEvent::PresetChosen(range),
                                    );
                                }
                            },
                            on_previous_year: move |_| {
                                if let Some(year) = displayed_year
                                    && let Some(range) = dial_year_range(year - 1, current_year, this_year_preset)
                                {
                                    dispatch_date_range_filter_event(
                                        DateRangeFilterPolicy::OptionalRange,
                                        filter_state,
                                        pending_route_selection,
                                        super::super::date_range_filter::DateRangeFilterEvent::PresetChosen(range),
                                    );
                                }
                            },
                            on_next_year: move |_| {
                                if let Some(year) = displayed_year
                                    && let Some(range) = dial_year_range(year + 1, current_year, this_year_preset)
                                {
                                    dispatch_date_range_filter_event(
                                        DateRangeFilterPolicy::OptionalRange,
                                        filter_state,
                                        pending_route_selection,
                                        super::super::date_range_filter::DateRangeFilterEvent::PresetChosen(range),
                                    );
                                }
                            },
                            secondary_row: Some(rsx! {
                                TransactionStatusFilterRow {
                                    filters: filters(),
                                    loading: loading(),
                                    on_filters_change: move |new_filters: ActiveFilters| {
                                        filters.set(new_filters.clone());
                                        loader.request(
                                            1,
                                            1,
                                            current_sort,
                                            &new_filters,
                                            current_route_selection,
                                            timezone,
                                        );
                                    },
                                }
                            }),
                        }
                    }
                }
            }

            if let Some(form_state) = custom_form() {
                {
                    let precision_helper_text = match response() {
                        Some(WalletAccountHistoryResponse::Custom(data)) => {
                            manual_assertion_precision_helper_text(&data)
                        }
                        _ => String::new(),
                    };
                    let decimal_precision = match response() {
                        Some(WalletAccountHistoryResponse::Custom(data)) => data.decimal_precision,
                        _ => 0,
                    };
                    rsx! {
                        ManualAssertionEditorModal {
                            form_state: custom_form,
                            precision_helper_text,
                            decimal_precision,
                            field_error: custom_form_field_error,
                            save_error: custom_form_save_error,
                            submitting: custom_form_submitting,
                            on_submit: move |_| {
                                let Some(form_state) = custom_form.peek().clone() else {
                                    return;
                                };
                                if !begin_submit(custom_form_submitting) {
                                    return;
                                }

                                custom_form_field_error.set(None);
                                custom_form_save_error.set(None);

                                let asserted_on =
                                    match form_state.asserted_on.parse::<ReportDateParam>() {
                                        Ok(value) => value,
                                        Err(_) => {
                                            custom_form_field_error
                                                .set(Some("Date must use YYYY-MM-DD.".to_string()));
                                            finish_submit(custom_form_submitting);
                                            return;
                                        }
                                    };
                                let note = if form_state.note.trim().is_empty() {
                                    None
                                } else {
                                    Some(RawManualAssetAssertionNote::new(
                                        form_state.note.trim().to_string(),
                                    ))
                                };

                                match form_state.mode {
                                    ManualAssertionFormMode::Add => {
                                        let request = AddManualAssetBalanceAssertionRequest {
                                            account_id,
                                            asserted_on,
                                            balance: RawManualAssetBalance::new(
                                                form_state.balance.trim().to_string(),
                                            ),
                                            note,
                                        };
                                        spawn(async move {
                                            match add_manual_asset_balance_assertion(request).await {
                                                Ok(_) => {
                                                    custom_form.set(None);
                                                    loader.request(
                                                        1,
                                                        1,
                                                        current_sort,
                                                        &filters(),
                                                        current_route_selection,
                                                        timezone,
                                                    );
                                                }
                                                Err(err) if err.is_unauthorized() => {
                                                    handle_session_expired(
                                                        auth_state,
                                                        banner_state,
                                                        "custom balance assertion add",
                                                    );
                                                }
                                                Err(err) if is_form_field_error(&err) => {
                                                    let message = primary_field_or_message(
                                                        &err,
                                                        &["asserted_on", "balance", "note"],
                                                    );
                                                    custom_form_field_error.set(Some(message));
                                                }
                                                Err(err) => {
                                                    custom_form_save_error
                                                        .set(Some(err.to_string()))
                                                }
                                            }
                                            finish_submit(custom_form_submitting);
                                        });
                                    }
                                    ManualAssertionFormMode::Edit(assertion_id) => {
                                        let request = UpdateManualAssetBalanceAssertionRequest {
                                            assertion_id,
                                            account_id,
                                            asserted_on,
                                            balance: RawManualAssetBalance::new(
                                                form_state.balance.trim().to_string(),
                                            ),
                                            note,
                                        };
                                        spawn(async move {
                                            match update_manual_asset_balance_assertion(request)
                                                .await
                                            {
                                                Ok(_) => {
                                                    custom_form.set(None);
                                                    loader.request(
                                                        1,
                                                        1,
                                                        current_sort,
                                                        &filters(),
                                                        current_route_selection,
                                                        timezone,
                                                    );
                                                }
                                                Err(err) if err.is_unauthorized() => {
                                                    handle_session_expired(
                                                        auth_state,
                                                        banner_state,
                                                        "custom balance assertion update",
                                                    );
                                                }
                                                Err(err) if is_form_field_error(&err) => {
                                                    let message = primary_field_or_message(
                                                        &err,
                                                        &["asserted_on", "balance", "note"],
                                                    );
                                                    custom_form_field_error.set(Some(message));
                                                }
                                                Err(err) => {
                                                    custom_form_save_error
                                                        .set(Some(err.to_string()))
                                                }
                                            }
                                            finish_submit(custom_form_submitting);
                                        });
                                    }
                                }
                            },
                            on_cancel: move |_| {
                                let _ = form_state;
                                custom_form.set(None);
                                custom_form_field_error.set(None);
                                custom_form_save_error.set(None);
                            },
                        }
                    }
                }
            }

            if let Some(data) = response() {
                match data {
                    WalletAccountHistoryResponse::Native(data) => {
                        let amount_context = amount_context_for_response(&data, number_format);

                        rsx! {
                            if data.pending.total > 0 {
                                TransactionsTableSection {
                                    title: "Pending / Unconfirmed".to_string(),
                                    heading_status: Some("pending".to_string()),
                                    asset: data.asset,
                                    network: data.network,
                                    amount_context: amount_context.clone(),
                                    active_quote: None,
                                    number_format,
                                    table: data.pending.clone(),
                                    loading: loading(),
                                    sort_toggle: None::<TransactionSortDirection>,
                                    empty_hint: None::<crate::wallets::TransactionsEmptyHint>,
                                    on_page_change: move |page: u32| {
                                        loader.request(
                                            page,
                                            data.confirmed.page,
                                            current_sort,
                                            &filters(),
                                            current_route_selection,
                                            timezone,
                                        );
                                    },
                                    on_sort_toggle: move |_: TransactionSortDirection| {},
                                }
                            }

                            TransactionsTableSection {
                                title: "Confirmed".to_string(),
                                heading_status: Some("confirmed".to_string()),
                                asset: data.asset,
                                network: data.network,
                                amount_context,
                                active_quote: None,
                                number_format,
                                table: data.confirmed.clone(),
                                loading: loading(),
                                sort_toggle: Some(current_sort),
                                empty_hint: data.confirmed_empty_hint,
                                on_page_change: move |page: u32| {
                                    loader.request(
                                        data.pending.page,
                                        page,
                                        current_sort,
                                        &filters(),
                                        current_route_selection,
                                        timezone,
                                    );
                                },
                                on_sort_toggle: move |new_sort: TransactionSortDirection| {
                                    loader.request(
                                        data.pending.page,
                                        1,
                                        new_sort,
                                        &filters(),
                                        current_route_selection,
                                        timezone,
                                    );
                                },
                            }
                        }
                    }
                    WalletAccountHistoryResponse::Custom(data) => {
                        let amount_context = amount_context_for_custom_response(&data, number_format);

                        rsx! {
                            ManualAssertionsSection {
                                data: data.clone(),
                                amount_context,
                                active_quote: None,
                                number_format,
                                date_format,
                                loading: loading(),
                                on_add: move |_| {
                                    custom_form.set(Some(ManualAssertionFormState::for_add(
                                        filter_state.peek().route().selection(),
                                        preset_today,
                                    )));
                                    custom_form_field_error.set(None);
                                    custom_form_save_error.set(None);
                                },
                                on_edit: move |row: ManualAssetBalanceAssertionRowResponse| {
                                    custom_form.set(Some(ManualAssertionFormState::for_edit(&row)));
                                    custom_form_field_error.set(None);
                                    custom_form_save_error.set(None);
                                },
                                on_delete: move |assertion_id| {
                                    spawn(async move {
                                        match delete_manual_asset_balance_assertion(
                                            DeleteManualAssetBalanceAssertionRequest { assertion_id },
                                        )
                                        .await
                                        {
                                            Ok(_) => {
                                                loader.request(
                                                    1,
                                                    1,
                                                    current_sort,
                                                    &filters(),
                                                    current_route_selection,
                                                    timezone,
                                                );
                                            }
                                            Err(err) if err.is_unauthorized() => {
                                                handle_session_expired(
                                                    auth_state,
                                                    banner_state,
                                                    "custom balance assertion delete",
                                                );
                                            }
                                            Err(err) => {
                                                error.set(Some(err.to_string()));
                                            }
                                        }
                                    });
                                },
                                on_page_change: move |page: u32| {
                                    loader.request(
                                        1,
                                        page,
                                        current_sort,
                                        &filters(),
                                        current_route_selection,
                                        timezone,
                                    );
                                },
                                on_sort_toggle: move |new_sort: TransactionSortDirection| {
                                    loader.request(
                                        1,
                                        1,
                                        new_sort,
                                        &filters(),
                                        current_route_selection,
                                        timezone,
                                    );
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

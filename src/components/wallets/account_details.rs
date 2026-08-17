use super::super::{
    CheckIcon, ChevronLeftIcon, ChevronRightIcon, CloseIcon, CopyIcon, ExternalLinkIcon,
    format_balance_for_asset,
};
use super::helpers::{
    TxCountDisplay, WalletMoveOption, address_explorer_url, addresses_total_pages,
    copy_to_clipboard, format_sync_relative_time, logical_account_count_label,
    move_wallet_error_message, sync_status_error_message, transaction_count_display,
};
use super::sync_state::AccountSyncNowSignal;
use crate::backend::AccountTransactionCountsView;
use crate::components::form_helpers::{begin_submit, finish_submit};
use crate::settings::SettingsState;
use crate::transactions::AccountAddressSyncStatus;
use crate::wallets::{Network, RawLabel, WALLET_LABEL_MAX_LENGTH, WalletAccountId, WalletId};
use crate::{AuthState, BannerState};
use dioxus::prelude::*;
use std::rc::Rc;

#[component]
pub(crate) fn ChangeWalletInline(
    account_id: WalletAccountId,
    current_wallet_id: WalletId,
    current_wallet_has_accessors: bool,
    destination_wallets: Vec<WalletMoveOption>,
    on_refresh: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let mut selected_destination = use_signal(String::new);
    let mut new_wallet_label = use_signal(String::new);
    let mut inline_error = use_signal(|| None::<String>);
    let submitting = use_signal(|| false);

    let destination_wallets_for_select = destination_wallets.clone();
    let destination_wallets_for_create = destination_wallets.clone();

    let submit_new_wallet_move: Rc<dyn Fn()> = {
        let submitting_signal = submitting;
        let inline_error_signal = inline_error;
        let new_wallet_label = new_wallet_label;
        let selected_destination = selected_destination;
        let on_refresh = on_refresh;
        let on_close = on_close;
        Rc::new(move || {
            let submitting = submitting_signal;
            let mut inline_error = inline_error_signal;
            if submitting() {
                return;
            }

            let trimmed_label = new_wallet_label().trim().to_string();
            if trimmed_label.is_empty() {
                inline_error.set(Some("Wallet label is required.".to_string()));
                return;
            }
            if let Err(err) =
                crate::wallets::Label::parse_with_limit(&trimmed_label, WALLET_LABEL_MAX_LENGTH)
            {
                inline_error.set(Some(err.to_string()));
                return;
            }

            if !begin_submit(submitting) {
                return;
            }
            inline_error.set(None);
            let mut selected_destination = selected_destination;
            spawn(async move {
                let request = crate::wallets::MoveAccountRequest {
                    account_id,
                    destination: crate::wallets::MoveDestination::NewWallet {
                        label: RawLabel::new(trimmed_label),
                    },
                };

                match crate::backend::move_wallet_account(request).await {
                    Ok(_) => {
                        on_refresh.call(());
                        on_close.call(());
                    }
                    Err(err) => {
                        if err.is_unauthorized() {
                            super::helpers::handle_session_expired(
                                auth_state,
                                banner_state,
                                "account move",
                            );
                        }
                        inline_error.set(Some(move_wallet_error_message(&err)));
                        selected_destination.set(super::CREATE_NEW_WALLET_OPTION_VALUE.to_string());
                    }
                }

                finish_submit(submitting);
            });
        })
    };

    let submit_new_wallet_move_for_keydown = submit_new_wallet_move.clone();
    let submit_new_wallet_move_for_click = submit_new_wallet_move.clone();

    rsx! {
        div { class: "change-wallet-inline",
            div { class: "change-wallet-select-row",
                select {
                    class: "change-wallet-select",
                    value: "{selected_destination}",
                    disabled: submitting(),
                    onmounted: move |e| async move {
                        let _ = e.set_focus(true).await;
                    },
                    onchange: move |event| {
                        let selected_value = event.value();
                        selected_destination.set(selected_value.clone());
                        inline_error.set(None);

                        if selected_value.is_empty()
                            || selected_value == super::CREATE_NEW_WALLET_OPTION_VALUE
                            || submitting()
                        {
                            return;
                        }

                        let target_wallet_id = destination_wallets_for_select
                            .iter()
                            .find(|candidate| candidate.wallet_id.to_string() == selected_value)
                            .map(|candidate| candidate.wallet_id);

                        let Some(target_wallet_id) = target_wallet_id else {
                            inline_error.set(Some("Selected wallet was not found.".to_string()));
                            return;
                        };

                        if target_wallet_id == current_wallet_id {
                            inline_error.set(Some(
                                "Account is already in the selected wallet.".to_string(),
                            ));
                            return;
                        }

                        if !begin_submit(submitting) {
                            return;
                        }
                        inline_error.set(None);
                        let auth_state = auth_state;
                        let banner_state = banner_state;
                        let submitting = submitting;
                        let mut inline_error = inline_error;
                        let on_refresh = on_refresh;
                        let on_close = on_close;
                        spawn(async move {
                            let request = crate::wallets::MoveAccountRequest {
                                account_id,
                                destination: crate::wallets::MoveDestination::ExistingWallet {
                                    wallet_id: target_wallet_id,
                                },
                            };

                            match crate::backend::move_wallet_account(request).await {
                                Ok(_) => {
                                    on_refresh.call(());
                                    on_close.call(());
                                }
                                Err(err) => {
                                    if err.is_unauthorized() {
                                        super::helpers::handle_session_expired(
                                            auth_state,
                                            banner_state,
                                            "account move",
                                        );
                                    }
                                    inline_error.set(Some(move_wallet_error_message(&err)));
                                }
                            }

                            finish_submit(submitting);
                        });
                    },
                    option {
                        value: "",
                        disabled: true,
                        "Select destination wallet..."
                    }
                    for destination in destination_wallets_for_create.clone() {
                        option {
                            value: destination.wallet_id.to_string(),
                            "{destination.label} ({logical_account_count_label(destination.logical_account_count)})"
                        }
                    }
                    option {
                        value: super::CREATE_NEW_WALLET_OPTION_VALUE,
                        "Create new wallet..."
                    }
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: submitting(),
                    onclick: move |_| on_close.call(()),
                    "Cancel"
                }
            }

            if current_wallet_has_accessors {
                p { class: "change-wallet-warning",
                    "This account was linked via a hardware wallet. Moving it does not move device links to the destination wallet."
                }
            }

            if selected_destination() == super::CREATE_NEW_WALLET_OPTION_VALUE {
                div { class: "change-wallet-new-wallet-row",
                    input {
                        r#type: "text",
                        class: "change-wallet-new-wallet-input",
                        autocomplete: "off",
                        maxlength: WALLET_LABEL_MAX_LENGTH,
                        placeholder: "New wallet label",
                        value: "{new_wallet_label}",
                        disabled: submitting(),
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        oninput: move |event| {
                            new_wallet_label.set(event.value());
                            inline_error.set(None);
                        },
                        onkeydown: move |event| {
                            if event.key() == Key::Enter {
                                submit_new_wallet_move_for_keydown();
                            }
                        },
                    }
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: submitting(),
                        onclick: move |_| submit_new_wallet_move_for_click(),
                        if submitting() { "Moving..." } else { "Create & Move" }
                    }
                }
            }

            if let Some(error) = inline_error() {
                p { class: "error-text change-wallet-error", "{error}" }
            }
        }
    }
}

#[component]
pub(crate) fn AccountAddressesModal(
    scheme_label: String,
    asset: crate::wallets::SyncedAssetId,
    network: Network,
    addresses_page: Option<crate::wallets::GetAccountAddressesResponse>,
    loading: bool,
    error: Option<String>,
    on_close: EventHandler<()>,
    on_prev_page: EventHandler<()>,
    on_next_page: EventHandler<()>,
    on_retry: EventHandler<()>,
) -> Element {
    let current_page = addresses_page.as_ref().map(|page| page.page).unwrap_or(1);
    let page_size = addresses_page
        .as_ref()
        .map(|page| page.page_size)
        .unwrap_or(crate::wallets::DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE);
    let total = addresses_page.as_ref().map(|page| page.total).unwrap_or(0);
    let total_pages = addresses_total_pages(total, page_size);
    let can_prev = !loading && current_page > 1;
    let can_next = !loading && current_page < total_pages && total > 0;

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal account-addresses-modal",
                div { class: "modal-header",
                    h3 { "Account addresses ({scheme_label})" }
                    button {
                        class: "modal-close-btn",
                        r#type: "button",
                        "aria-label": "Close",
                        title: "Close",
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        onclick: move |_| on_close.call(()),
                        CloseIcon {}
                    }
                }
                div { class: "modal-body account-addresses-modal-body",
                    if let Some(error_text) = error {
                        div { class: "error-block",
                            p { "{error_text}" }
                            button {
                                class: "btn btn-outline",
                                r#type: "button",
                                disabled: loading,
                                onclick: move |_| on_retry.call(()),
                                "Retry"
                            }
                        }
                    }

                    if loading && addresses_page.is_none() {
                        p { class: "muted", "Loading addresses..." }
                    }

                    if let Some(page) = addresses_page {
                        if page.rows.is_empty() {
                            p { class: "muted", "No addresses found for this page." }
                        } else {
                            div { class: "account-addresses-table-wrap",
                                table { class: "account-addresses-table",
                                    thead {
                                        tr {
                                            th { "Address" }
                                            th { "Sync" }
                                            th { "Number of transactions" }
                                            th { "Balance" }
                                            th { "Derivation path" }
                                        }
                                    }
                                    tbody {
                                        for row in page.rows.clone() {
                                            AccountAddressTableRow { asset, network, row }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "modal-actions account-addresses-modal-actions",
                    span { class: "muted",
                        "Page {current_page} of {total_pages} ({total} addresses)"
                    }
                    div { class: "account-addresses-pagination",
                        button {
                            class: "btn btn-secondary",
                            r#type: "button",
                            disabled: !can_prev,
                            onclick: move |_| on_prev_page.call(()),
                            ChevronLeftIcon {}
                            "Previous"
                        }
                        button {
                            class: "btn btn-secondary",
                            r#type: "button",
                            disabled: !can_next,
                            onclick: move |_| on_next_page.call(()),
                            "Next"
                            ChevronRightIcon {}
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AccountAddressTableRow(
    asset: crate::wallets::SyncedAssetId,
    network: Network,
    row: crate::wallets::AccountAddressRowResponse,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let now = use_context::<AccountSyncNowSignal>();
    let now = now();
    let address_value = row.address.clone();
    let address_display = address_value.clone();
    let explorer_url = address_explorer_url(
        &settings_state,
        crate::explorer_links::DigitalAssetAddressRef::from_asset(asset, network, &address_value),
    );
    let balance_display = format_balance_for_asset(&row.balance, (settings_state.number_format)());
    let derivation_path = row.derivation_path.clone();
    let tx_count_display = transaction_count_display(
        row.transaction_count.value(),
        row.reported_transaction_count.map(|c| c.value()),
    );

    let sync_status = row.sync.status;
    let sync_last_completed = row.sync.last_completed_at;
    let sync_error = row.sync.last_error;

    rsx! {
        tr {
            td {
                div { class: "account-address-cell",
                    match explorer_url {
                        Ok(url) => rsx! {
                            a {
                                class: "address-link",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                title: "Opens in block explorer (external site)",
                                "{address_display}"
                                ExternalLinkIcon {}
                            }
                        },
                        Err(_) => rsx! {
                            code { "{address_display}" }
                        },
                    }
                    CopyIconButton {
                        value: address_value,
                        aria_label: "Copy address".to_string(),
                    }
                }
            }
            td {
                AddressSyncStatusCell {
                    status: sync_status,
                    last_completed_at: sync_last_completed,
                    last_error: sync_error,
                    now,
                }
            }
            td {
                match tx_count_display {
                    TxCountDisplay::Unknown(n) => rsx! {
                        span { class: "tx-count", "{n}" }
                    },
                    TxCountDisplay::Complete(n) => rsx! {
                        span {
                            class: "tx-count tx-count-complete",
                            title: "Fully synced.",
                            "{n}"
                            CheckIcon {}
                        }
                    },
                    TxCountDisplay::Partial { synced, reported } => rsx! {
                        span {
                            class: "tx-count tx-count-partial",
                            title: "{synced} of {reported} transactions synced. Older transactions for this address haven't been downloaded yet — syncing full history may require a plan upgrade.",
                            "{synced}/{reported}"
                        }
                    },
                }
            }
            td { "{balance_display}" }
            td { class: "account-address-path", "{derivation_path}" }
        }
    }
}

#[component]
fn AddressSyncStatusCell(
    status: AccountAddressSyncStatus,
    last_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<crate::transactions::SyncErrorMessage>,
    now: Option<chrono::DateTime<chrono::Utc>>,
) -> Element {
    match status {
        AccountAddressSyncStatus::NotSynced => rsx! {
            div { class: "address-sync-cell",
                span { class: "sync-dot sync-dot-idle" }
                span { class: "address-sync-text", "Not yet synced" }
            }
        },
        AccountAddressSyncStatus::Syncing => rsx! {
            div { class: "address-sync-cell",
                span { class: "account-sync-spinner account-sync-spinner-small", "aria-label": "Syncing" }
                span { class: "address-sync-text", "Syncing" }
            }
        },
        AccountAddressSyncStatus::Synced => {
            let text = match (last_completed_at, now) {
                (Some(ts), Some(now_utc)) => {
                    format!("Synced {}", format_sync_relative_time(now_utc, ts))
                }
                (Some(ts), None) => format!("Synced {}", ts.format("%Y-%m-%d")),
                _ => "Synced".to_string(),
            };
            rsx! {
                div { class: "address-sync-cell",
                    span { class: "sync-dot sync-dot-success" }
                    span { class: "address-sync-text", "{text}" }
                }
            }
        }
        AccountAddressSyncStatus::Failed => {
            let error_tooltip = sync_status_error_message(last_error.as_ref());
            rsx! {
                div { class: "address-sync-cell", title: "Sync failed: {error_tooltip}",
                    span { class: "sync-dot sync-dot-failure" }
                    span { class: "address-sync-text", "Sync failed" }
                }
            }
        }
    }
}

#[component]
pub(super) fn AccountReferenceRow(
    account_reference: String,
    asset: crate::wallets::SyncedAssetId,
    network: Network,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let address_value = account_reference.clone();
    let explorer_link = address_explorer_url(
        &settings_state,
        crate::explorer_links::DigitalAssetAddressRef::from_asset(asset, network, &address_value),
    );

    rsx! {
        div { class: "pubkey-row",
            div { class: "pubkey-meta",
                span { class: "pubkey-path", "Single Address" }
            }

            div { class: "pubkey-value",
                match &explorer_link {
                    Ok(explorer_url) => rsx! {
                        a {
                            class: "address-link",
                            href: "{explorer_url}",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            title: "Opens in block explorer (external site)",
                            "{address_value}"
                            ExternalLinkIcon {}
                        }
                    },
                    Err(_) => rsx! {
                        code { "{address_value}" }
                    },
                }
                CopyIconButton {
                    value: address_value,
                    aria_label: "Copy address".to_string(),
                }
            }
        }
    }
}

#[component]
pub(super) fn TransactionCountsSummary(
    transaction_counts: AccountTransactionCountsView,
) -> Element {
    rsx! {
        table { class: "tx-counts-table",
            thead {
                tr {
                    th { "Total" }
                    th { "Confirmed" }
                    th { "Pending" }
                    th { "Dropped" }
                    th { "Failed" }
                }
            }
            tbody {
                tr {
                    td { "{transaction_counts.total}" }
                    td { "{transaction_counts.confirmed}" }
                    td { "{transaction_counts.pending}" }
                    td { "{transaction_counts.dropped}" }
                    td { "{transaction_counts.failed}" }
                }
            }
        }
    }
}

#[component]
pub(super) fn HdKeyRow(extended_pubkey: String, derivation_path: String) -> Element {
    rsx! {
        div { class: "pubkey-row",
            div { class: "pubkey-meta",
                span { class: "pubkey-path", "{derivation_path}" }
            }

            div { class: "pubkey-value",
                code { "{extended_pubkey}" }
                CopyIconButton {
                    value: extended_pubkey,
                    aria_label: "Copy extended public key".to_string(),
                }
            }
        }
    }
}

#[component]
pub(super) fn CopyIconButton(value: String, aria_label: String) -> Element {
    let mut copied = use_signal(|| false);

    rsx! {
        button {
            class: if copied() { "inline-copy-btn copied" } else { "inline-copy-btn" },
            r#type: "button",
            "aria-label": if copied() { "Copied!" } else { aria_label.as_str() },
            title: if copied() { "Copied!" } else { aria_label.as_str() },
            onclick: move |_| {
                copy_to_clipboard(&value);
                copied.set(true);
                spawn(async move {
                    let mut timer =
                        dioxus::document::eval(r#"setTimeout(() => { dioxus.send(null); }, 1500);"#);
                    let _ = timer.recv::<serde_json::Value>().await;
                    copied.set(false);
                });
            },
            if copied() {
                CheckIcon {}
            } else {
                CopyIcon {}
            }
        }
    }
}

#[component]
pub(crate) fn LabelEditor(
    current: crate::wallets::Label,
    max_len: usize,
    on_save: EventHandler<RawLabel>,
    on_cancel: EventHandler<()>,
) -> Element {
    let initial = current.as_str().to_string();
    let mut value = use_signal(|| initial);
    let mut error = use_signal(|| None::<String>);

    rsx! {
        div { class: "label-editor",
            input {
                r#type: "text",
                autocomplete: "off",
                maxlength: max_len,
                value: "{value}",
                onmounted: move |e| async move {
                    let _ = e.set_focus(true).await;
                },
                oninput: move |e| {
                    value.set(e.value());
                    error.set(None);
                }
            }
            button {
                class: "btn btn-primary",
                onclick: move |_| {
                    let trimmed = value().trim().to_string();
                    if trimmed.is_empty() {
                        error.set(Some("Label cannot be empty".to_string()));
                        return;
                    }
                    if let Err(err) = crate::wallets::Label::parse_with_limit(&trimmed, max_len) {
                        error.set(Some(err.to_string()));
                        return;
                    }
                    on_save.call(RawLabel::new(trimmed));
                },
                "Save"
            }
            button {
                class: "btn btn-secondary",
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
            if let Some(err) = error() {
                span { class: "label-error", "{err}" }
            }
        }
    }
}

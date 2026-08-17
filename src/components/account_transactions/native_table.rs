use super::super::{
    AmountDisplayContext, ArrowDownRightIcon, ArrowRightLeftIcon, ArrowUpRightIcon, CardViewIcon,
    CheckIcon, ChevronDoubleLeftIcon, ChevronDoubleRightIcon, ChevronLeftIcon, ChevronRightIcon,
    CopyIcon, DisplayAmountSign, EmptyTransactionsIllustration, ExternalLinkIcon,
    ManualConversionQuote, SortAscIcon, SortDescIcon, TableIcon, convert_amount,
};
use super::helpers::{
    amount_class, amount_sign, copy_to_clipboard, direction_label, format_closing_balance,
    format_fee_amount, format_transaction_amount, last_page, show_closing_balance_provisional,
    status_label, table_totals_text, tx_explorer_url,
};
use crate::Route;
use crate::components::wallets::truncate_reference_with_lengths;
use crate::models::NumberFormat;
use crate::settings::SettingsState;
use crate::timezone::format_timestamp;
use crate::transactions::AccountTransactionDirection;
use crate::wallets::{
    AccountTransactionRowResponse, AccountTransactionTableResponse, Network, SyncedAssetId,
    TransactionSortDirection, TransactionsEmptyHint,
};
use dioxus::document::eval;
use dioxus::prelude::*;

#[component]
pub(super) fn TransactionsPaginationRow(
    table: AccountTransactionTableResponse,
    loading: bool,
    on_page_change: EventHandler<u32>,
) -> Element {
    let total_pages = last_page(&table);
    let can_prev = !loading && table.page > 1;
    let can_next = !loading && table.page < total_pages;
    let can_first = can_prev;
    let can_last = can_next;
    let totals_text = table_totals_text(&table);

    let mut page_input = use_signal(String::new);

    if !can_prev && !can_next && total_pages <= 1 {
        return rsx! {};
    }

    rsx! {
        div { class: "transactions-pagination-row",
            span { class: "muted", "{totals_text}" }
            div { class: "transactions-pagination",
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_first,
                    onclick: move |_| on_page_change.call(1),
                    title: "First page",
                    ChevronDoubleLeftIcon {}
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_prev,
                    onclick: move |_| on_page_change.call(table.page - 1),
                    title: "Previous page",
                    ChevronLeftIcon {}
                }
                div { class: "tx-page-jump",
                    input {
                        class: "tx-page-input",
                        r#type: "text",
                        inputmode: "numeric",
                        placeholder: "{table.page}",
                        value: "{page_input}",
                        disabled: loading,
                        "aria-label": "Jump to page",
                        oninput: move |e| page_input.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                let input_value = page_input();
                                if let Ok(target) = input_value.trim().parse::<u32>() {
                                    let clamped = target.max(1).min(total_pages);
                                    on_page_change.call(clamped);
                                    page_input.set(String::new());
                                }
                            }
                        },
                    }
                    span { class: "tx-page-total muted", "/ {total_pages}" }
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_next,
                    onclick: move |_| on_page_change.call(table.page + 1),
                    title: "Next page",
                    ChevronRightIcon {}
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_last,
                    onclick: move |_| on_page_change.call(total_pages),
                    title: "Last page",
                    ChevronDoubleRightIcon {}
                }
            }
        }
    }
}

#[component]
pub(super) fn TransactionsTableSection(
    title: String,
    heading_status: Option<String>,
    asset: SyncedAssetId,
    network: Network,
    amount_context: AmountDisplayContext,
    active_quote: Option<ManualConversionQuote>,
    number_format: NumberFormat,
    table: AccountTransactionTableResponse,
    loading: bool,
    sort_toggle: Option<TransactionSortDirection>,
    empty_hint: Option<TransactionsEmptyHint>,
    on_page_change: EventHandler<u32>,
    on_sort_toggle: EventHandler<TransactionSortDirection>,
) -> Element {
    let totals_text = table_totals_text(&table);
    let bottom_table = table.clone();
    let mut use_table_view = use_signal(|| false);

    rsx! {
        section { class: "transactions-table-section card",
            div { class: "transactions-table-header",
                h2 { class: "transactions-table-title", "{title}" }
                div { class: "transactions-table-header-right",
                    // View toggle: card / table
                    div { class: "tx-view-toggle",
                        button {
                            class: if !use_table_view() { "tx-view-toggle-btn active" } else { "tx-view-toggle-btn" },
                            r#type: "button",
                            title: "Card view",
                            onclick: move |_| use_table_view.set(false),
                            CardViewIcon {}
                        }
                        button {
                            class: if use_table_view() { "tx-view-toggle-btn active" } else { "tx-view-toggle-btn" },
                            r#type: "button",
                            title: "Table view",
                            onclick: move |_| use_table_view.set(true),
                            TableIcon {}
                        }
                    }
                    span { class: "muted", "{totals_text}" }
                    if let Some(current_sort) = sort_toggle {
                        button {
                            class: "btn btn-secondary tx-sort-toggle",
                            r#type: "button",
                            disabled: loading,
                            title: match current_sort {
                                TransactionSortDirection::Ascending => "Sorted oldest first — click for newest first",
                                TransactionSortDirection::Descending => "Sorted newest first — click for oldest first",
                            },
                            onclick: move |_| on_sort_toggle.call(current_sort.toggled()),
                            match current_sort {
                                TransactionSortDirection::Ascending => rsx! { SortAscIcon {} },
                                TransactionSortDirection::Descending => rsx! { SortDescIcon {} },
                            }
                            match current_sort {
                                TransactionSortDirection::Ascending => "Oldest first",
                                TransactionSortDirection::Descending => "Newest first",
                            }
                        }
                    }
                }
            }

            TransactionsPaginationRow {
                table: table.clone(),
                loading,
                on_page_change,
            }


            if table.rows.is_empty() {
                div { class: "transactions-list",
                    div {
                        class: "transactions-empty-state",
                        EmptyTransactionsIllustration {}
                        match empty_hint {
                            Some(TransactionsEmptyHint::FreePlanNoHistory) => rsx! {
                                p { class: "empty-state-heading", "Transaction history requires a paid plan" }
                                p { class: "empty-state-body",
                                    "This Free synced account has balance-only data. Upgrade to sync transaction history."
                                }
                                Link {
                                    class: "btn btn-primary",
                                    to: Route::Payments,
                                    "View plans"
                                }
                            },
                            Some(TransactionsEmptyHint::FreePlanBalanceUnavailable) => rsx! {
                                p { class: "empty-state-heading", "Balance sync unavailable on Free" }
                                p { class: "empty-state-body",
                                    "This provider cannot expose a trustworthy balance without transaction-history calls. Upgrade to sync this account."
                                }
                                Link {
                                    class: "btn btn-primary",
                                    to: Route::Payments,
                                    "View plans"
                                }
                            },
                            Some(TransactionsEmptyHint::PaidPlanNoSyncedTransactions) => rsx! {
                                p { class: "empty-state-heading", "No synced transactions available yet" }
                                p { class: "empty-state-body",
                                    "A balance is available for this account. Transaction history sync is pending and transactions will appear here shortly."
                                }
                            },
                            Some(TransactionsEmptyHint::HistorySyncPending { expected_transactions }) => rsx! {
                                div { class: "history-sync-pending", "data-testid": "history-sync-pending",
                                    span {
                                        class: "account-sync-spinner account-sync-spinner-small",
                                        "aria-label": "Syncing",
                                        title: "Syncing",
                                    }
                                    if let Some(expected_transactions) = expected_transactions {
                                        p { class: "empty-state-heading",
                                            "Syncing your ~{expected_transactions} transactions…"
                                        }
                                    } else {
                                        p { class: "empty-state-heading",
                                            "Transaction history sync in progress…"
                                        }
                                    }
                                    p { class: "empty-state-body",
                                        "Transaction history sync is in progress. Transactions appear here as they come in."
                                    }
                                }
                            },
                            None => rsx! {
                                p { class: "empty-state-heading", "No transactions found" }
                                p { class: "empty-state-body", "Try adjusting your filters or check back later." }
                            },
                        }
                    }
                }
            } else if use_table_view() {
                // Dense table view for wide screens
                {
                    let settings_state = use_context::<SettingsState>();
    let _user_currency = (settings_state.currency)();
                    let tz = (settings_state.timezone)().0;
                    let date_fmt = (settings_state.date_time_format)();

                    rsx! {
                        table { class: "transactions-table-view",
                            thead {
                                tr {
                                    th { "Date" }
                                    th { "Type" }
                                    th { class: "tx-table-amount", "Amount" }
                                    th { class: "tx-table-fee", "Fee" }
                                    th { class: "tx-table-balance", "Balance" }
                                    th { "Status" }
                                    th { class: "tx-table-hash", "TX ID" }
                                }
                            }
                            tbody {
                                for row in table.rows.clone() {
                                    {
                                        let direction = row.direction;
                                        let type_label = direction_label(direction);
                                        let amount_display =
                                            format_transaction_amount(&row.value, direction, &amount_context);
                                        let amount_display_class = amount_class(direction);
                                        let fee_display =
                                            format_fee_amount(row.fee.as_ref(), &amount_context);
                                        let balance_display = format_closing_balance(
                                            row.closing_balance.as_ref(),
                                            &amount_context,
                                        );
                                        let converted_amount = active_quote.as_ref().map(|q| {
                                            convert_amount(&row.value.formatted_value, amount_sign(direction), q, number_format)
                                        });
                                        let converted_fee = active_quote.as_ref().and_then(|q| {
                                            row.fee.as_ref().map(|fee| {
                                                convert_amount(&fee.formatted_value, DisplayAmountSign::Negative, q, number_format)
                                            })
                                        });
                                        let converted_balance = active_quote.as_ref().and_then(|q| {
                                            row.closing_balance.as_ref().map(|bal| {
                                                convert_amount(&bal.formatted_value, DisplayAmountSign::Hidden, q, number_format)
                                            })
                                        });
                                        let status = row.status;
                                        let tx_hash = row.tx_hash.clone();
                                        let tx_explorer = tx_explorer_url(
                                            &settings_state,
                                            crate::explorer_links::DigitalAssetTransactionRef::from_asset(
                                                asset,
                                                network,
                                                &tx_hash,
                                            ),
                                        );
                                        let tx_hash_display =
                                            truncate_reference_with_lengths(&tx_hash, 8, 6);
                                        let timestamp_display = chrono::DateTime::parse_from_rfc3339(&row.occurred_at)
                                            .map(|parsed| format_timestamp(&parsed.with_timezone(&chrono::Utc), tz, date_fmt))
                                            .unwrap_or_else(|_| row.occurred_at.clone());

                                        rsx! {
                                            tr {
                                                td { "{timestamp_display}" }
                                                td { "{type_label}" }
                                                td { class: "tx-table-amount {amount_display_class}",
                                                    "{amount_display}"
                                                    if let Some(converted) = &converted_amount {
                                                        div { class: "tx-converted-secondary", "{converted}" }
                                                    }
                                                }
                                                td { class: "tx-table-fee",
                                                    "{fee_display}"
                                                    if let Some(converted) = &converted_fee {
                                                        div { class: "tx-converted-secondary", "{converted}" }
                                                    }
                                                }
                                                td { class: "tx-table-balance",
                                                    "{balance_display}"
                                                    if let Some(converted) = &converted_balance {
                                                        div { class: "tx-converted-secondary", "{converted}" }
                                                    }
                                                    if show_closing_balance_provisional(
                                                        row.closing_balance.as_ref(),
                                                        &row.balance_reliability,
                                                    ) {
                                                        span { class: "tx-status-indicator tx-status-provisional", " Provisional" }
                                                    }
                                                }
                                                td {
                                                    span { class: "tx-status-indicator tx-status-{status_label(status)}", "{status_label(status)}" }
                                                }
                                                td { class: "tx-table-hash",
                                                    match tx_explorer {
                                                        Ok(explorer_url) => rsx! {
                                                            a {
                                                                class: "address-link tx-external-link",
                                                                href: "{explorer_url}",
                                                                target: "_blank",
                                                                rel: "noopener noreferrer",
                                                                title: "{tx_hash}",
                                                                "{tx_hash_display}"
                                                                ExternalLinkIcon {}
                                                            }
                                                        },
                                                        Err(_) => rsx! {
                                                            code { title: "{tx_hash}", "{tx_hash_display}" }
                                                        },
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                // Card view (default)
                div { class: "transactions-list",
                    {
                        // Group transactions by date
                        let mut current_date = String::new();
                        let settings_state = use_context::<SettingsState>();
    let _user_currency = (settings_state.currency)();
                        let tz = (settings_state.timezone)().0;
                        let date_fmt = (settings_state.date_time_format)();

                        rsx! {
                            for row in table.rows.clone() {
                                {
                                    let date_str =
                                        super::helpers::format_transaction_group_date(&row.occurred_at, tz, date_fmt);

                                    let show_header = if date_str != current_date {
                                        current_date = date_str.clone();
                                        true
                                    } else {
                                        false
                                    };

                                    rsx! {
                                        if show_header {
                                            div { class: "tx-date-header",
                                                span { "{date_str}" }
                                            }
                                        }
                                        TransactionCardRow {
                                            asset,
                                            network,
                                            amount_context: amount_context.clone(),
                                            active_quote: active_quote.clone(),
                                            number_format,
                                            heading_status: heading_status.clone(),
                                            row,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            TransactionsPaginationRow {
                table: bottom_table,
                loading,
                on_page_change,
            }
        }
    }
}

#[component]
pub(super) fn TransactionCardRow(
    asset: SyncedAssetId,
    network: Network,
    amount_context: AmountDisplayContext,
    active_quote: Option<ManualConversionQuote>,
    number_format: NumberFormat,
    heading_status: Option<String>,
    row: AccountTransactionRowResponse,
) -> Element {
    let settings_state = use_context::<SettingsState>();
    let _user_currency = (settings_state.currency)();
    let tx_hash = row.tx_hash.clone();
    let tx_explorer = tx_explorer_url(
        &settings_state,
        crate::explorer_links::DigitalAssetTransactionRef::from_asset(asset, network, &tx_hash),
    );
    let tx_hash_display = truncate_reference_with_lengths(&tx_hash, 8, 6);
    let direction = row.direction;
    let type_label = direction_label(direction);
    let amount_display = format_transaction_amount(&row.value, direction, &amount_context);
    let amount_display_class = amount_class(direction);
    let fee_display = format_fee_amount(row.fee.as_ref(), &amount_context);
    let balance_display = format_closing_balance(row.closing_balance.as_ref(), &amount_context);
    let converted_amount = active_quote.as_ref().map(|q| {
        convert_amount(
            &row.value.formatted_value,
            amount_sign(direction),
            q,
            number_format,
        )
    });
    let converted_fee = active_quote.as_ref().and_then(|q| {
        row.fee.as_ref().map(|fee| {
            convert_amount(
                &fee.formatted_value,
                DisplayAmountSign::Negative,
                q,
                number_format,
            )
        })
    });
    let converted_balance = active_quote.as_ref().and_then(|q| {
        row.closing_balance.as_ref().map(|bal| {
            convert_amount(
                &bal.formatted_value,
                DisplayAmountSign::Hidden,
                q,
                number_format,
            )
        })
    });
    let status = row.status;

    let tz = (settings_state.timezone)().0;
    let date_fmt = (settings_state.date_time_format)();
    let timestamp_display = chrono::DateTime::parse_from_rfc3339(&row.occurred_at)
        .map(|parsed| format_timestamp(&parsed.with_timezone(&chrono::Utc), tz, date_fmt))
        .unwrap_or_else(|_| row.occurred_at.clone());

    rsx! {
        div { class: "tx-card",
            div { class: "tx-card-header",
                div { class: "tx-card-type-group",
                    div { class: "tx-card-icon {amount_display_class}",
                        match direction {
                            AccountTransactionDirection::Incoming => rsx! { ArrowDownRightIcon {} },
                            AccountTransactionDirection::Outgoing => rsx! { ArrowUpRightIcon {} },
                            AccountTransactionDirection::SelfTransfer => rsx! { ArrowRightLeftIcon {} },
                        }
                    }
                    div { class: "tx-card-title-group",
                        span { class: "tx-card-title", "{type_label}" }
                        span { class: "tx-card-timestamp", "{timestamp_display}" }
                    }
                }
                div { class: "tx-card-amount-group",
                    div { class: "tx-card-amount {amount_display_class}",
                        "{amount_display}"
                        if let Some(converted) = &converted_amount {
                            div { class: "tx-converted-secondary", "{converted}" }
                        }
                    }
                    if heading_status.as_deref() != Some(status_label(status)) {
                        span { class: "tx-status-indicator tx-status-{status_label(status)}", "{status_label(status)}" }
                    }
                }
            }
            div { class: "tx-card-details",
                div { class: "tx-card-detail-item",
                    span { class: "tx-card-detail-label", "Transaction ID" }
                    div { class: "tx-cell-with-copy",
                        match tx_explorer {
                            Ok(explorer_url) => rsx! {
                                a {
                                    class: "address-link tx-external-link",
                                    href: "{explorer_url}",
                                    target: "_blank",
                                    rel: "noopener noreferrer",
                                    title: "{tx_hash}",
                                    "{tx_hash_display}"
                                    ExternalLinkIcon {}
                                }
                            },
                            Err(_) => rsx! {
                                code { title: "{tx_hash}", "{tx_hash_display}" }
                            },
                        }
                        CopyIconButton {
                            value: tx_hash,
                            aria_label: "Copy transaction ID".to_string(),
                        }
                    }
                }
                div { class: "tx-card-detail-item",
                    span { class: "tx-card-detail-label", "Fee" }
                    div { class: "tx-fee-value",
                        "{fee_display}"
                        if let Some(converted) = &converted_fee {
                            div { class: "tx-converted-secondary", "{converted}" }
                        }
                    }
                }
                div { class: "tx-card-detail-item",
                    span { class: "tx-card-detail-label", "Balance" }
                    div { class: "tx-balance-cell",
                        span { "{balance_display}" }
                        if let Some(converted) = &converted_balance {
                            div { class: "tx-converted-secondary", "{converted}" }
                        }
                        if show_closing_balance_provisional(
                            row.closing_balance.as_ref(),
                            &row.balance_reliability,
                        ) {
                            span { class: "tx-status-indicator tx-status-provisional", "Provisional" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn CopyIconButton(value: String, aria_label: String) -> Element {
    let mut copied = use_signal(|| false);
    let toast_state = use_context::<super::super::ToastState>();

    rsx! {
        button {
            class: if copied() { "inline-copy-btn copied" } else { "inline-copy-btn" },
            r#type: "button",
            "aria-label": if copied() { "Copied!" } else { aria_label.as_str() },
            title: if copied() { "Copied!" } else { aria_label.as_str() },
            onclick: move |_| {
                copy_to_clipboard(&value);
                copied.set(true);
                super::super::push_toast(toast_state, super::super::ToastLevel::Success, "Copied to clipboard".to_string());
                spawn(async move {
                    let mut timer =
                        eval(r#"setTimeout(() => { dioxus.send(null); }, 1500);"#);
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

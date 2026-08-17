use super::super::{
    AmountDisplayContext, DisplayAmount, DisplayAmountSign, EmptyTransactionsIllustration,
    ManualConversionQuote, PencilIcon, PlusIcon, SortAscIcon, SortDescIcon, convert_amount,
};
use super::helpers::{
    ManualAssertionFormMode, ManualAssertionFormState, custom_last_page, custom_table_totals_text,
    format_balance_date,
};
use crate::models::{DateTimeFormat, NumberFormat};
use crate::wallets::{
    ManualAssetAccountTransactionsResponse, ManualAssetBalanceAssertionRowResponse,
    ManualAssetBalanceAssertionTableResponse, TransactionSortDirection,
};
use dioxus::prelude::*;

#[component]
pub(super) fn ManualAssertionEditorModal(
    form_state: Signal<Option<ManualAssertionFormState>>,
    precision_helper_text: String,
    decimal_precision: u8,
    field_error: Signal<Option<String>>,
    save_error: Signal<Option<String>>,
    submitting: Signal<bool>,
    on_submit: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let Some(current_form) = form_state() else {
        return rsx! {};
    };
    let mode_label = match current_form.mode {
        ManualAssertionFormMode::Add => "Add Balance Assertion",
        ManualAssertionFormMode::Edit(_) => "Edit Balance Assertion",
    };

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal",
                div { class: "modal-header",
                    h3 { "{mode_label}" }
                }
                div { class: "modal-body",
                    label { class: "form-label", r#for: "custom-assertion-date", "Date" }
                    input {
                        id: "custom-assertion-date",
                        class: "form-input",
                        r#type: "date",
                        value: "{current_form.asserted_on}",
                        disabled: submitting(),
                        oninput: move |event| {
                            if let Some(mut current) = form_state() {
                                current.asserted_on = event.value();
                                form_state.set(Some(current));
                                field_error.set(None);
                                save_error.set(None);
                            }
                        },
                    }

                    label { class: "form-label", r#for: "custom-assertion-balance", "Balance" }
                    input {
                        id: "custom-assertion-balance",
                        class: "form-input",
                        r#type: "text",
                        inputmode: "decimal",
                        placeholder: "0.0",
                        value: "{current_form.balance}",
                        disabled: submitting(),
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        oninput: move |event| {
                            if let Some(mut current) = form_state() {
                                let value = event.value();
                                if let Some(dot_pos) = value.find('.') {
                                    let decimals = value.len() - dot_pos - 1;
                                    if decimals > decimal_precision as usize {
                                        field_error.set(Some(format!(
                                            "Maximum {} decimal place{} allowed",
                                            decimal_precision,
                                            if decimal_precision == 1 { "" } else { "s" },
                                        )));
                                        return;
                                    }
                                }
                                current.balance = value;
                                form_state.set(Some(current));
                                field_error.set(None);
                                save_error.set(None);
                            }
                        },
                    }
                    if !precision_helper_text.is_empty() {
                        p { class: "form-help-text", "{precision_helper_text}" }
                    }

                    label { class: "form-label", r#for: "custom-assertion-note", "Note" }
                    textarea {
                        id: "custom-assertion-note",
                        class: "form-input",
                        rows: "4",
                        maxlength: "500",
                        placeholder: "Optional note",
                        disabled: submitting(),
                        value: "{current_form.note}",
                        oninput: move |event| {
                            if let Some(mut current) = form_state() {
                                current.note = event.value();
                                form_state.set(Some(current));
                                field_error.set(None);
                                save_error.set(None);
                            }
                        },
                    }

                    if let Some(message) = field_error() {
                        p { class: "error-text", "{message}" }
                    }
                    if let Some(message) = save_error() {
                        p { class: "error-text", "{message}" }
                    }

                    div { class: "modal-actions",
                        button {
                            class: "btn btn-secondary",
                            r#type: "button",
                            disabled: submitting(),
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            r#type: "button",
                            disabled: submitting(),
                            onclick: move |_| on_submit.call(()),
                            if submitting() {
                                "Saving..."
                            } else {
                                match current_form.mode {
                                    ManualAssertionFormMode::Add => "Add Balance Assertion",
                                    ManualAssertionFormMode::Edit(_) => "Save Changes",
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn ManualAssertionsPaginationRow(
    table: ManualAssetBalanceAssertionTableResponse,
    loading: bool,
    on_page_change: EventHandler<u32>,
) -> Element {
    let total_pages = custom_last_page(&table);
    // Nothing to page through — don't frame an empty or single-page list
    // with controls (one bar renders above and one below the list).
    if total_pages <= 1 {
        return rsx! {};
    }
    let can_first = !loading && table.page > 1 && table.total > 0;
    let can_prev = !loading && table.page > 1 && table.total > 0;
    let can_next = !loading && table.page < total_pages && table.total > 0;
    let can_last = can_next;

    rsx! {
        div { class: "transactions-pagination-row",
            div { class: "transactions-pagination-controls",
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_first,
                    onclick: move |_| on_page_change.call(1),
                    title: "First page",
                    super::super::ChevronDoubleLeftIcon {}
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_prev,
                    onclick: move |_| on_page_change.call(table.page.saturating_sub(1)),
                    title: "Previous page",
                    super::super::ChevronLeftIcon {}
                }
                span { class: "muted", "Page {table.page} of {total_pages}" }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_next,
                    onclick: move |_| on_page_change.call(table.page + 1),
                    title: "Next page",
                    super::super::ChevronRightIcon {}
                }
                button {
                    class: "btn btn-secondary",
                    r#type: "button",
                    disabled: !can_last,
                    onclick: move |_| on_page_change.call(total_pages),
                    title: "Last page",
                    super::super::ChevronDoubleRightIcon {}
                }
            }
        }
    }
}

#[component]
pub(super) fn ManualAssertionsSection(
    data: ManualAssetAccountTransactionsResponse,
    amount_context: AmountDisplayContext,
    active_quote: Option<ManualConversionQuote>,
    number_format: NumberFormat,
    date_format: DateTimeFormat,
    loading: bool,
    on_add: EventHandler<()>,
    on_edit: EventHandler<ManualAssetBalanceAssertionRowResponse>,
    on_delete: EventHandler<crate::wallets::ManualAssetBalanceAssertionId>,
    on_page_change: EventHandler<u32>,
    on_sort_toggle: EventHandler<TransactionSortDirection>,
) -> Element {
    let totals_text = custom_table_totals_text(&data.assertions);
    let current_sort = data.sort;
    let assertions_read_only = data.account_state == crate::backend::AccountStateView::Inactive;
    let read_only_message = "Upgrade to modify assertions for this inactive account.";

    rsx! {
        section { class: "transactions-table-section card",
            div { class: "transactions-table-header",
                h2 { class: "transactions-table-title", "Balance Assertions" }
                div { class: "transactions-table-header-right",
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        disabled: loading || assertions_read_only,
                        title: if assertions_read_only { read_only_message } else { "" },
                        onclick: move |_| on_add.call(()),
                        PlusIcon {}
                        "Add Balance Assertion"
                    }
                    span { class: "muted", "{totals_text}" }
                    button {
                        class: "btn btn-secondary tx-sort-toggle",
                        r#type: "button",
                        disabled: loading,
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

            ManualAssertionsPaginationRow {
                table: data.assertions.clone(),
                loading,
                on_page_change,
            }

            if assertions_read_only {
                div { class: "alert alert-info manual-assertions-read-only",
                    "This account is inactive. "
                    a { href: "/payments", "Upgrade" }
                    " to modify its balance assertions."
                }
            }

            if data.assertions.rows.is_empty() {
                div { class: "transactions-list",
                    div { class: "transactions-empty-state",
                        EmptyTransactionsIllustration {}
                        if assertions_read_only {
                            p { class: "empty-state-heading", "No balance assertions yet" }
                            p { class: "empty-state-body",
                                "This account is inactive. Balance assertions are read-only until you upgrade."
                            }
                        } else {
                            p { class: "empty-state-heading", "No balance assertions yet" }
                            p { class: "empty-state-body",
                                "Record a dated balance assertion to make this manual asset account visible in wallet views and exports."
                            }
                        }
                        if assertions_read_only {
                            a {
                                class: "manual-assertions-upgrade-link upgrade-link",
                                href: "/payments",
                                "Upgrade to record balance assertions"
                            }
                        } else {
                            button {
                                class: "btn btn-primary",
                                r#type: "button",
                                disabled: loading,
                                onclick: move |_| on_add.call(()),
                                "Add Balance Assertion"
                            }
                        }
                    }
                }
            } else {
                div { class: "transactions-list",
                    for row in data.assertions.rows.clone() {
                        {
                            let balance_display = match &active_quote {
                                Some(quote) => convert_amount(
                                    &row.asserted_balance.formatted_value,
                                    DisplayAmountSign::Hidden,
                                    quote,
                                    number_format,
                                ),
                                None => DisplayAmount::from_balance(&row.asserted_balance, &amount_context)
                                    .to_string(),
                            };
                            let date_display = format_balance_date(&row.asserted_on, date_format);
                            let edit_row = row.clone();
                            let delete_id = row.assertion_id;

                            rsx! {
                                div { class: "tx-card",
                                    div { class: "tx-card-header",
                                        div { class: "tx-card-title-group",
                                            span { class: "tx-card-title", "{date_display}" }
                                            span { class: "tx-card-timestamp", "Balance Assertion" }
                                        }
                                        div { class: "tx-card-amount-group",
                                            div { class: "tx-card-amount tx-amount-neutral", "{balance_display}" }
                                        }
                                    }
                                    div { class: "tx-card-details",
                                        div { class: "tx-card-detail-item",
                                            span { class: "tx-card-detail-label", "Date" }
                                            span { "{row.asserted_on}" }
                                        }
                                        div { class: "tx-card-detail-item",
                                            span { class: "tx-card-detail-label", "Balance" }
                                            span { "{balance_display}" }
                                        }
                                        if let Some(note) = &row.note {
                                            div { class: "tx-card-detail-item",
                                                span { class: "tx-card-detail-label", "Note" }
                                                span { "{note}" }
                                            }
                                        }
                                        div { class: "tx-card-detail-item tx-card-detail-actions",
                                            span { class: "tx-card-detail-label", "Actions" }
                                            div { class: "tx-toolbar-secondary-group",
                                                button {
                                                    class: "btn btn-secondary",
                                                    r#type: "button",
                                                    disabled: loading || assertions_read_only,
                                                    title: if assertions_read_only { read_only_message } else { "" },
                                                    onclick: move |_| on_edit.call(edit_row.clone()),
                                                    PencilIcon {}
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-secondary",
                                                    r#type: "button",
                                                    disabled: loading || assertions_read_only,
                                                    title: if assertions_read_only { read_only_message } else { "" },
                                                    onclick: move |_| on_delete.call(delete_id),
                                                    "Delete"
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

            ManualAssertionsPaginationRow {
                table: data.assertions.clone(),
                loading,
                on_page_change,
            }
        }
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn balance_amount(value: &str) -> crate::backend::BalanceAmountView {
        crate::backend::BalanceAmountView {
            raw_value: value.to_string(),
            formatted_value: value.to_string(),
        }
    }

    fn manual_assertions_response(
        account_state: crate::backend::AccountStateView,
    ) -> ManualAssetAccountTransactionsResponse {
        ManualAssetAccountTransactionsResponse {
            account_id: crate::wallets::WalletAccountId::new(),
            wallet_id: crate::wallets::WalletId::new(),
            wallet_label: "Wallet".to_string(),
            account_label: "AAA Test".to_string(),
            account_state,
            sync_control_enabled: false,
            unit_code: "AAA".to_string(),
            decimal_precision: 8,
            precision_status: crate::wallets::ManualAssetPrecisionStatus::LegacyBaseline,
            precision_shared_with_other_accounts: false,
            symbol: None,
            asset_name: None,
            network_name: None,
            opening_balance_state: crate::backend::AccountBalanceStateView::Known {
                amount: balance_amount("1.00000000"),
            },
            opening_balance_date: Some("2024-12-31".to_string()),
            closing_balance_state: crate::backend::AccountBalanceStateView::Known {
                amount: balance_amount("1.00000000"),
            },
            closing_balance_date: Some("2024-12-31".to_string()),
            sort: TransactionSortDirection::Descending,
            active_from_date: None,
            active_to_date: None,
            assertions: ManualAssetBalanceAssertionTableResponse {
                page: 1,
                page_size: 25,
                total: 1,
                start: 1,
                end: 1,
                rows: vec![ManualAssetBalanceAssertionRowResponse {
                    assertion_id: crate::wallets::ManualAssetBalanceAssertionId::new(),
                    asserted_on: "2024-12-31".to_string(),
                    asserted_balance: balance_amount("1.00000000"),
                    note: Some("fixture".to_string()),
                }],
            },
        }
    }

    #[test]
    fn supported_manual_response_omits_legacy_fields() {
        let response = manual_assertions_response(crate::backend::AccountStateView::Active);
        let value = serde_json::to_value(response).expect("response should serialize");

        assert!(value.get("is_legacy_custom").is_none());
        assert!(value.get("legacy_migration").is_none());
    }

    #[component]
    fn ManualAssertionsSectionHarness(account_state: crate::backend::AccountStateView) -> Element {
        rsx! {
            ManualAssertionsSection {
                data: manual_assertions_response(account_state),
                amount_context: AmountDisplayContext::new(
                    "AAA".to_string(),
                    None,
                    NumberFormat::CommaDot,
                ),
                active_quote: None,
                number_format: NumberFormat::CommaDot,
                date_format: DateTimeFormat::YearMonthDay24,
                loading: false,
                on_add: move |_| {},
                on_edit: move |_| {},
                on_delete: move |_| {},
                on_page_change: move |_| {},
                on_sort_toggle: move |_| {},
            }
        }
    }

    fn render_manual_assertions_section(account_state: crate::backend::AccountStateView) -> String {
        let mut dom = VirtualDom::new_with_props(
            ManualAssertionsSectionHarness,
            ManualAssertionsSectionHarnessProps { account_state },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn manual_assertions_section_shows_row_actions_for_manual_asset_accounts() {
        let rendered = render_manual_assertions_section(crate::backend::AccountStateView::Active);

        assert!(rendered.contains(">Edit<"));
        assert!(rendered.contains(">Delete<"));
    }

    #[test]
    fn manual_assertions_section_disables_inactive_manual_account_actions() {
        let rendered = render_manual_assertions_section(crate::backend::AccountStateView::Inactive);

        assert!(rendered.contains("This account is inactive."));
        assert!(rendered.contains(r#"href="/payments""#));
        assert!(rendered.contains(">Upgrade</a>"));
        assert!(rendered.contains("disabled"));
        assert!(rendered.contains(">Edit<"));
        assert!(rendered.contains(">Delete<"));
    }
}

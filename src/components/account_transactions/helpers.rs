use super::super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterEvent, DateRangeFilterPolicy, DateRangeFilterState,
    DateRangeSelection, transition_date_range_filter,
};
use super::super::{
    AmountDisplayContext, DisplayAmount, DisplayAmountSign, ManualConversionQuote, convert_amount,
    format_date_for_display,
};
use crate::models::{DateTimeFormat, NumberFormat, UserTimezone};
use crate::report_dates::{
    DateBoundaryKind, LocalReportDateRange, local_report_date_to_utc_boundary,
};
use crate::settings::SettingsState;
use crate::transactions::{AccountTransactionDirection, ChainTransactionStatus};
use crate::wallets::{
    AccountTransactionTableResponse, GetAccountTransactionsResponse,
    ManualAssetAccountTransactionsResponse, ManualAssetBalanceAssertionId,
    ManualAssetBalanceAssertionRowResponse, ManualAssetBalanceAssertionTableResponse,
    SyncedAssetId, TransactionSortDirection, WalletAccountHistoryResponse, WalletAccountId,
};
use crate::{AuthState, AuthStatus, BannerMessage, BannerState, Route};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use dioxus::logger::tracing;
use dioxus::prelude::*;

pub(super) fn copy_to_clipboard(value: &str) {
    let encoded_value = match serde_json::to_string(value) {
        Ok(encoded) => encoded,
        Err(err) => {
            tracing::warn!(error = %err, "transactions ui: failed to encode copy text");
            return;
        }
    };

    let script = format!(
        "const text = {encoded_value};
if (navigator && navigator.clipboard && navigator.clipboard.writeText) {{
  navigator.clipboard.writeText(text);
}} else {{
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  try {{ document.execCommand('copy'); }} catch (e) {{}}
  document.body.removeChild(textarea);
}}"
    );
    let _ = dioxus::document::eval(script.as_str());
}

pub(super) fn status_label(status: ChainTransactionStatus) -> &'static str {
    status.as_db_value()
}

pub(super) fn direction_label(direction: AccountTransactionDirection) -> &'static str {
    match direction {
        AccountTransactionDirection::Incoming => "Receive",
        AccountTransactionDirection::Outgoing => "Send",
        AccountTransactionDirection::SelfTransfer => "Self Transfer",
    }
}

pub(super) fn amount_sign(direction: AccountTransactionDirection) -> DisplayAmountSign {
    match direction {
        AccountTransactionDirection::Incoming | AccountTransactionDirection::SelfTransfer => {
            DisplayAmountSign::Hidden
        }
        AccountTransactionDirection::Outgoing => DisplayAmountSign::Negative,
    }
}

pub(super) fn amount_class(direction: AccountTransactionDirection) -> &'static str {
    match direction {
        AccountTransactionDirection::Incoming => "tx-amount-positive",
        AccountTransactionDirection::Outgoing => "tx-amount-negative",
        AccountTransactionDirection::SelfTransfer => "tx-amount-neutral",
    }
}

pub(super) fn amount_context_for_response(
    response: &GetAccountTransactionsResponse,
    number_format: NumberFormat,
) -> AmountDisplayContext {
    AmountDisplayContext::new(
        response.unit_code.clone(),
        response.symbol.clone(),
        number_format,
    )
}

pub(super) fn amount_context_for_custom_response(
    response: &ManualAssetAccountTransactionsResponse,
    number_format: NumberFormat,
) -> AmountDisplayContext {
    AmountDisplayContext::new(
        response.unit_code.clone(),
        response.symbol.clone(),
        number_format,
    )
}

pub(super) fn manual_assertion_precision_helper_text(
    response: &ManualAssetAccountTransactionsResponse,
) -> String {
    format!(
        "{} may have up to {} numbers after the decimal",
        response.unit_code, response.decimal_precision,
    )
}

pub(super) fn history_sort(response: &WalletAccountHistoryResponse) -> TransactionSortDirection {
    match response {
        WalletAccountHistoryResponse::Native(data) => data.sort,
        WalletAccountHistoryResponse::Custom(data) => data.sort,
    }
}

pub(super) fn history_pages(response: &WalletAccountHistoryResponse) -> (u32, u32) {
    match response {
        WalletAccountHistoryResponse::Native(data) => (data.pending.page, data.confirmed.page),
        WalletAccountHistoryResponse::Custom(data) => (1, data.assertions.page),
    }
}

pub(super) fn format_custom_balance_state_display(
    state: &crate::backend::AccountBalanceStateView,
    unit_code: &str,
    active_quote: &Option<ManualConversionQuote>,
    amount_context: &AmountDisplayContext,
    number_format: NumberFormat,
) -> String {
    match state {
        crate::backend::AccountBalanceStateView::Known { amount } => match active_quote {
            Some(quote) => convert_amount(
                &amount.formatted_value,
                DisplayAmountSign::Hidden,
                quote,
                number_format,
            ),
            None => DisplayAmount::from_balance(amount, amount_context).to_string(),
        },
        crate::backend::AccountBalanceStateView::Unknown => format!("Unknown {unit_code}"),
    }
}

pub(super) fn format_transaction_amount(
    amount: &crate::backend::BalanceAmountView,
    direction: AccountTransactionDirection,
    amount_context: &AmountDisplayContext,
) -> String {
    DisplayAmount::from_balance(amount, amount_context)
        .with_sign(amount_sign(direction))
        .to_string()
}

pub(super) fn format_fee_amount(
    fee: Option<&crate::backend::BalanceAmountView>,
    amount_context: &AmountDisplayContext,
) -> String {
    match fee {
        Some(value) => DisplayAmount::from_balance(value, amount_context)
            .with_sign(DisplayAmountSign::Negative)
            .to_string(),
        None => "-".to_string(),
    }
}

pub(super) fn format_closing_balance(
    amount: Option<&crate::backend::BalanceAmountView>,
    amount_context: &AmountDisplayContext,
) -> String {
    amount
        .map(|value| DisplayAmount::from_balance(value, amount_context).to_string())
        .unwrap_or_else(|| "Not available".to_string())
}

pub(super) fn show_closing_balance_provisional(
    amount: Option<&crate::backend::BalanceAmountView>,
    reliability: &crate::balance_reliability::BalanceReliability,
) -> bool {
    amount.is_some() && reliability.is_provisional()
}

pub(super) fn format_balance_date(date_str: &str, format: DateTimeFormat) -> String {
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map(|date| format_date_for_display(date, format))
        .unwrap_or_else(|_| date_str.to_string())
}

pub(super) fn zero_balance_canonical(asset: SyncedAssetId) -> String {
    let scale = match asset {
        SyncedAssetId::Bitcoin => 8,
        SyncedAssetId::Ethereum => 18,
    };
    if scale == 0 {
        return "0".to_string();
    }

    let mut canonical = String::with_capacity(scale + 2);
    canonical.push_str("0.");
    canonical.push_str(&"0".repeat(scale));
    canonical
}

pub(super) fn format_native_balance_state_display(
    balance_state: &crate::backend::NativeBalanceStateView,
    asset: SyncedAssetId,
    active_quote: &Option<ManualConversionQuote>,
    amount_context: &AmountDisplayContext,
    number_format: NumberFormat,
) -> String {
    match balance_state {
        crate::backend::NativeBalanceStateView::Known(amount) => match active_quote {
            Some(quote) => convert_amount(
                &amount.formatted_value,
                DisplayAmountSign::Hidden,
                quote,
                number_format,
            ),
            None => DisplayAmount::from_balance(amount, amount_context).to_string(),
        },
        crate::backend::NativeBalanceStateView::CanonicalZero => {
            let zero_canonical = zero_balance_canonical(asset);
            match active_quote {
                Some(quote) => convert_amount(
                    zero_canonical.as_str(),
                    DisplayAmountSign::Hidden,
                    quote,
                    number_format,
                ),
                None => DisplayAmount::new(zero_canonical, amount_context).to_string(),
            }
        }
        crate::backend::NativeBalanceStateView::Unknown => "Not available".to_string(),
    }
}

pub(super) fn format_balance_reliability_display(
    balance_display: String,
    balance_reliability: &crate::balance_reliability::BalanceReliability,
) -> String {
    if balance_display != "Not available" && balance_reliability.is_provisional() {
        return format!("{balance_display} Provisional");
    }

    balance_display
}

pub(super) fn manual_sync_outcome_message(
    completion: &crate::components::wallets::SyncRunCompletion,
    timezone: chrono_tz::Tz,
) -> String {
    if completion.failed {
        return completion.error.as_ref().map_or_else(
            || "Sync failed".to_string(),
            |error| format!("Sync failed: {}", error.as_str()),
        );
    }
    if completion.addresses_synced == 0
        && completion.new_tx_count == 0
        && completion.updated_tx_count == 0
    {
        let time = completion
            .occurred_at
            .with_timezone(&timezone)
            .format("%H:%M");
        return format!("Already up to date (checked {time})");
    }
    format!(
        "Synced {} addresses — {} new transactions",
        completion.addresses_synced, completion.new_tx_count
    )
}

pub(super) fn format_transaction_group_date(
    occurred_at: &str,
    tz: chrono_tz::Tz,
    date_format: DateTimeFormat,
) -> String {
    DateTime::parse_from_rfc3339(occurred_at)
        .map(|parsed| {
            let local = parsed.with_timezone(&tz);
            format_date_for_display(local.date_naive(), date_format)
        })
        .unwrap_or_else(|_| "Unknown Date".to_string())
}

pub(super) fn table_totals_text(table: &AccountTransactionTableResponse) -> String {
    format!("{} - {} of {}", table.start, table.end, table.total)
}

pub(super) fn last_page(table: &AccountTransactionTableResponse) -> u32 {
    if table.total == 0 || table.page_size == 0 {
        return 1;
    }
    table.total.div_ceil(table.page_size)
}

pub(super) fn custom_table_totals_text(table: &ManualAssetBalanceAssertionTableResponse) -> String {
    format!("{} - {} of {}", table.start, table.end, table.total)
}

pub(super) fn custom_last_page(table: &ManualAssetBalanceAssertionTableResponse) -> u32 {
    if table.total == 0 || table.page_size == 0 {
        return 1;
    }
    table.total.div_ceil(table.page_size)
}

pub(super) fn tx_explorer_url(
    settings_state: &SettingsState,
    target: crate::explorer_links::DigitalAssetTransactionRef<'_>,
) -> Result<String, String> {
    crate::explorer_links::explorer_url(
        settings_state,
        crate::explorer_links::ExplorerTarget::Transaction(target),
    )
    .map_err(|err| format!("Explorer unavailable: {err}"))
}

pub(super) fn handle_session_expired(
    mut auth_state: AuthState,
    mut banner_state: BannerState,
    context: &'static str,
) {
    let user_id = {
        let auth_snapshot = auth_state.read();
        match &*auth_snapshot {
            AuthStatus::Authenticated(auth) => Some(auth.user.user_id),
            _ => None,
        }
    };
    tracing::debug!(
        user_id = ?user_id,
        context,
        "transactions ui: session expired"
    );
    auth_state.set(AuthStatus::Unauthenticated);
    if user_id.is_some() {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

pub(super) fn route_for_account_transactions(
    account_id: WalletAccountId,
    selection: DateRangeSelection,
) -> Route {
    Route::AccountTransactions {
        account_id,
        start: selection.start_query_value(),
        end: selection.end_query_value(),
    }
}

pub(super) fn should_sync_initial_account_response(
    account_id: WalletAccountId,
    synced_account_id: Option<WalletAccountId>,
) -> bool {
    synced_account_id != Some(account_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransactionDateFilterBounds {
    pub(super) from_date: Option<String>,
    pub(super) to_date: Option<String>,
}

pub(super) fn transaction_date_filter_bounds(
    selection: DateRangeSelection,
    timezone: UserTimezone,
) -> TransactionDateFilterBounds {
    match selection {
        DateRangeSelection::Empty => TransactionDateFilterBounds {
            from_date: None,
            to_date: None,
        },
        DateRangeSelection::Range(range) => TransactionDateFilterBounds {
            from_date: Some(
                local_report_date_to_utc_boundary(
                    range.from(),
                    timezone,
                    DateBoundaryKind::StartOfDay,
                )
                .to_rfc3339(),
            ),
            to_date: Some(
                local_report_date_to_utc_boundary(range.to(), timezone, DateBoundaryKind::EndOfDay)
                    .to_rfc3339(),
            ),
        },
    }
}

pub(super) fn build_transaction_filters_query(
    active_filters: &ActiveFilters,
    date_selection: DateRangeSelection,
    timezone: UserTimezone,
) -> Option<String> {
    let date_bounds = transaction_date_filter_bounds(date_selection, timezone);
    if !active_filters.has_any() && date_bounds.from_date.is_none() && date_bounds.to_date.is_none()
    {
        return None;
    }

    let raw = crate::wallets::RawTransactionFilters {
        status: if active_filters.statuses.is_empty() {
            None
        } else {
            Some(
                active_filters
                    .statuses
                    .iter()
                    .map(|status| status.as_db_value().to_string())
                    .collect(),
            )
        },
        from_date: date_bounds.from_date,
        to_date: date_bounds.to_date,
    };

    serde_json::to_string(&raw).ok()
}

pub(super) fn local_today_in_timezone(now_utc: DateTime<Utc>, timezone: UserTimezone) -> NaiveDate {
    let tz: chrono_tz::Tz = timezone.into();
    now_utc.with_timezone(&tz).date_naive()
}

pub(super) fn current_year_to_date_range(today: NaiveDate) -> Option<LocalReportDateRange> {
    LocalReportDateRange::new(NaiveDate::from_ymd_opt(today.year(), 1, 1)?, today).ok()
}

pub(super) fn dispatch_date_range_filter_event(
    policy: DateRangeFilterPolicy,
    mut filter_state: Signal<DateRangeFilterState>,
    mut pending_route_selection: Signal<Option<DateRangeSelection>>,
    event: DateRangeFilterEvent,
) {
    let current_state = filter_state.peek().clone();
    let outcome = transition_date_range_filter(policy, &current_state, event);

    if current_state != outcome.state {
        filter_state.set(outcome.state);
    }

    if let DateRangeFilterEffect::ReplaceRoute(selection) = outcome.effect
        && pending_route_selection.peek().as_ref() != Some(&selection)
    {
        pending_route_selection.set(Some(selection));
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ActiveFilters {
    pub(super) statuses: Vec<ChainTransactionStatus>,
}

impl ActiveFilters {
    pub(super) fn has_any(&self) -> bool {
        !self.statuses.is_empty()
    }

    /// True when no status is explicitly selected, i.e. every status is shown.
    pub(super) fn is_all(&self) -> bool {
        self.statuses.is_empty()
    }

    /// True only when `status` is explicitly selected (not the "show all" state).
    pub(super) fn is_status_selected(&self, status: ChainTransactionStatus) -> bool {
        self.statuses.contains(&status)
    }

    pub(super) fn with_status_toggled(&self, status: ChainTransactionStatus) -> Self {
        let mut new_statuses = self.statuses.clone();
        if let Some(pos) = new_statuses.iter().position(|s| *s == status) {
            new_statuses.remove(pos);
        } else {
            new_statuses.push(status);
        }
        Self {
            statuses: new_statuses,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct AccountTransactionsLoader {
    pub(super) account_id: WalletAccountId,
    pub(super) auth_state: AuthState,
    pub(super) banner_state: BannerState,
    pub(super) loading: Signal<bool>,
    pub(super) error: Signal<Option<String>>,
    pub(super) response: Signal<Option<WalletAccountHistoryResponse>>,
}

impl AccountTransactionsLoader {
    pub(super) fn request(
        self,
        pending_page: u32,
        confirmed_page: u32,
        sort: TransactionSortDirection,
        filters: &ActiveFilters,
        date_selection: DateRangeSelection,
        timezone: UserTimezone,
    ) {
        if pending_page == 0 || confirmed_page == 0 || (self.loading)() {
            return;
        }

        let mut loading = self.loading;
        let mut error = self.error;
        let mut response = self.response;
        let auth_state = self.auth_state;
        let banner_state = self.banner_state;
        let account_id = self.account_id;
        let sort_value = Some(sort.as_query_value().to_string());
        let filters_value = build_transaction_filters_query(filters, date_selection, timezone);

        loading.set(true);
        error.set(None);

        spawn(async move {
            match crate::backend::get_account_transactions(
                account_id,
                Some(pending_page),
                Some(confirmed_page),
                sort_value,
                filters_value,
            )
            .await
            {
                Ok(value) => response.set(Some(value)),
                Err(err) => {
                    if err.is_unauthorized() {
                        handle_session_expired(auth_state, banner_state, "account transactions");
                    }
                    error.set(Some(err.to_string()));
                }
            }

            loading.set(false);
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ManualAssertionFormMode {
    Add,
    Edit(ManualAssetBalanceAssertionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManualAssertionFormState {
    pub(super) mode: ManualAssertionFormMode,
    pub(super) asserted_on: String,
    pub(super) balance: String,
    pub(super) note: String,
}

impl ManualAssertionFormState {
    pub(super) fn for_add(selection: DateRangeSelection, today: NaiveDate) -> Self {
        let asserted_on = match selection {
            DateRangeSelection::Empty => today.format("%Y-%m-%d").to_string(),
            DateRangeSelection::Range(range) => range.to().format("%Y-%m-%d").to_string(),
        };
        Self {
            mode: ManualAssertionFormMode::Add,
            asserted_on,
            balance: String::new(),
            note: String::new(),
        }
    }

    pub(super) fn for_edit(row: &ManualAssetBalanceAssertionRowResponse) -> Self {
        Self {
            mode: ManualAssertionFormMode::Edit(row.assertion_id),
            asserted_on: row.asserted_on.clone(),
            balance: row.asserted_balance.formatted_value.clone(),
            note: row.note.clone().unwrap_or_default(),
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::backend::AccountBalanceStateView;
    use crate::report_dates::LocalReportDateRange;
    use crate::wallets::ManualAssetPrecisionStatus;
    use chrono::NaiveDate;

    fn timezone(name: &str) -> UserTimezone {
        UserTimezone(name.parse().expect("valid timezone"))
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn range(
        start_year: i32,
        start_month: u32,
        start_day: u32,
        end_year: i32,
        end_month: u32,
        end_day: u32,
    ) -> DateRangeSelection {
        DateRangeSelection::Range(
            LocalReportDateRange::new(
                date(start_year, start_month, start_day),
                date(end_year, end_month, end_day),
            )
            .expect("valid range"),
        )
    }

    fn table(
        page: u32,
        page_size: u32,
        total: u32,
        start: u32,
        end: u32,
    ) -> AccountTransactionTableResponse {
        AccountTransactionTableResponse {
            page,
            page_size,
            total,
            start,
            end,
            rows: Vec::new(),
        }
    }

    fn custom_history_response(
        precision_status: ManualAssetPrecisionStatus,
        decimal_precision: u8,
        precision_shared_with_other_accounts: bool,
        unit_code: &str,
    ) -> ManualAssetAccountTransactionsResponse {
        ManualAssetAccountTransactionsResponse {
            account_id: WalletAccountId::new(),
            wallet_id: crate::wallets::WalletId::new(),
            wallet_label: "Wallet".to_string(),
            account_label: "Account".to_string(),
            account_state: crate::backend::AccountStateView::Active,
            sync_control_enabled: false,
            unit_code: unit_code.to_string(),
            decimal_precision,
            precision_status,
            precision_shared_with_other_accounts,
            symbol: None,
            asset_name: None,
            network_name: None,
            opening_balance_state: AccountBalanceStateView::Unknown,
            opening_balance_date: None,
            closing_balance_state: AccountBalanceStateView::Unknown,
            closing_balance_date: None,
            sort: TransactionSortDirection::Descending,
            active_from_date: None,
            active_to_date: None,
            assertions: ManualAssetBalanceAssertionTableResponse {
                page: 1,
                page_size: 50,
                total: 0,
                start: 0,
                end: 0,
                rows: Vec::new(),
            },
        }
    }

    #[test]
    fn manual_assertion_precision_helper_text_reports_not_inferred_status() {
        let response =
            custom_history_response(ManualAssetPrecisionStatus::NotInferredYet, 0, false, "ADA");

        assert_eq!(
            manual_assertion_precision_helper_text(&response),
            "ADA may have up to 0 numbers after the decimal"
        );
    }

    #[test]
    fn manual_assertion_precision_helper_text_reports_inferred_shared_precision() {
        let response =
            custom_history_response(ManualAssetPrecisionStatus::Inferred, 9, true, "ADA");

        assert_eq!(
            manual_assertion_precision_helper_text(&response),
            "ADA may have up to 9 numbers after the decimal"
        );
    }

    #[test]
    fn manual_assertion_precision_helper_text_reports_legacy_baseline() {
        let response =
            custom_history_response(ManualAssetPrecisionStatus::LegacyBaseline, 8, false, "ADA");

        assert_eq!(
            manual_assertion_precision_helper_text(&response),
            "ADA may have up to 8 numbers after the decimal"
        );
    }

    #[test]
    fn table_totals_text_formats_range() {
        assert_eq!(
            table_totals_text(&table(1, 50, 2458, 1, 50)),
            "1 - 50 of 2458"
        );
        assert_eq!(table_totals_text(&table(1, 50, 0, 0, 0)), "0 - 0 of 0");
    }

    #[test]
    fn last_page_computes_correctly() {
        assert_eq!(last_page(&table(1, 50, 0, 0, 0)), 1);
        assert_eq!(last_page(&table(1, 50, 50, 1, 50)), 1);
        assert_eq!(last_page(&table(1, 50, 51, 1, 50)), 2);
        assert_eq!(last_page(&table(1, 50, 2458, 1, 50)), 50);
    }

    #[test]
    fn active_filters_default_has_no_filters() {
        let f = ActiveFilters::default();
        assert!(!f.has_any());
        assert!(
            build_transaction_filters_query(
                &f,
                DateRangeSelection::Empty,
                timezone("Europe/Amsterdam")
            )
            .is_none()
        );
    }

    #[test]
    fn active_filters_with_status_toggled_adds_and_removes() {
        let f = ActiveFilters::default();
        assert!(f.is_all());
        assert!(!f.is_status_selected(ChainTransactionStatus::Confirmed));

        let f = f.with_status_toggled(ChainTransactionStatus::Confirmed);
        assert_eq!(f.statuses.len(), 1);
        assert!(f.has_any());
        assert!(!f.is_all());
        assert!(f.is_status_selected(ChainTransactionStatus::Confirmed));

        let f = f.with_status_toggled(ChainTransactionStatus::Confirmed);
        assert!(f.statuses.is_empty());
        assert!(!f.has_any());
        assert!(f.is_all());
    }

    #[test]
    fn transaction_filter_query_serializes_status_and_date_range() {
        let f = ActiveFilters {
            statuses: vec![ChainTransactionStatus::Confirmed],
        };
        let json = build_transaction_filters_query(
            &f,
            range(2026, 1, 1, 2026, 1, 31),
            timezone("Europe/Amsterdam"),
        )
        .expect("should produce JSON");
        assert!(json.contains("confirmed"));
        assert!(json.contains("\"from_date\":\"2025-12-31T23:00:00+00:00\""));
        assert!(json.contains("\"to_date\":\"2026-01-31T22:59:59+00:00\""));
    }

    #[test]
    fn route_for_account_transactions_uses_start_and_end_query_fields() {
        let account_id = WalletAccountId::new();
        let route = route_for_account_transactions(account_id, range(2026, 1, 1, 2026, 3, 31));

        assert!(matches!(
            route,
            Route::AccountTransactions {
                account_id: actual_account_id,
                start: Some(start),
                end: Some(end),
            } if actual_account_id == account_id && start == "2026-01-01" && end == "2026-03-31"
        ));
    }

    #[test]
    fn should_sync_initial_account_response_skips_already_synced_account() {
        let account_id = WalletAccountId::new();

        assert!(!should_sync_initial_account_response(
            account_id,
            Some(account_id)
        ));
    }

    #[test]
    fn should_sync_initial_account_response_syncs_after_account_route_changes() {
        let previous_account_id = WalletAccountId::new();
        let current_account_id = WalletAccountId::new();

        assert!(should_sync_initial_account_response(
            current_account_id,
            Some(previous_account_id)
        ));
    }

    #[test]
    fn current_year_to_date_range_uses_january_first_to_today() {
        let range = current_year_to_date_range(date(2026, 3, 31)).expect("range should exist");

        assert_eq!(range.from(), date(2026, 1, 1));
        assert_eq!(range.to(), date(2026, 3, 31));
    }

    #[test]
    fn zero_balance_canonical_uses_asset_scale() {
        assert_eq!(zero_balance_canonical(SyncedAssetId::Bitcoin), "0.00000000");
        assert_eq!(
            zero_balance_canonical(SyncedAssetId::Ethereum),
            "0.000000000000000000"
        );
    }

    #[test]
    fn format_native_balance_state_display_uses_scaled_zero_for_canonical_zero() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::CommaDot,
        );

        let formatted = format_native_balance_state_display(
            &crate::backend::NativeBalanceStateView::CanonicalZero,
            SyncedAssetId::Bitcoin,
            &None,
            &context,
            NumberFormat::CommaDot,
        );

        assert_eq!(formatted, "₿0,00000000");
    }

    #[test]
    fn format_native_balance_state_display_renders_unknown_as_not_available() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::CommaDot,
        );
        let formatted = format_native_balance_state_display(
            &crate::backend::NativeBalanceStateView::Unknown,
            SyncedAssetId::Bitcoin,
            &None,
            &context,
            NumberFormat::CommaDot,
        );
        assert_eq!(formatted, "Not available");
    }

    #[test]
    fn null_transaction_closing_renders_not_available_without_provisional_status() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::CommaDot,
        );
        let reliability = crate::balance_reliability::BalanceReliability::Provisional {
            reasons: vec![
                crate::balance_reliability::BalanceProvisionalReason::HistoricalBackfillInProgress,
            ],
        };

        assert_eq!(format_closing_balance(None, &context), "Not available");
        assert!(!show_closing_balance_provisional(None, &reliability));
    }

    #[test]
    fn known_transaction_closing_shows_provisional_status_only_when_reliability_is_provisional() {
        let closing = crate::backend::BalanceAmountView {
            raw_value: "1".to_string(),
            formatted_value: "0.00000001".to_string(),
        };

        assert!(show_closing_balance_provisional(
            Some(&closing),
            &crate::balance_reliability::BalanceReliability::Provisional {
                reasons: vec![
                    crate::balance_reliability::BalanceProvisionalReason::HistoricalBackfillInProgress,
                ],
            },
        ));
        assert!(!show_closing_balance_provisional(
            Some(&closing),
            &crate::balance_reliability::BalanceReliability::Final,
        ));
    }

    #[test]
    fn format_balance_reliability_display_does_not_qualify_unavailable_amount() {
        let formatted = format_balance_reliability_display(
            "Not available".to_string(),
            &crate::balance_reliability::BalanceReliability::Provisional {
                reasons: vec![
                    crate::balance_reliability::BalanceProvisionalReason::FirstSuccessfulSyncPending,
                ],
            },
        );

        assert_eq!(formatted, "Not available");
    }

    #[test]
    fn format_balance_reliability_display_leaves_final_values_unchanged() {
        let formatted = format_balance_reliability_display(
            "₿0.23702331".to_string(),
            &crate::balance_reliability::BalanceReliability::Final,
        );

        assert_eq!(formatted, "₿0.23702331");
    }

    #[test]
    fn manual_sync_outcome_message_reports_up_to_date_when_nothing_synced() {
        let completion = crate::components::wallets::SyncRunCompletion {
            run_id: None,
            occurred_at: "2026-07-11T17:51:19Z".parse().expect("test timestamp"),
            failed: false,
            new_tx_count: 0,
            updated_tx_count: 0,
            addresses_synced: 0,
            error: None,
        };
        assert_eq!(
            manual_sync_outcome_message(&completion, chrono_tz::UTC),
            "Already up to date (checked 17:51)"
        );
    }

    #[test]
    fn manual_sync_outcome_message_reports_synced_counts() {
        let completion = crate::components::wallets::SyncRunCompletion {
            run_id: None,
            occurred_at: "2026-07-11T17:58:53Z".parse().expect("test timestamp"),
            failed: false,
            new_tx_count: 12,
            updated_tx_count: 3,
            addresses_synced: 44,
            error: None,
        };
        assert_eq!(
            manual_sync_outcome_message(&completion, chrono_tz::UTC),
            "Synced 44 addresses — 12 new transactions"
        );
    }
}

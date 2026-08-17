use super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterEvent, DateRangeFilterPolicy, DateRangeFilterState,
    DateRangeRouteParams, DateRangeSelection, initialize_date_range_filter,
    transition_date_range_filter,
};
use super::date_range_toolbar::DateRangeToolbar;
use super::formatting::DisplayAmountSign;
use super::formatting::{ManualConversionQuote, convert_amount, format_date_for_display};
use super::wallet_report_prices::PricesSection;
use super::wallets::{
    AddBitcoinAddressFlow, AddDropdownButton, AddEthereumAddressFlow, AddManualAssetFlow,
    AddXpubFlow, route_for_added_manual_asset,
};
use super::{
    HoldingsReportFreeNotice, format_change_percent, format_fiat_view, free_window_aria_label,
    free_window_badge_label, free_window_tooltip, show_free_report_notice,
};
use crate::asset_views::CatalogAssetKey;
use crate::backend::get_wallet_report;
use crate::backend::{
    BalanceAmountView, FiatAmountView, ResolvedPriceView, WalletReportAccountRow,
    WalletReportBalanceStateView, WalletReportResponse, list_resolved_prices_for_report,
};
use crate::models::CurrencyCode;
use crate::models::NumberFormat;
use crate::report_dates::{dial_year_range, displayed_calendar_year};
use crate::services::price_overrides::{BoundaryKind, PriceSubject, price_subject_sort_key};
use crate::settings::SettingsState;
use crate::wallets::{ReportTimezoneParam, WalletId, WalletReportDateRange};
use crate::{AuthState, AuthStatus, BannerMessage, BannerState, Route};
use chrono::Datelike;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::HashMap;

const WALLETS_CSS: Asset = asset!("/assets/wallets.css");
const WALLET_REPORT_PRICES_CSS: Asset = asset!("/assets/wallet_report_prices.css");

// ── Pure helpers ────────────────────────────────────────────

fn handle_session_expired(
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
    tracing::debug!(user_id = ?user_id, context, "wallet report ui: session expired");
    auth_state.set(AuthStatus::Unauthenticated);
    if user_id.is_some() {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

fn route_for_wallet_report(wallet_id: WalletId, selection: DateRangeSelection) -> Route {
    Route::WalletReport {
        wallet_id,
        start: selection.start_query_value(),
        end: selection.end_query_value(),
    }
}

fn dispatch_date_range_filter_event(
    policy: DateRangeFilterPolicy,
    mut filter_state: Signal<DateRangeFilterState>,
    mut active_selection: Signal<DateRangeSelection>,
    mut pending_route_selection: Signal<Option<DateRangeSelection>>,
    event: DateRangeFilterEvent,
) {
    let current_state = filter_state.peek().clone();
    let outcome = transition_date_range_filter(policy, &current_state, event);

    if current_state != outcome.state {
        let next_selection = outcome.state.selection();
        if *active_selection.peek() != next_selection {
            active_selection.set(next_selection);
        }
        filter_state.set(outcome.state);
    }

    if let DateRangeFilterEffect::ReplaceRoute(selection) = outcome.effect
        && pending_route_selection.peek().as_ref() != Some(&selection)
    {
        pending_route_selection.set(Some(selection));
    }
}

fn report_price_boundary_times(report: &WalletReportResponse) -> (String, String) {
    (
        format!("{}T00:00:00", report.resolved_from),
        format!("{}T23:59:59", report.resolved_to),
    )
}

fn report_balance_view(balance_state: &WalletReportBalanceStateView) -> Option<&BalanceAmountView> {
    match balance_state {
        WalletReportBalanceStateView::NeedsPrice(balance) => Some(balance),
        WalletReportBalanceStateView::CanonicalZero | WalletReportBalanceStateView::Unknown => None,
    }
}

fn parsed_balance_amount(balance_state: &WalletReportBalanceStateView) -> Option<Decimal> {
    report_balance_view(balance_state)
        .and_then(|balance| balance.formatted_value.parse::<Decimal>().ok())
}

fn balance_needs_price(balance_state: &WalletReportBalanceStateView) -> bool {
    matches!(balance_state, WalletReportBalanceStateView::NeedsPrice(_))
}

fn compute_fiat_value(
    balance_state: &WalletReportBalanceStateView,
    price: Option<Decimal>,
) -> Option<Decimal> {
    match balance_state {
        WalletReportBalanceStateView::CanonicalZero => Some(Decimal::ZERO),
        WalletReportBalanceStateView::NeedsPrice(_) => parsed_balance_amount(balance_state)
            .and_then(|amount| price.map(|quote| amount * quote)),
        WalletReportBalanceStateView::Unknown => None,
    }
}

fn should_show_fiat_value(balance_state: &WalletReportBalanceStateView) -> bool {
    !matches!(balance_state, WalletReportBalanceStateView::Unknown)
}

#[derive(Debug, Clone, PartialEq)]
struct AccountFiat {
    opening_fiat: Option<Decimal>,
    closing_fiat: Option<Decimal>,
    change: Option<Decimal>,
    change_percent: Option<Decimal>,
    opening_needs_price: bool,
    closing_needs_price: bool,
}

fn compute_account_fiat(
    row: &WalletReportAccountRow,
    opening_price: Option<Decimal>,
    closing_price: Option<Decimal>,
) -> AccountFiat {
    let needs_opening = balance_needs_price(&row.opening_balance_state);
    let needs_closing = balance_needs_price(&row.closing_balance_state);

    let opening_fiat = compute_fiat_value(&row.opening_balance_state, opening_price);
    let closing_fiat = compute_fiat_value(&row.closing_balance_state, closing_price);

    let (change, change_percent) = match (opening_fiat, closing_fiat) {
        (Some(o), Some(c)) if o != Decimal::ZERO => {
            let change = c - o;
            (Some(change), Some(change / o * Decimal::from(100)))
        }
        (Some(o), Some(c)) => (Some(c - o), None),
        _ => (None, None),
    };

    AccountFiat {
        opening_fiat,
        closing_fiat,
        change,
        change_percent,
        opening_needs_price: needs_opening,
        closing_needs_price: needs_closing,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ReportTotals {
    opening: Option<Decimal>,
    closing: Option<Decimal>,
    change: Option<Decimal>,
    change_percent: Option<Decimal>,
}

fn compute_report_totals(account_fiats: &[AccountFiat]) -> ReportTotals {
    let opening = account_fiats
        .iter()
        .map(|a| a.opening_fiat)
        .collect::<Option<Vec<Decimal>>>()
        .map(|values| values.iter().sum());

    let closing = account_fiats
        .iter()
        .map(|a| a.closing_fiat)
        .collect::<Option<Vec<Decimal>>>()
        .map(|values| values.iter().sum());

    let (change, change_percent) = match (opening, closing) {
        (Some(o), Some(c)) if o != Decimal::ZERO => {
            let change = c - o;
            (Some(change), Some(change / o * Decimal::from(100)))
        }
        (Some(o), Some(c)) => (Some(c - o), None),
        _ => (None, None),
    };

    ReportTotals {
        opening,
        closing,
        change,
        change_percent,
    }
}

fn format_fiat(value: f64, currency: CurrencyCode, number_format: NumberFormat) -> String {
    let quote = ManualConversionQuote {
        currency,
        price_per_unit: 1.0,
    };
    let sign = if value < 0.0 {
        DisplayAmountSign::Negative
    } else {
        DisplayAmountSign::Hidden
    };
    convert_amount(&format!("{:.2}", value.abs()), sign, &quote, number_format)
}

fn format_fiat_decimal(
    value: Decimal,
    currency: CurrencyCode,
    number_format: NumberFormat,
) -> String {
    let fiat_value = FiatAmountView {
        raw_value: value.to_string(),
        formatted_value: format_fiat(value.to_f64().unwrap_or(0.0), currency, number_format),
    };
    format_fiat_view(&fiat_value, currency, number_format)
}

fn change_class(value: Decimal) -> &'static str {
    if value > Decimal::ZERO {
        "wr-change-positive"
    } else if value < Decimal::ZERO {
        "wr-change-negative"
    } else {
        ""
    }
}

fn asset_badge_class(_key: Option<&CatalogAssetKey>) -> &'static str {
    "account-asset-badge"
}

fn format_crypto_balance(balance_state: &WalletReportBalanceStateView, unit_code: &str) -> String {
    match report_balance_view(balance_state) {
        Some(b) => format!("{} {}", b.formatted_value, unit_code),
        None => "Not available".to_string(),
    }
}

fn price_subject_for_report_row(row: &WalletReportAccountRow) -> Option<PriceSubject> {
    row.catalog_asset_key
        .clone()
        .map(PriceSubject::CatalogAsset)
}

type PriceRequirement = (PriceSubject, BoundaryKind);
type ResolvedPricesMap = HashMap<(PriceSubject, BoundaryKind), Option<Decimal>>;

fn boundary_sort_key(boundary: BoundaryKind) -> u8 {
    match boundary {
        BoundaryKind::Opening => 0,
        BoundaryKind::Closing => 1,
    }
}

fn price_requirements_for_report(report: &WalletReportResponse) -> Vec<PriceRequirement> {
    let mut requirements = Vec::new();
    for row in &report.accounts {
        let Some(subject) = price_subject_for_report_row(row) else {
            continue;
        };
        if balance_needs_price(&row.opening_balance_state) {
            requirements.push((subject.clone(), BoundaryKind::Opening));
        }
        if balance_needs_price(&row.closing_balance_state) {
            requirements.push((subject, BoundaryKind::Closing));
        }
    }

    requirements.sort_by_key(|(subject, boundary)| {
        (
            price_subject_sort_key(subject),
            boundary_sort_key(*boundary),
        )
    });
    requirements.dedup();
    requirements
}

fn resolved_price_for_row(
    prices: &ResolvedPricesMap,
    row: &WalletReportAccountRow,
    boundary: BoundaryKind,
) -> Option<Decimal> {
    let subject = price_subject_for_report_row(row)?;
    prices.get(&(subject, boundary)).copied().flatten()
}

fn resolved_prices_map_from_views(views: &[ResolvedPriceView]) -> ResolvedPricesMap {
    views
        .iter()
        .map(|view| {
            let price = view
                .price
                .as_deref()
                .and_then(|s| s.parse::<Decimal>().ok());
            ((view.subject.clone(), view.boundary), price)
        })
        .collect()
}

// ── Main Component ──────────────────────────────────────────

#[component]
pub(crate) fn WalletReport(
    wallet_id: WalletId,
    start: Option<String>,
    end: Option<String>,
) -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();
    let settings_state = use_context::<SettingsState>();
    let user_currency = (settings_state.currency)();
    let navigator = use_navigator();

    let current_route = DateRangeRouteParams::new(start.clone(), end.clone());
    let initial_filter = initialize_date_range_filter(
        DateRangeFilterPolicy::RequiredCanonicalRange,
        current_route.clone(),
    );
    let initial_selection = initial_filter.state.selection();
    let initial_pending_route_selection = match initial_filter.effect {
        DateRangeFilterEffect::None => None,
        DateRangeFilterEffect::ReplaceRoute(selection) => Some(selection),
    };
    let mut filter_state = use_signal(|| initial_filter.state.clone());
    let active_selection = use_signal(|| initial_selection);
    let mut pending_route_selection = use_signal(|| initial_pending_route_selection);
    let mut unauthorized_handled = use_signal(|| false);
    let mut show_add_xpub = use_signal(|| false);
    let mut show_add_bitcoin = use_signal(|| false);
    let mut show_add_ethereum = use_signal(|| false);
    let mut show_add_manual_asset = use_signal(|| false);

    if filter_state.peek().route() != &current_route {
        dispatch_date_range_filter_event(
            DateRangeFilterPolicy::RequiredCanonicalRange,
            filter_state,
            active_selection,
            pending_route_selection,
            DateRangeFilterEvent::RouteChanged(current_route),
        );
    }

    use_effect(move || {
        if let Some(selection) = pending_route_selection() {
            navigator.replace(route_for_wallet_report(wallet_id, selection));
            pending_route_selection.set(None);
        }
    });

    let mut report_resource = use_server_future(move || {
        let timezone = ReportTimezoneParam((settings_state.timezone)());
        let selection = active_selection();
        async move {
            get_wallet_report(
                wallet_id,
                selection.start_param(),
                selection.end_param(),
                timezone,
            )
            .await
        }
    })?;

    let mut resolved_prices_resource = use_server_future(move || {
        let timezone = ReportTimezoneParam((settings_state.timezone)());
        let selection = active_selection();
        let from = selection.start_param();
        let to = selection.end_param();
        async move {
            match (from, to) {
                (Some(from), Some(to)) => {
                    list_resolved_prices_for_report(wallet_id, from, to, timezone).await
                }
                _ => Ok(Vec::new()),
            }
        }
    })?;

    let report_result = report_resource();
    let resolved_views: Vec<ResolvedPriceView> = resolved_prices_resource()
        .and_then(|result| result.ok())
        .unwrap_or_default();
    let resolved_prices = resolved_prices_map_from_views(&resolved_views);

    if let Some(Err(err)) = report_result.as_ref() {
        if err.is_unauthorized() && !*unauthorized_handled.peek() {
            unauthorized_handled.set(true);
            handle_session_expired(auth_state, banner_state, "wallet report");
        }
    } else if *unauthorized_handled.peek() {
        unauthorized_handled.set(false);
    }

    let report: Option<&WalletReportResponse> = report_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());

    if let Some(report) = report {
        let canonical_range =
            WalletReportDateRange::new(report.resolved_from, report.resolved_to).ok();
        if let Some(canonical_range) = canonical_range {
            if report.access.range_clamped {
                // The server clamped the requested range to the Free window. Show the
                // effective range in the controls without moving the requested route,
                // so the fetch (and the Free-window badge) still reflect the request.
                let outcome = transition_date_range_filter(
                    DateRangeFilterPolicy::RequiredCanonicalRange,
                    &filter_state.peek().clone(),
                    DateRangeFilterEvent::ClampedDisplay(canonical_range),
                );
                if *filter_state.peek() != outcome.state {
                    filter_state.set(outcome.state);
                }
            } else {
                dispatch_date_range_filter_event(
                    DateRangeFilterPolicy::RequiredCanonicalRange,
                    filter_state,
                    active_selection,
                    pending_route_selection,
                    DateRangeFilterEvent::ServerResolved(canonical_range),
                );
            }
        }
    }

    let this_year_preset = report.and_then(|data| {
        WalletReportDateRange::new(data.default_this_year_from, data.default_this_year_to).ok()
    });

    let number_format = (settings_state.number_format)();
    let date_format = (settings_state.date_time_format)();
    let filter_state_snapshot = filter_state();
    let start_input_value = filter_state_snapshot.start_input_value().to_string();
    let end_input_value = filter_state_snapshot.end_input_value().to_string();
    let validation_message = filter_state_snapshot
        .validation_message()
        .map(str::to_string);

    let user_timezone = (settings_state.timezone)();

    // Drive the controls from the filter state so a clamped report can display its
    // effective range while the requested route keeps driving the fetch.
    let active_range = match filter_state_snapshot.selection() {
        DateRangeSelection::Range(range) => Some(range),
        DateRangeSelection::Empty => None,
    };
    let current_year = this_year_preset.map(|range| range.from().year());
    let displayed_year =
        active_range.and_then(|range| displayed_calendar_year(range, this_year_preset));
    let year_label = displayed_year
        .map(|year| year.to_string())
        .unwrap_or_else(|| "Custom range".to_string());
    let disable_previous_year = displayed_year.is_none();
    let disable_next_year = match (displayed_year, current_year) {
        (Some(year), Some(current)) => year >= current,
        _ => true,
    };
    let show_this_year = displayed_year != current_year;
    let custom_range_open = displayed_year.is_none();

    rsx! {
        document::Stylesheet { href: WALLETS_CSS }
        document::Stylesheet { href: WALLET_REPORT_PRICES_CSS }

        div { class: "page-container transactions-page",
            div { class: "page-header transactions-page-header",
                div { class: "tx-header-top-row",
                    h1 { class: "tx-header-heading",
                        if let Some(report) = report {
                            "{report.wallet_label}"
                            span { class: "wr-report-kind", "Holdings Report" }
                            if let Some(label) = free_window_badge_label(&report.access) {
                                Link {
                                    class: "wr-free-window-badge",
                                    title: free_window_tooltip(&report.access).unwrap_or(""),
                                    "aria-label": free_window_aria_label(),
                                    to: Route::Payments,
                                    "{label}"
                                }
                            }
                        } else {
                            "Holdings Report"
                        }
                    }
                    AddDropdownButton {
                        on_link_trezor: move |_| {},
                        on_add_xpub: move |_| show_add_xpub.set(true),
                        on_add_bitcoin: move |_| show_add_bitcoin.set(true),
                        on_add_ethereum: move |_| show_add_ethereum.set(true),
                        on_add_manual_asset: move |_| show_add_manual_asset.set(true),
                    }
                }
            }

            if let Some(report) = report {
                if show_free_report_notice(&report.access) {
                    HoldingsReportFreeNotice {}
                }
            }

            div { class: "card",
                div { class: "card-body",
                    DateRangeToolbar {
                        start_input_id: "wallet-report-start".to_string(),
                        start_input_value,
                        end_input_id: "wallet-report-end".to_string(),
                        end_input_value,
                        validation_message,
                        disable_date_inputs: false,
                        year_label,
                        disable_previous_year,
                        disable_next_year,
                        show_this_year,
                        custom_range_open,
                        on_start_change: move |value| {
                            dispatch_date_range_filter_event(
                                DateRangeFilterPolicy::RequiredCanonicalRange,
                                filter_state,
                                active_selection,
                                pending_route_selection,
                                DateRangeFilterEvent::StartEdited(value),
                            );
                        },
                        on_end_change: move |value| {
                            dispatch_date_range_filter_event(
                                DateRangeFilterPolicy::RequiredCanonicalRange,
                                filter_state,
                                active_selection,
                                pending_route_selection,
                                DateRangeFilterEvent::EndEdited(value),
                            );
                        },
                        on_this_year: move |_| {
                            if let Some(range) = this_year_preset {
                                dispatch_date_range_filter_event(
                                    DateRangeFilterPolicy::RequiredCanonicalRange,
                                    filter_state,
                                    active_selection,
                                    pending_route_selection,
                                    DateRangeFilterEvent::PresetChosen(range),
                                );
                            }
                        },
                        on_previous_year: move |_| {
                            if let Some(year) = displayed_year
                                && let Some(range) = dial_year_range(
                                    year - 1,
                                    current_year.unwrap_or(year),
                                    this_year_preset,
                                )
                            {
                                dispatch_date_range_filter_event(
                                    DateRangeFilterPolicy::RequiredCanonicalRange,
                                    filter_state,
                                    active_selection,
                                    pending_route_selection,
                                    DateRangeFilterEvent::PresetChosen(range),
                                );
                            }
                        },
                        on_next_year: move |_| {
                            if let Some(year) = displayed_year
                                && let Some(range) = dial_year_range(
                                    year + 1,
                                    current_year.unwrap_or(year + 1),
                                    this_year_preset,
                                )
                            {
                                dispatch_date_range_filter_event(
                                    DateRangeFilterPolicy::RequiredCanonicalRange,
                                    filter_state,
                                    active_selection,
                                    pending_route_selection,
                                    DateRangeFilterEvent::PresetChosen(range),
                                );
                            }
                        },
                    }

                }
            }

            if let Some(Err(err)) = report_result.as_ref() {
                if !err.is_unauthorized() {
                    div { class: "error-block",
                        p {
                            if err.is_not_found() {
                                "Wallet not found"
                            } else {
                                "{err}"
                            }
                        }
                    }
                }
            } else if report_result.is_none() {
                div { class: "card skeleton-card",
                    div { class: "card-body",
                        div { class: "skeleton-line skeleton-line-title" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-full" }
                        div { class: "skeleton-line skeleton-line-medium" }
                    }
                }
            } else if let Some(report) = report {
                {
                    let prices_snapshot = resolved_prices.clone();
                    let account_fiats: Vec<AccountFiat> = report
                        .accounts
                        .iter()
                        .map(|row| {
                            compute_account_fiat(
                                row,
                                resolved_price_for_row(
                                    &prices_snapshot,
                                    row,
                                    BoundaryKind::Opening,
                                ),
                                resolved_price_for_row(
                                    &prices_snapshot,
                                    row,
                                    BoundaryKind::Closing,
                                ),
                            )
                        })
                        .collect();
                    let price_requirements = price_requirements_for_report(report);
                    let totals = compute_report_totals(&account_fiats);
                    let from_display = format_date_for_display(report.resolved_from, date_format);
                    let to_display = format_date_for_display(report.resolved_to, date_format);
                    let show_prices_section = !price_requirements.is_empty();
                    let subject_labels: Vec<(PriceSubject, String)> = report
                        .accounts
                        .iter()
                        .filter_map(|row| {
                            let subject = price_subject_for_report_row(row)?;
                            let label = row.asset_display_name.clone()
                                .unwrap_or_else(|| row.unit_code.clone());
                            Some((subject, label))
                        })
                        .collect();
                    let (opening_time_local_for_section, closing_time_local_for_section) =
                        report_price_boundary_times(report);
                    let resolved_views_for_section = resolved_views.clone();

                    rsx! {
                        if show_prices_section {
                            PricesSection {
                                user_currency,
                                price_requirements,
                                subject_labels,
                                opening_time_local: opening_time_local_for_section,
                                closing_time_local: closing_time_local_for_section,
                                user_timezone,
                                resolved_views: resolved_views_for_section,
                                can_edit_prices: report.access.can_edit_prices,
                                on_prices_changed: move |_| {
                                    resolved_prices_resource.restart();
                                },
                            }
                        }
                        // Desktop / Tablet table (hidden on mobile via CSS)
                        div { class: "card wr-desktop-view",
                            table { class: "wr-table",
                                thead {
                                    tr {
                                        th { class: "wr-th-account", "Account" }
                                        th { class: "wr-th-value",
                                            "Opening Value"
                                            br {}
                                            span { class: "wr-th-date", "{from_display}" }
                                        }
                                        th { class: "wr-th-value",
                                            "Closing Value"
                                            br {}
                                            span { class: "wr-th-date", "{to_display}" }
                                        }
                                        th { class: "wr-th-change",
                                            "Change ({user_currency.symbol()})"
                                        }
                                    }
                                }
                                tbody {
                                    for (index, row) in report.accounts.iter().enumerate() {
                                        {
                                            let fiat = &account_fiats[index];
                                            let account_id = row.account_id;

                                            rsx! {
                                                tr { class: "wr-row",
                                                    td { class: "wr-td-account",
                                                        div { class: "wr-account-info",
                                                            span { class: asset_badge_class(row.catalog_asset_key.as_ref()),
                                                                "{row.unit_code}"
                                                            }
                                                            Link {
                                                                class: "wr-account-label-link",
                                                                to: Route::AccountTransactions {
                                                                    account_id,
                                                                    start: None,
                                                                    end: None,
                                                                },
                                                                div { class: "wr-account-label", "{row.account_label}" }
                                                            }
                                                            if let Some(coverage) = row.bitcoin_history_coverage {
                                                                span { class: "muted",
                                                                    "History coverage: {coverage.label()}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                    td { class: "wr-td-value",
                                                        ReportValueCell {
                                                            fiat_value: fiat.opening_fiat,
                                                            balance_state: row.opening_balance_state.clone(),
                                                            unit_code: row.unit_code.clone(),
                                                            currency: user_currency,
                                                            number_format,
                                                            price: resolved_price_for_row(&prices_snapshot, row, BoundaryKind::Opening),
                                                        }
                                                    }
                                                    td { class: "wr-td-value",
                                                        ReportValueCell {
                                                            fiat_value: fiat.closing_fiat,
                                                            balance_state: row.closing_balance_state.clone(),
                                                            unit_code: row.unit_code.clone(),
                                                            currency: user_currency,
                                                            number_format,
                                                            price: resolved_price_for_row(&prices_snapshot, row, BoundaryKind::Closing),
                                                        }
                                                    }
                                                    td { class: "wr-td-change",
                                                        if let Some(change) = fiat.change {
                                                            div { class: "wr-change {change_class(change)}",
                                                                "{format_fiat_decimal(change, user_currency, number_format)}"
                                                            }
                                                            if let Some(percent) = fiat.change_percent {
                                                                div { class: "wr-change-percent",
                                                                    "{format_change_percent(percent)}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                tfoot {
                                    tr { class: "wr-totals-row",
                                        td { class: "wr-td-total-label", "Total" }
                                        td { class: "wr-td-value",
                                            if let Some(opening) = totals.opening {
                                                span { class: "wr-total-value",
                                                    "{format_fiat_decimal(opening, user_currency, number_format)}"
                                                }
                                            }
                                        }
                                        td { class: "wr-td-value",
                                            if let Some(closing) = totals.closing {
                                                span { class: "wr-total-value",
                                                    "{format_fiat_decimal(closing, user_currency, number_format)}"
                                                }
                                            }
                                        }
                                        td { class: "wr-td-change",
                                            if let Some(change) = totals.change {
                                                div { class: "wr-change wr-total-value {change_class(change)}",
                                                    "{format_fiat_decimal(change, user_currency, number_format)}"
                                                }
                                                if let Some(percent) = totals.change_percent {
                                                    div { class: "wr-change-percent",
                                                        "{format_change_percent(percent)}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Mobile card view (hidden on desktop via CSS)
                        div { class: "wr-mobile-view",
                            for (index, row) in report.accounts.iter().enumerate() {
                                {
                                    let fiat = &account_fiats[index];
                                    let account_id = row.account_id;
                                    let opening_date_str = row
                                        .opening_balance_date
                                        .map(|d| format_date_for_display(d, date_format))
                                        .unwrap_or_else(|| "—".to_string());
                                    let closing_date_str = row
                                        .closing_balance_date
                                        .map(|d| format_date_for_display(d, date_format))
                                        .unwrap_or_else(|| "—".to_string());

                                    rsx! {
                                        div { class: "card wr-card",
                                            div { class: "wr-card-header",
                                                div { class: "wr-account-info",
                                                    span { class: asset_badge_class(row.catalog_asset_key.as_ref()),
                                                        "{row.unit_code}"
                                                    }
                                                    Link {
                                                        class: "wr-account-label-link",
                                                        to: Route::AccountTransactions {
                                                            account_id,
                                                            start: None,
                                                            end: None,
                                                        },
                                                        span { class: "wr-account-label",
                                                            "{row.account_label}"
                                                        }
                                                    }
                                                    if let Some(coverage) = row.bitcoin_history_coverage {
                                                        span { class: "muted",
                                                            "History coverage: {coverage.label()}"
                                                        }
                                                    }
                                                }
                                                if let Some(change) = fiat.change {
                                                    div { class: "wr-card-change-container",
                                                        span { class: "wr-card-change-badge {change_class(change)}",
                                                            "{format_fiat_decimal(change, user_currency, number_format)}"
                                                        }
                                                        if let Some(percent) = fiat.change_percent {
                                                            span { class: "wr-card-change-percent {change_class(change)}",
                                                                "{format_change_percent(percent)}"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "wr-card-body",
                                                div { class: "wr-card-side",
                                                    div { class: "wr-card-side-header",
                                                        span { class: "wr-card-date-label", "{opening_date_str}" }
                                                        if let Some(fiat_val) = fiat.opening_fiat {
                                                            if should_show_fiat_value(&row.opening_balance_state) {
                                                                span { class: "wr-card-fiat-value",
                                                                    "{format_fiat_decimal(fiat_val, user_currency, number_format)}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                    div { class: "wr-card-side-detail",
                                                        span { class: "wr-card-crypto",
                                                            "{format_crypto_balance(&row.opening_balance_state, &row.unit_code)}"
                                                        }
                                                        if let Some(p) = resolved_price_for_row(&prices_snapshot, row, BoundaryKind::Opening) {
                                                            span { class: "wr-price-display",
                                                                span { class: "wr-price-display-prefix", "@ {user_currency.symbol()}" }
                                                                span { class: "wr-price-display-value", "{p}" }
                                                            }
                                                        }
                                                    }
                                                }
                                                div { class: "wr-card-side",
                                                    div { class: "wr-card-side-header",
                                                        span { class: "wr-card-date-label", "{closing_date_str}" }
                                                        if let Some(fiat_val) = fiat.closing_fiat {
                                                            if should_show_fiat_value(&row.closing_balance_state) {
                                                                span { class: "wr-card-fiat-value",
                                                                    "{format_fiat_decimal(fiat_val, user_currency, number_format)}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                    div { class: "wr-card-side-detail",
                                                        span { class: "wr-card-crypto",
                                                            "{format_crypto_balance(&row.closing_balance_state, &row.unit_code)}"
                                                        }
                                                        if let Some(p) = resolved_price_for_row(&prices_snapshot, row, BoundaryKind::Closing) {
                                                            span { class: "wr-price-display",
                                                                span { class: "wr-price-display-prefix", "@ {user_currency.symbol()}" }
                                                                span { class: "wr-price-display-value", "{p}" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if totals.opening.is_some() || totals.closing.is_some() {
                                div { class: "wr-summary-card",
                                    h3 { class: "wr-summary-title", "Total Portfolio Summary" }
                                    div { class: "wr-summary-rows",
                                        if let Some(opening) = totals.opening {
                                            div { class: "wr-summary-row",
                                                span { class: "wr-summary-label",
                                                    "Opening ({from_display})"
                                                }
                                                span { class: "wr-summary-value",
                                                    "{format_fiat_decimal(opening, user_currency, number_format)}"
                                                }
                                            }
                                        }
                                        if let Some(closing) = totals.closing {
                                            div { class: "wr-summary-row",
                                                span { class: "wr-summary-label",
                                                    "Closing ({to_display})"
                                                }
                                                span { class: "wr-summary-value",
                                                    "{format_fiat_decimal(closing, user_currency, number_format)}"
                                                }
                                            }
                                        }
                                        if let Some(change) = totals.change {
                                            div { class: "wr-summary-row wr-summary-change-row",
                                                span { class: "wr-summary-label wr-summary-change-label",
                                                    "Total Change"
                                                }
                                                span { class: "wr-summary-value wr-change {change_class(change)}",
                                                    "{format_fiat_decimal(change, user_currency, number_format)}"
                                                    if let Some(percent) = totals.change_percent {
                                                        " ({format_change_percent(percent)})"
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
            }

            if show_add_xpub() {
                AddXpubFlow {
                    default_wallet_id: Some(wallet_id),
                    on_complete: move |_| {
                        show_add_xpub.set(false);
                        report_resource.restart();
                    },
                    on_cancel: move |_| show_add_xpub.set(false),
                }
            }

            if show_add_bitcoin() {
                AddBitcoinAddressFlow {
                    default_wallet_id: Some(wallet_id),
                    on_complete: move |_| {
                        show_add_bitcoin.set(false);
                        report_resource.restart();
                    },
                    on_cancel: move |_| show_add_bitcoin.set(false),
                }
            }

            if show_add_ethereum() {
                AddEthereumAddressFlow {
                    default_wallet_id: Some(wallet_id),
                    on_complete: move |_| {
                        show_add_ethereum.set(false);
                        report_resource.restart();
                    },
                    on_cancel: move |_| show_add_ethereum.set(false),
                }
            }

            if show_add_manual_asset() {
                AddManualAssetFlow {
                    default_wallet_id: Some(wallet_id),
                    on_complete: move |account_id| {
                        show_add_manual_asset.set(false);
                        report_resource.restart();
                        navigator.push(route_for_added_manual_asset(account_id));
                    },
                    on_cancel: move |_| show_add_manual_asset.set(false),
                }
            }
        }
    }
}

// ── Sub-components ──────────────────────────────────────────

#[component]
fn ReportValueCell(
    fiat_value: Option<Decimal>,
    balance_state: WalletReportBalanceStateView,
    unit_code: String,
    currency: CurrencyCode,
    number_format: NumberFormat,
    price: Option<Decimal>,
) -> Element {
    let crypto_display = format_crypto_balance(&balance_state, &unit_code);
    let show_fiat = should_show_fiat_value(&balance_state);

    rsx! {
        div { class: "wr-value-cell",
            if show_fiat {
                if let Some(fiat_val) = fiat_value {
                    div { class: "wr-fiat-value",
                        "{format_fiat_decimal(fiat_val, currency, number_format)}"
                    }
                } else {
                    div { class: "wr-fiat-value wr-fiat-value-missing", "—" }
                }
            }
            div { class: "wr-crypto-value", "{crypto_display}" }
            if let Some(p) = price {
                div { class: "wr-price-display",
                    span { class: "wr-price-display-prefix", "@ {currency.symbol()}" }
                    span { class: "wr-price-display-value", "{p}" }
                }
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::backend::BalanceAmountView;
    use crate::report_dates::LocalReportDateRange;
    use crate::wallets::WalletAccountId;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn route_for_wallet_report_uses_start_and_end_query_fields() {
        let wallet_id = WalletId::new();
        let selection = DateRangeSelection::Range(
            LocalReportDateRange::new(
                NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
                NaiveDate::from_ymd_opt(2026, 3, 31).expect("valid date"),
            )
            .expect("valid range"),
        );
        let route = route_for_wallet_report(wallet_id, selection);

        assert!(matches!(
            route,
            Route::WalletReport {
                wallet_id: actual_wallet_id,
                start: Some(start),
                end: Some(end),
            } if actual_wallet_id == wallet_id && start == "2026-01-01" && end == "2026-03-31"
        ));
    }

    #[test]
    fn report_access_badge_text_only_when_clamped() {
        let access = crate::report_access::ReportAccessView {
            requested_from: date(2026, 1, 1),
            requested_to: date(2026, 12, 31),
            effective_from: date(2026, 3, 15),
            effective_to: date(2026, 12, 15),
            gate: crate::report_access::ReportAccessGate::RollingNineMonthWindow,
            range_clamped: true,
            can_edit_prices: false,
        };

        assert_eq!(
            free_window_badge_label(&access),
            Some("Free window applied")
        );
    }

    #[test]
    fn report_access_badge_hidden_when_not_clamped() {
        let access = crate::report_access::ReportAccessView {
            requested_from: date(2026, 5, 1),
            requested_to: date(2026, 6, 1),
            effective_from: date(2026, 5, 1),
            effective_to: date(2026, 6, 1),
            gate: crate::report_access::ReportAccessGate::RollingNineMonthWindow,
            range_clamped: false,
            can_edit_prices: false,
        };

        assert_eq!(free_window_badge_label(&access), None);
    }

    #[test]
    fn free_report_notice_shown_for_rolling_window_gate() {
        let access = crate::report_access::ReportAccessView {
            requested_from: date(2026, 1, 1),
            requested_to: date(2026, 6, 1),
            effective_from: date(2026, 1, 1),
            effective_to: date(2026, 6, 1),
            gate: crate::report_access::ReportAccessGate::RollingNineMonthWindow,
            range_clamped: false,
            can_edit_prices: false,
        };

        assert!(show_free_report_notice(&access));
    }

    #[test]
    fn free_report_notice_hidden_for_full_gate() {
        let access = crate::report_access::ReportAccessView {
            requested_from: date(2026, 1, 1),
            requested_to: date(2026, 6, 1),
            effective_from: date(2026, 1, 1),
            effective_to: date(2026, 6, 1),
            gate: crate::report_access::ReportAccessGate::Full,
            range_clamped: false,
            can_edit_prices: true,
        };

        assert!(!show_free_report_notice(&access));
    }

    #[test]
    fn report_access_badge_aria_label_describes_upgrade_destination() {
        assert_eq!(
            free_window_aria_label(),
            "Upgrade to view longer Holdings Report ranges"
        );
    }

    #[test]
    fn report_price_boundary_times_use_effective_report_dates() {
        let report = WalletReportResponse {
            wallet_label: "Test wallet".to_string(),
            resolved_from: date(2026, 3, 15),
            resolved_to: date(2026, 7, 3),
            default_this_year_from: date(2026, 1, 1),
            default_this_year_to: date(2026, 7, 3),
            access: crate::report_access::ReportAccessView {
                requested_from: date(2026, 1, 1),
                requested_to: date(2026, 7, 3),
                effective_from: date(2026, 3, 15),
                effective_to: date(2026, 7, 3),
                gate: crate::report_access::ReportAccessGate::RollingNineMonthWindow,
                range_clamped: true,
                can_edit_prices: true,
            },
            accounts: Vec::new(),
        };

        assert_eq!(
            report_price_boundary_times(&report),
            (
                "2026-03-15T00:00:00".to_string(),
                "2026-07-03T23:59:59".to_string()
            )
        );
    }

    // ── Fiat Computation Tests ────────────────────────────────

    fn make_balance(raw: &str, formatted: &str) -> BalanceAmountView {
        BalanceAmountView {
            raw_value: raw.to_string(),
            formatted_value: formatted.to_string(),
        }
    }

    fn needs_price_state(raw: &str, formatted: &str) -> WalletReportBalanceStateView {
        WalletReportBalanceStateView::NeedsPrice(make_balance(raw, formatted))
    }

    fn dec(value: &str) -> Decimal {
        value.parse().expect("valid decimal")
    }

    fn balance_from_state(
        balance_state: &WalletReportBalanceStateView,
    ) -> Option<BalanceAmountView> {
        report_balance_view(balance_state).cloned()
    }

    fn make_row(
        opening: WalletReportBalanceStateView,
        closing: WalletReportBalanceStateView,
    ) -> WalletReportAccountRow {
        make_row_for_subject(
            Some(CatalogAssetKey::try_new("bitcoin").expect("valid key")),
            "BTC",
            opening,
            closing,
        )
    }

    fn make_row_for_subject(
        catalog_key: Option<CatalogAssetKey>,
        unit_code: &str,
        opening: WalletReportBalanceStateView,
        closing: WalletReportBalanceStateView,
    ) -> WalletReportAccountRow {
        WalletReportAccountRow {
            account_id: WalletAccountId::new(),
            account_label: "Test".to_string(),
            catalog_asset_key: catalog_key,
            asset_display_name: None,
            unit_code: unit_code.to_string(),
            symbol: None,
            bitcoin_history_coverage: None,
            opening_balance_state: opening.clone(),
            opening_balance: balance_from_state(&opening),
            opening_balance_date: None,
            closing_balance_state: closing.clone(),
            closing_balance: balance_from_state(&closing),
            closing_balance_date: None,
        }
    }

    #[test]
    fn balance_needs_price_returns_false_for_canonical_zero() {
        assert!(!balance_needs_price(
            &WalletReportBalanceStateView::CanonicalZero
        ));
    }

    #[test]
    fn balance_needs_price_returns_false_for_unknown() {
        assert!(!balance_needs_price(&WalletReportBalanceStateView::Unknown));
    }

    #[test]
    fn balance_needs_price_returns_true_for_nonzero() {
        let balance = needs_price_state("150000000", "1.5");
        assert!(balance_needs_price(&balance));
    }

    #[test]
    fn should_show_fiat_value_returns_true_for_canonical_zero() {
        assert!(should_show_fiat_value(
            &WalletReportBalanceStateView::CanonicalZero
        ));
    }

    #[test]
    fn should_show_fiat_value_returns_false_for_unknown() {
        assert!(!should_show_fiat_value(
            &WalletReportBalanceStateView::Unknown
        ));
    }

    #[test]
    fn compute_account_fiat_zero_balances_no_prices_needed() {
        let row = make_row(
            WalletReportBalanceStateView::CanonicalZero,
            WalletReportBalanceStateView::CanonicalZero,
        );
        let fiat = compute_account_fiat(&row, None, None);
        assert_eq!(fiat.opening_fiat, Some(Decimal::ZERO));
        assert_eq!(fiat.closing_fiat, Some(Decimal::ZERO));
        assert_eq!(fiat.change, Some(Decimal::ZERO));
        assert!(!fiat.opening_needs_price);
        assert!(!fiat.closing_needs_price);
    }

    #[test]
    fn compute_account_fiat_nonzero_without_price_returns_none() {
        let row = make_row(
            needs_price_state("100000000", "1"),
            needs_price_state("200000000", "2"),
        );
        let fiat = compute_account_fiat(&row, None, None);
        assert_eq!(fiat.opening_fiat, None);
        assert_eq!(fiat.closing_fiat, None);
        assert_eq!(fiat.change, None);
        assert!(fiat.opening_needs_price);
        assert!(fiat.closing_needs_price);
    }

    fn insert_price(
        prices: &mut ResolvedPricesMap,
        subject: PriceSubject,
        boundary: BoundaryKind,
        value: &str,
    ) {
        prices.insert((subject, boundary), Some(dec(value)));
    }

    #[test]
    fn account_fiat_uses_subject_level_price() {
        let row = make_row(
            needs_price_state("100000000", "1"),
            needs_price_state("200000000", "2"),
        );
        let mut prices = ResolvedPricesMap::new();
        insert_price(
            &mut prices,
            PriceSubject::CatalogAsset(CatalogAssetKey::try_new("bitcoin").expect("valid key")),
            BoundaryKind::Opening,
            "50000",
        );
        insert_price(
            &mut prices,
            PriceSubject::CatalogAsset(CatalogAssetKey::try_new("bitcoin").expect("valid key")),
            BoundaryKind::Closing,
            "60000",
        );

        let fiat = compute_account_fiat(
            &row,
            resolved_price_for_row(&prices, &row, BoundaryKind::Opening),
            resolved_price_for_row(&prices, &row, BoundaryKind::Closing),
        );

        assert_eq!(fiat.opening_fiat, Some(dec("50000")));
        assert_eq!(fiat.closing_fiat, Some(dec("120000")));
        assert_eq!(fiat.change, Some(dec("70000")));
    }

    #[test]
    fn account_fiat_missing_price_renders_none_for_needed_price() {
        let row = make_row(
            needs_price_state("100000000", "1"),
            needs_price_state("200000000", "2"),
        );
        let mut prices = ResolvedPricesMap::new();
        prices.insert(
            (
                PriceSubject::CatalogAsset(CatalogAssetKey::try_new("bitcoin").expect("valid key")),
                BoundaryKind::Opening,
            ),
            None,
        );

        let fiat = compute_account_fiat(
            &row,
            resolved_price_for_row(&prices, &row, BoundaryKind::Opening),
            resolved_price_for_row(&prices, &row, BoundaryKind::Closing),
        );

        assert_eq!(fiat.opening_fiat, None);
        assert_eq!(fiat.closing_fiat, None);
        assert_eq!(fiat.change, None);
    }

    #[test]
    fn price_requirements_skip_zero_balance_boundaries() {
        let report = WalletReportResponse {
            wallet_label: "Test wallet".to_string(),
            resolved_from: date(2026, 1, 1),
            resolved_to: date(2026, 6, 16),
            default_this_year_from: date(2026, 1, 1),
            default_this_year_to: date(2026, 12, 31),
            access: crate::report_access::ReportAccessView {
                requested_from: date(2026, 1, 1),
                requested_to: date(2026, 12, 31),
                effective_from: date(2026, 1, 1),
                effective_to: date(2026, 12, 31),
                gate: crate::report_access::ReportAccessGate::Full,
                range_clamped: false,
                can_edit_prices: true,
            },
            accounts: vec![
                make_row_for_subject(
                    Some(CatalogAssetKey::try_new("ethereum").expect("valid key")),
                    "ETH",
                    WalletReportBalanceStateView::CanonicalZero,
                    needs_price_state("2441190093160", "0.00000244119009316"),
                ),
                make_row_for_subject(
                    Some(CatalogAssetKey::try_new("solana").expect("valid key")),
                    "SOL",
                    WalletReportBalanceStateView::CanonicalZero,
                    needs_price_state("123123456789", "123.123456789"),
                ),
            ],
        };

        assert_eq!(
            price_requirements_for_report(&report),
            vec![
                (
                    PriceSubject::CatalogAsset(
                        CatalogAssetKey::try_new("ethereum").expect("valid key")
                    ),
                    BoundaryKind::Closing,
                ),
                (
                    PriceSubject::CatalogAsset(
                        CatalogAssetKey::try_new("solana").expect("valid key")
                    ),
                    BoundaryKind::Closing,
                ),
            ]
        );
    }

    #[test]
    fn manual_report_row_uses_catalog_asset_price_subject() {
        let row = make_row_for_subject(
            Some(CatalogAssetKey::try_new("cardano").expect("valid key")),
            "ADA",
            needs_price_state("1500000", "1.5"),
            needs_price_state("2000000", "2"),
        );

        assert_eq!(
            price_subject_for_report_row(&row),
            Some(PriceSubject::CatalogAsset(
                CatalogAssetKey::try_new("cardano").expect("valid key")
            ))
        );
    }

    #[test]
    fn compute_account_fiat_with_both_prices() {
        let row = make_row(
            needs_price_state("100000000", "1"),
            needs_price_state("200000000", "2"),
        );
        let fiat = compute_account_fiat(&row, Some(dec("50000")), Some(dec("60000")));
        assert_eq!(fiat.opening_fiat, Some(dec("50000")));
        assert_eq!(fiat.closing_fiat, Some(dec("120000")));
        assert_eq!(fiat.change, Some(dec("70000")));
    }

    #[test]
    fn compute_account_fiat_partial_opening_only() {
        let row = make_row(
            needs_price_state("100000000", "1"),
            needs_price_state("200000000", "2"),
        );
        let fiat = compute_account_fiat(&row, Some(dec("50000")), None);
        assert_eq!(fiat.opening_fiat, Some(dec("50000")));
        assert_eq!(fiat.closing_fiat, None);
        assert_eq!(fiat.change, None);
    }

    #[test]
    fn compute_account_fiat_missing_balance_stays_unknown() {
        let row = make_row(
            WalletReportBalanceStateView::Unknown,
            needs_price_state("100000000", "1"),
        );
        let fiat = compute_account_fiat(&row, None, Some(dec("50000")));
        assert_eq!(fiat.opening_fiat, None);
        assert_eq!(fiat.closing_fiat, Some(dec("50000")));
        assert_eq!(fiat.change, None);
    }

    #[test]
    fn compute_fiat_value_returns_none_for_unknown_state() {
        assert_eq!(
            compute_fiat_value(&WalletReportBalanceStateView::Unknown, Some(dec("50000"))),
            None
        );
    }

    #[test]
    fn compute_fiat_value_returns_zero_for_canonical_zero_state() {
        assert_eq!(
            compute_fiat_value(&WalletReportBalanceStateView::CanonicalZero, None),
            Some(Decimal::ZERO)
        );
    }

    #[test]
    fn compute_fiat_value_uses_formatted_value_not_raw() {
        // raw_value is satoshis (100 million = 1 BTC), formatted_value is the decimal amount
        // This test verifies we use formatted_value for conversion, not raw_value
        let balance = needs_price_state("150000000", "1.5");
        let fiat = compute_fiat_value(&balance, Some(dec("50000")));
        // 1.5 BTC * $50000 = $75000, NOT 150000000 * $50000 (which would be wrong)
        assert_eq!(fiat, Some(dec("75000")));
    }

    #[test]
    fn compute_fiat_value_eth_uses_formatted_value() {
        // raw_value is wei, formatted_value is the decimal ETH amount
        let balance = needs_price_state("2000000000000000000", "2");
        let fiat = compute_fiat_value(&balance, Some(dec("3000")));
        // 2 ETH * $3000 = $6000, NOT 2000000000000000000 * $3000
        assert_eq!(fiat, Some(dec("6000")));
    }

    #[test]
    fn wallet_report_unknown_renders_not_available_without_fiat() {
        assert_eq!(
            format_crypto_balance(&WalletReportBalanceStateView::Unknown, "BTC"),
            "Not available"
        );
        assert!(!should_show_fiat_value(
            &WalletReportBalanceStateView::Unknown
        ));
    }

    #[test]
    fn format_crypto_balance_shows_needs_price_amount() {
        assert_eq!(
            format_crypto_balance(&needs_price_state("150000000", "1.5"), "BTC"),
            "1.5 BTC"
        );
    }

    #[test]
    fn compute_account_fiat_calculates_percentage_change() {
        let row = make_row(
            needs_price_state("100000000", "1"), // 1 BTC opening
            needs_price_state("200000000", "2"), // 2 BTC closing
        );
        let fiat = compute_account_fiat(&row, Some(dec("50000")), Some(dec("60000")));
        assert_eq!(fiat.opening_fiat, Some(dec("50000")));
        assert_eq!(fiat.closing_fiat, Some(dec("120000")));
        assert_eq!(fiat.change, Some(dec("70000")));
        // 70000 / 50000 * 100 = 140%
        assert_eq!(fiat.change_percent, Some(dec("140")));
    }

    #[test]
    fn compute_account_fiat_percentage_change_zero_opening() {
        let row = make_row(
            WalletReportBalanceStateView::CanonicalZero,
            needs_price_state("100000000", "1"), // 1 BTC closing
        );
        let fiat = compute_account_fiat(&row, Some(dec("50000")), Some(dec("60000")));
        assert_eq!(fiat.opening_fiat, Some(Decimal::ZERO));
        assert_eq!(fiat.closing_fiat, Some(dec("60000")));
        assert_eq!(fiat.change, Some(dec("60000")));
        // Cannot calculate percentage when opening is 0
        assert_eq!(fiat.change_percent, None);
    }

    // ── Report Totals Tests ──────────────────────────────────

    #[test]
    fn format_change_percent_positive() {
        assert_eq!(format_change_percent(dec("50")), "+50.00%");
    }

    #[test]
    fn format_change_percent_negative() {
        assert_eq!(format_change_percent(dec("-25.5")), "-25.50%");
    }

    #[test]
    fn format_change_percent_zero() {
        assert_eq!(format_change_percent(Decimal::ZERO), "+0.00%");
    }

    #[test]
    fn totals_all_prices_present() {
        let fiats = vec![
            AccountFiat {
                opening_fiat: Some(dec("100")),
                closing_fiat: Some(dec("150")),
                change: Some(dec("50")),
                change_percent: Some(dec("50")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
            AccountFiat {
                opening_fiat: Some(dec("200")),
                closing_fiat: Some(dec("180")),
                change: Some(dec("-20")),
                change_percent: Some(dec("-10")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
        ];
        let totals = compute_report_totals(&fiats);
        assert_eq!(totals.opening, Some(dec("300")));
        assert_eq!(totals.closing, Some(dec("330")));
        assert_eq!(totals.change, Some(dec("30")));
        assert_eq!(totals.change_percent, Some(dec("10")));
    }

    #[test]
    fn totals_missing_one_opening_price() {
        let fiats = vec![
            AccountFiat {
                opening_fiat: Some(dec("100")),
                closing_fiat: Some(dec("150")),
                change: Some(dec("50")),
                change_percent: Some(dec("50")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
            AccountFiat {
                opening_fiat: None,
                closing_fiat: Some(dec("180")),
                change: None,
                change_percent: None,
                opening_needs_price: true,
                closing_needs_price: true,
            },
        ];
        let totals = compute_report_totals(&fiats);
        assert_eq!(totals.opening, None);
        assert_eq!(totals.closing, Some(dec("330")));
        assert_eq!(totals.change, None);
    }

    #[test]
    fn totals_zero_balance_rows_dont_block() {
        let fiats = vec![
            AccountFiat {
                opening_fiat: Some(dec("100")),
                closing_fiat: Some(dec("150")),
                change: Some(dec("50")),
                change_percent: Some(dec("50")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
            AccountFiat {
                opening_fiat: Some(Decimal::ZERO),
                closing_fiat: Some(Decimal::ZERO),
                change: Some(Decimal::ZERO),
                change_percent: None,
                opening_needs_price: false,
                closing_needs_price: false,
            },
        ];
        let totals = compute_report_totals(&fiats);
        assert_eq!(totals.opening, Some(dec("100")));
        assert_eq!(totals.closing, Some(dec("150")));
        assert_eq!(totals.change, Some(dec("50")));
    }

    #[test]
    fn totals_empty_accounts() {
        let totals = compute_report_totals(&[]);
        assert_eq!(totals.opening, Some(Decimal::ZERO));
        assert_eq!(totals.closing, Some(Decimal::ZERO));
        assert_eq!(totals.change, Some(Decimal::ZERO));
    }

    #[test]
    fn report_totals_use_decimal_values() {
        let fiats = vec![
            AccountFiat {
                opening_fiat: Some(dec("0.10")),
                closing_fiat: Some(dec("0.20")),
                change: Some(dec("0.10")),
                change_percent: Some(dec("100")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
            AccountFiat {
                opening_fiat: Some(dec("0.30")),
                closing_fiat: Some(dec("0.60")),
                change: Some(dec("0.30")),
                change_percent: Some(dec("100")),
                opening_needs_price: true,
                closing_needs_price: true,
            },
        ];

        let totals = compute_report_totals(&fiats);

        assert_eq!(totals.opening, Some(dec("0.40")));
        assert_eq!(totals.closing, Some(dec("0.80")));
        assert_eq!(totals.change, Some(dec("0.40")));
        assert_eq!(totals.change_percent, Some(dec("100")));
    }

    // ── Format Fiat Tests ────────────────────────────────────

    #[test]
    fn format_fiat_positive_value() {
        let result = format_fiat(
            50000.0,
            CurrencyCode::from_code("EUR").unwrap(),
            NumberFormat::DotComma,
        );
        assert_eq!(result, "€50,000");
    }

    #[test]
    fn format_fiat_negative_value() {
        let result = format_fiat(
            -1234.56,
            CurrencyCode::from_code("USD").unwrap(),
            NumberFormat::DotComma,
        );
        assert_eq!(result, "-$1,234.56");
    }

    #[test]
    fn format_fiat_zero_value() {
        let result = format_fiat(
            0.0,
            CurrencyCode::from_code("EUR").unwrap(),
            NumberFormat::DotComma,
        );
        assert_eq!(result, "€0");
    }

    // ── Change Class Tests ───────────────────────────────────

    #[test]
    fn change_class_positive() {
        assert_eq!(change_class(dec("100")), "wr-change-positive");
    }

    #[test]
    fn change_class_negative() {
        assert_eq!(change_class(dec("-50")), "wr-change-negative");
    }

    #[test]
    fn change_class_zero() {
        assert_eq!(change_class(Decimal::ZERO), "");
    }
}

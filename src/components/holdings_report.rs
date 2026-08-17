use super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterEvent, DateRangeFilterPolicy, DateRangeFilterState,
    DateRangeRouteParams, DateRangeSelection, initialize_date_range_filter,
    transition_date_range_filter,
};
use super::date_range_toolbar::DateRangeToolbar;
use super::report_common::{
    HoldingsReportFreeNotice, fiat_decimal, format_change_percent, format_fiat_view,
    free_window_aria_label, free_window_badge_label, free_window_tooltip, show_free_report_notice,
};
use super::wallet_report_prices::PricesSection;
use crate::backend::{
    HoldingsReportResponse, ResolvedPriceView, get_holdings_report,
    list_resolved_prices_for_holdings_report,
};
use crate::models::{CurrencyCode, NumberFormat};
use crate::report_dates::{dial_year_range, displayed_calendar_year};
use crate::settings::SettingsState;
use crate::wallets::{ReportTimezoneParam, WalletReportDateRange};
use crate::{AuthState, AuthStatus, BannerMessage, BannerState, Route};
use chrono::Datelike;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use rust_decimal::Decimal;

const WALLETS_CSS: Asset = asset!("/assets/wallets.css");
const WALLET_REPORT_PRICES_CSS: Asset = asset!("/assets/wallet_report_prices.css");

#[derive(Debug, Clone, PartialEq)]
struct HoldingsTotals {
    opening: Option<Decimal>,
    closing: Option<Decimal>,
    change: Option<Decimal>,
    change_percent: Option<Decimal>,
}

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
    tracing::debug!(user_id = ?user_id, context, "holdings report ui: session expired");
    auth_state.set(AuthStatus::Unauthenticated);
    if user_id.is_some() {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

fn route_for_holdings_report(selection: DateRangeSelection) -> Route {
    Route::HoldingsReport {
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

fn sum_fiat_amount_views(values: Vec<Option<crate::backend::FiatAmountView>>) -> Option<Decimal> {
    values
        .into_iter()
        .map(|value| value.and_then(|amount| fiat_decimal(&amount)))
        .collect::<Option<Vec<_>>>()
        .map(|values| values.into_iter().sum())
}

fn compute_holdings_totals(report: &HoldingsReportResponse) -> HoldingsTotals {
    let opening = sum_fiat_amount_views(
        report
            .wallets
            .iter()
            .map(|row| row.opening_fiat.clone())
            .collect(),
    );
    let closing = sum_fiat_amount_views(
        report
            .wallets
            .iter()
            .map(|row| row.closing_fiat.clone())
            .collect(),
    );

    let (change, change_percent) = match (opening, closing) {
        (Some(o), Some(c)) if o != Decimal::ZERO => {
            let change = c - o;
            (Some(change), Some(change / o * Decimal::from(100)))
        }
        (Some(o), Some(c)) => (Some(c - o), None),
        _ => (None, None),
    };

    HoldingsTotals {
        opening,
        closing,
        change,
        change_percent,
    }
}

fn format_decimal_fiat(
    value: Decimal,
    currency: CurrencyCode,
    number_format: NumberFormat,
) -> String {
    let fiat_value = crate::backend::FiatAmountView {
        raw_value: value.to_string(),
        formatted_value: value.to_string(),
    };
    format_fiat_view(&fiat_value, currency, number_format)
}

fn format_optional_fiat(
    value: &Option<crate::backend::FiatAmountView>,
    currency: CurrencyCode,
    number_format: NumberFormat,
) -> String {
    value
        .as_ref()
        .map(|amount| format_fiat_view(amount, currency, number_format))
        .unwrap_or_else(|| "—".to_string())
}

fn format_optional_percent(value: &Option<String>) -> Option<String> {
    value.as_ref().map(|percent| {
        percent
            .parse::<Decimal>()
            .map(format_change_percent)
            .unwrap_or_else(|_| percent.clone())
    })
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

fn change_class_for_fiat(value: &Option<crate::backend::FiatAmountView>) -> &'static str {
    value
        .as_ref()
        .and_then(fiat_decimal)
        .map(change_class)
        .unwrap_or("")
}

#[component]
pub(crate) fn HoldingsReport(start: Option<String>, end: Option<String>) -> Element {
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
            navigator.replace(route_for_holdings_report(selection));
            pending_route_selection.set(None);
        }
    });

    let mut report_resource = use_server_future(move || {
        let timezone = ReportTimezoneParam((settings_state.timezone)());
        let selection = active_selection();
        async move {
            get_holdings_report(selection.start_param(), selection.end_param(), timezone).await
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
                    list_resolved_prices_for_holdings_report(from, to, timezone).await
                }
                _ => Ok(Vec::new()),
            }
        }
    })?;

    let report_result = report_resource();
    let resolved_views: Vec<ResolvedPriceView> = resolved_prices_resource()
        .and_then(|result| result.ok())
        .unwrap_or_default();

    if let Some(Err(err)) = report_result.as_ref() {
        if err.is_unauthorized() && !*unauthorized_handled.peek() {
            unauthorized_handled.set(true);
            handle_session_expired(auth_state, banner_state, "holdings report");
        }
    } else if *unauthorized_handled.peek() {
        unauthorized_handled.set(false);
    }

    let report: Option<&HoldingsReportResponse> = report_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());

    if let Some(report) = report {
        let canonical_range =
            WalletReportDateRange::new(report.resolved_from, report.resolved_to).ok();
        if let Some(canonical_range) = canonical_range {
            if report.access.range_clamped {
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
    let filter_state_snapshot = filter_state();
    let start_input_value = filter_state_snapshot.start_input_value().to_string();
    let end_input_value = filter_state_snapshot.end_input_value().to_string();
    let validation_message = filter_state_snapshot
        .validation_message()
        .map(str::to_string);
    let user_timezone = (settings_state.timezone)();

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
                        "Holdings Report"
                        if let Some(report) = report {
                            if let Some(label) = free_window_badge_label(&report.access) {
                                Link {
                                    class: "wr-free-window-badge",
                                    title: free_window_tooltip(&report.access).unwrap_or(""),
                                    "aria-label": free_window_aria_label(),
                                    to: Route::Payments,
                                    "{label}"
                                }
                            }
                        }
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
                        start_input_id: "holdings-report-start".to_string(),
                        start_input_value,
                        end_input_id: "holdings-report-end".to_string(),
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
                    div { class: "error-block", p { "{err}" } }
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
                    let totals = compute_holdings_totals(report);
                    let from_display = report.resolved_from.to_string();
                    let to_display = report.resolved_to.to_string();
                    let resolved_views_for_section = resolved_views.clone();

                    rsx! {
                        if !report.price_requirements.is_empty() {
                            PricesSection {
                                user_currency,
                                price_requirements: report.price_requirements.clone(),
                                subject_labels: report.subject_labels.clone(),
                                opening_time_local: format!("{}T00:00:00", report.resolved_from),
                                closing_time_local: format!("{}T23:59:59", report.resolved_to),
                                user_timezone,
                                resolved_views: resolved_views_for_section,
                                can_edit_prices: report.access.can_edit_prices,
                                on_prices_changed: move |_| {
                                    resolved_prices_resource.restart();
                                    report_resource.restart();
                                },
                            }
                        }

                        if report.wallets.is_empty() {
                            div { class: "empty-state",
                                p { class: "empty-state-heading", "No wallets yet." }
                                p { class: "empty-state-description", "Add a Bitcoin xpub or an Ethereum address to start tracking holdings." }
                            }
                        } else {
                            div { class: "card wr-desktop-view",
                                table { class: "wr-table",
                                    thead {
                                        tr {
                                            th { class: "wr-th-account", "Wallet" }
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
                                        for row in &report.wallets {
                                            tr { class: "wr-row",
                                                td { class: "wr-td-account",
                                                    div { class: "wr-account-info",
                                                        Link {
                                                            class: "wr-account-label-link",
                                                            to: Route::WalletReport {
                                                                wallet_id: row.wallet_id,
                                                                start: active_selection().start_query_value(),
                                                                end: active_selection().end_query_value(),
                                                            },
                                                            "{row.wallet_label}"
                                                        }
                                                    }
                                                }
                                                td { class: "wr-td-value",
                                                    div { class: "wr-value-cell",
                                                        div { class: "wr-fiat-value",
                                                            "{format_optional_fiat(&row.opening_fiat, user_currency, number_format)}"
                                                        }
                                                    }
                                                }
                                                td { class: "wr-td-value",
                                                    div { class: "wr-value-cell",
                                                        div { class: "wr-fiat-value",
                                                            "{format_optional_fiat(&row.closing_fiat, user_currency, number_format)}"
                                                        }
                                                    }
                                                }
                                                td { class: "wr-td-change",
                                                    if let Some(change) = &row.change_fiat {
                                                        div { class: "wr-change {change_class_for_fiat(&row.change_fiat)}",
                                                            "{format_fiat_view(change, user_currency, number_format)}"
                                                        }
                                                        if let Some(percent) = format_optional_percent(&row.change_percent) {
                                                            div { class: "wr-change-percent", "{percent}" }
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
                                                        "{format_decimal_fiat(opening, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                            td { class: "wr-td-value",
                                                if let Some(closing) = totals.closing {
                                                    span { class: "wr-total-value",
                                                        "{format_decimal_fiat(closing, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                            td { class: "wr-td-change",
                                                if let Some(change) = totals.change {
                                                    div { class: "wr-change wr-total-value {change_class(change)}",
                                                        "{format_decimal_fiat(change, user_currency, number_format)}"
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

                            div { class: "wr-mobile-view",
                                for row in &report.wallets {
                                    div { class: "card wr-card",
                                        div { class: "wr-card-header",
                                            div { class: "wr-account-info",
                                                Link {
                                                    class: "wr-account-label-link",
                                                    to: Route::WalletReport {
                                                        wallet_id: row.wallet_id,
                                                        start: active_selection().start_query_value(),
                                                        end: active_selection().end_query_value(),
                                                    },
                                                    span { class: "wr-account-label", "{row.wallet_label}" }
                                                }
                                            }
                                            if let Some(change) = &row.change_fiat {
                                                div { class: "wr-card-change-container",
                                                    span { class: "wr-card-change-badge {change_class_for_fiat(&row.change_fiat)}",
                                                        "{format_fiat_view(change, user_currency, number_format)}"
                                                    }
                                                    if let Some(percent) = format_optional_percent(&row.change_percent) {
                                                        span { class: "wr-card-change-percent {change_class_for_fiat(&row.change_fiat)}",
                                                            "{percent}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        div { class: "wr-card-body",
                                            div { class: "wr-card-side",
                                                div { class: "wr-card-side-header",
                                                    span { class: "wr-card-date-label", "{from_display}" }
                                                    span { class: "wr-card-fiat-value",
                                                        "{format_optional_fiat(&row.opening_fiat, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                            div { class: "wr-card-side",
                                                div { class: "wr-card-side-header",
                                                    span { class: "wr-card-date-label", "{to_display}" }
                                                    span { class: "wr-card-fiat-value",
                                                        "{format_optional_fiat(&row.closing_fiat, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                if totals.opening.is_some() || totals.closing.is_some() {
                                    div { class: "wr-summary-card",
                                        h3 { class: "wr-summary-title", "Total Holdings Summary" }
                                        div { class: "wr-summary-rows",
                                            if let Some(opening) = totals.opening {
                                                div { class: "wr-summary-row",
                                                    span { class: "wr-summary-label", "Opening ({from_display})" }
                                                    span { class: "wr-summary-value",
                                                        "{format_decimal_fiat(opening, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                            if let Some(closing) = totals.closing {
                                                div { class: "wr-summary-row",
                                                    span { class: "wr-summary-label", "Closing ({to_display})" }
                                                    span { class: "wr-summary-value",
                                                        "{format_decimal_fiat(closing, user_currency, number_format)}"
                                                    }
                                                }
                                            }
                                            if let Some(change) = totals.change {
                                                div { class: "wr-summary-row wr-summary-change-row",
                                                    span { class: "wr-summary-label wr-summary-change-label", "Total Change" }
                                                    span { class: "wr-summary-value wr-change {change_class(change)}",
                                                        "{format_decimal_fiat(change, user_currency, number_format)}"
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{FiatAmountView, HoldingsReportWalletRow};
    use crate::report_access::{ReportAccessGate, ReportAccessView};
    use crate::wallets::WalletId;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn fiat(raw: &str) -> FiatAmountView {
        FiatAmountView {
            raw_value: raw.to_string(),
            formatted_value: raw.to_string(),
        }
    }

    fn test_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid test date")
    }

    fn report(wallets: Vec<HoldingsReportWalletRow>) -> HoldingsReportResponse {
        let date = test_date();
        HoldingsReportResponse {
            resolved_from: date,
            resolved_to: date,
            default_this_year_from: date,
            default_this_year_to: date,
            access: ReportAccessView {
                requested_from: date,
                requested_to: date,
                effective_from: date,
                effective_to: date,
                gate: ReportAccessGate::Full,
                range_clamped: false,
                can_edit_prices: true,
            },
            wallets,
            price_requirements: Vec::new(),
            subject_labels: Vec::new(),
        }
    }

    fn wallet_row(
        opening_fiat: Option<FiatAmountView>,
        closing_fiat: Option<FiatAmountView>,
    ) -> HoldingsReportWalletRow {
        HoldingsReportWalletRow {
            wallet_id: WalletId::new(),
            wallet_label: "Test wallet".to_string(),
            opening_fiat,
            closing_fiat,
            change_fiat: None,
            change_percent: None,
        }
    }

    #[test]
    fn holdings_total_requires_all_opening_values() {
        let values = vec![Some(fiat("1")), None];
        assert_eq!(sum_fiat_amount_views(values), None);
    }

    #[test]
    fn holdings_total_sums_complete_values() {
        let values = vec![Some(fiat("1")), Some(fiat("2"))];
        assert_eq!(sum_fiat_amount_views(values), Some(Decimal::from(3)));
    }

    #[test]
    fn holdings_totals_require_complete_boundary_values() {
        let missing_opening = compute_holdings_totals(&report(vec![
            wallet_row(Some(fiat("1")), Some(fiat("10"))),
            wallet_row(None, Some(fiat("20"))),
        ]));
        assert_eq!(missing_opening.opening, None);
        assert_eq!(missing_opening.closing, Some(Decimal::from(30)));
        assert_eq!(missing_opening.change, None);
        assert_eq!(missing_opening.change_percent, None);

        let missing_closing = compute_holdings_totals(&report(vec![
            wallet_row(Some(fiat("10")), Some(fiat("20"))),
            wallet_row(Some(fiat("30")), None),
        ]));
        assert_eq!(missing_closing.opening, Some(Decimal::from(40)));
        assert_eq!(missing_closing.closing, None);
        assert_eq!(missing_closing.change, None);
        assert_eq!(missing_closing.change_percent, None);
    }

    #[test]
    fn holdings_totals_calculate_change_and_percent_for_complete_values() {
        let totals = compute_holdings_totals(&report(vec![
            wallet_row(Some(fiat("10")), Some(fiat("15"))),
            wallet_row(Some(fiat("30")), Some(fiat("45"))),
        ]));

        assert_eq!(totals.opening, Some(Decimal::from(40)));
        assert_eq!(totals.closing, Some(Decimal::from(60)));
        assert_eq!(totals.change, Some(Decimal::from(20)));
        assert_eq!(totals.change_percent, Some(Decimal::from(50)));
    }
}

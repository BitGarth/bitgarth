use super::date_range_filter::{
    DateRangeFilterEffect, DateRangeFilterEvent, DateRangeFilterPolicy, DateRangeFilterState,
    DateRangeSelection, transition_date_range_filter,
};
use crate::Route;
use crate::backend::{BitcoinBalanceObservationView, FiatAmountView};
use crate::components::formatting::{format_current_value_for_display, format_number_for_display};
use crate::models::{CurrencyCode, NumberFormat};
use crate::report_access::{ReportAccessGate, ReportAccessView};
use crate::settings::SettingsState;
use crate::timezone::format_timestamp;
use dioxus::prelude::*;
use rust_decimal::Decimal;

pub(super) fn dispatch_report_date_event(
    mut filter_state: Signal<DateRangeFilterState>,
    mut active_selection: Signal<DateRangeSelection>,
    mut pending_route_selection: Signal<Option<DateRangeSelection>>,
    event: DateRangeFilterEvent,
) {
    let current_state = filter_state.peek().clone();
    let outcome = transition_date_range_filter(
        DateRangeFilterPolicy::RequiredCanonicalRange,
        &current_state,
        event,
    );

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

/// The full Free-preview notice. When the range was clamped, the compact
/// "Free window applied" badge already says the same thing.
pub(crate) fn show_free_report_notice(access: &ReportAccessView) -> bool {
    matches!(access.gate, ReportAccessGate::RollingNineMonthWindow) && !access.range_clamped
}

pub(crate) fn free_window_badge_label(access: &ReportAccessView) -> Option<&'static str> {
    access.range_clamped.then_some("Free window applied")
}

pub(crate) fn free_window_tooltip(access: &ReportAccessView) -> Option<&'static str> {
    access.range_clamped.then_some(
        "Free reports cover the last 9 months. Upgrade to view longer historical ranges.",
    )
}

pub(crate) fn free_window_aria_label() -> &'static str {
    "Upgrade to view longer Holdings Report ranges"
}

#[component]
pub(super) fn LastConfirmedBitcoin(
    observation: BitcoinBalanceObservationView,
    show_account: bool,
) -> Element {
    let settings = use_context::<SettingsState>();
    let timezone = (settings.timezone)().into();
    let date_format = (settings.date_time_format)();
    let amount = format_number_for_display(
        &observation.amount.formatted_value,
        (settings.number_format)(),
    );
    let first = observation.observed_at;
    let first_display = format_timestamp(&first, timezone, date_format);
    rsx! {
        div { class: "wr-last-confirmed",
            if show_account {
                div { "{observation.account_label}" }
            }
            div { "Last confirmed {amount} BTC" }
            if let Some(last) = observation.latest_observed_at {
                div {
                    "Addresses synced between "
                    time { datetime: first.to_rfc3339(), "{first_display}" }
                    " and "
                    time { datetime: last.to_rfc3339(), "{format_timestamp(&last, timezone, date_format)}" }
                }
            } else {
                div {
                    "Synced "
                    time { datetime: first.to_rfc3339(), "{first_display}" }
                }
            }
        }
    }
}

#[component]
pub(crate) fn HoldingsReportFreeNotice() -> Element {
    rsx! {
        div { class: "wr-free-notice",
            div { class: "wr-free-notice-body",
                span { class: "wr-free-notice-title", "Free 9-month preview" }
                span { class: "wr-free-notice-text",
                    "Holdings Reports support reconciliation, accounting, and tax-prep evidence. You always see a rolling nine-month window free. Upgrade for full history."
                }
            }
            Link { class: "wr-free-notice-cta", to: Route::Payments, "Upgrade" }
        }
    }
}

pub(crate) fn fiat_decimal(value: &FiatAmountView) -> Option<Decimal> {
    value.raw_value.parse::<Decimal>().ok()
}

pub(crate) fn format_change_percent(percent: Decimal) -> String {
    if percent >= Decimal::ZERO {
        format!("+{:.2}%", percent)
    } else {
        format!("{:.2}%", percent)
    }
}

pub(crate) fn format_fiat_view(
    value: &FiatAmountView,
    currency: CurrencyCode,
    number_format: NumberFormat,
) -> String {
    // Fixed currency decimals: a total of 47088.70 must never read "47088.7".
    match fiat_decimal(value) {
        Some(amount) => {
            let sign = if amount < Decimal::ZERO { "-" } else { "" };
            let digits = format_current_value_for_display(
                &amount.abs().to_string(),
                currency,
                number_format,
            );
            format!("{sign}{digits}")
        }
        None => value.formatted_value.clone(),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn view(raw: &str) -> FiatAmountView {
        FiatAmountView {
            raw_value: raw.to_string(),
            formatted_value: raw.to_string(),
        }
    }

    #[test]
    fn fiat_view_keeps_currency_decimals_and_sign() {
        let eur = CurrencyCode::from_code("EUR").unwrap();
        assert_eq!(
            format_fiat_view(&view("47088.7"), eur, NumberFormat::DotComma),
            "€47,088.70"
        );
        assert_eq!(
            format_fiat_view(&view("-21.5"), eur, NumberFormat::DotComma),
            "-€21.50"
        );
    }
}

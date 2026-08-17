use crate::Route;
use crate::backend::FiatAmountView;
use crate::components::formatting::{DisplayAmountSign, ManualConversionQuote, convert_amount};
use crate::models::{CurrencyCode, NumberFormat};
use crate::report_access::{ReportAccessGate, ReportAccessView};
use dioxus::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

pub(crate) fn show_free_report_notice(access: &ReportAccessView) -> bool {
    matches!(access.gate, ReportAccessGate::RollingNineMonthWindow)
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
    let parsed = fiat_decimal(value).and_then(|decimal| decimal.to_f64());
    match parsed {
        Some(amount) => {
            let quote = ManualConversionQuote {
                currency,
                price_per_unit: 1.0,
            };
            let sign = if amount < 0.0 {
                DisplayAmountSign::Negative
            } else {
                DisplayAmountSign::Hidden
            };
            convert_amount(&format!("{:.2}", amount.abs()), sign, &quote, number_format)
        }
        None => value.formatted_value.clone(),
    }
}

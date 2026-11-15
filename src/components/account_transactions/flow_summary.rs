use super::super::{AmountDisplayContext, DisplayAmount, DisplayAmountSign};
use super::helpers::{
    format_balance_date, format_balance_reliability_display, format_fiat_amount,
    format_native_balance_state_display,
};
use crate::Route;
use crate::backend::NativeBalanceStateView;
use crate::models::{DateTimeFormat, NumberFormat};
use crate::wallets::GetAccountTransactionsResponse;
use dioxus::prelude::*;

/// The range as one ledger line: opening, money in, money out, fees, closing.
/// Opening is left out when it is zero, so the range starts at the first transaction.
#[component]
pub(super) fn AccountFlowSummary(
    data: GetAccountTransactionsResponse,
    amount_context: AmountDisplayContext,
    number_format: NumberFormat,
    date_format: DateTimeFormat,
) -> Element {
    let balance_display = |state: &NativeBalanceStateView, reliability| {
        format_balance_reliability_display(
            format_native_balance_state_display(
                state,
                data.asset,
                &None,
                &amount_context,
                number_format,
            ),
            reliability,
        )
    };
    let opening_display = balance_display(
        &data.opening_balance_state,
        &data.opening_balance_reliability,
    );
    let closing_display = balance_display(
        &data.closing_balance_state,
        &data.closing_balance_reliability,
    );
    let date_line = |date: &Option<String>| match date {
        Some(value) => format!("on {}", format_balance_date(value, date_format)),
        None => "Awaiting first sync".to_string(),
    };
    let opening_date = date_line(&data.opening_balance_date);
    let closing_date = date_line(&data.closing_balance_date);
    // A zero opening only repeats that the range starts with the account's
    // first transaction; a provisional zero is shown, since it may be wrong.
    let opening_is_zero = match &data.opening_balance_state {
        NativeBalanceStateView::CanonicalZero => true,
        NativeBalanceStateView::Known(amount) => {
            amount.raw_value == "0" && !data.opening_balance_reliability.is_provisional()
        }
        NativeBalanceStateView::Unknown => false,
    };
    let show_opening = !opening_is_zero;
    let opening_unknown = data.opening_balance_state == NativeBalanceStateView::Unknown;

    let coins = |amount: &crate::backend::BalanceAmountView, sign| {
        DisplayAmount::from_balance(amount, &amount_context)
            .with_sign(sign)
            .to_string()
    };
    let flows = data.flow_summary.as_ref().map(|summary| {
        let fiat = |pick: fn(&crate::wallets::AccountFlowFiatView) -> rust_decimal::Decimal,
                    sign| {
            data.fiat_currency
                .zip(summary.fiat.as_ref())
                .map(|(currency, fiat)| {
                    format_fiat_amount(pick(fiat), sign, currency, number_format)
                })
        };
        [
            (
                "Received",
                coins(&summary.received, DisplayAmountSign::Hidden),
                fiat(|fiat| fiat.received, DisplayAmountSign::Hidden),
            ),
            (
                "Sent",
                coins(&summary.sent, DisplayAmountSign::Negative),
                fiat(|fiat| fiat.sent, DisplayAmountSign::Negative),
            ),
            (
                "Fees",
                coins(&summary.fees, DisplayAmountSign::Negative),
                fiat(|fiat| fiat.fees, DisplayAmountSign::Negative),
            ),
        ]
    });
    let unpriced = data
        .flow_summary
        .as_ref()
        .filter(|_| data.fiat_currency.is_some())
        .map_or(0, |summary| summary.unpriced_transactions);

    rsx! {
        dl { class: "tx-flow-summary", "data-testid": "tx-flow-summary",
            if show_opening {
                div { class: "tx-flow-item tx-header-opening-balance",
                    dt { class: "tx-flow-label", "Opening balance" }
                    dd { class: "tx-flow-amount", "{opening_display}" }
                    dd { class: "tx-flow-note",
                        if opening_unknown { "History not synced yet" } else { "{opening_date}" }
                    }
                }
            }
            if let Some(flows) = flows {
                for (label, amount, fiat) in flows {
                    div { class: "tx-flow-item",
                        dt { class: "tx-flow-label", "{label}" }
                        dd { class: "tx-flow-amount", "{amount}" }
                        if let Some(fiat) = fiat {
                            dd { class: "tx-flow-note tx-flow-fiat", "{fiat}" }
                        }
                    }
                }
            }
            div { class: "tx-flow-item tx-flow-closing tx-header-closing-balance",
                dt { class: "tx-flow-label", "Closing balance" }
                dd { class: "tx-flow-amount", "{closing_display}" }
                dd { class: "tx-flow-note", "{closing_date}" }
            }
        }
        if unpriced > 0 {
            p { class: "tx-flow-hint", "data-testid": "tx-flow-unpriced",
                if unpriced == 1 {
                    "1 transaction has no price for its day, so totals in your currency are hidden. "
                } else {
                    "{unpriced} transactions have no price for their day, so totals in your currency are hidden. "
                }
                Link {
                    to: Route::Prices { asset: Some(data.asset.as_str().to_string()), start: None, end: None },
                    "Add prices"
                }
            }
        }
    }
}

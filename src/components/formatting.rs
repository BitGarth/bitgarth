use crate::backend::{AccountBalanceStateView, BalanceAmountView, WalletBalanceView};
use crate::models::{CurrencyCode, DateTimeFormat, NumberFormat};
use chrono::NaiveDate;
use std::fmt::{self, Display};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmountDisplayContext {
    pub(crate) unit_code: String,
    pub(crate) symbol: Option<String>,
    pub(crate) number_format: NumberFormat,
}

impl AmountDisplayContext {
    pub(crate) fn new(
        unit_code: String,
        symbol: Option<String>,
        number_format: NumberFormat,
    ) -> Self {
        Self {
            unit_code,
            symbol,
            number_format,
        }
    }

    pub(crate) fn from_wallet_balance(
        balance: &WalletBalanceView,
        number_format: NumberFormat,
    ) -> Self {
        Self::new(
            balance.unit_code.clone(),
            balance.symbol.clone(),
            number_format,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayAmountSign {
    Hidden,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayAmount {
    canonical_value: String,
    sign: DisplayAmountSign,
    context: AmountDisplayContext,
}

impl DisplayAmount {
    pub(crate) fn new(canonical_value: impl Into<String>, context: &AmountDisplayContext) -> Self {
        Self {
            canonical_value: canonical_value.into(),
            sign: DisplayAmountSign::Hidden,
            context: context.clone(),
        }
    }

    pub(crate) fn from_balance(amount: &BalanceAmountView, context: &AmountDisplayContext) -> Self {
        Self::new(amount.formatted_value.clone(), context)
    }

    pub(crate) fn with_sign(mut self, sign: DisplayAmountSign) -> Self {
        self.sign = sign;
        self
    }

    /// Localized number (carrying sign and any prefix symbol) plus an optional
    /// postfix unit. Symbol-prefixed amounts fold the symbol into the number and
    /// return `None` for the unit, so callers render them as a single token.
    fn render_parts(&self) -> (String, Option<String>) {
        let localized_value =
            format_number_for_display(&self.canonical_value, self.context.number_format);
        let sign_prefix = match self.sign {
            DisplayAmountSign::Hidden => "",
            DisplayAmountSign::Negative => "-",
        };
        match &self.context.symbol {
            Some(symbol) => (format!("{sign_prefix}{symbol}{localized_value}"), None),
            None => (
                format!("{sign_prefix}{localized_value}"),
                Some(self.context.unit_code.clone()),
            ),
        }
    }

    pub(crate) fn into_parts(self) -> (String, Option<String>) {
        self.render_parts()
    }
}

impl Display for DisplayAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.render_parts() {
            (value, Some(unit)) => write!(f, "{value} {unit}"),
            (value, None) => write!(f, "{value}"),
        }
    }
}

pub(crate) fn format_balance_for_asset(
    balance: &WalletBalanceView,
    number_format: NumberFormat,
) -> String {
    let context = AmountDisplayContext::from_wallet_balance(balance, number_format);
    match &balance.balance_state {
        AccountBalanceStateView::Known { amount } => {
            DisplayAmount::from_balance(amount, &context).to_string()
        }
        AccountBalanceStateView::Unknown => "Not available".to_string(),
    }
}

pub(crate) fn format_wallet_balance_parts(
    balance: &WalletBalanceView,
    number_format: NumberFormat,
) -> (String, Option<String>) {
    let context = AmountDisplayContext::new(balance.unit_code.clone(), None, number_format);
    match &balance.balance_state {
        AccountBalanceStateView::Known { amount } => {
            DisplayAmount::from_balance(amount, &context).into_parts()
        }
        AccountBalanceStateView::Unknown => ("Not available".to_string(), None),
    }
}

pub(crate) fn format_date_for_display(date: NaiveDate, format: DateTimeFormat) -> String {
    match format {
        DateTimeFormat::YearMonthDay24 => date.format("%Y-%m-%d").to_string(),
        DateTimeFormat::DayMonthYear24 => date.format("%d/%m/%Y").to_string(),
        DateTimeFormat::MonthDayYear12 => date.format("%b %d, %Y").to_string(),
    }
}

pub(crate) fn format_number_for_display(
    canonical_value: &str,
    number_format: NumberFormat,
) -> String {
    let (whole, fraction) = match canonical_value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (canonical_value, None),
    };

    let whole_is_valid = !whole.is_empty() && whole.chars().all(|ch| ch.is_ascii_digit());
    let fraction_is_valid = fraction
        .map(|value| value.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(true);
    if !whole_is_valid || !fraction_is_valid {
        return canonical_value.to_string();
    }

    let (group_separator, decimal_separator) = match number_format {
        NumberFormat::DotComma => (',', '.'),
        NumberFormat::CommaDot => ('.', ','),
        NumberFormat::CommaSpace => (' ', ','),
    };
    let grouped_whole = group_whole_digits(whole, group_separator);

    match fraction {
        Some(value) if !value.is_empty() => {
            format!("{grouped_whole}{decimal_separator}{value}")
        }
        _ => grouped_whole,
    }
}

// ── Manual Price Conversion ───────────────────────────────
// ── Manual Price Conversion ───────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ManualConversionQuote {
    pub(crate) currency: CurrencyCode,
    pub(crate) price_per_unit: f64,
}

/// Convert a canonical decimal string (e.g. "1.234") using the given quote.
/// Returns the formatted converted amount string with fiat symbol prefix.
/// The result uses the given number format for locale-appropriate display.
pub(crate) fn convert_amount(
    canonical_value: &str,
    sign: DisplayAmountSign,
    quote: &ManualConversionQuote,
    number_format: NumberFormat,
) -> String {
    let native: f64 = match canonical_value.parse() {
        Ok(value) => value,
        Err(_) => return format!("{}{canonical_value}", quote.currency.symbol()),
    };

    let converted = native * quote.price_per_unit;
    let canonical_converted = format_converted_value(converted);
    let localized = format_number_for_display(&canonical_converted, number_format);
    let sign_prefix = match sign {
        DisplayAmountSign::Hidden => "",
        DisplayAmountSign::Negative => "-",
    };
    format!("{sign_prefix}{}{localized}", quote.currency.symbol())
}

pub(crate) fn format_current_value_for_display(
    converted_value: &str,
    currency: CurrencyCode,
    number_format: NumberFormat,
) -> String {
    let canonical = match converted_value.parse::<f64>() {
        Ok(value) if value.is_finite() => format_fiat_value(value, currency.decimal_places()),
        _ => converted_value.to_string(),
    };
    let localized = format_number_for_display(&canonical, number_format);
    format!("{}{localized}", currency.symbol())
}

/// Format a fiat value to a fixed number of decimal places (currency
/// convention), without trimming trailing zeroes. A net-worth total of
/// 52197.20 must read "52197.20", never "52197.2".
fn format_fiat_value(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}")
}

/// Format a converted f64 value to a canonical decimal string.
/// Rounds to 2 decimal places (fiat precision).
fn format_converted_value(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    let formatted = format!("{value:.2}");
    let (whole, fraction) = match formatted.split_once('.') {
        Some((w, f)) => (w, f),
        None => return formatted,
    };

    let trimmed = fraction.trim_end_matches('0');
    if trimmed.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{trimmed}")
    }
}

fn group_whole_digits(whole: &str, separator: char) -> String {
    if whole.len() <= 3 {
        return whole.to_string();
    }

    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (index, ch) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(separator);
        }
        grouped.push(ch);
    }
    grouped
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn display_amount_with_symbol_uses_prefix_without_space() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::DotComma,
        );
        let formatted = DisplayAmount::new("1234.56", &context).to_string();
        assert_eq!(formatted, "₿1,234.56");
    }

    #[test]
    fn display_amount_without_symbol_uses_unit_suffix() {
        let context = AmountDisplayContext::new("BTC".to_string(), None, NumberFormat::CommaDot);
        let formatted = DisplayAmount::new("1234.56", &context).to_string();
        assert_eq!(formatted, "1.234,56 BTC");
    }

    #[test]
    fn display_amount_places_negative_sign_before_symbol() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::DotComma,
        );
        let formatted = DisplayAmount::new("1.234", &context)
            .with_sign(DisplayAmountSign::Negative)
            .to_string();
        assert_eq!(formatted, "-₿1.234");
    }

    #[test]
    fn display_amount_places_negative_sign_before_value_without_symbol() {
        let context = AmountDisplayContext::new("BTC".to_string(), None, NumberFormat::DotComma);
        let formatted = DisplayAmount::new("1.234", &context)
            .with_sign(DisplayAmountSign::Negative)
            .to_string();
        assert_eq!(formatted, "-1.234 BTC");
    }

    // ── Conversion Output Tests ───────────────────────────────

    #[test]
    fn convert_amount_uses_usd_prefix() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("USD").unwrap(),
            price_per_unit: 50000.0,
        };
        let result = convert_amount(
            "1",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "$50,000");
    }

    #[test]
    fn convert_amount_uses_eur_prefix() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("EUR").unwrap(),
            price_per_unit: 45000.0,
        };
        let result = convert_amount(
            "1",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "€45,000");
    }

    #[test]
    fn convert_amount_negative_sign_for_outgoing() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("USD").unwrap(),
            price_per_unit: 50000.0,
        };
        let result = convert_amount(
            "0.5",
            DisplayAmountSign::Negative,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "-$25,000");
    }

    #[test]
    fn convert_amount_trims_trailing_zeroes() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("USD").unwrap(),
            price_per_unit: 10.0,
        };
        let result = convert_amount(
            "1.5",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "$15");
    }

    #[test]
    fn convert_amount_rounds_to_two_decimals() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("EUR").unwrap(),
            price_per_unit: 45000.0,
        };
        // 0.23702331 * 45000 = 10666.04895 → rounded to 10666.05
        let result = convert_amount(
            "0.23702331",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "€10,666.05");
    }

    #[test]
    fn convert_amount_zero_produces_zero() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("USD").unwrap(),
            price_per_unit: 50000.0,
        };
        let result = convert_amount(
            "0",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::DotComma,
        );
        assert_eq!(result, "$0");
    }

    #[test]
    fn convert_amount_respects_comma_dot_format() {
        let quote = ManualConversionQuote {
            currency: CurrencyCode::from_code("EUR").unwrap(),
            price_per_unit: 50000.0,
        };
        let result = convert_amount(
            "1.5",
            DisplayAmountSign::Hidden,
            &quote,
            NumberFormat::CommaDot,
        );
        assert_eq!(result, "€75.000");
    }

    #[test]
    fn format_converted_value_rounds_to_two_decimals() {
        let result = format_converted_value(123.456);
        assert_eq!(result, "123.46");
    }

    #[test]
    fn format_converted_value_tiny_amount_rounds_to_zero() {
        let result = format_converted_value(0.001);
        assert_eq!(result, "0");
    }

    #[test]
    fn format_converted_value_trims_trailing_zeroes() {
        let result = format_converted_value(123.40);
        assert_eq!(result, "123.4");
    }

    #[test]
    fn format_converted_value_whole_number() {
        let result = format_converted_value(500.0);
        assert_eq!(result, "500");
    }

    #[test]
    fn format_current_value_for_display_localizes_and_prefixes_currency() {
        let result = format_current_value_for_display(
            "1234.5",
            CurrencyCode::from_code("EUR").unwrap(),
            NumberFormat::CommaDot,
        );
        assert_eq!(result, "€1.234,50");
    }

    #[test]
    fn format_current_value_for_display_keeps_fixed_fiat_decimals() {
        // A net-worth total of 52197.20 must not drop the trailing zero.
        let result = format_current_value_for_display(
            "52197.20",
            CurrencyCode::from_code("EUR").unwrap(),
            NumberFormat::CommaSpace,
        );
        assert_eq!(result, "€52 197,20");
    }

    #[test]
    fn format_current_value_for_display_omits_decimals_for_zero_minor_unit() {
        let result = format_current_value_for_display(
            "200000",
            CurrencyCode::from_code("JPY").unwrap(),
            NumberFormat::CommaDot,
        );
        assert_eq!(result, "¥200.000");
    }

    #[test]
    fn currency_symbol_returns_correct_symbols() {
        assert_eq!(CurrencyCode::from_code("USD").unwrap().symbol(), "$");
        assert_eq!(CurrencyCode::from_code("EUR").unwrap().symbol(), "€");
    }

    #[test]
    fn currency_label_returns_correct_labels() {
        assert_eq!(
            CurrencyCode::from_code("USD").unwrap().label(),
            "USD ($) — US Dollar"
        );
        assert_eq!(
            CurrencyCode::from_code("EUR").unwrap().label(),
            "EUR (€) — Euro"
        );
    }

    // ── into_parts Tests ─────────────────────────────────────

    #[test]
    fn into_parts_splits_number_and_unit_for_postfix() {
        let context = AmountDisplayContext::new("BTC".to_string(), None, NumberFormat::DotComma);
        let (number, unit) = DisplayAmount::new("1234.56", &context).into_parts();
        assert_eq!(number, "1,234.56");
        assert_eq!(unit, Some("BTC".to_string()));
    }

    #[test]
    fn into_parts_folds_symbol_and_returns_no_unit() {
        let context = AmountDisplayContext::new(
            "BTC".to_string(),
            Some("₿".to_string()),
            NumberFormat::DotComma,
        );
        let (number, unit) = DisplayAmount::new("1234.56", &context).into_parts();
        assert_eq!(number, "₿1,234.56");
        assert_eq!(unit, None);
    }

    #[test]
    fn into_parts_keeps_negative_sign_with_unit() {
        let context = AmountDisplayContext::new("BTC".to_string(), None, NumberFormat::CommaSpace);
        let (number, unit) = DisplayAmount::new("1234.5", &context)
            .with_sign(DisplayAmountSign::Negative)
            .into_parts();
        assert_eq!(number, "-1 234,5");
        assert_eq!(unit, Some("BTC".to_string()));
    }

    // ── Date Display Tests ────────────────────────────────────

    #[test]
    fn format_date_for_display_matches_year_month_day_preference() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 22).expect("valid date");
        let formatted = format_date_for_display(date, DateTimeFormat::YearMonthDay24);
        assert_eq!(formatted, "2026-03-22");
    }

    #[test]
    fn format_date_for_display_matches_day_month_year_preference() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 22).expect("valid date");
        let formatted = format_date_for_display(date, DateTimeFormat::DayMonthYear24);
        assert_eq!(formatted, "22/03/2026");
    }

    #[test]
    fn format_date_for_display_matches_month_day_year_preference() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 22).expect("valid date");
        let formatted = format_date_for_display(date, DateTimeFormat::MonthDayYear12);
        assert_eq!(formatted, "Mar 22, 2026");
    }
}

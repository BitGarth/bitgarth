use crate::i18n::Locale;
use crate::models::{
    CurrencyCode, DateTimeFormat, NumberFormat, RawEtherscanBaseUrl, RawMempoolBaseUrl,
    SessionDuration, UserSettings, UserTimezone,
};
use chrono_tz::Tz;
use dioxus::prelude::{Signal, WritableExt};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AppDefaults {
    pub language: Locale,
    pub date_time_format: DateTimeFormat,
    pub number_format: NumberFormat,
    pub currency: CurrencyCode,
    pub timezone: UserTimezone,
    pub session_duration: SessionDuration,
    pub mempool_base_url: Option<RawMempoolBaseUrl>,
    pub etherscan_base_url: Option<RawEtherscanBaseUrl>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedSettings {
    pub language: Locale,
    pub date_time_format: DateTimeFormat,
    pub number_format: NumberFormat,
    pub currency: CurrencyCode,
    pub timezone: UserTimezone,
    pub session_duration: SessionDuration,
    pub mempool_base_url: Option<RawMempoolBaseUrl>,
    pub etherscan_base_url: Option<RawEtherscanBaseUrl>,
    pub price_fetching_enabled: bool,
    pub has_coingecko_api_key: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsState {
    pub language: Signal<Locale>,
    pub date_time_format: Signal<DateTimeFormat>,
    pub number_format: Signal<NumberFormat>,
    pub currency: Signal<CurrencyCode>,
    pub timezone: Signal<UserTimezone>,
    pub session_duration: Signal<SessionDuration>,
    pub mempool_base_url: Signal<Option<RawMempoolBaseUrl>>,
    pub etherscan_base_url: Signal<Option<RawEtherscanBaseUrl>>,
    pub price_fetching_enabled: Signal<bool>,
    pub has_coingecko_api_key: Signal<bool>,
}

impl SettingsState {
    /// Apply a fully resolved settings snapshot to UI state signals.
    pub(crate) fn apply_resolved(&self, resolved: ResolvedSettings) {
        let mut language = self.language;
        let mut date_time_format = self.date_time_format;
        let mut number_format = self.number_format;
        let mut currency = self.currency;
        let mut timezone = self.timezone;
        let mut session_duration = self.session_duration;
        let mut mempool_base_url = self.mempool_base_url;
        let mut etherscan_base_url = self.etherscan_base_url;
        let mut price_fetching_enabled = self.price_fetching_enabled;
        let mut has_coingecko_api_key = self.has_coingecko_api_key;

        language.set(resolved.language);
        date_time_format.set(resolved.date_time_format);
        number_format.set(resolved.number_format);
        currency.set(resolved.currency);
        timezone.set(resolved.timezone);
        session_duration.set(resolved.session_duration);
        mempool_base_url.set(resolved.mempool_base_url);
        etherscan_base_url.set(resolved.etherscan_base_url);
        price_fetching_enabled.set(resolved.price_fetching_enabled);
        has_coingecko_api_key.set(resolved.has_coingecko_api_key);
    }

    /// Resolve optional persisted settings with defaults, then apply to signals.
    pub(crate) fn apply_user_settings_with_defaults(
        &self,
        settings: &UserSettings,
        defaults: &AppDefaults,
    ) {
        self.apply_resolved(resolve_settings(settings, defaults));
    }
}

const COMMON_CURRENCY_CODES: [&str; 8] = ["USD", "EUR", "GBP", "ZAR", "JPY", "CHF", "AUD", "CAD"];

pub(crate) fn common_currencies() -> Vec<CurrencyCode> {
    COMMON_CURRENCY_CODES
        .iter()
        .filter_map(|code| CurrencyCode::from_code(code))
        .collect()
}

pub(crate) fn defaults_for_locale(locale: Locale, timezone: Tz) -> AppDefaults {
    AppDefaults {
        language: locale,
        date_time_format: default_date_time_format(locale),
        number_format: default_number_format(locale),
        currency: default_currency(locale),
        timezone: UserTimezone::from(timezone),
        session_duration: SessionDuration::default(),
        mempool_base_url: None,
        etherscan_base_url: None,
    }
}

pub(crate) fn resolve_settings(
    settings: &UserSettings,
    defaults: &AppDefaults,
) -> ResolvedSettings {
    ResolvedSettings {
        language: settings.language.unwrap_or(defaults.language),
        date_time_format: settings
            .date_time_format
            .unwrap_or(defaults.date_time_format),
        number_format: settings.number_format.unwrap_or(defaults.number_format),
        currency: settings.currency.unwrap_or(defaults.currency),
        timezone: settings.timezone.unwrap_or(defaults.timezone),
        session_duration: settings
            .session_duration
            .unwrap_or(defaults.session_duration),
        mempool_base_url: settings.mempool_base_url.clone(),
        etherscan_base_url: settings.etherscan_base_url.clone(),
        price_fetching_enabled: settings.price_fetching_enabled,
        has_coingecko_api_key: settings.has_coingecko_api_key,
    }
}

pub(crate) fn default_date_time_format(locale: Locale) -> DateTimeFormat {
    let _ = locale;
    DateTimeFormat::MonthDayYear12
}

pub(crate) fn default_number_format(locale: Locale) -> NumberFormat {
    let _ = locale;
    NumberFormat::DotComma
}

pub(crate) fn default_currency(locale: Locale) -> CurrencyCode {
    let _ = locale;
    currency_from_code_or_fallback("USD")
}

fn currency_from_code_or_fallback(code: &str) -> CurrencyCode {
    match CurrencyCode::from_code(code) {
        Some(currency) => currency,
        None => fallback_currency(),
    }
}

fn fallback_currency() -> CurrencyCode {
    for code in COMMON_CURRENCY_CODES.iter() {
        if let Some(currency) = CurrencyCode::from_code(code) {
            return currency;
        }
    }
    eprintln!("No valid ISO 4217 currency codes available for defaults");
    std::process::exit(1);
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_english() {
        let defaults = defaults_for_locale(Locale::default(), Tz::UTC);
        assert_eq!(defaults.language, Locale::English);
        assert_eq!(defaults.date_time_format, DateTimeFormat::MonthDayYear12);
        assert_eq!(defaults.number_format, NumberFormat::DotComma);
        assert_eq!(defaults.currency.code(), "USD");
    }

    #[test]
    fn resolve_settings_uses_defaults_for_missing_values() {
        let defaults = defaults_for_locale(Locale::default(), Tz::UTC);
        let settings = UserSettings::default();
        let resolved = resolve_settings(&settings, &defaults);
        assert_eq!(resolved.language, Locale::English);
        assert_eq!(resolved.number_format, NumberFormat::DotComma);
        assert_eq!(resolved.currency.code(), "USD");
    }

    #[test]
    fn resolve_settings_carries_price_fetching_flag() {
        let defaults = defaults_for_locale(Locale::default(), Tz::UTC);

        let settings = UserSettings {
            price_fetching_enabled: true,
            ..Default::default()
        };
        let resolved = resolve_settings(&settings, &defaults);
        assert!(resolved.price_fetching_enabled);

        let settings_off = UserSettings::default();
        let resolved_off = resolve_settings(&settings_off, &defaults);
        assert!(!resolved_off.price_fetching_enabled);
    }

    #[test]
    fn resolve_settings_carries_coingecko_key_flag() {
        let defaults = defaults_for_locale(Locale::default(), Tz::UTC);

        let settings = UserSettings {
            has_coingecko_api_key: true,
            ..Default::default()
        };
        let resolved = resolve_settings(&settings, &defaults);
        assert!(resolved.has_coingecko_api_key);

        let settings_without_key = UserSettings::default();
        let resolved_without_key = resolve_settings(&settings_without_key, &defaults);
        assert!(!resolved_without_key.has_coingecko_api_key);
    }
}

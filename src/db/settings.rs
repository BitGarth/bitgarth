//! User settings CRUD operations

use super::error::DbError;
use super::user_db::{with_user_db, with_user_db_mut};
use super::{clear_api_key, get_price_fetching_enabled, has_api_key, load_api_key, save_api_key};
use crate::i18n::Locale;
use crate::models::{
    ApiKeyProvider, CurrencyCode, DateTimeFormat, EtherscanBaseUrl, HledgerAccountPrefix,
    MempoolBaseUrl, NumberFormat, RawEtherscanApiKey, RawEtherscanBaseUrl, RawMempoolBaseUrl,
    SessionDuration, SimpleApiKey, UserId, UserSettings, UserTimezone,
};
use chrono::Utc;
use dioxus::logger::tracing;

const SETTINGS_ROW_ID: &str = "settings";

/// Load user settings from the database
pub(crate) fn load_settings(user_id: UserId) -> Result<UserSettings, DbError> {
    let mut settings = with_user_db(user_id, |conn| {
        let result = conn.query_row(
            "SELECT language, date_time_format, number_format, currency, timezone, session_duration, mempool_base_url, etherscan_base_url, hledger_account_prefix \
             FROM settings WHERE settings_id = ?1",
            [SETTINGS_ROW_ID],
            |row| {
                let language: Option<String> = row.get(0)?;
                let date_time_format: Option<String> = row.get(1)?;
                let number_format: Option<String> = row.get(2)?;
                let currency: Option<String> = row.get(3)?;
                let timezone: Option<String> = row.get(4)?;
                let session_duration: Option<String> = row.get(5)?;
                let mempool_base_url: Option<String> = row.get(6)?;
                let etherscan_base_url: Option<String> = row.get(7)?;
                let hledger_account_prefix: Option<String> = row.get(8)?;

                Ok(UserSettings {
                    language: parse_language(language.as_deref()),
                    date_time_format: parse_date_time_format(date_time_format.as_deref()),
                    number_format: parse_number_format(number_format.as_deref()),
                    currency: parse_currency(currency.as_deref()),
                    timezone: parse_timezone(timezone.as_deref()),
                    session_duration: parse_session_duration(session_duration.as_deref()),
                    mempool_base_url: parse_mempool_base_url(mempool_base_url.as_deref()),
                    etherscan_base_url: parse_etherscan_base_url(etherscan_base_url.as_deref()),
                    hledger_account_prefix: parse_hledger_account_prefix(
                        hledger_account_prefix.as_deref(),
                    ),
                    etherscan_api_key: None,
                    has_etherscan_api_key: false,
                    has_coingecko_api_key: false,
                    price_fetching_enabled: false,
                })
            },
        );

        match result {
            Ok(settings) => Ok(settings),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(UserSettings::default()),
            Err(e) => Err(DbError::new(format!("Failed to load settings: {}", e))),
        }
    })?;

    let etherscan_api_key =
        load_api_key(user_id, ApiKeyProvider::Etherscan)?.map(RawEtherscanApiKey::from);
    debug_assert_eq!(
        has_api_key(user_id, ApiKeyProvider::Etherscan)?,
        etherscan_api_key.is_some()
    );
    settings.has_etherscan_api_key = etherscan_api_key.is_some();
    settings.etherscan_api_key = etherscan_api_key;
    settings.has_coingecko_api_key = has_api_key(user_id, ApiKeyProvider::CoinGecko)?;
    settings.price_fetching_enabled = get_price_fetching_enabled(user_id)?;
    Ok(settings)
}

/// Save the language setting
pub(crate) fn save_language(user_id: UserId, language: Locale) -> Result<(), DbError> {
    upsert_setting(user_id, "language", language.code())
}

/// Save the date/time format setting
pub(crate) fn save_date_time_format(
    user_id: UserId,
    date_time_format: DateTimeFormat,
) -> Result<(), DbError> {
    upsert_setting(user_id, "date_time_format", date_time_format.code())
}

/// Save the number format setting
pub(crate) fn save_number_format(
    user_id: UserId,
    number_format: NumberFormat,
) -> Result<(), DbError> {
    upsert_setting(user_id, "number_format", number_format.code())
}

/// Save the currency setting
pub(crate) fn save_currency(user_id: UserId, currency: CurrencyCode) -> Result<(), DbError> {
    upsert_setting(user_id, "currency", currency.code())
}

/// Save the timezone setting
pub(crate) fn save_timezone(user_id: UserId, timezone: UserTimezone) -> Result<(), DbError> {
    upsert_setting(user_id, "timezone", &timezone.name())
}

/// Save the session duration setting
pub(crate) fn save_session_duration(
    user_id: UserId,
    session_duration: SessionDuration,
) -> Result<(), DbError> {
    upsert_setting(user_id, "session_duration", &session_duration.code())
}

pub(crate) fn save_mempool_base_url(
    user_id: UserId,
    mempool_base_url: Option<&MempoolBaseUrl>,
) -> Result<(), DbError> {
    upsert_optional_setting(
        user_id,
        "mempool_base_url",
        mempool_base_url.map(MempoolBaseUrl::as_str),
    )
}

pub(crate) fn save_etherscan_api_key(
    user_id: UserId,
    api_key: Option<&RawEtherscanApiKey>,
) -> Result<(), DbError> {
    match api_key {
        Some(api_key) => {
            let Some(api_key) = SimpleApiKey::new(api_key.as_str().to_string()) else {
                return clear_api_key(user_id, ApiKeyProvider::Etherscan);
            };
            save_api_key(user_id, ApiKeyProvider::Etherscan, &api_key)
        }
        None => clear_api_key(user_id, ApiKeyProvider::Etherscan),
    }
}

pub(crate) fn save_coingecko_api_key(
    user_id: UserId,
    api_key: Option<&SimpleApiKey>,
) -> Result<(), DbError> {
    match api_key {
        Some(api_key) => save_api_key(user_id, ApiKeyProvider::CoinGecko, api_key),
        None => clear_api_key(user_id, ApiKeyProvider::CoinGecko),
    }
}

pub(crate) fn save_etherscan_base_url(
    user_id: UserId,
    etherscan_base_url: Option<&EtherscanBaseUrl>,
) -> Result<(), DbError> {
    upsert_optional_setting(
        user_id,
        "etherscan_base_url",
        etherscan_base_url.map(EtherscanBaseUrl::as_str),
    )
}

pub(crate) fn save_hledger_account_prefix(
    user_id: UserId,
    hledger_account_prefix: Option<&HledgerAccountPrefix>,
) -> Result<(), DbError> {
    upsert_optional_setting(
        user_id,
        "hledger_account_prefix",
        hledger_account_prefix.map(HledgerAccountPrefix::as_str),
    )
}

fn upsert_setting(user_id: UserId, column: &str, value: &str) -> Result<(), DbError> {
    upsert_optional_setting(user_id, column, Some(value))
}

fn upsert_optional_setting(
    user_id: UserId,
    column: &str,
    value: Option<&str>,
) -> Result<(), DbError> {
    let now = Utc::now().to_rfc3339();
    let sql = format!(
        "INSERT INTO settings (settings_id, {column}, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(settings_id) DO UPDATE SET {column} = excluded.{column}, updated_at = excluded.updated_at"
    );

    with_user_db_mut(user_id, |conn| {
        conn.execute(&sql, rusqlite::params![SETTINGS_ROW_ID, value, now])
            .map_err(|e| DbError::new(format!("Failed to save setting: {}", e)))?;
        Ok(())
    })
}

fn parse_language(value: Option<&str>) -> Option<Locale> {
    match value {
        Some(value) => match Locale::try_from_code(value) {
            Some(language) => Some(language),
            None => {
                tracing::warn!("settings: unknown language '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_date_time_format(value: Option<&str>) -> Option<DateTimeFormat> {
    match value {
        Some(value) => match DateTimeFormat::from_code(value) {
            Some(format) => Some(format),
            None => {
                tracing::warn!("settings: unknown date_time_format '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_number_format(value: Option<&str>) -> Option<NumberFormat> {
    match value {
        Some(value) => match NumberFormat::from_code(value) {
            Some(format) => Some(format),
            None => {
                tracing::warn!("settings: unknown number_format '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_currency(value: Option<&str>) -> Option<CurrencyCode> {
    match value {
        Some(value) => match CurrencyCode::from_code(value) {
            Some(currency) => Some(currency),
            None => {
                tracing::warn!("settings: unknown currency '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_timezone(value: Option<&str>) -> Option<UserTimezone> {
    match value {
        Some(value) => match value.parse() {
            Ok(tz) => Some(UserTimezone(tz)),
            Err(_) => {
                tracing::warn!("settings: unknown timezone '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_session_duration(value: Option<&str>) -> Option<SessionDuration> {
    match value {
        Some(value) => match SessionDuration::from_code(value) {
            Some(duration) => Some(duration),
            None => {
                tracing::warn!("settings: unknown session_duration '{}', ignoring", value);
                None
            }
        },
        None => None,
    }
}

fn parse_mempool_base_url(value: Option<&str>) -> Option<RawMempoolBaseUrl> {
    match value.map(str::trim) {
        Some("") | None => None,
        Some(url) => Some(RawMempoolBaseUrl::new(url.to_string())),
    }
}

fn parse_etherscan_base_url(value: Option<&str>) -> Option<RawEtherscanBaseUrl> {
    match value.map(str::trim) {
        Some("") | None => None,
        Some(url) => Some(RawEtherscanBaseUrl::new(url.to_string())),
    }
}

fn parse_hledger_account_prefix(value: Option<&str>) -> Option<HledgerAccountPrefix> {
    match value {
        Some(value) => match HledgerAccountPrefix::parse(value) {
            Ok(prefix) => Some(prefix),
            Err(err) => {
                tracing::warn!(
                    "settings: invalid hledger_account_prefix '{}': {}, ignoring",
                    value,
                    err
                );
                None
            }
        },
        None => None,
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{setup_test_user, unique_user_id};
    use rusqlite::OptionalExtension;

    #[test]
    fn test_load_empty_settings() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let settings = load_settings(user_id).expect("Should load settings");
        assert!(settings.language.is_none());
        assert!(settings.currency.is_none());
    }

    #[test]
    fn test_has_etherscan_api_key_reflects_stored_state() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        // Initially no key configured
        let settings = load_settings(user_id).expect("Should load settings");
        assert!(!settings.has_etherscan_api_key);

        // After saving a key, has_etherscan_api_key should be true
        let key = RawEtherscanApiKey::new("TEST_KEY_123".to_string());
        save_etherscan_api_key(user_id, Some(&key)).expect("Should save key");
        let settings = load_settings(user_id).expect("Should load settings with key");
        assert!(settings.has_etherscan_api_key);

        with_user_db(user_id, |conn| {
            let api_key_row: Option<String> = conn
                .query_row(
                    "SELECT api_key FROM api_keys WHERE provider = 'etherscan'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .expect("api key query should work");
            assert_eq!(api_key_row.as_deref(), Some("TEST_KEY_123"));

            let legacy_column_exists: bool = conn
                .query_row(
                    "SELECT EXISTS (
                        SELECT 1
                        FROM pragma_table_info('settings')
                        WHERE name = 'etherscan_api_key'
                     )",
                    [],
                    |row| row.get(0),
                )
                .expect("legacy column catalog query should work");
            assert!(
                !legacy_column_exists,
                "migrated schema must not keep settings.etherscan_api_key"
            );
            Ok::<(), DbError>(())
        })
        .expect("legacy column absence check should succeed");

        // After clearing, should be false again
        save_etherscan_api_key(user_id, None).expect("Should clear key");
        let settings = load_settings(user_id).expect("Should load settings without key");
        assert!(!settings.has_etherscan_api_key);

        with_user_db(user_id, |conn| {
            let api_key_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM api_keys WHERE provider = 'etherscan'",
                    [],
                    |row| row.get(0),
                )
                .expect("api key count should load");
            assert_eq!(api_key_count, 0, "clearing must delete the api_keys row");
            Ok::<(), DbError>(())
        })
        .expect("api key clear check should succeed");
    }
}

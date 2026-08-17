//! User settings server functions

#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;

use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::session_context::{require_initialized_session, require_session_token};
#[cfg(feature = "server")]
use crate::db::{
    get_price_fetching_enabled as db_get_price_fetching_enabled, load_settings as db_load_settings,
    save_coingecko_api_key as db_save_coingecko_api_key, save_currency as db_save_currency,
    save_date_time_format as db_save_date_time_format,
    save_etherscan_api_key as db_save_etherscan_api_key,
    save_etherscan_base_url as db_save_etherscan_base_url,
    save_hledger_account_prefix as db_save_hledger_account_prefix,
    save_language as db_save_language, save_mempool_base_url as db_save_mempool_base_url,
    save_number_format as db_save_number_format, save_session_duration as db_save_session_duration,
    save_timezone as db_save_timezone,
    set_price_fetching_enabled_with_transition as db_set_price_fetching_enabled_with_transition,
    with_db,
};
#[cfg(feature = "server")]
use crate::hledger_owner::hledger_owner_segments_from_username;
use crate::i18n::Locale;
#[cfg(all(feature = "server", not(test)))]
use crate::integrations::etherscan::{EtherscanClient, EtherscanError, EtherscanNetwork};
#[cfg(feature = "server")]
use crate::models::FieldErrors;
use crate::models::SimpleApiKey;
use crate::models::{
    CurrencyCode, DateTimeFormat, HledgerAccountPrefix, NumberFormat, RawEtherscanApiKey,
    RawEtherscanBaseUrl, RawMempoolBaseUrl, SessionDuration, UserSettings, UserTimezone,
};
#[cfg(feature = "server")]
use crate::models::{
    EtherscanBaseUrl, MempoolBaseUrl, normalize_etherscan_base_url_override_for_storage,
    normalize_mempool_base_url_override_for_storage,
};
#[cfg(feature = "server")]
use crate::models::{SessionToken, UserId};
#[cfg(all(feature = "server", not(test)))]
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};

// ============ Custom Error Type ============

/// Error type for settings operations.
/// Implements the traits required by Dioxus for custom server function errors.
pub(crate) type SettingsError = ApiErrorEnvelope;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HledgerExportSettingsView {
    pub(crate) hledger_account_prefix: Option<HledgerAccountPrefix>,
    pub(crate) hledger_default_account_prefix: String,
}

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> SettingsError {
    SettingsError::unauthorized(message)
}

#[cfg(feature = "server")]
fn validation_error(field: &str, message: String) -> SettingsError {
    let mut errors = FieldErrors::new();
    errors.add(field, message);
    SettingsError::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> SettingsError {
    tracing::error!(
        context,
        error = %detail,
        "settings: internal failure"
    );
    SettingsError::internal()
}

#[cfg(feature = "server")]
fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, SettingsError> {
    require_session_token("settings", cookies, unauthorized_error)
}

#[cfg(feature = "server")]
fn load_username_for_hledger_export_settings(user_id: UserId) -> Result<String, SettingsError> {
    let username = with_db(|conn| {
        match conn.query_row(
            "SELECT username FROM users WHERE user_id = ?1",
            [user_id.to_string()],
            |row| row.get::<_, String>(0),
        ) {
            Ok(username) => Ok(Some(username)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(crate::db::DbError::from_rusqlite_error(
                "Failed to load username for hledger export settings",
                error,
            )),
        }
    })
    .map_err(|err| internal_error("settings_db", err))?;

    username.ok_or_else(|| {
        internal_error(
            "settings_db",
            format!("No username found for user {user_id} while loading hledger export settings"),
        )
    })
}

#[cfg(feature = "server")]
fn validate_mempool_base_url_override(
    raw_override: Option<RawMempoolBaseUrl>,
) -> Result<Option<MempoolBaseUrl>, SettingsError> {
    normalize_mempool_base_url_override_for_storage(raw_override)
        .map_err(|err| validation_error("mempool_base_url", err.to_string()))
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct RawSaveMempoolBaseUrlInput {
    mempool_base_url: Option<RawMempoolBaseUrl>,
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct ValidatedSaveMempoolBaseUrlInput {
    mempool_base_url: Option<MempoolBaseUrl>,
}

#[cfg(feature = "server")]
impl RawSaveMempoolBaseUrlInput {
    fn try_into_validated(self) -> Result<ValidatedSaveMempoolBaseUrlInput, SettingsError> {
        let mempool_base_url = validate_mempool_base_url_override(self.mempool_base_url)?;
        Ok(ValidatedSaveMempoolBaseUrlInput { mempool_base_url })
    }
}

#[cfg(feature = "server")]
fn validate_etherscan_base_url_override(
    raw_override: Option<RawEtherscanBaseUrl>,
) -> Result<Option<EtherscanBaseUrl>, SettingsError> {
    normalize_etherscan_base_url_override_for_storage(raw_override)
        .map_err(|err| validation_error("etherscan_base_url", err.to_string()))
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct RawSaveEtherscanBaseUrlInput {
    etherscan_base_url: Option<RawEtherscanBaseUrl>,
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct ValidatedSaveEtherscanBaseUrlInput {
    etherscan_base_url: Option<EtherscanBaseUrl>,
}

#[cfg(feature = "server")]
impl RawSaveEtherscanBaseUrlInput {
    fn try_into_validated(self) -> Result<ValidatedSaveEtherscanBaseUrlInput, SettingsError> {
        let etherscan_base_url = validate_etherscan_base_url_override(self.etherscan_base_url)?;
        Ok(ValidatedSaveEtherscanBaseUrlInput { etherscan_base_url })
    }
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct RawSaveEtherscanApiKeyInput {
    api_key: Option<RawEtherscanApiKey>,
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct ValidatedSaveEtherscanApiKeyInput {
    api_key: Option<RawEtherscanApiKey>,
}

#[cfg(feature = "server")]
impl RawSaveEtherscanApiKeyInput {
    fn try_into_validated(self) -> ValidatedSaveEtherscanApiKeyInput {
        // Treat empty/whitespace-only input as clear.
        let api_key = self
            .api_key
            .filter(|value| !value.as_str().trim().is_empty());
        ValidatedSaveEtherscanApiKeyInput { api_key }
    }
}

#[cfg(all(feature = "server", not(test)))]
fn map_etherscan_validation_error(error: EtherscanError) -> SettingsError {
    match error {
        EtherscanError::Deserialize { .. } | EtherscanError::ApiError { .. } => {
            SettingsError::bad_request("Failed to validate Etherscan API key")
        }
        other => internal_error("validate_etherscan_api_key", other),
    }
}

#[cfg(feature = "server")]
fn validate_etherscan_api_key(
    user_id: crate::models::UserId,
    api_key: &RawEtherscanApiKey,
) -> Result<(), SettingsError> {
    #[cfg(test)]
    {
        let _ = (user_id, api_key);
        Ok(())
    }

    #[cfg(not(test))]
    {
        use crate::models::resolve_effective_etherscan_base_url;

        // Load user's etherscan base URL override if configured.
        let settings = db_load_settings(user_id).map_err(|e| internal_error("settings_db", e))?;
        let (effective_base_url, _source) =
            resolve_effective_etherscan_base_url(settings.etherscan_base_url.as_ref())
                .map_err(|err| validation_error("etherscan_base_url", err.to_string()))?;

        let network = EtherscanNetwork::EthereumMainnet;
        let http_client =
            TracedBlockingClient::builder(IntegrationLabel::new("etherscan"), user_id)
                .configure(|builder| builder.timeout(std::time::Duration::from_secs(15)))
                .redact_query_params(&["apikey"])
                .build()
                .map_err(|err| internal_error("build_etherscan_http_client", err))?;
        let client = EtherscanClient::new(
            http_client,
            api_key.as_str(),
            effective_base_url.as_str(),
            network.chain_id(),
        );
        let _ = client
            .fetch_block_number()
            .map_err(map_etherscan_validation_error)?;
        Ok(())
    }
}

// ============ Settings Endpoints ============

/// Fetch user's settings from the database.
///
/// Note: This uses GET and relies on the HttpOnly session cookie. No session data is
/// placed in the URL or request body.
#[get("/_app/user/settings", cookies: CookieJar)]
pub(crate) async fn get_settings() -> Result<UserSettings, SettingsError> {
    tracing::debug!("settings: get_settings requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    let mut settings = db_load_settings(user_id).map_err(|e| internal_error("settings_db", e))?;
    // Expose whether the key is configured, but never return the actual key to clients.
    settings.has_etherscan_api_key = settings.etherscan_api_key.is_some();
    settings.etherscan_api_key = None;
    tracing::debug!(
        user_id = %user_id,
        "settings: get_settings succeeded"
    );
    Ok(settings)
}

/// Fetch settings and computed defaults used by the hledger export page.
#[get("/_app/user/settings/hledger_export", cookies: CookieJar)]
pub(crate) async fn get_hledger_export_settings() -> Result<HledgerExportSettingsView, SettingsError>
{
    tracing::debug!("settings: get_hledger_export_settings requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    let settings = db_load_settings(user_id).map_err(|e| internal_error("settings_db", e))?;
    let username = load_username_for_hledger_export_settings(user_id)?;
    let (_, owner_posting_segment) = hledger_owner_segments_from_username(&username);

    tracing::debug!(
        user_id = %user_id,
        "settings: get_hledger_export_settings succeeded"
    );

    Ok(HledgerExportSettingsView {
        hledger_account_prefix: settings.hledger_account_prefix,
        hledger_default_account_prefix: format!("assets:{owner_posting_segment}"),
    })
}

/// Fetch whether optional market price fetching is enabled for this user.
#[get("/_app/user/preferences/price_fetching", cookies: CookieJar)]
pub(crate) async fn get_price_fetching_enabled() -> Result<bool, SettingsError> {
    tracing::debug!("settings: get_price_fetching_enabled requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    let enabled =
        db_get_price_fetching_enabled(user_id).map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        enabled,
        "settings: get_price_fetching_enabled succeeded"
    );
    Ok(enabled)
}

/// Enable or disable optional market price fetching for this user.
#[post("/_app/user/preferences/price_fetching", cookies: CookieJar)]
pub(crate) async fn set_price_fetching_enabled(enabled: bool) -> Result<bool, SettingsError> {
    tracing::debug!(enabled, "settings: set_price_fetching_enabled requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    let changed_to_enabled = db_set_price_fetching_enabled_with_transition(user_id, enabled)
        .map_err(|e| internal_error("settings_db", e))?;
    if enabled && changed_to_enabled {
        let enqueue_result = crate::tasks::enqueue_price_history_reconciliation(
            user_id,
            crate::tasks::PriceHistoryReconciliationReason::PriceFetchingEnabled,
        )
        .await;
        if matches!(
            enqueue_result,
            crate::tasks::TriggerEnqueueResult::RejectedInvalidKey
                | crate::tasks::TriggerEnqueueResult::RejectedShuttingDown
        ) {
            tracing::debug!(
                user_id = %user_id,
                result = ?enqueue_result,
                "settings: price history reconciliation enqueue ignored"
            );
        }
    }
    tracing::debug!(
        user_id = %user_id,
        enabled,
        "settings: set_price_fetching_enabled succeeded"
    );
    Ok(enabled)
}

/// Save user's language preference
#[post("/_app/user/settings/language", cookies: CookieJar)]
pub(crate) async fn save_language(language: Locale) -> Result<(), SettingsError> {
    tracing::debug!(
        language = %language.code(),
        "settings: save_language requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_language(user_id, language).map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_language succeeded"
    );
    Ok(())
}

/// Save user's date/time format preference
#[post("/_app/user/settings/date_time_format", cookies: CookieJar)]
pub(crate) async fn save_date_time_format(
    date_time_format: DateTimeFormat,
) -> Result<(), SettingsError> {
    tracing::debug!(
        format = %date_time_format.code(),
        "settings: save_date_time_format requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_date_time_format(user_id, date_time_format)
        .map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_date_time_format succeeded"
    );
    Ok(())
}

/// Save user's numeric format preference
#[post("/_app/user/settings/number_format", cookies: CookieJar)]
pub(crate) async fn save_number_format(number_format: NumberFormat) -> Result<(), SettingsError> {
    tracing::debug!(
        format = %number_format.code(),
        "settings: save_number_format requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_number_format(user_id, number_format).map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_number_format succeeded"
    );
    Ok(())
}

/// Save user's default currency
#[post("/_app/user/settings/currency", cookies: CookieJar)]
pub(crate) async fn save_currency(currency: CurrencyCode) -> Result<(), SettingsError> {
    tracing::debug!(
        currency = %currency.code(),
        "settings: save_currency requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_currency(user_id, currency).map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_currency succeeded"
    );
    Ok(())
}

/// Save user's timezone
#[post("/_app/user/settings/timezone", cookies: CookieJar)]
pub(crate) async fn save_timezone(timezone: UserTimezone) -> Result<(), SettingsError> {
    tracing::debug!(
        timezone = %timezone.name(),
        "settings: save_timezone requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_timezone(user_id, timezone).map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_timezone succeeded"
    );
    Ok(())
}

/// Save user's session duration preference
#[post("/_app/user/settings/session_duration", cookies: CookieJar)]
pub(crate) async fn save_session_duration(
    session_duration: SessionDuration,
) -> Result<(), SettingsError> {
    tracing::debug!(
        duration = %session_duration.code(),
        "settings: save_session_duration requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;
    db_save_session_duration(user_id, session_duration)
        .map_err(|e| internal_error("settings_db", e))?;
    tracing::debug!(
        user_id = %user_id,
        "settings: save_session_duration succeeded"
    );
    Ok(())
}

/// Save the optional per-user mempool base URL override.
///
/// `None` means use the public default (`https://mempool.space`).
/// A configured override is validated and persisted exactly as provided.
#[post("/_app/user/settings/mempool_base_url", cookies: CookieJar)]
pub(crate) async fn save_mempool_base_url(
    mempool_base_url: Option<RawMempoolBaseUrl>,
) -> Result<Option<RawMempoolBaseUrl>, SettingsError> {
    tracing::debug!(
        has_value = mempool_base_url.is_some(),
        "settings: save_mempool_base_url requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = RawSaveMempoolBaseUrlInput { mempool_base_url }.try_into_validated()?;
    db_save_mempool_base_url(user_id, validated.mempool_base_url.as_ref())
        .map_err(|e| internal_error("settings_db", e))?;

    tracing::debug!(
        user_id = %user_id,
        configured_override = validated.mempool_base_url.is_some(),
        configured_base_url = ?validated.mempool_base_url.as_ref().map(MempoolBaseUrl::as_str),
        "settings: save_mempool_base_url succeeded"
    );

    Ok(validated
        .mempool_base_url
        .map(|value| RawMempoolBaseUrl::new(value.as_str().to_string())))
}

/// Save the optional per-user Etherscan base URL override.
///
/// `None` means use the public default (`https://api.etherscan.io/v2/api`).
/// A configured override is validated and persisted exactly as provided.
#[post("/_app/user/settings/etherscan_base_url", cookies: CookieJar)]
pub(crate) async fn save_etherscan_base_url(
    etherscan_base_url: Option<RawEtherscanBaseUrl>,
) -> Result<Option<RawEtherscanBaseUrl>, SettingsError> {
    tracing::debug!(
        has_value = etherscan_base_url.is_some(),
        "settings: save_etherscan_base_url requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = RawSaveEtherscanBaseUrlInput { etherscan_base_url }.try_into_validated()?;
    db_save_etherscan_base_url(user_id, validated.etherscan_base_url.as_ref())
        .map_err(|e| internal_error("settings_db", e))?;

    tracing::debug!(
        user_id = %user_id,
        configured_override = validated.etherscan_base_url.is_some(),
        configured_base_url = ?validated.etherscan_base_url.as_ref().map(EtherscanBaseUrl::as_str),
        "settings: save_etherscan_base_url succeeded"
    );

    Ok(validated
        .etherscan_base_url
        .map(|value| RawEtherscanBaseUrl::new(value.as_str().to_string())))
}

/// Save the optional hledger account prefix.
///
/// `None` or an empty trimmed value clears the override.
#[post("/_app/user/settings/hledger_account_prefix", cookies: CookieJar)]
pub(crate) async fn save_hledger_account_prefix(
    hledger_account_prefix: Option<String>,
) -> Result<Option<HledgerAccountPrefix>, SettingsError> {
    tracing::debug!(
        has_value = hledger_account_prefix.is_some(),
        "settings: save_hledger_account_prefix requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    let hledger_account_prefix = match hledger_account_prefix.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(value) => Some(
            HledgerAccountPrefix::parse(value)
                .map_err(|err| validation_error("hledger_account_prefix", err.to_string()))?,
        ),
    };

    db_save_hledger_account_prefix(user_id, hledger_account_prefix.as_ref())
        .map_err(|e| internal_error("settings_db", e))?;

    tracing::debug!(
        user_id = %user_id,
        configured = hledger_account_prefix.is_some(),
        "settings: save_hledger_account_prefix succeeded"
    );

    Ok(hledger_account_prefix)
}

/// Save the optional per-user Etherscan API key.
///
/// `None` clears the stored key. A non-empty string is stored as-is.
#[post("/_app/user/settings/etherscan_api_key", cookies: CookieJar)]
pub(crate) async fn save_etherscan_api_key(
    api_key: Option<RawEtherscanApiKey>,
) -> Result<(), SettingsError> {
    tracing::debug!(
        has_value = api_key.is_some(),
        "settings: save_etherscan_api_key requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = RawSaveEtherscanApiKeyInput { api_key }.try_into_validated();

    if let Some(candidate) = validated.api_key.clone() {
        // validate_etherscan_api_key builds a reqwest::blocking client, which
        // spins up (and drops) its own tokio runtime. That panics if called
        // directly from an async handler, so run it on a blocking thread.
        let validation_result =
            tokio::task::spawn_blocking(move || validate_etherscan_api_key(user_id, &candidate))
                .await
                .map_err(|err| internal_error("validate_etherscan_api_key_join", err))?;

        if let Err(error) = validation_result {
            tracing::error!(
                user_id = %user_id,
                error = %error,
                "settings: save_etherscan_api_key validation failed"
            );
            return Err(error);
        }
    }

    db_save_etherscan_api_key(user_id, validated.api_key.as_ref())
        .map_err(|e| internal_error("settings_db", e))?;

    tracing::debug!(
        user_id = %user_id,
        configured = validated.api_key.is_some(),
        "settings: save_etherscan_api_key succeeded"
    );

    Ok(())
}

/// Save or clear the user's CoinGecko Pro API key.
#[post("/_app/user/settings/coingecko_api_key", cookies: CookieJar)]
pub(crate) async fn save_coingecko_api_key(
    api_key: Option<SimpleApiKey>,
) -> Result<(), SettingsError> {
    tracing::debug!(
        configured = api_key.is_some(),
        "settings: save_coingecko_api_key requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("settings", &session_token, unauthorized_error, |_message| {
            SettingsError::internal()
        })?;
    let user_id = initialized_session.session.user_id;

    db_save_coingecko_api_key(user_id, api_key.as_ref())
        .map_err(|e| internal_error("settings_db", e))?;

    tracing::debug!(
        user_id = %user_id,
        configured = api_key.is_some(),
        "settings: save_coingecko_api_key succeeded"
    );

    Ok(())
}

//! Integration tests for settings endpoint contracts.
//!
//! Scope:
//! - Representative settings read/write contract coverage
//! - Secret non-exposure contract coverage
//! - Minimal auth and malformed-body smoke tests

use crate::models::{RawEtherscanApiKey, SimpleApiKey, UserSettings};
use dioxus::fullstack::StatusCode;
use serde_json::{Value, json};

use super::fixtures::{legal_acknowledgement_json, register_user};
use super::{IntegrationTestServer, setup_test_server, setup_test_server_no_db};

async fn fetch_settings(server: &IntegrationTestServer) -> UserSettings {
    let response = server.get("/_app/user/settings").await;

    response.assert_status_ok();

    let body: Value = response.json();
    serde_json::from_value(body)
        .unwrap_or_else(|e| panic!("Failed to parse settings response: {}", e))
}

async fn register_user_named(server: &IntegrationTestServer, username: &str) {
    server
        .post("/_app/auth/register")
        .json(&json!({
            "username": username,
            "password": "SecurePass123",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await
        .assert_status_ok();
}

async fn assert_malformed_json_returns_bad_request(server: &IntegrationTestServer, path: &str) {
    let response = server
        .post(path)
        .bytes(vec![b'{'].into())
        .content_type("application/json")
        .await;
    let body = response.text();

    assert_eq!(
        response.status_code(),
        StatusCode::BAD_REQUEST,
        "Expected malformed JSON to return 400 for endpoint {path}. Body: {body}",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_settings_returns_empty_for_new_user() {
    let server = setup_test_server();
    register_user(&server).await;

    let settings = fetch_settings(&server).await;

    assert!(settings.language.is_none());
    assert!(settings.date_time_format.is_none());
    assert!(settings.number_format.is_none());
    assert!(settings.currency.is_none());
    assert!(settings.timezone.is_none());
    assert!(settings.session_duration.is_none());
    assert!(settings.mempool_base_url.is_none());
    assert!(settings.etherscan_base_url.is_none());
    assert!(settings.etherscan_api_key.is_none());
    assert!(!settings.has_coingecko_api_key);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_settings_reports_price_fetching_flag() {
    let server = setup_test_server();
    register_user(&server).await;

    let settings = fetch_settings(&server).await;
    assert!(!settings.price_fetching_enabled);

    let response = server
        .post("/_app/user/preferences/price_fetching")
        .json(&json!({ "enabled": true }))
        .await;
    response.assert_status_ok();

    let settings = fetch_settings(&server).await;
    assert!(settings.price_fetching_enabled);
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_language_persists_language_setting() {
    use crate::i18n::Locale;
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/language")
        .json(&json!({
            "language": Locale::English
        }))
        .await;

    response.assert_status_ok();

    let settings = fetch_settings(&server).await;
    assert_eq!(settings.language, Some(Locale::English));
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_settings_with_invalid_token_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server.get("/_app/user/settings").await;

    response.assert_status_unauthorized();
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_language_with_invalid_token_returns_unauthorized() {
    use crate::i18n::Locale;
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/settings/language")
        .json(&json!({
            "language": Locale::English
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_etherscan_api_key_happy_path_and_not_exposed() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/etherscan_api_key")
        .json(&json!({
            "api_key": RawEtherscanApiKey::new("TEST_ETHERSCAN_KEY".to_string())
        }))
        .await;
    response.assert_status_ok();

    let settings = fetch_settings(&server).await;
    assert!(
        settings.etherscan_api_key.is_none(),
        "Etherscan API key should never be returned from get_settings"
    );

    let response = server
        .post("/_app/user/settings/etherscan_api_key")
        .json(&json!({
            "api_key": Value::Null
        }))
        .await;
    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_coingecko_api_key_happy_path_and_not_exposed() {
    let server = setup_test_server();
    register_user(&server).await;

    let settings = fetch_settings(&server).await;
    assert!(!settings.has_coingecko_api_key);

    let response = server
        .post("/_app/user/settings/coingecko_api_key")
        .json(&json!({
            "api_key": SimpleApiKey::new("TEST_COINGECKO_KEY".to_string())
                .expect("non-empty test key")
        }))
        .await;
    response.assert_status_ok();

    let response = server.get("/_app/user/settings").await;
    response.assert_status_ok();
    let raw: Value = response.json();
    assert_eq!(raw["has_coingecko_api_key"], true);
    assert!(
        raw.get("coingecko_api_key").is_none(),
        "CoinGecko API key must never be returned from get_settings"
    );

    let settings: UserSettings = serde_json::from_value(raw).expect("settings");
    assert!(settings.has_coingecko_api_key);

    let response = server
        .post("/_app/user/settings/coingecko_api_key")
        .json(&json!({ "api_key": Value::Null }))
        .await;
    response.assert_status_ok();

    let settings = fetch_settings(&server).await;
    assert!(!settings.has_coingecko_api_key);
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_coingecko_api_key_rejects_blank_key() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/coingecko_api_key")
        .json(&json!({ "api_key": "   " }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let settings = fetch_settings(&server).await;
    assert!(!settings.has_coingecko_api_key);
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_hledger_account_prefix_persists_trims_and_clears() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/hledger_account_prefix")
        .json(&json!({ "hledger_account_prefix": " assets:My Wallet " }))
        .await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>(), json!("assets:My Wallet"));

    let settings = fetch_settings(&server).await;
    assert_eq!(
        settings
            .hledger_account_prefix
            .as_ref()
            .map(|prefix| prefix.as_str()),
        Some("assets:My Wallet")
    );

    let response = server
        .post("/_app/user/settings/hledger_account_prefix")
        .json(&json!({ "hledger_account_prefix": "   " }))
        .await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>(), Value::Null);

    let settings = fetch_settings(&server).await;
    assert!(settings.hledger_account_prefix.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_hledger_export_settings_returns_saved_and_default_account_prefixes() {
    let server = setup_test_server();
    register_user_named(&server, "wind-runner").await;

    let response = server.get("/_app/user/settings/hledger_export").await;
    response.assert_status_ok();
    assert_eq!(
        response.json::<Value>(),
        json!({
            "hledger_account_prefix": null,
            "hledger_default_account_prefix": "assets:WindRunner"
        })
    );

    let response = server
        .post("/_app/user/settings/hledger_account_prefix")
        .json(&json!({ "hledger_account_prefix": " assets:My Wallet " }))
        .await;
    response.assert_status_ok();

    let response = server.get("/_app/user/settings/hledger_export").await;
    response.assert_status_ok();
    assert_eq!(
        response.json::<Value>(),
        json!({
            "hledger_account_prefix": "assets:My Wallet",
            "hledger_default_account_prefix": "assets:WindRunner"
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_save_hledger_account_prefix_rejects_invalid_internal_spacing_without_change() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/hledger_account_prefix")
        .json(&json!({ "hledger_account_prefix": "assets:Cash" }))
        .await;
    response.assert_status_ok();

    let response = server
        .post("/_app/user/settings/hledger_account_prefix")
        .json(&json!({ "hledger_account_prefix": "assets:My  Wallet" }))
        .await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    let settings = fetch_settings(&server).await;
    assert_eq!(
        settings
            .hledger_account_prefix
            .as_ref()
            .map(|prefix| prefix.as_str()),
        Some("assets:Cash")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_auth_me_reports_saved_coingecko_api_key_flag() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/settings/coingecko_api_key")
        .json(&json!({
            "api_key": SimpleApiKey::new("TEST_COINGECKO_KEY".to_string())
                .expect("non-empty test key")
        }))
        .await;
    response.assert_status_ok();

    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let raw: Value = response.json();
    assert_eq!(raw["settings"]["has_coingecko_api_key"], true);
    assert!(
        raw["settings"].get("coingecko_api_key").is_none(),
        "CoinGecko API key must never be returned from auth/me"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_settings_body_endpoints_reject_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();

    for path in [
        "/_app/user/settings/language",
        "/_app/user/settings/date_time_format",
        "/_app/user/settings/number_format",
        "/_app/user/settings/currency",
        "/_app/user/settings/timezone",
        "/_app/user/settings/session_duration",
        "/_app/user/settings/mempool_base_url",
        "/_app/user/settings/etherscan_base_url",
        "/_app/user/settings/hledger_account_prefix",
        "/_app/user/settings/etherscan_api_key",
        "/_app/user/settings/coingecko_api_key",
    ] {
        assert_malformed_json_returns_bad_request(&server, path).await;
    }
}

//! Integration tests for price override endpoint contracts.
//!
//! Scope:
//! - Auth guard (unauthenticated returns unauthorized)
//! - Generic upsert/delete contract
//! - Validation rejection (non-positive price)
//! - Report resolved-prices endpoint

use dioxus::fullstack::StatusCode;
use serde_json::{Value, json};

use crate::backend::{
    DeletePriceOverrideInput, PriceOverrideView, UpsertPriceOverrideInput, delete_price_override,
    list_price_overrides, list_resolved_prices_for_report, upsert_price_override,
};

use super::fixtures::{
    activate_signed_full_report_entitlements, add_ethereum_wallet_account, register_user,
    register_user_with_prefix,
};
use super::{IntegrationTestServer, setup_test_server, setup_test_server_no_db};

const ETH_ADDRESS: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

#[test]
fn backend_price_exports_are_visible() {
    fn assert_clone<T: Clone>() {}

    assert_clone::<PriceOverrideView>();
    assert_clone::<UpsertPriceOverrideInput>();
    assert_clone::<DeletePriceOverrideInput>();
    let _ = list_price_overrides;
    let _ = upsert_price_override;
    let _ = delete_price_override;
    let _ = list_resolved_prices_for_report;
}

/// Maps query-string subject types to the body's `PriceSubject` variant kind.
fn body_subject_kind(query_subject_kind: &str) -> &'static str {
    match query_subject_kind {
        "native_asset" | "catalog_asset" => "catalog_asset",
        "custom_unit_code" => "legacy_custom_unit_code",
        other => panic!("unsupported subject_kind for body: {other}"),
    }
}

fn upsert_body(
    subject_kind: &str,
    subject_id: &str,
    currency: &str,
    local_time: &str,
    price: &str,
) -> Value {
    json!({
        "input": {
            "subject": { "kind": body_subject_kind(subject_kind), "id": subject_id },
            "quote_currency": currency,
            "price_time_local": local_time,
            "price": price,
            "source_note": null
        }
    })
}

fn delete_body(subject_kind: &str, subject_id: &str, currency: &str, local_time: &str) -> Value {
    json!({
        "input": {
            "subject": { "kind": body_subject_kind(subject_kind), "id": subject_id },
            "quote_currency": currency,
            "price_time_local": local_time
        }
    })
}

fn list_overrides_url(
    subject_kind: &str,
    subject_id: &str,
    currency: &str,
    from: &str,
    to: &str,
) -> String {
    format!(
        "/_app/user/prices/overrides?subject_type={subject_kind}&subject_id={subject_id}&quote_currency={currency}&from={}&to={}",
        encode_query_value(from),
        encode_query_value(to)
    )
}

fn encode_query_value(value: &str) -> String {
    value.replace(':', "%3A").replace('+', "%2B")
}

async fn save_currency(server: &IntegrationTestServer, currency: &str) {
    server
        .post("/_app/user/settings/currency")
        .json(&json!({ "currency": currency }))
        .await
        .assert_status_ok();
}

/// Builds the `asset_instance_id` payload for the manual asset endpoint.
/// Currently only ADA on cardano-mainnet is exercised by the prices tests.
fn asset_instance_id_for_unit_code(unit_code: &str) -> Value {
    match unit_code {
        "ADA" => json!({
            "asset_id": "cardano",
            "network_id": "cardano-mainnet",
            "namespace": { "type": "native" }
        }),
        other => panic!("unsupported unit_code in prices tests: {other}"),
    }
}

async fn add_manual_asset_account(
    server: &IntegrationTestServer,
    wallet_id: Option<&str>,
    unit_code: &str,
) -> Value {
    let mut request = serde_json::Map::new();
    request.insert(
        "asset_instance_id".to_string(),
        asset_instance_id_for_unit_code(unit_code),
    );
    if let Some(wallet_id) = wallet_id {
        request.insert(
            "wallet_id".to_string(),
            Value::String(wallet_id.to_string()),
        );
    }

    let response = server
        .post("/_app/user/wallets/manual-assets/add")
        .json(&json!({ "request": Value::Object(request) }))
        .await;
    response.assert_status_ok();
    response.json()
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_price_override_unauthenticated_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "42000",
        ))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 for unauthenticated upsert"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_price_override_rejects_legacy_custom_subject() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "custom_unit_code",
            "ADA",
            "USD",
            "2025-01-01T00:00:00",
            "0.45",
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn test_list_price_overrides_rejects_custom_unit_code() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .get(&list_overrides_url(
            "custom_unit_code",
            "ADA",
            "USD",
            "2025-01-01T00:00:00Z",
            "2025-01-01T00:00:00Z",
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_price_override_rejects_zero_price() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "0",
        ))
        .await;

    assert_eq!(
        response.status_code(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "Expected 422 for zero price"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_price_override_rejects_negative_price() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "-1",
        ))
        .await;

    assert_eq!(
        response.status_code(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "Expected 422 for negative price"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_delete_price_override_is_idempotent() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .post("/_app/user/prices/overrides/delete")
        .json(&delete_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
        ))
        .await;

    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_and_delete_round_trip() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "42000",
        ))
        .await;
    response.assert_status_ok();

    let response = server
        .post("/_app/user/prices/overrides/delete")
        .json(&delete_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
        ))
        .await;
    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_list_price_overrides_unauthenticated_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .get(&list_overrides_url(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00Z",
            "2025-12-31T23:59:59Z",
        ))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 for unauthenticated list"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_upsert_price_override_allows_free_tier_entitlement() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "42000",
        ))
        .await;

    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_list_price_overrides_get_returns_matching_rows() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "42000",
        ))
        .await
        .assert_status_ok();

    let response = server
        .get(&list_overrides_url(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00Z",
            "2025-01-01T00:00:00Z",
        ))
        .await;
    response.assert_status_ok();
    let rows: Value = response.json();
    assert_eq!(rows.as_array().expect("rows should be array").len(), 1);
    assert_eq!(rows[0]["price"], "42000");
}

#[tokio::test(flavor = "current_thread")]
async fn test_user_cannot_see_another_users_overrides() {
    let server = setup_test_server();
    register_user_with_prefix(&server, "prices_user_a").await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;

    server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00",
            "42000",
        ))
        .await
        .assert_status_ok();

    register_user_with_prefix(&server, "prices_user_b").await;

    let response = server
        .get(&list_overrides_url(
            "native_asset",
            "bitcoin",
            "USD",
            "2025-01-01T00:00:00Z",
            "2025-01-01T00:00:00Z",
        ))
        .await;
    response.assert_status_ok();
    let rows: Value = response.json();
    assert_eq!(rows.as_array().expect("rows should be array").len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn test_report_resolved_prices_returns_only_wallet_visible_subjects() {
    let server = setup_test_server();
    register_user(&server).await;
    let wallet = add_ethereum_wallet_account(&server, ETH_ADDRESS, "Prices Report").await;
    add_manual_asset_account(&server, Some(wallet.wallet_id.as_str()), "ADA").await;

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report/resolved-prices?from=2025-01-01&to=2025-01-01&timezone=UTC",
            wallet.wallet_id
        ))
        .await;
    response.assert_status_ok();
    let rows: Value = response.json();
    let rows = rows.as_array().expect("rows should be array");
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().any(|row| row["subject"]["id"] == "ethereum"));
    assert!(rows.iter().any(|row| row["subject"]["id"] == "cardano"));
    assert!(!rows.iter().any(|row| row["subject"]["id"] == "bitcoin"));
}

#[tokio::test(flavor = "current_thread")]
async fn test_report_resolved_prices_uses_current_settings_currency() {
    let server = setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;
    let wallet = add_ethereum_wallet_account(&server, ETH_ADDRESS, "Prices Currency").await;

    server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "ethereum",
            "USD",
            "2025-01-01T00:00:00",
            "3000",
        ))
        .await
        .assert_status_ok();
    server
        .post("/_app/user/prices/overrides")
        .json(&upsert_body(
            "native_asset",
            "ethereum",
            "EUR",
            "2025-01-01T00:00:00",
            "2800",
        ))
        .await
        .assert_status_ok();
    save_currency(&server, "EUR").await;

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report/resolved-prices?from=2025-01-01&to=2025-01-01&timezone=UTC",
            wallet.wallet_id
        ))
        .await;
    response.assert_status_ok();
    let rows: Value = response.json();
    let rows = rows.as_array().expect("rows should be array");
    let opening = rows
        .iter()
        .find(|row| row["subject"]["id"] == "ethereum" && row["boundary"] == "Opening")
        .expect("ethereum opening row should exist");
    assert_eq!(opening["price"], "2800");
}

#[tokio::test(flavor = "current_thread")]
async fn test_malformed_json_returns_bad_request() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/prices/overrides")
        .bytes(vec![b'{'].into())
        .content_type("application/json")
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::BAD_REQUEST,
        "Expected 400 for malformed JSON"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_delete_malformed_json_returns_bad_request() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/prices/overrides/delete")
        .bytes(vec![b'{'].into())
        .content_type("application/json")
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::BAD_REQUEST,
        "Expected 400 for malformed JSON"
    );
}

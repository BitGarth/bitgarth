//! Integration tests for export endpoint contracts.
//!
//! Scope:
//! - Representative export/import happy-path contracts
//! - Minimal unauthorized and malformed-input smoke tests

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode as AxumStatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use chrono::{Duration, Utc};
use dioxus::fullstack::StatusCode;
use ed25519_dalek::{Signer, SigningKey};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use super::fixtures::{
    add_ethereum_wallet_account, add_xpub_wallet_account, deterministic_test_xpub, register_user,
};
use super::{IntegrationTestServer, setup_test_server, setup_test_server_no_db};
use crate::models::{ApiKeyProvider, AuthResponse, SimpleApiKey, UserId};
use crate::payments::keys::{expected_signing_key_hash, set_signing_public_key_override_for_test};
use crate::payments::types::{
    CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementHolderId, EntitlementTier,
    PaymentAmount, PaymentOrderId, PaymentOrderStatus, PaymentSecret, ProductTier,
    SubscriptionSubjectId, TokenClaims, TokenId,
};
use crate::wallets::BIP44_GAP_LIMIT;

const TEST_PUBLIC_KEY_B64: &str = "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";
const PAYMENT_ORDER_ID: &str = "01JQABCDEF000000000000000E";
const PAYMENT_ORDER_SECRET: &str = "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI";
const PAYMENT_MANAGEMENT_SECRET: &str = "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw";
const TRANSFER_TOKEN_ID: &str = "01JQABCDEF000000000000000F";
const TRANSFER_SUBSCRIPTION_SUBJECT_ID: &str = "01JQABCDEF000000000000000G";

#[derive(Clone, Copy)]
enum MockTransferOutcome {
    Active,
    ServiceUnavailable,
    InvalidManagementSecret,
}

struct MockCentral {
    base_url: String,
    state: Arc<Mutex<MockCentralState>>,
}

struct MockCentralState {
    expected_signing_key_hash: String,
    transfer_outcome: MockTransferOutcome,
}

#[derive(Deserialize)]
struct TransferSubscriptionRequest {
    new_entitlement_holder_id: EntitlementHolderId,
    new_management_secret: String,
}

impl MockCentral {
    async fn start(expected_signing_key_hash: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock Central should bind");
        let base_url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("mock Central should have addr")
        );
        let state = Arc::new(Mutex::new(MockCentralState {
            expected_signing_key_hash,
            transfer_outcome: MockTransferOutcome::Active,
        }));
        let router = Router::new()
            .route(
                "/api/v1/payments/subscription/transfer",
                post(transfer_subscription),
            )
            .with_state(Arc::clone(&state));

        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("mock Central should serve");
        });

        Self { base_url, state }
    }

    fn set_transfer_outcome(&self, outcome: MockTransferOutcome) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .transfer_outcome = outcome;
    }
}

fn central_guard(mock: &MockCentral) -> crate::payments::client::CentralBaseUrlOverrideGuard {
    crate::payments::client::set_central_base_url_override_for_test(mock.base_url.clone())
}

fn signing_key_error(headers: &HeaderMap, state: &MockCentralState) -> Option<Response> {
    let Some(actual) = headers
        .get("X-BitGarth-Expected-Signing-Key-Hash")
        .and_then(|value| value.to_str().ok())
    else {
        return Some(upgrade_required());
    };

    if actual != state.expected_signing_key_hash {
        return Some(upgrade_required());
    }

    None
}

fn upgrade_required() -> Response {
    (
        AxumStatusCode::UPGRADE_REQUIRED,
        Json(json!({
            "error_code": "invalid_expected_signing_key",
            "message": "upgrade required"
        })),
    )
        .into_response()
}

async fn transfer_subscription(
    State(state): State<Arc<Mutex<MockCentralState>>>,
    headers: HeaderMap,
    Json(request): Json<TransferSubscriptionRequest>,
) -> Response {
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(response) = signing_key_error(&headers, &state) {
        return response;
    }

    match state.transfer_outcome {
        MockTransferOutcome::Active => {
            if headers
                .get("Authorization")
                .and_then(|value| value.to_str().ok())
                != Some(&format!("Bearer {PAYMENT_MANAGEMENT_SECRET}"))
            {
                return invalid_management_secret();
            }
            let new_management_secret = PaymentSecret::from_raw(request.new_management_secret)
                .expect("new management secret should be valid");
            assert_ne!(new_management_secret.as_str(), PAYMENT_MANAGEMENT_SECRET);
            let claims = transfer_token_claims(request.new_entitlement_holder_id);
            let token = sign_transfer_token(&claims);
            Json(json!({
                "status": "active",
                "premium_access_token": token,
                "token_id": TRANSFER_TOKEN_ID,
                "subscription_valid_until": claims.subscription_valid_until,
                "token_expires_at": claims.token_expires_at
            }))
            .into_response()
        }
        MockTransferOutcome::ServiceUnavailable => (
            AxumStatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error_code": "service_unavailable",
                "message": "try again"
            })),
        )
            .into_response(),
        MockTransferOutcome::InvalidManagementSecret => invalid_management_secret(),
    }
}

fn invalid_management_secret() -> Response {
    (
        AxumStatusCode::UNAUTHORIZED,
        Json(json!({
            "error_code": "invalid_management_secret",
            "message": "invalid management secret"
        })),
    )
        .into_response()
}

fn transfer_token_claims(holder: EntitlementHolderId) -> TokenClaims {
    let now = Utc::now();
    TokenClaims {
        token_id: TokenId::from_str(TRANSFER_TOKEN_ID).expect("test token id should parse"),
        subscription_subject_id: SubscriptionSubjectId::from_str(TRANSFER_SUBSCRIPTION_SUBJECT_ID)
            .expect("test subscription subject id should parse"),
        entitlement_holder_id: holder,
        tier: EntitlementTier::Premium,
        capability_set_id: Some("capset_premium_v1".to_string()),
        capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
        capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
        subscription_valid_until: now + Duration::days(365),
        token_expires_at: now + Duration::days(7),
        issued_at: now - Duration::minutes(1),
    }
}

fn sign_transfer_token(claims: &TokenClaims) -> String {
    let claims_json = serde_json::to_vec(claims).expect("claims should serialize");
    let signing_key = SigningKey::from_bytes(&[0_u8; 32]);
    let signature = signing_key.sign(&claims_json);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(claims_json),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

/// Builds the `asset_instance_id` payload for the manual asset endpoint.
/// Only ADA on cardano-mainnet is exercised by these export tests.
fn asset_instance_id_for_unit_code(unit_code: &str) -> Value {
    match unit_code {
        "ADA" => json!({
            "asset_id": "cardano",
            "network_id": "cardano-mainnet",
            "namespace": { "type": "native" }
        }),
        other => panic!("unsupported unit_code in exports tests: {other}"),
    }
}

async fn add_manual_asset_account(
    server: &super::IntegrationTestServer,
    wallet_id: &str,
    unit_code: &str,
) -> Value {
    let response = server
        .post("/_app/user/wallets/manual-assets/add")
        .json(&json!({
            "request": {
                "wallet_id": wallet_id,
                "asset_instance_id": asset_instance_id_for_unit_code(unit_code)
            }
        }))
        .await;
    response.assert_status_ok();
    response.json()
}

async fn add_custom_balance_assertion(
    server: &super::IntegrationTestServer,
    account_id: &str,
    asserted_on: &str,
    balance: &str,
    note: Option<&str>,
) {
    let response = server
        .post("/_app/user/manual-asset-assertions/add")
        .json(&json!({
            "request": {
                "account_id": account_id,
                "asserted_on": asserted_on,
                "balance": balance,
                "note": note
            }
        }))
        .await;
    response.assert_status_ok();
}

fn zip_base64_for_payload_json(payload_json: &str, password: Option<&str>) -> String {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let options = if let Some(password) = password {
        options.with_aes_encryption(zip::AesMode::Aes256, password)
    } else {
        options
    };
    archive
        .start_file("wallet-data.json", options)
        .expect("zip entry should start");
    archive
        .write_all(payload_json.as_bytes())
        .expect("payload should write");
    let cursor = archive.finish().expect("zip should finish");
    BASE64.encode(cursor.into_inner())
}

fn wallet_data_payload_from_zip(zip_base64: &str, password: Option<&str>) -> Value {
    let zip_bytes = BASE64.decode(zip_base64).expect("zip base64 should decode");
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).expect("zip should open");
    let mut entry = if let Some(password) = password {
        archive
            .by_index_decrypt(0, password.as_bytes())
            .expect("encrypted zip should open")
    } else {
        archive.by_index(0).expect("zip entry should open")
    };
    let mut payload_json = String::new();
    entry
        .read_to_string(&mut payload_json)
        .expect("zip entry should read");
    serde_json::from_str(&payload_json).expect("inner JSON should parse")
}

fn import_request_body(file_name: &str, payload_json: &str, password: Option<&str>) -> Value {
    json!({
        "request": {
            "file_name": file_name,
            "payload_base64": zip_base64_for_payload_json(payload_json, password),
            "password": password
        }
    })
}

fn import_request_body_from_zip(
    file_name: &str,
    payload_base64: &str,
    password: Option<&str>,
) -> Value {
    json!({
        "request": {
            "file_name": file_name,
            "payload_base64": payload_base64,
            "password": password
        }
    })
}

fn import_raw_json_request_body(file_name: &str, payload_json: &str) -> Value {
    json!({
        "request": {
            "file_name": file_name,
            "payload_base64": BASE64.encode(payload_json.as_bytes()),
            "password": null
        }
    })
}

fn wallet_data_export_request(
    include_premium_transfer: bool,
    encrypted: bool,
    password: Option<&str>,
) -> Value {
    json!({
        "request": {
            "include_premium_transfer": include_premium_transfer,
            "encrypted": encrypted,
            "password": password
        }
    })
}

fn premium_transfer_payload_json(management_secret: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "version": 3,
        "exported_at": "2026-04-25T12:00:00Z",
        "bitgarth_version": "0.1.0",
        "wallets": [],
        "settings": null,
        "premium_transfer": {
            "exported_at": "2026-04-25T12:00:00Z",
            "management_secret": management_secret,
            "active_token": null,
            "token_id": null,
            "subscription_subject_id": null,
            "subscription_valid_until": null,
            "token_expires_at": null,
            "token_issued_at": null,
            "orders": [{
                "order_id": PAYMENT_ORDER_ID,
                "product_tier": "premium",
                "order_amount_minor_units": 999,
                "order_currency": "USD",
                "order_display_scale": 2,
                "status": "paid",
                "paid_at": "2026-04-25T12:00:00Z"
            }]
        }
    }))
    .expect("payload should serialize")
}

fn wallet_data_v4_payload_json_with_api_keys(api_keys: &[(&str, &str)]) -> String {
    let api_keys = api_keys
        .iter()
        .map(|(provider, api_key)| {
            json!({
                "provider": provider,
                "api_key": api_key,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "version": 4,
        "exported_at": "2026-04-25T12:00:00Z",
        "bitgarth_version": "0.1.0",
        "wallets": [],
        "settings": null,
        "api_keys": api_keys,
    }))
    .expect("payload should serialize")
}

fn save_api_key_for_user(user_id: UserId, provider: ApiKeyProvider, value: &str) {
    let api_key = SimpleApiKey::new(value.to_string()).expect("test API key should be valid");
    crate::db::save_api_key(user_id, provider, &api_key).expect("test API key should save");
}

fn load_api_key_for_user(user_id: UserId, provider: ApiKeyProvider) -> Option<String> {
    crate::db::load_api_key(user_id, provider)
        .expect("test API key should load")
        .map(|value| value.as_str().to_string())
}

async fn import_pending_premium_transfer(server: &IntegrationTestServer) -> String {
    let payload_json = premium_transfer_payload_json(PAYMENT_MANAGEMENT_SECRET);
    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body(
            "premium-wallet-data.zip",
            &payload_json,
            None,
        ))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["premium_transfer_status"], "pending_confirmation");
    body["pending_premium_transfer_id"]
        .as_str()
        .expect("pending transfer id should be returned")
        .to_string()
}

fn confirm_premium_transfer_request(pending_transfer_id: &str) -> Value {
    json!({
        "request": {
            "pending_transfer_id": pending_transfer_id
        }
    })
}

async fn current_user_id(server: &IntegrationTestServer) -> UserId {
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();
    let body: AuthResponse = response.json();
    body.user.user_id
}

fn assert_imported_xpub_bootstrap_state(user_id: UserId) {
    let hd_bundles =
        crate::db::get_hd_account_sync_bundles(user_id).expect("HD bundles should load");
    assert_eq!(
        hd_bundles.len(),
        1,
        "imported wallet-data fixture should contain one Bitcoin HD account"
    );

    let bundle = &hd_bundles[0];
    assert_eq!(bundle.external_addresses.len(), BIP44_GAP_LIMIT as usize);
    assert_eq!(bundle.internal_addresses.len(), BIP44_GAP_LIMIT as usize);

    let sync_state = bundle
        .sync_state
        .as_ref()
        .expect("imported HD account should have account_sync_state");
    assert_eq!(sync_state.gap_limit, BIP44_GAP_LIMIT);
    assert_eq!(
        sync_state.last_derived_external_index,
        Some(BIP44_GAP_LIMIT - 1)
    );
    assert_eq!(
        sync_state.last_derived_internal_index,
        Some(BIP44_GAP_LIMIT - 1)
    );
}

fn seed_premium_transfer_state(user_id: UserId) {
    let now = chrono::Utc::now();
    crate::db::payments::load_or_create_payment_subject(user_id, now)
        .expect("payment subject should be created");
    let management_secret =
        PaymentSecret::from_raw(PAYMENT_MANAGEMENT_SECRET).expect("management secret should parse");
    crate::db::payments::update_payment_management_secret(user_id, &management_secret, now)
        .expect("management secret should persist");
    let order_id = PaymentOrderId::from_str(PAYMENT_ORDER_ID).expect("order id should parse");
    crate::db::payments::insert_payment_order(
        user_id,
        &crate::db::payments::NewPaymentOrder {
            order_id,
            order_secret: PaymentSecret::from_raw(PAYMENT_ORDER_SECRET)
                .expect("order secret should parse"),
            product_tier: ProductTier::Premium,
            amount: PaymentAmount {
                minor_units: 999,
                currency: "USD".to_string(),
                currency_symbol: None,
                decimal_precision: 2,
            },
        },
        now,
    )
    .expect("payment order should persist");
    crate::db::payments::mark_payment_order_status(
        user_id,
        order_id,
        PaymentOrderStatus::Paid,
        Some(now),
        now,
    )
    .expect("payment order should be marked paid");
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

fn assert_wallet_data_file_name(file_name: &str) {
    assert!(
        file_name.starts_with("bitgarth-walletdata-"),
        "wallet-data export filename should start with expected prefix, got: {file_name}"
    );
    assert!(
        file_name.ends_with(".zip"),
        "wallet-data export filename should end with .zip, got: {file_name}"
    );
}

fn read_zip_entry(
    zip_bytes: &[u8],
    entry_name: &str,
    password: Option<&str>,
) -> Result<String, String> {
    let cursor = Cursor::new(zip_bytes.to_vec());
    let mut archive = zip::ZipArchive::new(cursor).map_err(|err| format!("open zip: {err}"))?;
    let mut entry = match password {
        Some(password) => archive
            .by_name_decrypt(entry_name, password.as_bytes())
            .map_err(|err| format!("decrypt entry {entry_name}: {err}"))?,
        None => archive
            .by_name(entry_name)
            .map_err(|err| format!("open entry {entry_name}: {err}"))?,
    };
    let mut contents = String::new();
    entry
        .read_to_string(&mut contents)
        .map_err(|err| format!("read entry {entry_name}: {err}"))?;
    Ok(contents)
}

fn list_zip_entries(zip_bytes: &[u8]) -> Vec<String> {
    let cursor = Cursor::new(zip_bytes.to_vec());
    let mut archive = zip::ZipArchive::new(cursor).expect("zip should open");
    (0..archive.len())
        .map(|index| {
            archive
                .by_index_raw(index)
                .expect("zip entry should exist")
                .name()
                .to_string()
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn test_download_hledger_happy_path_unencrypted() {
    let server = setup_test_server();
    let username = register_user(&server).await;
    let wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Main Wallet",
    )
    .await;
    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let custom_account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be a string");
    add_custom_balance_assertion(
        &server,
        custom_account_id,
        "2026-02-10",
        "1.25",
        Some(" corrected;\nmanual snapshot "),
    )
    .await;
    add_custom_balance_assertion(&server, custom_account_id, "2026-02-20", "2.00", None).await;

    let response = server
        .post("/_app/user/exports/hledger/download")
        .json(&json!({ "encrypted": false }))
        .await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header should be present")
        .to_str()
        .expect("content-type should be ascii");
    assert_eq!(content_type, "application/zip");

    let disposition = response
        .headers()
        .get("content-disposition")
        .expect("content-disposition header should be present")
        .to_str()
        .expect("content-disposition should be ascii");
    assert!(
        disposition.starts_with("attachment;"),
        "content-disposition should be attachment, got: {disposition}"
    );
    assert!(
        disposition.contains("bitgarth-hledger-"),
        "content-disposition should reference hledger filename, got: {disposition}"
    );
    assert!(
        disposition.contains(&username),
        "content-disposition should contain username segment, got: {disposition}"
    );
    assert!(
        disposition.ends_with(".zip\""),
        "content-disposition filename should end with .zip, got: {disposition}"
    );

    let zip_bytes = response.into_bytes();
    let entries = list_zip_entries(&zip_bytes);
    assert!(
        entries.iter().any(|name| name == "directives.j.txt"),
        "zip should contain directives.j.txt, got entries: {entries:?}"
    );
    assert!(
        entries.iter().any(|name| name == "bitgarth.j.txt"),
        "zip should contain bitgarth.j.txt, got entries: {entries:?}"
    );
    let expected_journal = format!("{username}/MainWallet/ADAAccount1/journal/2026/2026.j.txt");
    assert!(
        entries.iter().any(|name| name == &expected_journal),
        "zip should contain {expected_journal}, got entries: {entries:?}"
    );

    let directives = read_zip_entry(&zip_bytes, "directives.j.txt", None).expect("read directives");
    assert!(directives.contains("commodity 0.000000 ADA"));

    let root_entry = read_zip_entry(&zip_bytes, "bitgarth.j.txt", None).expect("read root entry");
    assert_eq!(
        root_entry,
        "; Generated by https://bitgarth.app/\n\ninclude directives.j.txt\ninclude all-years.j.txt\n"
    );

    let root_all_years =
        read_zip_entry(&zip_bytes, "all-years.j.txt", None).expect("read root all-years");
    assert!(root_all_years.starts_with("; Generated by https://bitgarth.app/"));
    assert!(root_all_years.contains("include 2026-include.j.txt"));

    let root_year_include =
        read_zip_entry(&zip_bytes, "2026-include.j.txt", None).expect("read root include");
    assert!(root_year_include.contains(&format!("include {username}/2026-include.j.txt")));

    let user_year_include =
        read_zip_entry(&zip_bytes, &format!("{username}/2026-include.j.txt"), None)
            .expect("read user include");
    assert!(user_year_include.contains("include MainWallet/2026-include.j.txt"));

    let wallet_year_include = read_zip_entry(
        &zip_bytes,
        &format!("{username}/MainWallet/2026-include.j.txt"),
        None,
    )
    .expect("read wallet include");
    assert!(wallet_year_include.contains("include ADAAccount1/2026-include.j.txt"));

    let custom_journal =
        read_zip_entry(&zip_bytes, &expected_journal, None).expect("read custom journal");
    assert!(custom_journal.contains("Balance Assertion: corrected, manual snapshot"));
    assert!(custom_journal.contains("= 1.250000 ADA"));
    assert!(custom_journal.contains("= 2.000000 ADA"));

    let include_path = format!("{username}/MainWallet/ADAAccount1/2026-include.j.txt");
    let include_contents = read_zip_entry(&zip_bytes, &include_path, None).expect("read include");
    assert!(!include_contents.contains("include 2026-opening.j.txt"));
    assert!(!include_contents.contains("include 2026-closing.j.txt"));
    assert!(include_contents.contains("include journal/2026/2026.j.txt"));
}

#[tokio::test(flavor = "current_thread")]
async fn test_download_hledger_happy_path_encrypted() {
    let server = setup_test_server();
    let username = register_user(&server).await;
    let wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Main Wallet",
    )
    .await;
    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let custom_account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be a string");
    add_custom_balance_assertion(&server, custom_account_id, "2026-02-15", "3.50", None).await;

    let password = "the-correct-horse-battery-staple-passphrase";
    let response = server
        .post("/_app/user/exports/hledger/download")
        .json(&json!({ "encrypted": true, "password": password }))
        .await;
    response.assert_status_ok();
    let zip_bytes = response.into_bytes();

    let directives =
        read_zip_entry(&zip_bytes, "directives.j.txt", Some(password)).expect("decrypt directives");
    assert!(directives.contains("commodity 0.000000 ADA"));

    let journal_path = format!("{username}/MainWallet/ADAAccount1/journal/2026/2026.j.txt");
    let journal =
        read_zip_entry(&zip_bytes, &journal_path, Some(password)).expect("decrypt custom journal");
    assert!(journal.contains("Balance Assertion"));

    let wrong = read_zip_entry(&zip_bytes, &journal_path, Some("wrong-password"));
    assert!(
        wrong.is_err(),
        "decrypting with the wrong password should fail"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_download_hledger_encrypted_without_password_returns_422() {
    let server = setup_test_server();
    let _ = register_user(&server).await;

    let response = server
        .post("/_app/user/exports/hledger/download")
        .json(&json!({ "encrypted": true, "password": "" }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "missing password on encrypted download should be 422"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_download_hledger_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/exports/hledger/download")
        .json(&json!({ "encrypted": false }))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_download_hledger_zero_data_user() {
    let server = setup_test_server();
    let _ = register_user(&server).await;

    let response = server
        .post("/_app/user/exports/hledger/download")
        .json(&json!({ "encrypted": false }))
        .await;
    response.assert_status_ok();
    let zip_bytes = response.into_bytes();
    let entries = list_zip_entries(&zip_bytes);
    assert_eq!(
        entries,
        vec![
            "directives.j.txt".to_string(),
            "all-years.j.txt".to_string(),
            "bitgarth.j.txt".to_string()
        ],
        "zero-data user should produce directives, root all-years, and root entry, got: {entries:?}"
    );
    let directives = read_zip_entry(&zip_bytes, "directives.j.txt", None).expect("read directives");
    assert!(directives.starts_with("; Generated by https://bitgarth.app/"));
    assert!(
        !directives.contains("commodity"),
        "zero-data directives.j.txt should not declare any commodities, got: {directives:?}"
    );
    let all_years = read_zip_entry(&zip_bytes, "all-years.j.txt", None).expect("read all-years");
    assert_eq!(all_years, "; Generated by https://bitgarth.app/\n\n");
    let root_entry = read_zip_entry(&zip_bytes, "bitgarth.j.txt", None).expect("read root entry");
    assert_eq!(
        root_entry,
        "; Generated by https://bitgarth.app/\n\ninclude directives.j.txt\ninclude all-years.j.txt\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_export_wallet_data_happy_path_filters_derived_and_returns_summary() {
    let server = setup_test_server();
    let wallet = {
        register_user(&server).await;
        add_ethereum_wallet_account(
            &server,
            "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
            "Main Wallet",
        )
        .await
    };
    let xpub = deterministic_test_xpub(0);
    add_xpub_wallet_account(&server, &xpub, "legacy", Some(&wallet.wallet_id), None).await;

    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let custom_account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be a string");
    add_custom_balance_assertion(
        &server,
        custom_account_id,
        "2026-03-01",
        "15000.000000",
        Some("Post-epoch snapshot"),
    )
    .await;

    let response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(false, false, None))
        .await;
    response.assert_status_ok();

    let payload: Value = response.json();
    let file_name = payload["file_name"]
        .as_str()
        .expect("file_name should be a string");
    assert_wallet_data_file_name(file_name);

    assert_eq!(payload["summary"]["wallets"], 1);
    assert_eq!(payload["summary"]["native_accounts"], 2);
    assert_eq!(payload["summary"]["addresses"], 1);
    assert_eq!(payload["summary"]["custom_accounts"], 1);
    assert_eq!(payload["summary"]["balance_assertions"], 1);
    assert_eq!(payload["summary"]["api_keys"], 0);
    assert_eq!(payload["summary"]["premium_transfer_exported"], false);
    assert_eq!(payload["summary"]["encrypted"], false);
    let export_payload = wallet_data_payload_from_zip(
        payload["zip_base64"]
            .as_str()
            .expect("zip_base64 should be a string"),
        None,
    );
    assert_eq!(export_payload["version"], 5);
    assert!(
        export_payload["settings"]
            .get("etherscan_api_key")
            .is_none()
    );
    assert!(export_payload.get("api_keys").is_some());
    assert!(export_payload.get("premium_transfer").is_none());
    assert!(export_payload.get("subscription_transfer").is_none());

    let wallet_rows = export_payload["wallets"]
        .as_array()
        .expect("wallet export rows should be an array");
    assert_eq!(wallet_rows.len(), 1);

    let native_accounts = wallet_rows[0]["digital_asset_accounts"]
        .as_array()
        .expect("digital_asset_accounts should be an array");
    assert_eq!(native_accounts.len(), 2);

    let hd_account = native_accounts
        .iter()
        .find(|row| row["account_kind"] == "hd_pubkey")
        .expect("one hd_pubkey account should be present");
    let hd_addresses = hd_account["addresses"]
        .as_array()
        .expect("hd account addresses should be an array");
    assert!(
        hd_addresses.is_empty(),
        "derived addresses should not be exported for hd accounts"
    );

    let custom_accounts = wallet_rows[0]["manual_asset_accounts"]
        .as_array()
        .expect("manual_asset_accounts should be an array");
    assert_eq!(custom_accounts.len(), 1);
    let assertions = custom_accounts[0]["balance_assertions"]
        .as_array()
        .expect("balance_assertions should be an array");
    assert_eq!(assertions.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn test_export_wallet_data_with_premium_opt_in_includes_transfer_without_order_secret() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;
    seed_premium_transfer_state(user_id);

    let response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(true, true, Some("weak")))
        .await;
    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["summary"]["premium_transfer_exported"], true);
    assert_eq!(payload["summary"]["encrypted"], true);
    let export_payload = wallet_data_payload_from_zip(
        payload["zip_base64"]
            .as_str()
            .expect("zip_base64 should be a string"),
        Some("weak"),
    );
    assert_eq!(export_payload["version"], 5);
    assert!(export_payload.get("premium_transfer").is_none());
    assert_eq!(
        export_payload["subscription_transfer"]["management_secret"],
        PAYMENT_MANAGEMENT_SECRET
    );
    assert_eq!(
        export_payload["subscription_transfer"]["orders"][0]["order_id"],
        PAYMENT_ORDER_ID
    );

    let serialized = payload.to_string();
    assert!(!serialized.contains(PAYMENT_ORDER_SECRET));
}

#[tokio::test(flavor = "current_thread")]
async fn test_export_wallet_data_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(false, false, None))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_export_wallet_data_encrypted_without_password_returns_validation() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(false, true, None))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_export_wallet_data_unencrypted_with_password_returns_validation() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(
            false,
            false,
            Some("unused-password"),
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_happy_path_into_empty_account() {
    let server = setup_test_server();
    let source_wallet = {
        register_user(&server).await;
        add_ethereum_wallet_account(
            &server,
            "0x52908400098527886E0F7030069857D2E4169EE7",
            "Source Wallet",
        )
        .await
    };
    let xpub = deterministic_test_xpub(0);
    add_xpub_wallet_account(
        &server,
        &xpub,
        "legacy",
        Some(&source_wallet.wallet_id),
        None,
    )
    .await;

    let export_response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(false, false, None))
        .await;
    export_response.assert_status_ok();
    let export_payload: Value = export_response.json();
    let zip_base64 = export_payload["zip_base64"]
        .as_str()
        .expect("zip_base64 should be a string")
        .to_string();
    let file_name = export_payload["file_name"]
        .as_str()
        .expect("file_name should be a string")
        .to_string();

    register_user(&server).await;
    let target_user_id = current_user_id(&server).await;

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body_from_zip(&file_name, &zip_base64, None))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["wallets_created"], json!(["Source Wallet"]));
    assert_eq!(
        body["native_accounts_created"][0]["wallet_label"],
        "Source Wallet"
    );
    assert_eq!(body["sync_scope"], "user");
    assert_eq!(body["sync_triggered"], true);
    assert_eq!(body["premium_transfer_status"], "not_present");
    assert!(body["pending_premium_transfer_id"].is_null());
    assert_imported_xpub_bootstrap_state(target_user_id);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_v3_with_premium_transfer_creates_pending_confirmation() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;

    let payload_json = serde_json::to_string_pretty(&json!({
        "version": 3,
        "exported_at": "2026-04-25T12:00:00Z",
        "bitgarth_version": "0.1.0",
        "wallets": [],
        "settings": {
            "etherscan_api_key": "v3-etherscan-key"
        },
        "premium_transfer": {
            "exported_at": "2026-04-25T12:00:00Z",
            "management_secret": PAYMENT_MANAGEMENT_SECRET,
            "active_token": null,
            "token_id": null,
            "subscription_subject_id": null,
            "subscription_valid_until": null,
            "token_expires_at": null,
            "token_issued_at": null,
            "orders": [{
                "order_id": PAYMENT_ORDER_ID,
                "product_tier": "premium",
                "order_amount_minor_units": 999,
                "order_currency": "USD",
                "order_display_scale": 2,
                "status": "paid",
                "paid_at": "2026-04-25T12:00:00Z"
            }]
        }
    }))
    .expect("payload should serialize");

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body(
            "premium-wallet-data.zip",
            &payload_json,
            None,
        ))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["premium_transfer_status"], "pending_confirmation");
    assert!(
        body["pending_premium_transfer_id"].as_str().is_some(),
        "pending transfer id should be returned"
    );
    assert_eq!(body["api_keys_imported"], 1);
    assert_eq!(
        load_api_key_for_user(user_id, ApiKeyProvider::Etherscan),
        Some("v3-etherscan-key".to_string())
    );

    let history = crate::db::payments::load_all_payment_order_history(user_id)
        .expect("payment history should load");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].order_id.to_storage_value(), PAYMENT_ORDER_ID);
    assert_eq!(history[0].status, PaymentOrderStatus::Paid);
    assert_eq!(history[0].amount.atlos_decimal_amount(), "9.99");
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_imports_missing_api_keys_without_overwriting_existing() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;
    save_api_key_for_user(user_id, ApiKeyProvider::Etherscan, "local-etherscan");

    let payload_json = wallet_data_v4_payload_json_with_api_keys(&[
        ("etherscan", "backup-etherscan"),
        ("coingecko", "backup-coingecko"),
    ]);

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body(
            "wallet-data.json",
            &payload_json,
            None,
        ))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["api_keys_imported"], 1);
    assert_eq!(body["api_keys_skipped_already_present"], 1);
    assert_eq!(
        load_api_key_for_user(user_id, ApiKeyProvider::Etherscan),
        Some("local-etherscan".to_string())
    );
    assert_eq!(
        load_api_key_for_user(user_id, ApiKeyProvider::CoinGecko),
        Some("backup-coingecko".to_string())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_confirm_premium_transfer_success_activates_local_premium() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;
    let pending_transfer_id = import_pending_premium_transfer(&server).await;
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    let _central_guard = central_guard(&mock);

    let response = server
        .post("/_app/user/imports/wallet-data/premium-transfer")
        .json(&confirm_premium_transfer_request(&pending_transfer_id))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["status"], "active");
    assert!(body["paid_through"].as_str().is_some());
    assert!(body["offline_access_until"].as_str().is_some());

    let subject = crate::db::payments::load_payment_subject(user_id)
        .expect("payment subject should load")
        .expect("payment subject should exist");
    let management_secret = subject
        .management_secret
        .expect("transferred management secret should be stored");
    assert_ne!(management_secret.as_str(), PAYMENT_MANAGEMENT_SECRET);
    assert!(subject.active_token_history_id.is_some());
    let history = crate::db::payments::load_active_token_history(user_id)
        .expect("token history should load")
        .expect("token history should exist");
    assert_eq!(history.token_id.to_storage_value(), TRANSFER_TOKEN_ID);

    let pending = crate::db::payments::load_pending_premium_transfer(user_id, &pending_transfer_id)
        .expect("pending transfer should load")
        .expect("pending transfer should exist");
    assert_eq!(pending.status, "completed");

    let export_response = server
        .post("/_app/user/exports/wallet-data")
        .json(&wallet_data_export_request(true, false, None))
        .await;
    export_response.assert_status_ok();
    let export_payload: Value = export_response.json();
    let wallet_data = wallet_data_payload_from_zip(
        export_payload["zip_base64"]
            .as_str()
            .expect("zip_base64 should be a string"),
        None,
    );
    assert_eq!(
        wallet_data["subscription_transfer"]["orders"][0]["order_id"],
        PAYMENT_ORDER_ID
    );
    assert_eq!(
        wallet_data["subscription_transfer"]["orders"][0]["status"],
        "paid"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_confirm_premium_transfer_service_error_stays_retryable() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;
    let pending_transfer_id = import_pending_premium_transfer(&server).await;
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_transfer_outcome(MockTransferOutcome::ServiceUnavailable);
    let _central_guard = central_guard(&mock);

    let response = server
        .post("/_app/user/imports/wallet-data/premium-transfer")
        .json(&confirm_premium_transfer_request(&pending_transfer_id))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["status"], "retryable_failure");
    let pending = crate::db::payments::load_pending_premium_transfer(user_id, &pending_transfer_id)
        .expect("pending transfer should load")
        .expect("pending transfer should exist");
    assert_eq!(pending.status, "retryable_failure");
}

#[tokio::test(flavor = "current_thread")]
async fn test_confirm_premium_transfer_invalid_secret_is_non_retryable() {
    let server = setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;
    let pending_transfer_id = import_pending_premium_transfer(&server).await;
    let _key_guard = set_signing_public_key_override_for_test(TEST_PUBLIC_KEY_B64);
    let mock = MockCentral::start(expected_signing_key_hash().expect("hash should derive")).await;
    mock.set_transfer_outcome(MockTransferOutcome::InvalidManagementSecret);
    let _central_guard = central_guard(&mock);

    let response = server
        .post("/_app/user/imports/wallet-data/premium-transfer")
        .json(&confirm_premium_transfer_request(&pending_transfer_id))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["status"], "non_retryable_failure");
    let pending = crate::db::payments::load_pending_premium_transfer(user_id, &pending_transfer_id)
        .expect("pending transfer should load")
        .expect("pending transfer should exist");
    assert_eq!(pending.status, "non_retryable_failure");
}

#[tokio::test(flavor = "current_thread")]
async fn test_confirm_premium_transfer_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/imports/wallet-data/premium-transfer")
        .json(&confirm_premium_transfer_request(
            "01JQABCDEF000000000000000H",
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_v3_invalid_premium_transfer_does_not_fail_wallet_import() {
    let server = setup_test_server();
    register_user(&server).await;

    let payload_json = serde_json::to_string_pretty(&json!({
        "version": 3,
        "exported_at": "2026-04-25T12:00:00Z",
        "bitgarth_version": "0.1.0",
        "wallets": [],
        "premium_transfer": {
            "management_secret": "not-a-valid-secret"
        }
    }))
    .expect("payload should serialize");

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body(
            "invalid-premium-wallet-data.zip",
            &payload_json,
            None,
        ))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(body["premium_transfer_status"], "invalid_metadata");
    assert!(body["pending_premium_transfer_id"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_unknown_version_returns_validation() {
    let server = setup_test_server();
    register_user(&server).await;

    let payload_json = serde_json::to_string_pretty(&json!({
        "version": 99,
        "exported_at": "2026-04-04T12:00:00Z",
        "bitgarth_version": "0.1.0",
        "wallets": []
    }))
    .expect("payload should serialize");

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body("new-version.zip", &payload_json, None))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_zip_payload_json_parse_failure_returns_bad_request() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body("bad.zip", "{ not valid json", None))
        .await;
    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_encrypted_without_password_returns_password_required() {
    let server = setup_test_server();
    register_user(&server).await;

    let payload_json = r#"{"version":3,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
    let zip_base64 = zip_base64_for_payload_json(payload_json, Some("correct-password"));
    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body_from_zip(
            "wallet-data.zip",
            &zip_base64,
            None,
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_encrypted_with_wrong_password_returns_auth_failed() {
    let server = setup_test_server();
    register_user(&server).await;

    let payload_json = r#"{"version":3,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
    let zip_base64 = zip_base64_for_payload_json(payload_json, Some("correct-password"));
    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body_from_zip(
            "wallet-data.zip",
            &zip_base64,
            Some("wrong-password"),
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_accepts_raw_json_file() {
    let server = setup_test_server();
    register_user(&server).await;

    let payload_json = r#"{"version":4,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[],"api_keys":[]}"#;
    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_raw_json_request_body(
            "wallet-data.json",
            payload_json,
        ))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["premium_transfer_status"], "not_present");
    assert_eq!(body["api_keys_imported"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/imports/wallet-data")
        .json(&import_request_body(
            "wallet-data.zip",
            "{\"version\":1,\"exported_at\":\"2026-04-04T12:00:00Z\",\"bitgarth_version\":\"0.1.0\",\"wallets\":[]}",
            None,
        ))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_import_wallet_data_rejects_malformed_json_request_with_bad_request() {
    let server = setup_test_server_no_db();
    assert_malformed_json_returns_bad_request(&server, "/_app/user/imports/wallet-data").await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_describe_wallet_data_rejects_malformed_json_request_with_bad_request() {
    let server = setup_test_server_no_db();
    assert_malformed_json_returns_bad_request(&server, "/_app/user/imports/wallet-data/describe")
        .await;
}

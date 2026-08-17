//! Integration tests for wallets endpoint contracts.
//!
//! Scope:
//! - Representative wallet route-family contract coverage
//! - Minimal auth, validation, and malformed-input contract checks

use crate::amounts::{UnsignedAmount, global_split_config};
use crate::backend::ApiErrorEnvelope;
use crate::models::{CurrencyCode, UserId};
use dioxus::fullstack::StatusCode;
use rusqlite::{OpenFlags, params};
use serde_json::{Value, json};
use std::str::FromStr;
use ulid::Ulid;

use super::fixtures::{
    TEST_NATIVE_SEGWIT_ZPUB, activate_signed_full_report_entitlements, add_ethereum_wallet_account,
    add_native_segwit_xpub_account, add_xpub_wallet_account, register_user,
};
use super::{IntegrationTestServer, setup_test_server_no_db};

fn parse_error_envelope(body: Value) -> ApiErrorEnvelope {
    serde_json::from_value(body["data"].clone())
        .expect("expected standardized ApiErrorEnvelope in response body")
}

async fn current_user_id(server: &IntegrationTestServer) -> UserId {
    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();

    let body: Value = response.json();
    let user_id = body["user"]["user_id"]
        .as_str()
        .expect("auth me should include user_id");
    UserId::from_str(user_id).expect("auth me should return a valid user id")
}

/// Builds the `asset_instance_id` payload for the manual asset endpoint.
fn asset_instance_id_for_unit_code(unit_code: &str) -> Value {
    match unit_code {
        "ADA" => json!({
            "asset_id": "cardano",
            "network_id": "cardano-mainnet"
        }),
        "BTC" => json!({
            "asset_id": "bitcoin",
            "network_id": "bitcoin-mainnet"
        }),
        other => panic!("unsupported unit_code in wallets tests: {other}"),
    }
}

async fn add_manual_asset_account(
    server: &IntegrationTestServer,
    wallet_id: Option<&str>,
    wallet_label: Option<&str>,
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
    if let Some(wallet_label) = wallet_label {
        request.insert(
            "wallet_label".to_string(),
            Value::String(wallet_label.to_string()),
        );
    }

    let response = server
        .post("/_app/user/wallets/manual-assets/add")
        .json(&json!({ "request": Value::Object(request) }))
        .await;
    response.assert_status_ok();
    response.json()
}

fn open_test_user_db(server: &IntegrationTestServer, user_id: UserId) -> rusqlite::Connection {
    let db_path = server.user_database_path(user_id);
    let connection =
        rusqlite::Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .expect("test user db should open");
    let dek = crate::db::get_user_db_dek(&user_id)
        .expect("test user db should resolve DEK")
        .expect("test user db should be encrypted");
    let sqlcipher_compatibility = crate::db::encryption::read_envelope(user_id)
        .expect("test user db should read envelope")
        .sqlcipher_compatibility()
        .expect("test user db should expose SQLCipher compatibility");
    connection
        .execute_batch(&format!("PRAGMA key = \"x'{}'\"", dek.as_hex()))
        .expect("test user db should set SQLCipher key");
    connection
        .pragma_update(
            None,
            "cipher_compatibility",
            sqlcipher_compatibility.as_u32().to_string(),
        )
        .expect("test user db should set SQLCipher compatibility");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("foreign keys should enable");
    connection
}

struct WalletReportEthLedgerRowFixture<'a> {
    account_id: &'a str,
    tx_hash: &'a str,
    block_height: i64,
    occurred_at: &'a str,
    tx_type: &'a str,
    from_addresses_json: &'a str,
    to_addresses_json: &'a str,
    value_amount: UnsignedAmount,
    closing_balance: UnsignedAmount,
}

fn seed_wallet_report_eth_ledger_row(
    tx: &rusqlite::Transaction<'_>,
    row: WalletReportEthLedgerRowFixture<'_>,
) {
    let chain_transaction_id = Ulid::new().to_string();
    let value_amount = global_split_config()
        .encode_unsigned(row.value_amount)
        .expect("wallet report fixture value should encode");
    let closing_balance = global_split_config()
        .encode_unsigned(row.closing_balance)
        .expect("wallet report fixture balance should encode");

    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            chain_transaction_id,
            "ethereum",
            "mainnet",
            row.tx_hash,
            "confirmed",
            row.block_height,
            format!("blockhash-{}", row.block_height),
            row.occurred_at,
            Option::<i64>::None,
            Option::<i64>::None,
            Option::<i64>::None,
            row.occurred_at,
            row.occurred_at,
        ],
    )
    .expect("wallet report chain transaction fixture should insert");

    tx.execute(
        "INSERT INTO account_transaction_ledger
         (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status, occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type, from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo, fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            Ulid::new().to_string(),
            row.account_id,
            chain_transaction_id,
            "ethereum",
            "mainnet",
            row.tx_hash,
            "confirmed",
            row.occurred_at,
            row.occurred_at,
            row.block_height,
            Option::<i64>::None,
            Option::<i64>::None,
            row.tx_type,
            row.from_addresses_json,
            row.to_addresses_json,
            value_amount.hi,
            value_amount.lo,
            Option::<i64>::None,
            Option::<i64>::None,
            closing_balance.hi,
            closing_balance.lo,
            row.occurred_at,
            row.occurred_at,
        ],
    )
    .expect("wallet report ledger fixture should insert");
}

fn seed_wallet_report_prehistory_fixture(
    server: &IntegrationTestServer,
    user_id: UserId,
    account_id: &str,
    owned_address: &str,
) {
    let mut conn = open_test_user_db(server, user_id);
    let tx = conn.transaction().expect("seed transaction should start");

    seed_wallet_report_eth_ledger_row(
        &tx,
        WalletReportEthLedgerRowFixture {
            account_id,
            tx_hash: "1111111111111111111111111111111111111111111111111111111111111111",
            block_height: 1,
            occurred_at: "2025-01-08T12:00:00Z",
            tx_type: "receive",
            from_addresses_json: "[\"0x0000000000000000000000000000000000000001\"]",
            to_addresses_json: &format!("[\"{owned_address}\"]"),
            value_amount: UnsignedAmount::from_u128(500_000_000_000_000_000_u128),
            closing_balance: UnsignedAmount::from_u128(500_000_000_000_000_000_u128),
        },
    );

    tx.commit().expect("seed transaction should commit");
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
async fn test_wallets_body_endpoints_reject_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();

    for path in [
        "/_app/user/wallets/by-fingerprint",
        "/_app/user/wallets/trezor/link",
        "/_app/user/wallets/label",
        "/_app/user/wallets/account/label",
        "/_app/user/wallets/account/addresses",
        "/_app/user/wallets/account/delete",
        "/_app/user/wallets/account/move",
        "/_app/user/wallets/delete",
        "/_app/user/wallets/xpub/validate",
        "/_app/user/wallets/xpub/add",
        "/_app/user/wallets/manual-assets/add",
        "/_app/user/wallets/ethereum/add",
        "/_app/user/wallets/bitcoin/add",
    ] {
        assert_malformed_json_returns_bad_request(&server, path).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_generate_addresses_endpoint_removed_returns_not_found() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/wallets/account/addresses/generate")
        .json(&json!({
            "account_id": Ulid::new().to_string(),
            "address_branch": "receive",
            "address_scheme": "native_segwit"
        }))
        .await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallets_happy_path_returns_balances_contract() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let add_response = server
        .post("/_app/user/wallets/ethereum/add")
        .json(&json!({
            "request": {
                "address": "0x52908400098527886E0F7030069857D2E4169EE7",
                "network": "mainnet",
                "wallet_label": "Integration ETH Wallet"
            }
        }))
        .await;
    add_response.assert_status_ok();

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert!(body.get("totals").is_none(), "totals key should be absent");

    let wallets = body["wallets"]
        .as_array()
        .expect("wallets should be an array");
    assert_eq!(wallets.len(), 1);

    let wallet = &wallets[0];
    assert!(wallet.get("wallet_kind").is_none());
    let balances = wallet["balances"]
        .as_array()
        .expect("balances should be an array");
    assert_eq!(balances.len(), 1);

    let balance = &balances[0];
    assert_eq!(balance["asset_id"], "ethereum");
    assert_eq!(balance["network_id"], "ethereum-mainnet");
    assert_eq!(balance["unit_code"], "ETH");
    assert_eq!(balance["symbol"], "Ξ");
    assert_eq!(
        balance["balance_state"],
        json!({
            "kind": "known",
            "amount": {
                "raw_value": "0",
                "formatted_value": "0"
            }
        })
    );
    assert_eq!(
        balance["balance_reliability"],
        json!({
            "kind": "provisional",
            "reasons": ["first_successful_sync_pending"]
        })
    );

    let accounts = wallet["accounts"]
        .as_array()
        .expect("accounts should be an array");
    assert_eq!(accounts.len(), 1);

    let account = &accounts[0];
    assert_eq!(account["kind"], "native");
    assert_eq!(account["asset"], "ethereum");
    assert_eq!(account["scheme"], "standard");
    assert_eq!(account["account_reference_kind"], "single_address");
    assert_eq!(
        account["account_reference"],
        "0x52908400098527886E0F7030069857D2E4169EE7"
    );
    assert_eq!(account["derivation_path"], Value::Null);
    assert_eq!(account["transaction_counts"]["pending"], 0);
    assert_eq!(account["transaction_counts"]["confirmed"], 0);
    assert_eq!(account["transaction_counts"]["dropped"], 0);
    assert_eq!(account["transaction_counts"]["failed"], 0);
    assert_eq!(account["transaction_counts"]["total"], 0);
    assert!(account.get("addresses").is_none());
    assert!(account.get("transactions").is_none());

    let account_balance = &account["balance"];
    assert_eq!(account_balance["asset_id"], "ethereum");
    assert_eq!(account_balance["context"]["network"], "mainnet");
    assert_eq!(account_balance["unit_code"], "ETH");
    assert_eq!(account_balance["symbol"], "Ξ");
    assert_eq!(
        account_balance["balance_state"],
        json!({
            "kind": "known",
            "amount": {
                "raw_value": "0",
                "formatted_value": "0"
            }
        })
    );
    assert_eq!(
        account_balance["balance_reliability"],
        json!({
            "kind": "provisional",
            "reasons": ["first_successful_sync_pending"]
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn wallets_summary_uses_cached_free_tier_account_limit() {
    let server = super::setup_test_server();
    register_user(&server).await;

    crate::db::upsert_free_tier_entitlement_cache(
        &crate::payments::free_tier::FreeTierObservation {
            observed_at: crate::payments::free_tier::baked_free_tier_snapshot().captured_at
                + chrono::TimeDelta::seconds(1),
            capability_schema_version: crate::payments::types::CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: crate::payments::free_tier::free_tier_capabilities_for_test(22),
        },
    )
    .expect("free tier cache should seed");

    add_ethereum_wallet_account(
        &server,
        "0x0000000000000000000000000000000000000001",
        "Ethereum Wallet",
    )
    .await;

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["account_limit"]["active_limit"], json!(22));
    assert_eq!(
        body["account_limit"]["summary"],
        json!("1 of 22 active accounts used")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn wallets_summary_uses_baked_free_tier_when_cache_empty() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["account_limit"]["active_limit"], json!(50));
    assert_eq!(
        body["account_limit"]["summary"],
        json!("0 of 50 active accounts used")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallets_includes_value_summary_when_price_fetching_enabled() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let user_id = current_user_id(&server).await;

    add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Valued ETH Wallet",
    )
    .await;
    crate::db::set_price_fetching_enabled(user_id, true).expect("price fetching flag should save");
    crate::services::current_prices::reset_cache_for_test();
    crate::services::current_prices::seed_price_for_test(
        "ethereum",
        CurrencyCode::from_code("USD").unwrap(),
        "1000",
    );

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();
    let body: Value = response.json();

    let summary = body["value_summary"]
        .as_object()
        .expect("summary should be present");
    assert_eq!(summary["total_asset_count"], 1);
    assert_eq!(summary["priced_asset_count"], 1);
    assert_eq!(summary["priced_total"], "0");
    assert_eq!(summary["currency"], "USD");

    let current_value = &body["wallets"][0]["balances"][0]["current_value"];
    assert_eq!(current_value["price"], "1000");
    assert_eq!(current_value["converted_value"], "0");
    assert_eq!(current_value["currency"], "USD");
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallets_has_no_value_summary_when_price_fetching_disabled() {
    let server = super::setup_test_server();
    register_user(&server).await;
    add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Unvalued ETH Wallet",
    )
    .await;

    let response = server.get("/_app/user/wallets").await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert!(body["value_summary"].is_null());
    assert!(body["wallets"][0]["balances"][0]["current_value"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallet_report_happy_path_returns_account_rows() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let _entitlement_guard = activate_signed_full_report_entitlements(&server).await;
    let user_id = current_user_id(&server).await;

    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Integration Report Wallet",
    )
    .await;
    seed_wallet_report_prehistory_fixture(
        &server,
        user_id,
        &account.account_id,
        "0x52908400098527886E0F7030069857D2E4169EE7",
    );

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report?from=2024-01-01&to=2025-12-31&timezone=UTC",
            account.wallet_id
        ))
        .await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["wallet_label"], "Integration Report Wallet");
    assert_eq!(body["resolved_from"], "2024-01-01");
    assert_eq!(body["resolved_to"], "2025-12-31");

    let rows = body["accounts"]
        .as_array()
        .expect("accounts should be an array");
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    assert_eq!(row["account_id"], account.account_id);
    assert_eq!(row["catalog_asset_key"], "ethereum");
    assert_eq!(row["asset_display_name"], "Ethereum");
    assert_eq!(row["unit_code"], "ETH");
    assert_eq!(row["symbol"], "Ξ");
    assert_eq!(row["opening_balance_state"]["kind"], "canonical_zero");
    assert_eq!(row["closing_balance_state"]["kind"], "needs_price");
    assert!(row["opening_balance"].is_null());
    assert_eq!(row["opening_balance_date"], "2024-01-01");
    assert_eq!(row["closing_balance"]["raw_value"], "500000000000000000");
    assert_eq!(row["closing_balance"]["formatted_value"], "0.5");
    assert_eq!(row["closing_balance_date"], "2025-12-31");
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallet_report_wallet_not_found_returns_not_found() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report?from=2026-01-01&to=2026-03-31&timezone=UTC",
            Ulid::new()
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallet_report_validation_error_returns_unprocessable_entity() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Validation Report Wallet",
    )
    .await;

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report?from=2026-04-01&to=2026-03-01&timezone=UTC",
            account.wallet_id
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_validation());
    assert!(error.first_field_error("to").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_wallet_report_malformed_date_returns_bad_request() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Malformed Report Wallet",
    )
    .await;

    let response = server
        .get(&format!(
            "/_app/user/wallets/{}/report?from=not-a-date&to=2026-03-31&timezone=UTC",
            account.wallet_id
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_ethereum_address_requires_wallet_label_for_new_wallet() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/wallets/ethereum/add")
        .json(&json!({
            "request": {
                "address": "0x52908400098527886E0F7030069857D2E4169EE7",
                "network": "mainnet"
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_validation());
    assert!(error.first_field_error("wallet_label").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_xpub_requires_wallet_label_for_new_wallet() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/wallets/xpub/add")
        .json(&json!({
            "request": {
                "extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB,
                "address_scheme": "native_segwit"
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_validation());
    assert!(error.first_field_error("wallet_label").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_validate_xpub_returns_mixed_scheme_link_state_for_same_normalized_key() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let added = add_xpub_wallet_account(
        &server,
        TEST_NATIVE_SEGWIT_ZPUB,
        "legacy",
        None,
        Some("Mixed Scheme Wallet"),
    )
    .await;

    let response = server
        .post("/_app/user/wallets/xpub/validate")
        .json(&json!({
            "request": {
                "extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB
            }
        }))
        .await;
    response.assert_status_ok();

    let body: Value = response.json();
    let schemes = body["schemes"]
        .as_array()
        .expect("schemes should be an array");

    let legacy = schemes
        .iter()
        .find(|scheme| scheme["address_scheme"] == "legacy")
        .expect("legacy scheme should be present");
    let nested = schemes
        .iter()
        .find(|scheme| scheme["address_scheme"] == "nested_segwit")
        .expect("nested segwit scheme should be present");
    let native = schemes
        .iter()
        .find(|scheme| scheme["address_scheme"] == "native_segwit")
        .expect("native segwit scheme should be present");

    assert_eq!(legacy["already_linked"], true);
    assert_eq!(legacy["linked_wallet_label"], "Mixed Scheme Wallet");
    assert!(
        legacy["linked_account_label"]
            .as_str()
            .is_some_and(|label| !label.is_empty()),
        "linked account label should be populated",
    );
    assert_eq!(nested["already_linked"], false);
    assert_eq!(native["already_linked"], false);
    assert_eq!(body["existing_wallet"]["wallet_id"], added.wallet_id);
    assert_eq!(
        body["existing_wallet"]["wallet_label"],
        "Mixed Scheme Wallet"
    );
    assert!(
        body["already_linked"].is_null(),
        "key-level already_linked should only be set when every scheme is linked",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_xpub_auto_routes_to_existing_wallet_when_normalized_key_already_linked() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let existing = add_xpub_wallet_account(
        &server,
        TEST_NATIVE_SEGWIT_ZPUB,
        "legacy",
        None,
        Some("Bound Xpub Wallet"),
    )
    .await;
    let other_wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Different Wallet",
    )
    .await;

    let response = server
        .post("/_app/user/wallets/xpub/add")
        .json(&json!({
            "request": {
                "extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB,
                "address_scheme": "nested_segwit",
                "wallet_id": other_wallet.wallet_id
            }
        }))
        .await;
    response.assert_status_ok();
    let body: Value = response.json();

    assert_eq!(
        body["wallet_id"], existing.wallet_id,
        "add should be routed to existing normalized-key wallet",
    );
    assert_ne!(
        body["wallet_id"], other_wallet.wallet_id,
        "submitted conflicting wallet should not be used",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_trezor_link_skips_duplicate_scheme_and_adds_new_variant_in_same_wallet() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let existing = add_xpub_wallet_account(
        &server,
        TEST_NATIVE_SEGWIT_ZPUB,
        "legacy",
        None,
        Some("Manual Xpub Wallet"),
    )
    .await;

    let link_response = server
        .post("/_app/user/wallets/trezor/link")
        .json(&json!({
            "request": {
                "master_fingerprint": "a1b2c3d4",
                "wallet_label": "Trezor Wallet",
                "accounts": [
                    {
                        "account_index": 0,
                        "address_scheme": "legacy",
                        "extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB
                    },
                    {
                        "account_index": 0,
                        "address_scheme": "native_segwit",
                        "extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB
                    }
                ]
            }
        }))
        .await;
    link_response.assert_status_ok();

    let payload: Value = link_response.json();
    assert_eq!(
        payload["wallet_id"], existing.wallet_id,
        "normalized-key affinity should keep trezor link in existing wallet",
    );
    let created_account_ids = payload["created_account_ids"]
        .as_array()
        .expect("created_account_ids should be an array");
    let skipped_account_indexes = payload["skipped_account_indexes"]
        .as_array()
        .expect("skipped_account_indexes should be an array");
    assert_eq!(
        created_account_ids.len(),
        1,
        "expected one new scheme variant account to be created",
    );
    assert!(
        skipped_account_indexes
            .iter()
            .any(|index| index.as_u64() == Some(0)),
        "expected duplicate scheme account index to be skipped",
    );

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let body: Value = wallets_response.json();
    let wallets = body["wallets"]
        .as_array()
        .expect("wallets should be an array");
    let wallet = wallets
        .iter()
        .find(|wallet| wallet["id"].as_str() == Some(existing.wallet_id.as_str()))
        .expect("expected existing wallet to exist");
    let accounts = wallet["accounts"]
        .as_array()
        .expect("accounts should be an array");

    let xpub_accounts: Vec<&Value> = accounts
        .iter()
        .filter(|account| account["account_reference_kind"] == "extended_pubkey")
        .collect();
    assert_eq!(
        xpub_accounts.len(),
        2,
        "expected duplicate to be skipped and one new scheme variant added",
    );
    assert!(
        xpub_accounts
            .iter()
            .any(|account| account["scheme"] == "legacy"),
        "legacy scheme should remain linked",
    );
    assert!(
        xpub_accounts
            .iter()
            .any(|account| account["scheme"] == "native_segwit"),
        "native segwit scheme should be linked as a separate account",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_ethereum_address_duplicate_wallet_label_returns_conflict() {
    let server = super::setup_test_server();
    register_user(&server).await;

    add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Main Wallet",
    )
    .await;

    let conflict = server
        .post("/_app/user/wallets/ethereum/add")
        .json(&json!({
            "request": {
                "address": "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe",
                "network": "mainnet",
                "wallet_label": "  main   wallet  "
            }
        }))
        .await;

    assert_eq!(conflict.status_code(), StatusCode::CONFLICT);
    let body: Value = conflict.json();
    let error = parse_error_envelope(body);
    assert!(error.is_conflict());
    assert!(error.first_field_error("wallet_label").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_move_wallet_account_happy_path_moves_account_to_existing_wallet() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let source = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Source Wallet",
    )
    .await;
    let target = add_ethereum_wallet_account(
        &server,
        "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe",
        "Target Wallet",
    )
    .await;

    let move_response = server
        .post("/_app/user/wallets/account/move")
        .json(&json!({
            "request": {
                "account_id": source.account_id,
                "destination": {
                    "kind": "existing_wallet",
                    "wallet_id": target.wallet_id
                }
            }
        }))
        .await;
    move_response.assert_status_ok();

    let payload: Value = move_response.json();
    assert_eq!(payload["destination_wallet_id"], target.wallet_id);

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let wallets_payload: Value = wallets_response.json();

    let wallets = wallets_payload["wallets"]
        .as_array()
        .expect("wallets should be array");
    let mut source_contains_account = false;
    let mut target_contains_account = false;
    let mut moved_label: Option<String> = None;

    for wallet in wallets {
        let wallet_id = wallet["id"].as_str().expect("wallet id should be string");
        let accounts = wallet["accounts"]
            .as_array()
            .expect("accounts should be array");
        let contains_source = accounts
            .iter()
            .any(|account| account["account_id"].as_str() == Some(source.account_id.as_str()));
        if wallet_id == source.wallet_id {
            source_contains_account = contains_source;
        }
        if wallet_id == target.wallet_id {
            target_contains_account = contains_source;
            for account in accounts {
                if account["account_id"].as_str() == Some(source.account_id.as_str()) {
                    moved_label = account["label"].as_str().map(str::to_string);
                }
            }
        }
    }

    assert!(
        !source_contains_account,
        "source wallet should no longer contain moved account",
    );
    assert!(
        target_contains_account,
        "target wallet should contain moved account",
    );
    assert_eq!(
        moved_label.as_deref(),
        Some("Ethereum Account 1 moved from wallet Source Wallet")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_manual_asset_account_happy_path_creates_new_wallet() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let add_body = add_manual_asset_account(&server, None, Some("Manual Assets"), "ADA").await;
    let wallet_id = add_body["wallet_id"]
        .as_str()
        .expect("wallet_id should be present");
    let account_id = add_body["account_id"]
        .as_str()
        .expect("account_id should be present");

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let body: Value = wallets_response.json();
    let wallets = body["wallets"]
        .as_array()
        .expect("wallets should be an array");
    let created_wallet = wallets
        .iter()
        .find(|wallet| wallet["id"] == wallet_id)
        .expect("created wallet should be present");

    assert_eq!(created_wallet["label"], "Manual Assets");
    let accounts = created_wallet["accounts"]
        .as_array()
        .expect("wallet accounts should be an array");
    let custom_account = accounts
        .iter()
        .find(|account| account["account_id"] == account_id)
        .expect("custom account should be present");

    assert_eq!(custom_account["label"], "ADA Account 1");
    assert_eq!(custom_account["unit_code"], "ADA");
    assert_eq!(custom_account["decimal_precision"], 6);
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_manual_btc_account_happy_path_creates_separate_manual_account() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let synced_wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Synced Wallet",
    )
    .await;
    let add_body =
        add_manual_asset_account(&server, Some(synced_wallet.wallet_id.as_str()), None, "BTC")
            .await;
    let account_id = add_body["account_id"]
        .as_str()
        .expect("account_id should be present");

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let body: Value = wallets_response.json();
    let accounts = body["wallets"][0]["accounts"]
        .as_array()
        .expect("wallet accounts should be an array");

    let manual_btc = accounts
        .iter()
        .find(|account| account["account_id"] == account_id)
        .expect("manual BTC account should be present");
    assert_eq!(manual_btc["label"], "BTC Account 1");
    assert_eq!(manual_btc["unit_code"], "BTC");
    assert_eq!(manual_btc["decimal_precision"], 8);
}

#[tokio::test(flavor = "current_thread")]
async fn test_delete_wallet_account_happy_path_deletes_custom_account() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Custom Delete Wallet",
    )
    .await;
    let custom_account =
        add_manual_asset_account(&server, Some(wallet.wallet_id.as_str()), None, "ADA").await;
    let custom_account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be string");

    let delete_response = server
        .post("/_app/user/wallets/account/delete")
        .json(&json!({
            "request": {
                "account_id": custom_account_id
            }
        }))
        .await;
    delete_response.assert_status_ok();

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let body: Value = wallets_response.json();
    let accounts = body["wallets"][0]["accounts"]
        .as_array()
        .expect("accounts should be array");
    assert!(
        !accounts
            .iter()
            .any(|account| account["account_id"] == custom_account_id)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_move_wallet_account_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/wallets/account/move")
        .json(&json!({
            "request": {
                "account_id": Ulid::new().to_string(),
                "destination": {
                    "kind": "existing_wallet",
                    "wallet_id": Ulid::new().to_string()
                }
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_unauthorized());
}

#[tokio::test(flavor = "current_thread")]
async fn test_move_wallet_account_same_wallet_returns_unprocessable_entity() {
    let server = super::setup_test_server();
    register_user(&server).await;

    let fixture = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Source Wallet",
    )
    .await;

    let response = server
        .post("/_app/user/wallets/account/move")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "destination": {
                    "kind": "existing_wallet",
                    "wallet_id": fixture.wallet_id
                }
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_validation());
    assert!(
        error.first_field_error("destination").is_some()
            || error.first_field_error("destination.wallet_id").is_some()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_update_account_label_happy_path_persists_label_in_wallets_response() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let fixture = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Label Wallet",
    )
    .await;

    let update_response = server
        .post("/_app/user/wallets/account/label")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "label": "Savings"
            }
        }))
        .await;
    update_response.assert_status_ok();

    let wallets_response = server.get("/_app/user/wallets").await;
    wallets_response.assert_status_ok();
    let body: Value = wallets_response.json();
    let wallets = body["wallets"]
        .as_array()
        .expect("wallets should be an array");

    let mut observed_label: Option<String> = None;
    for wallet in wallets {
        let accounts = wallet["accounts"]
            .as_array()
            .expect("accounts should be an array");
        for account in accounts {
            if account["account_id"].as_str() == Some(fixture.account_id.as_str()) {
                observed_label = account["label"].as_str().map(str::to_string);
            }
        }
    }

    assert_eq!(observed_label.as_deref(), Some("Savings"));
}

#[tokio::test(flavor = "current_thread")]
async fn test_update_account_label_empty_returns_unprocessable_entity() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let fixture = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Label Wallet",
    )
    .await;

    let response = server
        .post("/_app/user/wallets/account/label")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "label": "   "
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let error = parse_error_envelope(body);
    assert!(error.is_validation());
    assert!(error.first_field_error("label").is_some());
}

fn derivation_change_and_index(derivation_path: &str) -> (u32, u32) {
    let segments: Vec<&str> = derivation_path.split('/').collect();
    let change = segments
        .get(4)
        .and_then(|segment| segment.parse::<u32>().ok())
        .expect("expected derivation change segment");
    let index = segments
        .get(5)
        .and_then(|segment| segment.parse::<u32>().ok())
        .expect("expected derivation index segment");
    (change, index)
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_addresses_happy_path_returns_paged_rows() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let fixture = add_native_segwit_xpub_account(&server, "Xpub Wallet").await;

    let response = server
        .post("/_app/user/wallets/account/addresses")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "address_scheme": "native_segwit",
                "page": 1,
                "page_size": 50
            }
        }))
        .await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["page"], 1);
    assert_eq!(body["page_size"], 50);

    let total = body["total"].as_u64().expect("total should be a number");
    assert!(total > 0, "expected at least one derived address");

    let rows = body["rows"].as_array().expect("rows should be an array");
    assert!(!rows.is_empty(), "rows should not be empty");
    assert!(rows.len() <= 50, "rows should be capped to page size");

    let first = &rows[0];
    let first_address = first["address"]
        .as_str()
        .expect("address should be a string");
    assert!(!first_address.is_empty(), "address should not be empty");
    assert!(first["transaction_count"].is_number());
    assert_eq!(
        first["reported_transaction_count"],
        Value::Null,
        "a never-synced address should report no integration tx count",
    );
    assert_eq!(first["balance"]["asset_id"], "bitcoin");
    assert_eq!(first["balance"]["unit_code"], "BTC");
    assert_eq!(first["balance"]["symbol"], "₿");
    assert_eq!(first["balance"]["context"]["network"], "mainnet");
    assert_eq!(first["sync"]["status"], "not_synced");
    assert_eq!(first["sync"]["last_completed_at"], Value::Null);
    assert_eq!(first["sync"]["last_error"], Value::Null);
    let first_derivation_path = first["derivation_path"]
        .as_str()
        .expect("derivation_path should be a string");
    assert!(
        first_derivation_path.starts_with("m/84'/0'/0'/"),
        "unexpected derivation path: {first_derivation_path}",
    );

    let mut previous: Option<(u32, u32)> = None;
    for row in rows {
        let derivation_path = row["derivation_path"]
            .as_str()
            .expect("derivation_path should be a string");
        let current = derivation_change_and_index(derivation_path);
        if let Some(prev) = previous {
            assert!(
                prev <= current,
                "rows are not sorted by derivation path ascending: {prev:?} !<= {current:?}",
            );
        }
        previous = Some(current);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_addresses_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/wallets/account/addresses")
        .json(&json!({
            "request": {
                "account_id": Ulid::new().to_string(),
                "address_scheme": "native_segwit",
                "page": 1,
                "page_size": 50
            }
        }))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_addresses_validation_errors_return_unprocessable_entity() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let fixture = add_native_segwit_xpub_account(&server, "Validation Wallet").await;

    let invalid_page_response = server
        .post("/_app/user/wallets/account/addresses")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "address_scheme": "native_segwit",
                "page": 0,
                "page_size": 50
            }
        }))
        .await;
    assert_eq!(
        invalid_page_response.status_code(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_scheme_response = server
        .post("/_app/user/wallets/account/addresses")
        .json(&json!({
            "request": {
                "account_id": fixture.account_id,
                "address_scheme": "legacy",
                "page": 1,
                "page_size": 50
            }
        }))
        .await;
    assert_eq!(
        invalid_scheme_response.status_code(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

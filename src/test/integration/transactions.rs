//! Integration tests for transactions endpoint contracts.
//!
//! Scope:
//! - Representative route-family happy paths
//! - Minimal auth, validation, and malformed-input contract checks

use crate::backend::ApiErrorEnvelope;
use crate::models::UserId;
use crate::sync_control::{
    SyncControlMode, SyncControlModeOverrideGuard, set_sync_control_mode_override_for_tests,
};
use chrono::{Duration, Utc};
use dioxus::fullstack::StatusCode;
use rusqlite::{OpenFlags, params};
use serde_json::{Value, json};
use std::str::FromStr;
use ulid::Ulid;

use super::fixtures::{add_ethereum_wallet_account, register_user, select_account_sync_slot};
use super::{IntegrationTestServer, setup_test_server, setup_test_server_no_db};

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

fn sync_trigger_request_body(request: Value) -> Value {
    json!({ "request": request })
}

/// Builds the `asset_instance_id` payload for the manual asset endpoint.
/// Only ADA on cardano-mainnet is exercised by these transaction tests.
fn asset_instance_id_for_unit_code(unit_code: &str) -> Value {
    match unit_code {
        "ADA" => json!({
            "asset_id": "cardano",
            "network_id": "cardano-mainnet",
            "namespace": { "type": "native" }
        }),
        other => panic!("unsupported unit_code in transactions tests: {other}"),
    }
}

async fn add_manual_asset_account(
    server: &IntegrationTestServer,
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
    server: &IntegrationTestServer,
    account_id: &str,
    asserted_on: &str,
    balance: &str,
    note: Option<&str>,
) -> Value {
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
    response.json()
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

fn set_sync_control_mode_for_test(mode: SyncControlMode) -> SyncControlModeOverrideGuard {
    set_sync_control_mode_override_for_tests(Some(mode))
}

fn mark_address_sync_recent_success(
    server: &IntegrationTestServer,
    user_id: UserId,
    address_id: &str,
) {
    let connection = open_test_user_db(server, user_id);
    let timestamp = (Utc::now() - Duration::seconds(30)).to_rfc3339();
    connection
        .execute(
            "INSERT INTO transaction_sync_state
             (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result, last_error, last_tip_height, new_tx_count, updated_tx_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                Ulid::new().to_string(),
                "address",
                address_id,
                Ulid::new().to_string(),
                &timestamp,
                &timestamp,
                "success",
                Option::<String>::None,
                Option::<i64>::None,
                0_i64,
                0_i64,
                &timestamp,
                &timestamp,
            ],
        )
        .expect("recent sync success should persist");
}

fn mark_address_etherscan_history_gap(
    server: &IntegrationTestServer,
    user_id: UserId,
    address_id: &str,
) {
    let connection = open_test_user_db(server, user_id);
    connection
        .execute(
            "UPDATE transaction_sync_state
             SET etherscan_history_status = 'gap'
             WHERE scope = 'address'
               AND address_id = ?1",
            params![address_id],
        )
        .expect("history gap status should persist");
}

#[tokio::test(flavor = "current_thread")]
async fn test_trigger_sync_happy_path() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/transactions/sync")
        .json(&sync_trigger_request_body(json!({
            "source": "manual"
        })))
        .await;

    response.assert_status_ok();

    let payload: Value = response.json();
    let outcome = payload["outcome"]
        .as_str()
        .expect("outcome should be a string");
    assert!(outcome == "started" || outcome == "queued");
    assert!(payload["sync_run_id"].as_str().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_trigger_sync_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/user/transactions/sync")
        .json(&sync_trigger_request_body(json!({
            "source": "manual"
        })))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_trigger_sync_invalid_source_returns_field_keyed_validation_error() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/transactions/sync")
        .json(&sync_trigger_request_body(json!({
            "source": "invalid-source"
        })))
        .await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    let body: Value = response.json();
    let transactions_error: ApiErrorEnvelope = serde_json::from_value(body["data"].clone())
        .expect("Should parse TransactionsError from data field");

    assert!(transactions_error.is_validation());
    assert!(transactions_error.first_field_error("source").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_trigger_sync_account_scope_missing_returns_not_found() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server
        .post("/_app/user/transactions/sync")
        .json(&sync_trigger_request_body(json!({
            "source": "manual",
            "scope": {
                "kind": "account",
                "account_id": Ulid::new().to_string()
            }
        })))
        .await;

    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn test_trigger_sync_rejects_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();
    assert_malformed_json_returns_bad_request(&server, "/_app/user/transactions/sync").await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_sync_state_happy_path() {
    let server = setup_test_server();
    register_user(&server).await;

    let response = server.get("/_app/user/transactions/sync/state").await;
    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["is_running"], false);
    assert_eq!(payload["addresses_total"], 0);
    assert_eq!(payload["addresses_synced"], 0);
    assert_eq!(payload["addresses_failed"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_sync_state_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server.get("/_app/user/transactions/sync/state").await;

    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_sync_control_state_happy_path() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Sync Control",
    )
    .await;
    select_account_sync_slot(&server, &account.account_id).await;
    let user_id = current_user_id(&server).await;
    mark_address_sync_recent_success(
        &server,
        user_id,
        account
            .address_id
            .as_deref()
            .expect("fixture should include address"),
    );
    let _mode = set_sync_control_mode_for_test(SyncControlMode::Enabled);

    let response = server
        .get(&format!(
            "/_app/user/account/{}/sync-control/state",
            account.account_id
        ))
        .await;

    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["account_id"], account.account_id);
    assert_eq!(payload["addresses_total"], 1);
    assert_eq!(payload["integration"], "etherscan");
    assert_eq!(payload["addresses"][0]["last_result"], "success");
    assert_eq!(payload["addresses"][0]["backfill_active"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_sync_control_state_disabled_returns_forbidden() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
        "Sync Control Disabled",
    )
    .await;
    let _mode = set_sync_control_mode_for_test(SyncControlMode::Disabled);

    let response = server
        .get(&format!(
            "/_app/user/account/{}/sync-control/state",
            account.account_id
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::FORBIDDEN);

    let body: Value = response.json();
    let error: ApiErrorEnvelope =
        serde_json::from_value(body["data"].clone()).expect("should parse API error envelope");
    assert_eq!(
        error.message,
        "Sync control is disabled in this environment"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_run_account_sync_control_happy_path() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0xde709f2102306220921060314715629080e2fb77",
        "Sync Control Run",
    )
    .await;
    select_account_sync_slot(&server, &account.account_id).await;
    let user_id = current_user_id(&server).await;
    mark_address_sync_recent_success(
        &server,
        user_id,
        account
            .address_id
            .as_deref()
            .expect("fixture should include address"),
    );
    let _mode = set_sync_control_mode_for_test(SyncControlMode::Enabled);

    let response = server
        .post(&format!(
            "/_app/user/account/{}/sync-control/run",
            account.account_id
        ))
        .json(&json!({
            "request": {
                "iterations": 1
            }
        }))
        .await;

    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["iterations_requested"], 1);
    assert_eq!(payload["iterations_completed"], 0);
    assert_eq!(payload["addresses_touched"], 0);
    assert_eq!(payload["stopped_early"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn test_run_account_sync_control_rejects_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();
    let _mode = set_sync_control_mode_for_test(SyncControlMode::Enabled);
    let path = format!("/_app/user/account/{}/sync-control/run", Ulid::new());
    assert_malformed_json_returns_bad_request(&server, &path).await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_sync_snapshots_happy_path() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Sync Snapshot Wallet",
    )
    .await;

    let response = server.get("/_app/user/transactions/sync/accounts").await;
    response.assert_status_ok();

    let payload: Value = response.json();
    let entries = payload
        .as_array()
        .expect("snapshot response should be an array");
    assert_eq!(entries.len(), 1);
    let snapshot = &entries[0];
    assert_eq!(snapshot["account_id"], account.account_id.to_string());
    assert_eq!(snapshot["addresses_total"], 1);
    assert_eq!(snapshot["addresses_never_synced"], 1);
    assert_eq!(snapshot["addresses_synced"], 0);
    assert_eq!(snapshot["addresses_failed"], 0);
    assert_eq!(snapshot["addresses_in_progress"], 0);
    assert!(snapshot["last_result"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn test_etherscan_history_gap_is_exposed_in_snapshots_and_transactions() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Gap Wallet",
    )
    .await;
    let user_id = current_user_id(&server).await;
    let address_id = account
        .address_id
        .as_deref()
        .expect("fixture should include address");
    mark_address_sync_recent_success(&server, user_id, address_id);
    mark_address_etherscan_history_gap(&server, user_id, address_id);

    let snapshots_response = server.get("/_app/user/transactions/sync/accounts").await;
    snapshots_response.assert_status_ok();
    let snapshots_payload: Value = snapshots_response.json();
    assert_eq!(snapshots_payload[0]["etherscan_history_status"], "gap");
    assert_eq!(
        snapshots_payload[0]["integration_states"][0]["etherscan_history_status"],
        "gap"
    );

    let transactions_response = server
        .get(&format!(
            "/_app/user/account/{}/transactions",
            account.account_id
        ))
        .await;
    transactions_response.assert_status_ok();
    let transactions_payload: Value = transactions_response.json();
    assert_eq!(transactions_payload["etherscan_history_status"], "gap");
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_transactions_happy_path_returns_dual_table_payload() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Tx View Wallet",
    )
    .await;

    let response = server
        .get(&format!(
            "/_app/user/account/{}/transactions",
            account.account_id
        ))
        .await;
    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["account_id"], account.account_id);
    assert_eq!(payload["wallet_label"], "Tx View Wallet");
    assert_eq!(payload["asset"], "ethereum");
    assert_eq!(payload["network"], "mainnet");
    assert_eq!(payload["sync_control_enabled"], false);
    assert_eq!(payload["unit_code"], "ETH");
    assert_eq!(payload["symbol"], "Ξ");
    assert_eq!(payload["sort"], "Descending");
    assert_eq!(payload["opening_balance_state"]["kind"], "unknown");
    assert_eq!(payload["closing_balance_state"]["kind"], "unknown");
    assert_eq!(
        payload["opening_balance_reliability"]["kind"],
        "provisional"
    );
    assert_eq!(
        payload["opening_balance_reliability"]["reasons"],
        json!(["first_successful_sync_pending"])
    );
    assert_eq!(
        payload["closing_balance_reliability"]["kind"],
        "provisional"
    );
    assert_eq!(
        payload["closing_balance_reliability"]["reasons"],
        json!(["first_successful_sync_pending"])
    );
    assert!(payload["opening_balance_date"].is_null());
    assert!(payload["closing_balance_date"].is_null());
    assert_eq!(
        payload["active_status_filter"]
            .as_array()
            .expect("active_status_filter should be an array")
            .len(),
        0
    );
    assert!(payload["active_from_date"].is_null());
    assert!(payload["active_to_date"].is_null());

    for table_name in ["pending", "confirmed"] {
        let table = &payload[table_name];
        assert_eq!(table["page"], 1);
        assert_eq!(table["page_size"], 50);
        assert_eq!(table["total"], 0);
        assert_eq!(table["start"], 0);
        assert_eq!(table["end"], 0);
        assert_eq!(
            table["rows"]
                .as_array()
                .expect("table rows should be an array")
                .len(),
            0
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_transactions_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let account_id = Ulid::new();
    let response = server
        .get(&format!(
            "/_app/user/account/{account_id}/transactions?pending_page=1&confirmed_page=1"
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_transactions_validation_error_returns_unprocessable_entity() {
    let server = setup_test_server();
    register_user(&server).await;
    let account = add_ethereum_wallet_account(
        &server,
        "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe",
        "Tx Validation Wallet",
    )
    .await;

    let response = server
        .get(&format!(
            "/_app/user/account/{}/transactions?pending_page=0",
            account.account_id
        ))
        .await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    let body: Value = response.json();
    let transactions_error: ApiErrorEnvelope = serde_json::from_value(body["data"].clone())
        .expect("Should parse TransactionsError from data field");
    assert!(transactions_error.is_validation());
    assert!(
        transactions_error
            .first_field_error("pending_page")
            .is_some()
    );
}

// TODO: rewrite for new shape after task11 — manual asset accounts use registry-fixed
// decimal_precision and precision_status semantics, so legacy-custom-style assertions
// (inferred precision 3 from "123.456") no longer apply.
#[ignore = "TODO: rewrite for new shape after task11"]
#[tokio::test(flavor = "current_thread")]
async fn test_get_account_transactions_custom_history_returns_assertion_projection() {
    let server = setup_test_server();
    register_user(&server).await;
    let wallet = add_ethereum_wallet_account(
        &server,
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "Custom History Wallet",
    )
    .await;
    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be string");

    add_custom_balance_assertion(
        &server,
        account_id,
        "2026-01-10",
        "123.456",
        Some("Initial"),
    )
    .await;
    add_custom_balance_assertion(&server, account_id, "2026-01-20", "0", Some("Sold")).await;

    let response = server
        .get(&format!("/_app/user/account/{account_id}/transactions"))
        .await;
    response.assert_status_ok();

    let payload: Value = response.json();
    assert_eq!(payload["kind"], "custom");
    assert_eq!(payload["account_id"], account_id);
    assert_eq!(payload["wallet_label"], "Custom History Wallet");
    assert_eq!(payload["account_label"], "ADA Account 1");
    assert_eq!(payload["sync_control_enabled"], false);
    assert_eq!(payload["unit_code"], "ADA");
    assert_eq!(payload["decimal_precision"], 3);
    assert_eq!(payload["precision_status"], "inferred");
    assert_eq!(payload["precision_shared_with_other_accounts"], false);
    assert_eq!(payload["opening_balance_state"]["kind"], "known");
    assert_eq!(
        payload["opening_balance_state"]["amount"]["formatted_value"],
        "123.456"
    );
    assert_eq!(payload["opening_balance_date"], "2026-01-10");
    assert_eq!(payload["closing_balance_state"]["kind"], "known");
    assert_eq!(
        payload["closing_balance_state"]["amount"]["formatted_value"],
        "0"
    );
    assert_eq!(payload["closing_balance_date"], "2026-01-20");
    assert_eq!(payload["sort"], "Descending");

    let assertions = payload["assertions"]["rows"]
        .as_array()
        .expect("assertions rows should be array");
    assert_eq!(payload["assertions"]["total"], 2);
    assert_eq!(assertions[0]["asserted_on"], "2026-01-20");
    assert_eq!(assertions[0]["asserted_balance"]["formatted_value"], "0");
    assert_eq!(assertions[0]["note"], "Sold");
    assert_eq!(assertions[1]["asserted_on"], "2026-01-10");
    assert_eq!(
        assertions[1]["asserted_balance"]["formatted_value"],
        "123.456"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_add_custom_balance_assertion_future_date_returns_field_keyed_validation_error() {
    let server = setup_test_server();
    register_user(&server).await;
    let wallet = add_ethereum_wallet_account(
        &server,
        "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe",
        "Custom Assertion Validation Wallet",
    )
    .await;
    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be string");

    let response = server
        .post("/_app/user/manual-asset-assertions/add")
        .json(&json!({
            "request": {
                "account_id": account_id,
                "asserted_on": "2100-01-01",
                "balance": "1.0",
                "note": "future"
            }
        }))
        .await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    let body: Value = response.json();
    let transactions_error: ApiErrorEnvelope = serde_json::from_value(body["data"].clone())
        .expect("Should parse TransactionsError from data field");
    assert!(transactions_error.is_validation());
    assert!(
        transactions_error
            .first_field_error("asserted_on")
            .is_some()
    );
}

// TODO: rewrite for new shape after task11 — manual asset accounts have registry-fixed
// precision (ADA scale=6) and reject over-precision input, so the dynamic precision-grow
// path tested here ("2.500000000001" growing scale to 12) no longer applies.
#[ignore = "TODO: rewrite for new shape after task11"]
#[tokio::test(flavor = "current_thread")]
async fn test_update_and_delete_custom_balance_assertion_updates_history_projection() {
    let server = setup_test_server();
    register_user(&server).await;
    let wallet = add_ethereum_wallet_account(
        &server,
        "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
        "Custom Assertion Edit Wallet",
    )
    .await;
    let custom_account = add_manual_asset_account(&server, &wallet.wallet_id, "ADA").await;
    let account_id = custom_account["account_id"]
        .as_str()
        .expect("custom account id should be string");

    add_custom_balance_assertion(&server, account_id, "2026-01-10", "1.0", Some("First")).await;
    let middle =
        add_custom_balance_assertion(&server, account_id, "2026-01-15", "2.0", Some("Middle"))
            .await;
    add_custom_balance_assertion(&server, account_id, "2026-01-20", "3.0", Some("Last")).await;
    let middle_assertion_id = middle["assertion_id"]
        .as_str()
        .expect("middle assertion id should be string");

    let update_response = server
        .post("/_app/user/manual-asset-assertions/update")
        .json(&json!({
            "request": {
                "assertion_id": middle_assertion_id,
                "asserted_on": "2026-01-15",
                "balance": "2.500000000001",
                "note": "Adjusted"
            }
        }))
        .await;
    update_response.assert_status_ok();

    let updated_history = server
        .get(&format!(
            "/_app/user/account/{account_id}/transactions?sort=asc"
        ))
        .await;
    updated_history.assert_status_ok();
    let updated_payload: Value = updated_history.json();
    assert_eq!(updated_payload["decimal_precision"], 12);
    let updated_rows = updated_payload["assertions"]["rows"]
        .as_array()
        .expect("assertions rows should be array");
    assert_eq!(
        updated_rows[1]["asserted_balance"]["formatted_value"],
        "2.500000000001"
    );
    assert_eq!(updated_rows[1]["note"], "Adjusted");

    let delete_response = server
        .post("/_app/user/manual-asset-assertions/delete")
        .json(&json!({
            "request": {
                "assertion_id": middle_assertion_id
            }
        }))
        .await;
    delete_response.assert_status_ok();

    let deleted_history = server
        .get(&format!(
            "/_app/user/account/{account_id}/transactions?sort=asc"
        ))
        .await;
    deleted_history.assert_status_ok();
    let deleted_payload: Value = deleted_history.json();
    assert_eq!(deleted_payload["decimal_precision"], 12);
    let deleted_rows = deleted_payload["assertions"]["rows"]
        .as_array()
        .expect("assertions rows should be array");
    assert_eq!(deleted_payload["assertions"]["total"], 2);
    assert_eq!(deleted_rows[0]["asserted_on"], "2026-01-10");
    assert_eq!(deleted_rows[1]["asserted_on"], "2026-01-20");
}

#[tokio::test(flavor = "current_thread")]
async fn test_get_account_transactions_malformed_query_returns_bad_request() {
    let server = setup_test_server();
    register_user(&server).await;
    let account_id = Ulid::new();
    let response = server
        .get(&format!(
            "/_app/user/account/{account_id}/transactions?pending_page=abc"
        ))
        .await;

    assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn test_custom_balance_assertion_body_endpoints_reject_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();
    assert_malformed_json_returns_bad_request(&server, "/_app/user/manual-asset-assertions/add")
        .await;
    assert_malformed_json_returns_bad_request(&server, "/_app/user/manual-asset-assertions/update")
        .await;
    assert_malformed_json_returns_bad_request(&server, "/_app/user/manual-asset-assertions/delete")
        .await;
}

/// This guards the route registration in this test harness
/// (`src/test/integration/mod.rs`), which mirrors the hand-registered
/// routes in `src/main.rs` rather than a server-function macro. A wrong
/// path in `main.rs` itself fails silently in the browser — the page
/// still renders and sync still runs, only the live progress stops — and
/// is caught separately by the e2e suite, which exercises the real
/// server. An unauthenticated request is answered before any stream
/// opens, so `401` here means "registered and guarded" and `404` means
/// "gone".
#[tokio::test(flavor = "current_thread")]
async fn sync_events_sse_route_is_registered_and_requires_auth() {
    let server = setup_test_server_no_db();
    let response = server.get("/_app/user/transactions/sync/events").await;

    assert_eq!(
        response.status_code(),
        StatusCode::UNAUTHORIZED,
        "expected 401 for an unauthenticated SSE request; 404 means the route \
         is not registered at this path"
    );
}

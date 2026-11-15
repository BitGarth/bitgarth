//! `/api/v1` endpoints used by the CLI with read and full Client Keys.

use axum::http::{StatusCode, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

use super::IntegrationTestServer;
use super::fixtures::{
    TEST_NATIVE_SEGWIT_ZPUB, activate_signed_full_report_entitlements, register_user,
};

const BTC_ADDRESS: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
const ETH_ADDRESS: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

fn bearer(key_byte: u8) -> String {
    format!("Bearer {}", URL_SAFE_NO_PAD.encode([key_byte; 32]))
}

/// Pair a client for the signed-in browser user with `permission`.
async fn pair(server: &IntegrationTestServer, key_byte: u8, permission: &str) -> Value {
    let verifier = crate::client_capabilities::ClientKeyVerifier::from_raw_key(&[key_byte; 32]);
    let started = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .json(&json!({
            "client_name": format!("cli {permission}"),
            "key_verifier": URL_SAFE_NO_PAD.encode(verifier.as_bytes()),
            "permissions": [permission]
        }))
        .await;
    started.assert_status_ok();
    let pairing: Value = started.json();
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&json!({
            "request": {
                "pairing_id": pairing["pairing_id"],
                "code": pairing["code"],
                "permissions": [permission],
                "code_matches": true,
                "expires_at": Value::Null
            }
        }))
        .await
        .assert_status_ok();
    let claimed = server
        .post(&format!(
            "/api/v1/pairings/{}/claim",
            pairing["pairing_id"].as_str().unwrap()
        ))
        .add_header(header::AUTHORIZATION, bearer(key_byte))
        .await;
    claimed.assert_status_ok();
    let claim: Value = claimed.json();
    assert_eq!(claim["permissions"], json!([permission]));
    pairing
}

async fn call(
    server: &IntegrationTestServer,
    key_byte: u8,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let url = format!("/api/v1/{path}");
    let request = match method {
        "GET" => server.get(&url),
        "POST" => server.post(&url),
        "PUT" => server.put(&url),
        "PATCH" => server.patch(&url),
        "DELETE" => server.delete(&url),
        _ => unreachable!("unsupported test method"),
    }
    .add_header(header::AUTHORIZATION, bearer(key_byte));
    let response = match body {
        Some(body) => request.json(&body).await,
        None => request.await,
    };
    let status = response.status_code();
    let text = response.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn ok(
    server: &IntegrationTestServer,
    key_byte: u8,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, value) = call(server, key_byte, method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {value}");
    value
}

#[tokio::test]
async fn full_client_manages_wallets_accounts_and_reads_every_view() {
    let server = super::setup_test_server();
    let _mode = crate::sync_control::set_sync_control_mode_override_for_tests(Some(
        crate::sync_control::SyncControlMode::Enabled,
    ));
    register_user(&server).await;
    pair(&server, 81, "full").await;

    let me = ok(&server, 81, "GET", "me", None).await;
    assert_eq!(me["client"]["permission"], "full");
    assert_eq!(me["client"]["name"], "cli full");

    let bitcoin = ok(
        &server,
        81,
        "POST",
        "accounts/bitcoin-address",
        Some(json!({"address": BTC_ADDRESS, "network": "mainnet", "wallet_label": "Cold", "account_label": "Savings"})),
    )
    .await;
    let wallet_id = bitcoin["wallet_id"].as_str().unwrap().to_owned();
    let bitcoin_account = bitcoin["account_id"].as_str().unwrap().to_owned();
    let ethereum = ok(
        &server,
        81,
        "POST",
        "accounts/ethereum-address",
        Some(json!({"address": ETH_ADDRESS, "network": "mainnet", "wallet_id": wallet_id})),
    )
    .await;
    assert_eq!(ethereum["wallet_id"], wallet_id.as_str());
    let inspected = ok(
        &server,
        81,
        "POST",
        "xpubs/inspect",
        Some(json!({"extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB})),
    )
    .await;
    assert_eq!(inspected["suggested_scheme"], "native_segwit");
    let xpub = ok(
        &server,
        81,
        "POST",
        "accounts/xpub",
        Some(json!({"extended_pubkey": TEST_NATIVE_SEGWIT_ZPUB, "address_scheme": "native_segwit", "wallet_label": "Trezor"})),
    )
    .await;
    let xpub_account = xpub["account_id"].as_str().unwrap().to_owned();

    let search = ok(&server, 81, "GET", "assets/search?query=usd-coin", None).await;
    let asset = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| !row["asset_instance_id"].is_null())
        .expect("the catalog should list a USD Coin instance")["asset_instance_id"]
        .clone();
    let manual = ok(
        &server,
        81,
        "POST",
        "accounts/manual",
        Some(json!({"asset": {"source": "bit_garth_catalog", "asset_instance_id": asset}, "wallet_label": "Exchange"})),
    )
    .await;
    let manual_account = manual["account_id"].as_str().unwrap().to_owned();

    let wallets = ok(&server, 81, "GET", "wallets", None).await;
    assert_eq!(wallets["wallets"].as_array().unwrap().len(), 3);

    ok(
        &server,
        81,
        "PATCH",
        &format!("wallets/{wallet_id}"),
        Some(json!({"label": "Vault"})),
    )
    .await;
    ok(
        &server,
        81,
        "PATCH",
        &format!("accounts/{bitcoin_account}"),
        Some(json!({"label": "Long term"})),
    )
    .await;
    let moved = ok(
        &server,
        81,
        "POST",
        &format!("accounts/{bitcoin_account}/move"),
        Some(json!({"destination": {"kind": "new_wallet", "label": "Moved"}})),
    )
    .await;
    assert!(moved["destination_wallet_id"].is_string());
    let wallets = ok(&server, 81, "GET", "wallets", None).await;
    let labels: Vec<&str> = wallets["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|wallet| wallet["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"Vault") && labels.contains(&"Moved"));

    let added = ok(
        &server,
        81,
        "POST",
        &format!("accounts/{manual_account}/balance-assertions"),
        Some(json!({"asserted_on": "2026-01-31", "balance": "12.5", "note": "statement"})),
    )
    .await;
    let assertion = added["assertion_id"].as_str().unwrap().to_owned();
    ok(
        &server,
        81,
        "PUT",
        &format!("accounts/{manual_account}/balance-assertions/{assertion}"),
        Some(json!({"asserted_on": "2026-01-31", "balance": "13"})),
    )
    .await;
    let history = ok(
        &server,
        81,
        "GET",
        &format!("accounts/{manual_account}/transactions"),
        None,
    )
    .await;
    assert_eq!(history["kind"], "custom");
    assert_eq!(
        history["assertions"]["rows"][0]["asserted_balance"]["formatted_value"],
        "13"
    );
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair(
            "filters",
            &json!({"from_date": "2026-02-01T00:00:00Z"}).to_string(),
        )
        .finish();
    let filtered = ok(
        &server,
        81,
        "GET",
        &format!("accounts/{manual_account}/transactions?{query}"),
        None,
    )
    .await;
    assert_eq!(filtered["assertions"]["total"], 0);
    assert_eq!(filtered["active_from_date"], "2026-02-01");
    ok(
        &server,
        81,
        "DELETE",
        &format!("accounts/{manual_account}/balance-assertions/{assertion}"),
        None,
    )
    .await;

    let native_history = ok(
        &server,
        81,
        "GET",
        &format!("accounts/{bitcoin_account}/transactions?sort=asc"),
        None,
    )
    .await;
    assert_eq!(native_history["kind"], "native");
    let addresses = ok(
        &server,
        81,
        "GET",
        &format!("accounts/{xpub_account}/addresses?scheme=native_segwit&page=1"),
        None,
    )
    .await;
    assert!(addresses["rows"].is_array());

    let report = ok(
        &server,
        81,
        "GET",
        &format!("wallets/{wallet_id}/report?from=2026-01-01&to=2026-01-31"),
        None,
    )
    .await;
    assert_eq!(report["wallet_label"], "Vault");
    let holdings = ok(
        &server,
        81,
        "GET",
        "reports/holdings?timezone=Europe/Amsterdam",
        None,
    )
    .await;
    assert!(holdings["wallets"].is_array());

    let status = ok(&server, 81, "GET", "sync", None).await;
    assert!(status["accounts"].is_array() && status["overall"].is_object());

    ok(
        &server,
        81,
        "DELETE",
        &format!("accounts/{xpub_account}"),
        None,
    )
    .await;
    ok(&server, 81, "DELETE", &format!("wallets/{wallet_id}"), None).await;
    let wallets = ok(&server, 81, "GET", "wallets", None).await;
    assert!(
        wallets["wallets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|wallet| wallet["id"] != wallet_id.as_str())
    );
}

#[tokio::test]
async fn paired_sync_finishes_its_run_after_browser_logout() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let user = super::fixtures::current_user_id(&server).await;
    pair(&server, 89, "full").await;
    server.post("/_app/auth/logout").await.assert_status_ok();
    let mut events = crate::tasks::subscribe_transaction_sync_events(user).unwrap();
    let started = ok(&server, 89, "POST", "sync", None).await;
    let id = started["sync_run_id"].as_str().unwrap();
    let run_id = Some(id.parse().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.run_id == run_id
                && event.event_type == crate::transactions::TransactionSyncEventType::Completed
            {
                break;
            }
        }
        loop {
            let status = ok(&server, 89, "GET", &format!("sync/{id}"), None).await;
            if status["state"] == "completed" {
                assert_eq!(status["result"], "success");
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the worker should publish completion and release the capability lease");
    assert!(
        !crate::db::list_open_user_db_users()
            .unwrap()
            .contains(&user)
    );
}

#[tokio::test]
async fn full_client_changes_settings_secrets_and_prices_without_exposing_secrets() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let _entitlements = activate_signed_full_report_entitlements(&server).await;
    pair(&server, 82, "full").await;

    let settings = ok(
        &server,
        82,
        "PUT",
        "settings/currency",
        Some(json!({"value": "eur"})),
    )
    .await;
    assert_eq!(settings["currency"], "EUR");
    ok(
        &server,
        82,
        "PUT",
        "settings/timezone",
        Some(json!({"value": "Europe/Amsterdam"})),
    )
    .await;
    ok(
        &server,
        82,
        "PUT",
        "settings/number-format",
        Some(json!({"value": "comma_dot"})),
    )
    .await;
    ok(
        &server,
        82,
        "PUT",
        "settings/date-format",
        Some(json!({"value": "dmy_24"})),
    )
    .await;
    ok(
        &server,
        82,
        "PUT",
        "settings/price-fetching",
        Some(json!({"value": false})),
    )
    .await;
    let custom = ok(
        &server,
        82,
        "PUT",
        "settings/mempool-url",
        Some(json!({"value": "https://mempool.example.com"})),
    )
    .await;
    assert!(
        custom["mempool_url"]
            .as_str()
            .unwrap()
            .starts_with("https://mempool.example.com")
    );
    let reset = ok(
        &server,
        82,
        "PUT",
        "settings/mempool-url",
        Some(json!({"value": null})),
    )
    .await;
    assert!(reset["mempool_url"].is_null());
    ok(
        &server,
        82,
        "PUT",
        "settings/etherscan-url",
        Some(json!({"value": null})),
    )
    .await;
    let (status, _) = call(
        &server,
        82,
        "PUT",
        "settings/etherscan-url",
        Some(json!({"value": "https://alternate.example/api"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (endpoint, field) in [
        ("mempool_base_url", "mempool_base_url"),
        ("etherscan_base_url", "etherscan_base_url"),
    ] {
        server
            .post(&format!("/_app/user/settings/{endpoint}"))
            .json(&json!({(field): "https://reader:sample@provider.example/api/"}))
            .await
            .assert_status_ok();
    }
    let redacted = ok(&server, 82, "GET", "settings", None).await;
    for field in [
        "etherscan_url",
        "effective_etherscan_url",
        "mempool_url",
        "effective_mempool_url",
    ] {
        assert!(
            redacted[field]
                .as_str()
                .expect("provider URL")
                .starts_with("https://provider.example/")
        );
    }
    ok(
        &server,
        82,
        "PUT",
        "settings/etherscan-url",
        Some(json!({"value": "https://provider.example/updated/"})),
    )
    .await;
    let (status, _) = call(
        &server,
        82,
        "PUT",
        "settings/etherscan-url",
        Some(json!({"value": null})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let prefix = ok(
        &server,
        82,
        "PUT",
        "settings/hledger-prefix",
        Some(json!({"value": "assets:crypto"})),
    )
    .await;
    assert_eq!(prefix["hledger_prefix"], "assets:crypto");

    let (status, _) = call(
        &server,
        82,
        "PUT",
        "settings/currency",
        Some(json!({"value": "XYZ"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &server,
        82,
        "PUT",
        "settings/colour",
        Some(json!({"value": "red"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let stored = ok(
        &server,
        82,
        "PUT",
        "secrets/etherscan-api-key",
        Some(json!({"value": "ETHERSCAN-SECRET"})),
    )
    .await;
    assert_eq!(
        stored,
        json!({"etherscan_api_key": true, "coingecko_api_key": false})
    );
    ok(
        &server,
        82,
        "PUT",
        "secrets/coingecko-api-key",
        Some(json!({"value": "CG-SECRET"})),
    )
    .await;
    let settings = ok(&server, 82, "GET", "settings", None).await;
    assert_eq!(
        settings["secrets"],
        json!({"etherscan_api_key": true, "coingecko_api_key": true})
    );
    assert!(!settings.to_string().contains("ETHERSCAN-SECRET"));
    assert!(!settings.to_string().contains("CG-SECRET"));
    let cleared = ok(&server, 82, "DELETE", "secrets/coingecko-api-key", None).await;
    assert_eq!(cleared["coingecko_api_key"], false);

    ok(&server, 82, "POST", "accounts/manual", Some(json!({
        "asset": {"source": "bit_garth_catalog", "asset_instance_id": {"asset_id":"algorand", "network_id":"algorand-mainnet"}},
        "wallet_label": "Exchange"
    }))).await;
    let price = json!({
        "subject": {"kind": "catalog_asset", "id": "algorand"},
        "quote_currency": "EUR",
        "price_time_local": "2026-01-01T00:00:00",
        "price": "50000",
        "source_note": "test"
    });
    let saved = ok(&server, 82, "PUT", "prices/overrides", Some(price)).await;
    assert_eq!(saved["price"], "50000");
    let listed = ok(
        &server,
        82,
        "GET",
        "prices/overrides?asset=algorand&currency=EUR&from=2025-12-31T00:00:00Z&to=2026-01-02T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    let history = ok(
        &server,
        82,
        "GET",
        "prices/history?asset=algorand&source=YourPrices&from=2026-01-01&to=2026-01-02",
        None,
    )
    .await;
    assert_eq!(history["source"], "YourPrices");
    assert_eq!(
        history["entries"][0]["YourPrice"]["at"],
        saved["price_time_utc"]
    );
    for (path, expected) in [
        ("prices/history?asset=ethereum", StatusCode::NOT_FOUND),
        ("prices/history?asset=not-held", StatusCode::NOT_FOUND),
        (
            "prices/history?source=CoinGecko",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(call(&server, 82, "GET", path, None).await.0, expected);
    }
    ok(
        &server,
        82,
        "DELETE",
        "prices/overrides",
        Some(json!({"subject": {"kind": "catalog_asset", "id": "algorand"}, "quote_currency": "EUR", "price_time_local": history["entries"][0]["YourPrice"]["at"]})),
    )
    .await;
    let remaining = ok(&server, 82, "GET", "prices/overrides?asset=algorand&currency=EUR&from=2025-12-31T00:00:00Z&to=2026-01-02T00:00:00Z", None).await;
    assert_eq!(remaining, json!([]));
    let (status, _) = call(
        &server,
        82,
        "PUT",
        "prices/overrides",
        Some(json!({
            "subject": {"kind":"catalog_asset", "id":"not-a-real-asset"}, "quote_currency":"EUR",
            "price_time_local":"2026-01-01T00:00:00", "price":"1", "source_note":null
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn read_and_legacy_clients_cannot_change_anything() {
    let server = super::setup_test_server();
    register_user(&server).await;
    pair(&server, 83, "read").await;
    pair(&server, 84, "balances_read").await;

    ok(&server, 83, "GET", "wallets", None).await;
    ok(&server, 83, "GET", "settings", None).await;
    for (method, path, body) in [
        (
            "POST",
            "accounts/bitcoin-address",
            Some(json!({"address": BTC_ADDRESS, "network": "mainnet", "wallet_label": "W"})),
        ),
        ("PUT", "settings/currency", Some(json!({"value": "EUR"}))),
        (
            "PUT",
            "secrets/etherscan-api-key",
            Some(json!({"value": "x"})),
        ),
        ("POST", "sync", None),
        ("DELETE", "wallets/01ARZ3NDEKTSV4RRFFQ69G5FAV", None),
    ] {
        let (status, error) = call(&server, 83, method, path, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert!(error.to_string().contains("full permission"));
    }

    let (status, _) = call(&server, 84, "GET", "wallets", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    ok(&server, 84, "GET", "wallet-balances", None).await;
    let me = ok(&server, 84, "GET", "me", None).await;
    assert_eq!(me["client"]["permission"], "balances_read");
    assert!(me.get("username").is_none());
}

#[tokio::test]
async fn approval_must_repeat_the_requested_permission() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let verifier = crate::client_capabilities::ClientKeyVerifier::from_raw_key(&[85; 32]);
    let pairing: Value = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .json(&json!({
            "client_name": "escalation",
            "key_verifier": URL_SAFE_NO_PAD.encode(verifier.as_bytes()),
            "permissions": ["read"]
        }))
        .await
        .json();
    let approve = |permissions: Value| {
        server
            .post("/_app/pairings/approve")
            .add_header(header::HOST, "example.com")
            .add_header(header::ORIGIN, "http://example.com")
            .json(&json!({
                "request": {
                    "pairing_id": pairing["pairing_id"],
                    "code": pairing["code"],
                    "permissions": permissions,
                    "code_matches": true,
                    "expires_at": Value::Null
                }
            }))
    };
    approve(json!(["full"]))
        .await
        .assert_status(StatusCode::CONFLICT);
    approve(json!(["read", "full"]))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    approve(json!(["read"])).await.assert_status_ok();
}

#[tokio::test]
async fn the_browser_can_make_a_full_client_read_only_but_never_raise_it() {
    let server = super::setup_test_server();
    register_user(&server).await;
    pair(&server, 87, "full").await;
    pair(&server, 88, "read").await;
    let clients: Value = server.get("/_app/pairings/clients").await.json();
    let id_of = |name: &str| {
        clients
            .as_array()
            .unwrap()
            .iter()
            .find(|client| client["name"] == name)
            .unwrap()["capability_id"]
            .clone()
    };
    let (full_id, read_id) = (id_of("cli full"), id_of("cli read"));
    let limit = |capability_id: &Value, permission: &str| {
        server
            .post("/_app/pairings/limit")
            .add_header(header::HOST, "example.com")
            .add_header(header::ORIGIN, "http://example.com")
            .json(&json!({
                "request": { "capability_id": capability_id, "permission": permission }
            }))
    };

    ok(
        &server,
        87,
        "PUT",
        "settings/currency",
        Some(json!({"value": "EUR"})),
    )
    .await;
    // Cross-site requests are refused like revoke.
    server
        .post("/_app/pairings/limit")
        .json(&json!({ "request": { "capability_id": full_id, "permission": "read" } }))
        .await
        .assert_status_forbidden();
    // Raising or keeping the level is refused.
    limit(&read_id, "full")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    limit(&full_id, "full")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    limit(&full_id, "admin")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    limit(&full_id, "read").await.assert_status_ok();
    let (status, error) = call(
        &server,
        87,
        "PUT",
        "settings/currency",
        Some(json!({"value": "USD"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error.to_string().contains("full permission"));
    ok(&server, 87, "GET", "wallets", None).await;
    let me = ok(&server, 87, "GET", "me", None).await;
    assert_eq!(me["client"]["permission"], "read");
    let clients: Value = server.get("/_app/pairings/clients").await.json();
    let limited = clients
        .as_array()
        .unwrap()
        .iter()
        .find(|client| client["capability_id"] == full_id)
        .unwrap();
    assert_eq!(limited["permission"], "read");
    limit(&full_id, "read")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_client_can_revoke_its_own_key() {
    let server = super::setup_test_server();
    register_user(&server).await;
    let paired = pair(&server, 86, "read").await;
    let user = super::fixtures::current_user_id(&server).await;
    let capability = paired["pairing_id"].as_str().unwrap().parse().unwrap();
    let shutdown = crate::auth::lifecycle::begin_capability_shutdown(user, capability).unwrap();
    let (status, _) = call(&server, 86, "DELETE", "clients/self", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    drop(shutdown);
    let revoked = ok(&server, 86, "DELETE", "clients/self", None).await;
    assert_eq!(revoked["status"], "revoked");
    let (status, _) = call(&server, 86, "GET", "me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_json_bodies_are_bad_requests_and_unknown_clients_are_unauthorized() {
    let server = super::setup_test_server();
    register_user(&server).await;
    pair(&server, 87, "full").await;
    let account = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    for (method, path) in [
        ("PATCH", format!("wallets/{account}")),
        ("POST", "accounts/bitcoin-address".to_owned()),
        ("POST", "accounts/ethereum-address".to_owned()),
        ("POST", "accounts/xpub".to_owned()),
        ("POST", "accounts/manual".to_owned()),
        ("PATCH", format!("accounts/{account}")),
        ("POST", format!("accounts/{account}/move")),
        ("POST", format!("accounts/{account}/balance-assertions")),
        (
            "PUT",
            format!("accounts/{account}/balance-assertions/{account}"),
        ),
        ("POST", "exports/wallet-data".to_owned()),
        ("POST", "imports/wallet-data".to_owned()),
        ("POST", "xpubs/inspect".to_owned()),
        ("POST", "sync".to_owned()),
        ("PUT", "settings/currency".to_owned()),
        ("PUT", "secrets/etherscan-api-key".to_owned()),
        ("PUT", "prices/overrides".to_owned()),
        ("DELETE", "prices/overrides".to_owned()),
    ] {
        let url = format!("/api/v1/{path}");
        let request = match method {
            "POST" => server.post(&url),
            "PUT" => server.put(&url),
            "PATCH" => server.patch(&url),
            _ => server.delete(&url),
        };
        let response = request
            .add_header(header::AUTHORIZATION, bearer(87))
            .bytes(vec![b'{'].into())
            .content_type("application/json")
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::BAD_REQUEST,
            "{method} {path}: {}",
            response.text()
        );
    }

    let (status, _) = call(&server, 88, "GET", "wallets", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn paired_wallet_archive_is_encrypted_and_round_trips_wallets() {
    let server = super::setup_test_server();
    let manual_sync = crate::sync_control::set_sync_control_mode_override_for_tests(Some(
        crate::sync_control::SyncControlMode::Enabled,
    ));
    register_user(&server).await;
    let user = super::fixtures::current_user_id(&server).await;
    pair(&server, 90, "full").await;
    pair(&server, 91, "read").await;
    let account = ok(
        &server,
        90,
        "POST",
        "accounts/ethereum-address",
        Some(json!({"address": ETH_ADDRESS, "network": "mainnet", "wallet_label": "Cold"})),
    )
    .await;
    let wallet_id = account["wallet_id"].as_str().unwrap();
    let manual = super::exports::add_manual_asset_account(&server, wallet_id, "ADA").await;
    super::exports::add_custom_balance_assertion(
        &server,
        manual["account_id"].as_str().unwrap(),
        "2026-03-01",
        "125.25",
        Some("Savings balance"),
    )
    .await;
    ok(
        &server,
        90,
        "PUT",
        "settings/currency",
        Some(json!({"value":"EUR"})),
    )
    .await;
    ok(
        &server,
        90,
        "PUT",
        "secrets/coingecko-api-key",
        Some(json!({"value":"CG-archive-fixture"})),
    )
    .await;
    let password = "archive test passphrase";
    for path in ["exports/wallet-data", "imports/wallet-data"] {
        assert_eq!(
            call(
                &server,
                91,
                "POST",
                path,
                Some(json!({"password": password}))
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let archive = ok(
        &server,
        90,
        "POST",
        "exports/wallet-data",
        Some(json!({"password": password})),
    )
    .await;
    assert_eq!(archive["summary"]["encrypted"], true);
    let payload = archive["zip_base64"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(zip.by_index_raw(0).unwrap().encrypted());
    let contents = super::exports::wallet_data_payload_from_zip(payload, Some(password));
    assert!(contents["settings"].is_null());
    assert_eq!(contents["api_keys"], json!([]));
    assert!(contents.get("subscription_transfer").is_none());
    assert_eq!(archive["summary"]["balance_assertions"], 1);
    assert_eq!(archive["summary"]["addresses"], 1);
    assert_eq!(archive["summary"]["custom_accounts"], 1);
    assert!(contents.to_string().contains("Savings balance"));
    assert!(!contents.to_string().contains("CG-archive-fixture"));

    let browser_export = |encrypted| {
        server.post("/_app/user/exports/wallet-data").json(
            &super::exports::wallet_data_export_request(
                false,
                encrypted,
                encrypted.then_some(password),
            ),
        )
    };
    let plaintext: Value = browser_export(false).await.json();
    let (status, _) = call(
        &server,
        90,
        "POST",
        "imports/wallet-data",
        Some(json!({"payload_base64":plaintext["zip_base64"],"password":password})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let browser_zip: Value = browser_export(true).await.json();
    assert_eq!(browser_zip["summary"]["api_keys"], 1);
    ok(&server, 90, "DELETE", "secrets/coingecko-api-key", None).await;
    ok(
        &server,
        90,
        "PUT",
        "settings/currency",
        Some(json!({"value":"USD"})),
    )
    .await;
    ok(&server, 90, "DELETE", &format!("wallets/{wallet_id}"), None).await;
    let wrong = json!({"payload_base64": payload, "password":"incorrect"});
    assert_eq!(
        call(&server, 90, "POST", "imports/wallet-data", Some(wrong))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    server.post("/_app/auth/logout").await.assert_status_ok();
    drop(manual_sync);
    let _automatic = crate::sync_control::set_sync_control_mode_override_for_tests(Some(
        crate::sync_control::SyncControlMode::Disabled,
    ));
    let mut sync_events = crate::tasks::subscribe_transaction_sync_events(user).unwrap();
    let imported = ok(
        &server,
        90,
        "POST",
        "imports/wallet-data",
        Some(json!({"payload_base64":payload, "password":password})),
    )
    .await;
    assert_eq!(imported["sync_triggered"], true);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !matches!(
            sync_events.recv().await.unwrap().event_type,
            crate::transactions::TransactionSyncEventType::Completed
                | crate::transactions::TransactionSyncEventType::Failed
        ) {}
    })
    .await
    .expect("the import's background sync should run with the paired client's authority");
    assert_eq!(imported["wallets_created"], json!(["Cold"]));
    let wallets = ok(&server, 90, "GET", "wallets", None).await;
    assert_eq!(
        wallets["wallets"][0]["accounts"].as_array().unwrap().len(),
        2
    );
    assert_eq!(imported["assertions_created"], 1);
    let merged = ok(
        &server,
        90,
        "POST",
        "imports/wallet-data",
        Some(json!({"payload_base64":browser_zip["zip_base64"],"password":password})),
    )
    .await;
    assert_eq!(merged["assertions_created"], 0);
    assert_eq!(merged["wallets_matched"], json!(["Cold"]));
    assert_eq!(merged["settings_imported"], false);
    assert_eq!(merged["api_keys_imported"], 0);
    let settings = ok(&server, 90, "GET", "settings", None).await;
    assert_eq!(settings["currency"], "USD");
    assert_eq!(settings["secrets"]["coingecko_api_key"], false);
}

use super::{
    setup_app_test_server, setup_app_test_server_no_db, setup_test_server, setup_test_server_no_db,
    setup_test_server_with_proxy_trust,
};
use axum::http::{StatusCode, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber as _;

const NO_STORE: &str = "no-store, max-age=0";

#[derive(Clone)]
struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn captured_logs() -> (
    Arc<Mutex<Vec<u8>>>,
    impl tracing::Subscriber + Send + Sync + 'static,
) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedLogWriter(Arc::clone(&logs));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    (logs, subscriber)
}

fn assert_no_store(response: &axum_test::TestResponse) {
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some(NO_STORE)
    );
}

fn start_request() -> Value {
    serde_json::from_str::<Value>(include_str!(
        "../../../tests/fixtures/client_api/pairing-start.json"
    ))
    .unwrap()["request"]
        .clone()
}

async fn start_pairing(
    server: &super::IntegrationTestServer,
    key_byte: u8,
    client_name: &str,
) -> Value {
    let verifier = crate::client_capabilities::ClientKeyVerifier::from_raw_key(&[key_byte; 32]);
    let response = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .json(&json!({
            "client_name": client_name,
            "key_verifier": URL_SAFE_NO_PAD.encode(verifier.as_bytes()),
            "permissions": ["balances_read"]
        }))
        .await;
    response.assert_status_ok();
    response.json()
}

fn client_key(key_byte: u8) -> String {
    URL_SAFE_NO_PAD.encode([key_byte; 32])
}

fn claim_path(pairing: &Value) -> String {
    format!(
        "/api/v1/pairings/{}/claim",
        pairing["pairing_id"].as_str().unwrap()
    )
}

async fn approve_pairing(server: &super::IntegrationTestServer, pairing: &Value) {
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&approve_body(pairing))
        .await
        .assert_status_ok();
}

async fn activate_client(
    server: &super::IntegrationTestServer,
    key_byte: u8,
    client_name: &str,
) -> Value {
    let pairing = start_pairing(server, key_byte, client_name).await;
    approve_pairing(server, &pairing).await;
    server
        .post(&claim_path(&pairing))
        .add_header(
            header::AUTHORIZATION,
            format!("Bearer {}", client_key(key_byte)),
        )
        .await
        .assert_status_ok();
    pairing
}

async fn add_ethereum_wallet(server: &super::IntegrationTestServer, wallet_label: &str) {
    server
        .post("/_app/user/wallets/ethereum/add")
        .json(&json!({
            "request": {
                "address": "0x52908400098527886E0F7030069857D2E4169EE7",
                "network": "mainnet",
                "wallet_label": wallet_label
            }
        }))
        .await
        .assert_status_ok();
}

fn approve_body(pairing: &Value) -> Value {
    json!({
        "request": {
            "pairing_id": pairing["pairing_id"],
            "code": pairing["code"],
            "permissions": ["balances_read"],
            "code_matches": true,
            "expires_at": Value::Null
        }
    })
}

#[tokio::test]
async fn pairing_start_returns_public_contract_without_caching() {
    let server = setup_test_server();
    let response = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .json(&start_request())
        .await;

    response.assert_status_ok();
    assert_no_store(&response);
    let body: Value = response.json();
    assert_eq!(body["code"].as_str().map(str::len), Some(9));
    assert_eq!(body["pairing_id"].as_str().map(str::len), Some(43));
    assert_eq!(
        body["approval_url"],
        json!(format!(
            "http://example.com/pair?code={}",
            body["code"].as_str().unwrap()
        ))
    );
    assert!(body["expires_at"].as_str().is_some());
}

#[tokio::test]
async fn pairing_start_accepts_a_body_at_the_exact_limit() {
    let server = setup_test_server();
    let mut body = serde_json::to_vec(&start_request()).unwrap();
    body.resize(crate::pairing::MAX_START_BODY_BYTES, b' ');
    let response = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .bytes(body.into())
        .content_type("application/json")
        .await;

    response.assert_status_ok();
    assert_no_store(&response);
}

#[tokio::test]
async fn malformed_and_oversized_start_bodies_fail_before_database_access() {
    let server = setup_test_server_no_db();
    for body in ["{".to_owned(), " ".repeat(1025)] {
        let response = server
            .post("/api/v1/pairings")
            .add_header(header::HOST, "example.com")
            .text(body)
            .await;
        response.assert_status_bad_request();
        assert_no_store(&response);
        assert_eq!(response.json::<Value>()["code"], "bad_request");
    }
}

#[tokio::test]
async fn validation_and_unknown_public_routes_are_explicit_and_not_cached() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/api/v1/pairings")
        .add_header(header::HOST, "example.com")
        .json(&json!({
            "client_name": " bad ",
            "key_verifier": "invalid",
            "permissions": []
        }))
        .await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_no_store(&response);
    assert_eq!(response.json::<Value>()["code"], "validation");

    let response = server.get("/api/v1/not-a-route").await;
    response.assert_status_not_found();
    assert_no_store(&response);
}

#[tokio::test]
async fn trusted_proxy_fails_closed_for_missing_malformed_or_multiple_addresses() {
    let server = setup_test_server_with_proxy_trust(crate::backend::ProxyHeaderTrust::Trusted);
    for forwarded_for in [None, Some("not-an-ip"), Some("192.0.2.1, 192.0.2.2")] {
        let mut request = server
            .post("/api/v1/pairings")
            .add_header(header::HOST, "example.com")
            .json(&start_request());
        if let Some(forwarded_for) = forwarded_for {
            request = request.add_header("x-forwarded-for", forwarded_for);
        }
        let response = request.await;
        response.assert_status_bad_request();
        assert_no_store(&response);
    }
}

#[tokio::test]
async fn pairing_review_is_ssr_private_and_not_cached() {
    let server = setup_app_test_server();
    let pairing = start_pairing(&server, 41, "private workstation").await;
    let path = format!("/pair?code={}", pairing["code"].as_str().unwrap());

    let response = server.get(&path).await;
    response.assert_status_ok();
    assert_no_store(&response);
    let body = response.text();
    assert!(body.contains("Sign in to review this pairing"));
    assert!(!body.contains("private workstation"));

    super::fixtures::register_user(&server).await;
    let response = server.get(&path).await;
    response.assert_status_ok();
    assert_no_store(&response);
    let body = response.text();
    assert!(body.contains("private workstation"));
    assert!(body.contains("cannot read transactions"));
}

#[tokio::test]
async fn unknown_pairing_review_is_not_found_and_not_cached() {
    let server = setup_app_test_server_no_db();
    let response = server.get("/pair?code=0000-0000").await;
    response.assert_status_not_found();
    assert_no_store(&response);
    assert!(response.text().contains("Pairing unavailable"));
}

#[tokio::test]
async fn pairing_actions_validate_fields_csrf_auth_and_replay() {
    let server = setup_test_server();
    let pairing = start_pairing(&server, 42, "approval matrix").await;
    let mut body = approve_body(&pairing);

    body["request"]["code_matches"] = json!(false);
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    body["request"]["code_matches"] = json!(true);
    body["request"]["permissions"] = json!(["balances_read", "transactions_read"]);
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    body["request"]["permissions"] = json!(["balances_read"]);
    body["request"]["expires_at"] = json!("invalid");
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    body["request"]["expires_at"] = Value::Null;

    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .json(&body)
        .await
        .assert_status_forbidden();
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "https://evil.example")
        .json(&body)
        .await
        .assert_status_forbidden();
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await
        .assert_status_unauthorized();

    super::fixtures::register_user(&server).await;
    body["request"]["expires_at"] = json!("2126-07-31T15:00:00Z");
    let response = server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await;
    response.assert_status_ok();
    assert_no_store(&response);
    server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&body)
        .await
        .assert_status_conflict();
}

#[tokio::test]
async fn pairing_denial_accepts_same_origin_referer_and_is_single_use() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let pairing = start_pairing(&server, 43, "denial").await;
    let body = json!({
        "request": {
            "pairing_id": pairing["pairing_id"],
            "code": pairing["code"]
        }
    });
    let response = server
        .post("/_app/pairings/deny")
        .add_header(header::HOST, "example.com")
        .add_header(header::REFERER, "http://example.com/pair")
        .json(&body)
        .await;
    response.assert_status_ok();
    assert_no_store(&response);
    server
        .post("/_app/pairings/deny")
        .add_header(header::HOST, "example.com")
        .add_header(header::REFERER, "http://example.com/pair")
        .json(&body)
        .await
        .assert_status_conflict();
}

#[tokio::test]
async fn concurrent_pairing_approval_and_denial_have_one_winner() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let pairing = start_pairing(&server, 44, "concurrent transition").await;
    let approve = server
        .post("/_app/pairings/approve")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&approve_body(&pairing));
    let deny = server
        .post("/_app/pairings/deny")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&json!({
            "request": {
                "pairing_id": pairing["pairing_id"],
                "code": pairing["code"]
            }
        }));

    let (approve_response, deny_response) = tokio::join!(approve, deny);
    let mut statuses = [approve_response.status_code(), deny_response.status_code()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
    assert_no_store(&approve_response);
    assert_no_store(&deny_response);
}

#[tokio::test]
async fn malformed_pairing_actions_fail_before_database_access() {
    let server = setup_test_server_no_db();
    for path in ["/_app/pairings/approve", "/_app/pairings/deny"] {
        let response = server
            .post(path)
            .bytes(vec![b'{'].into())
            .content_type("application/json")
            .await;
        response.assert_status_bad_request();
        assert_no_store(&response);
    }
}

#[tokio::test]
async fn pairing_claim_rejects_invalid_credentials_body_and_unknown_ids_without_caching() {
    let server = setup_test_server();
    let pairing = start_pairing(&server, 51, "claim validation").await;
    let path = claim_path(&pairing);

    for authorization in [None, Some("Basic invalid"), Some("Bearer invalid")] {
        let mut request = server.post(&path);
        if let Some(authorization) = authorization {
            request = request.add_header(header::AUTHORIZATION, authorization);
        }
        let response = request.await;
        response.assert_status_unauthorized();
        assert_no_store(&response);
    }

    let response = server
        .post(&path)
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(52)))
        .await;
    response.assert_status_unauthorized();
    assert_no_store(&response);

    let response = server
        .post(&path)
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(51)))
        .text("{}")
        .await;
    response.assert_status_bad_request();
    assert_no_store(&response);

    let unknown_id = crate::client_capabilities::CapabilityId::from_bytes([99; 32]);
    let response = server
        .post(&format!("/api/v1/pairings/{unknown_id}/claim"))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(51)))
        .await;
    response.assert_status_not_found();
    assert_no_store(&response);
}

#[tokio::test]
async fn pairing_claim_reports_pending_and_denied_states_without_caching() {
    let server = setup_test_server();
    let pending = start_pairing(&server, 53, "pending claim").await;
    let response = server
        .post(&claim_path(&pending))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(53)))
        .await;
    response.assert_status(StatusCode::ACCEPTED);
    assert_no_store(&response);
    assert_eq!(
        response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("5")
    );
    assert_eq!(response.json::<Value>(), json!({"status": "pending"}));

    super::fixtures::register_user(&server).await;
    let denied = start_pairing(&server, 54, "denied claim").await;
    server
        .post("/_app/pairings/deny")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&json!({
            "request": {
                "pairing_id": denied["pairing_id"],
                "code": denied["code"]
            }
        }))
        .await
        .assert_status_ok();
    let response = server
        .post(&claim_path(&denied))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(54)))
        .await;
    response.assert_status_forbidden();
    assert_no_store(&response);
}

#[tokio::test]
async fn approved_pairing_claim_is_exactly_once_and_same_key_idempotent() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let user_id = super::fixtures::current_user_id(&server).await;
    let pairing = start_pairing(&server, 55, "durable private name").await;
    approve_pairing(&server, &pairing).await;
    let path = claim_path(&pairing);

    let first = server
        .post(&path)
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(55)))
        .await;
    first.assert_status_ok();
    assert_no_store(&first);
    assert_eq!(
        first.json::<Value>(),
        json!({
            "status": "active",
            "remote_user_id": user_id.to_string(),
            "permissions": ["balances_read"]
        })
    );

    let retry = server
        .post(&path)
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(55)))
        .await;
    retry.assert_status_ok();
    assert_eq!(retry.json::<Value>()["status"], "active");

    server
        .post(&path)
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(56)))
        .await
        .assert_status_unauthorized();

    let capability_id = pairing["pairing_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        crate::db::load_paired_client_name(user_id, capability_id).unwrap(),
        Some("durable private name".to_owned())
    );
    assert_eq!(
        crate::db::load_client_capabilities_for_user(user_id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn concurrent_pairing_claims_converge_on_one_capability() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let user_id = super::fixtures::current_user_id(&server).await;
    let pairing = start_pairing(&server, 57, "concurrent claims").await;
    approve_pairing(&server, &pairing).await;
    let path = claim_path(&pairing);
    let authorization = format!("Bearer {}", client_key(57));

    let first = server
        .post(&path)
        .add_header(header::AUTHORIZATION, authorization.clone());
    let second = server
        .post(&path)
        .add_header(header::AUTHORIZATION, authorization);
    let (first, second) = tokio::join!(first, second);
    first.assert_status_ok();
    second.assert_status_ok();
    assert_eq!(
        crate::db::load_client_capabilities_for_user(user_id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn paired_clients_list_and_revocation_preserve_other_access() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let user_id = super::fixtures::current_user_id(&server).await;

    let revoked_pairing = start_pairing(&server, 61, "revoked workstation").await;
    approve_pairing(&server, &revoked_pairing).await;
    server
        .post(&claim_path(&revoked_pairing))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(61)))
        .await
        .assert_status_ok();

    let retained_pairing = start_pairing(&server, 62, "retained workstation").await;
    approve_pairing(&server, &retained_pairing).await;
    server
        .post(&claim_path(&retained_pairing))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(62)))
        .await
        .assert_status_ok();

    let response = server.get("/_app/pairings/clients").await;
    response.assert_status_ok();
    assert_no_store(&response);
    let clients: Value = response.json();
    let clients = clients.as_array().unwrap();
    assert_eq!(clients.len(), 2);
    let revoked = clients
        .iter()
        .find(|client| client["name"] == "revoked workstation")
        .unwrap();
    assert_eq!(revoked["permission"], "balances_read");
    assert!(revoked["created_at"].is_string());
    assert!(revoked["expires_at"].is_null());
    assert!(revoked["last_used_at"].is_null());
    assert!(revoked["revoked_at"].is_null());

    let revoke_body = json!({
        "request": { "capability_id": revoked["capability_id"] }
    });
    server
        .post("/_app/pairings/revoke")
        .json(&revoke_body)
        .await
        .assert_status_forbidden();
    for _ in 0..2 {
        let response = server
            .post("/_app/pairings/revoke")
            .add_header(header::HOST, "example.com")
            .add_header(header::ORIGIN, "http://example.com")
            .json(&revoke_body)
            .await;
        response.assert_status_ok();
        assert_no_store(&response);
    }

    let clients: Value = server.get("/_app/pairings/clients").await.json();
    let revoked = clients
        .as_array()
        .unwrap()
        .iter()
        .find(|client| client["name"] == "revoked workstation")
        .unwrap();
    assert!(revoked["revoked_at"].is_string());

    server
        .post(&claim_path(&revoked_pairing))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(61)))
        .await
        .assert_status_unauthorized();
    server
        .post(&claim_path(&retained_pairing))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(62)))
        .await
        .assert_status_ok();
    server.get("/_app/auth/me").await.assert_status_ok();
    assert!(crate::db::get_user_db_dek(&user_id).unwrap().is_some());
}

#[tokio::test]
async fn wallet_balances_authenticates_client_key_without_creating_a_session() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let user_id = super::fixtures::current_user_id(&server).await;
    add_ethereum_wallet(&server, "Client Key Wallet").await;
    let pairing = activate_client(&server, 71, "balance reader").await;
    server.post("/_app/auth/logout").await.assert_status_ok();
    assert!(
        !crate::db::list_open_user_db_users()
            .unwrap()
            .contains(&user_id)
    );

    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(71)))
        .await;
    response.assert_status_ok();
    assert_no_store(&response);
    let body: Value = response.json();
    assert_eq!(body["wallets"][0]["name"], "Client Key Wallet");
    assert_eq!(body["wallets"][0]["balances"][0]["asset_id"], "ethereum");
    assert_eq!(
        body["wallets"][0]["balances"][0]["network_id"],
        "ethereum-mainnet"
    );
    assert_eq!(body["wallets"][0]["balances"][0]["amount"], "0");

    let capability_id = pairing["pairing_id"].as_str().unwrap().parse().unwrap();
    let capability = crate::db::load_client_capability(capability_id)
        .unwrap()
        .unwrap();
    assert!(capability.last_used_at.is_some());
    let session_count: i64 = crate::db::with_db(|connection| {
        connection
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| crate::db::DbError::from_rusqlite_error("count sessions", error))
    })
    .unwrap();
    assert_eq!(session_count, 0);
    assert!(
        !crate::db::list_open_user_db_users()
            .unwrap()
            .contains(&user_id)
    );
}

#[tokio::test]
async fn wallet_balances_logs_success_and_auth_failure_without_credentials() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    activate_client(&server, 79, "logged balance reader").await;
    let valid_key = client_key(79);
    let invalid_key = client_key(80);
    let (logs, subscriber) = captured_logs();

    async {
        server
            .get("/api/v1/wallet-balances")
            .add_header(header::AUTHORIZATION, format!("Bearer {valid_key}"))
            .await
            .assert_status_ok();
        server
            .get("/api/v1/wallet-balances")
            .add_header(header::AUTHORIZATION, format!("Bearer {invalid_key}"))
            .await
            .assert_status_unauthorized();
    }
    .with_subscriber(subscriber)
    .await;

    let logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let logs = String::from_utf8_lossy(&logs);
    assert!(logs.contains("INFO") && logs.contains("public API: wallet balances retrieved"));
    assert!(logs.contains("WARN") && logs.contains("public API: wallet balances failed"));
    assert!(!logs.contains(&valid_key));
    assert!(!logs.contains(&invalid_key));
}

#[cfg(feature = "dev-config")]
#[tokio::test]
async fn wallet_balances_supports_an_unencrypted_dev_pairing_until_revoked() {
    let server = setup_test_server();
    let user_id = crate::models::UserId::new();
    crate::db::setup_unencrypted_dev_test_user(user_id);
    crate::db::close_user_db(user_id).unwrap();

    let raw_key = [77_u8; 32];
    let capability_id = crate::client_capabilities::CapabilityId::from_bytes([77_u8; 32]);
    crate::db::insert_active_client_capability(
        &crate::client_capabilities::ClientCapabilityRecord {
            capability_id,
            user_id,
            key_verifier: crate::client_capabilities::ClientKeyVerifier::from_raw_key(&raw_key),
            wrapped_dek: None,
            wrap_nonce: None,
            permission: crate::client_capabilities::ClientPermission::BalancesRead,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        },
    )
    .unwrap();

    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(77)))
        .await;
    response.assert_status_ok();
    assert_no_store(&response);
    assert_eq!(response.json::<Value>(), json!({ "wallets": [] }));

    let active = crate::db::load_client_capability(capability_id)
        .unwrap()
        .unwrap();
    assert!(active.last_used_at.is_some());
    assert_eq!((active.wrapped_dek, active.wrap_nonce), (None, None));

    assert_eq!(
        crate::db::revoke_client_capability(user_id, capability_id, chrono::Utc::now()).unwrap(),
        crate::db::RevokeClientCapabilityResult::Revoked
    );
    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(77)))
        .await;
    response.assert_status_unauthorized();
    assert_no_store(&response);
}

#[tokio::test]
async fn authorized_activity_is_recorded_before_projection_failure() {
    let server = setup_test_server();
    super::fixtures::register_user(&server).await;
    let user_id = super::fixtures::current_user_id(&server).await;
    add_ethereum_wallet(&server, "Conflicting Projection Wallet").await;
    let wallet_id = crate::db::load_wallet_summary_bundle(user_id)
        .unwrap()
        .wallets[0]
        .wallet
        .id;
    let pairing = activate_client(&server, 76, "failing projection reader").await;
    let old_activity = "2000-01-01T00:00:00+00:00";
    crate::db::with_db_mut(|connection| {
        connection
            .execute(
                "UPDATE users SET last_login_at = ?1 WHERE user_id = ?2",
                rusqlite::params![old_activity, user_id.to_string()],
            )
            .map_err(|error| {
                crate::db::DbError::from_rusqlite_error("set old activity fixture", error)
            })?;
        Ok::<(), crate::db::DbError>(())
    })
    .unwrap();
    crate::db::with_user_db_mut(user_id, |connection| {
        let now = chrono::Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
                 VALUES (?1, ?2, 'Conflicting ETH', 'conflicting eth', 'ethereum',
                         'ethereum-mainnet', 18, 'WETH', NULL, 'Ethereum', 'Ethereum',
                         'ethereum', 'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?3, ?3)",
                rusqlite::params![
                    crate::wallets::WalletAccountId::new().to_string(),
                    wallet_id.to_string(),
                    now,
                ],
            )
            .map_err(|error| {
                crate::db::DbError::from_rusqlite_error("insert projection conflict", error)
            })?;
        Ok::<(), crate::db::DbError>(())
    })
    .unwrap();

    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(76)))
        .await;
    response.assert_status_internal_server_error();
    assert_no_store(&response);

    let capability_id = pairing["pairing_id"].as_str().unwrap().parse().unwrap();
    assert!(
        crate::db::load_client_capability(capability_id)
            .unwrap()
            .unwrap()
            .last_used_at
            .is_some()
    );
    let user_activity: String = crate::db::with_db(|connection| {
        connection
            .query_row(
                "SELECT last_login_at FROM users WHERE user_id = ?1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| crate::db::DbError::from_rusqlite_error("load user activity", error))
    })
    .unwrap();
    assert_ne!(user_activity, old_activity);
}

#[tokio::test]
async fn wallet_balances_rejects_invalid_revoked_and_expired_keys() {
    let server = setup_test_server();
    for authorization in [None, Some("Basic invalid"), Some("Bearer invalid")] {
        let mut request = server.get("/api/v1/wallet-balances");
        if let Some(authorization) = authorization {
            request = request.add_header(header::AUTHORIZATION, authorization);
        }
        let response = request.await;
        response.assert_status_unauthorized();
        assert_no_store(&response);
    }

    super::fixtures::register_user(&server).await;
    let revoked = activate_client(&server, 72, "revoked balance reader").await;
    server
        .post("/_app/pairings/revoke")
        .add_header(header::HOST, "example.com")
        .add_header(header::ORIGIN, "http://example.com")
        .json(&json!({
            "request": { "capability_id": revoked["pairing_id"] }
        }))
        .await
        .assert_status_ok();
    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(72)))
        .await;
    response.assert_status_unauthorized();
    assert_no_store(&response);
    let revoked_id = revoked["pairing_id"].as_str().unwrap().parse().unwrap();
    assert!(
        crate::db::load_client_capability(revoked_id)
            .unwrap()
            .unwrap()
            .last_used_at
            .is_none()
    );

    let expired = activate_client(&server, 73, "expired balance reader").await;
    let expired_id: crate::client_capabilities::CapabilityId =
        expired["pairing_id"].as_str().unwrap().parse().unwrap();
    crate::db::with_db_mut(|connection| {
        connection
            .execute(
                "UPDATE client_capabilities SET expires_at = ?1 WHERE capability_id = ?2",
                rusqlite::params![
                    (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
                    expired_id.to_string()
                ],
            )
            .map_err(|error| {
                crate::db::DbError::from_rusqlite_error("expire capability fixture", error)
            })?;
        Ok::<(), crate::db::DbError>(())
    })
    .unwrap();
    let response = server
        .get("/api/v1/wallet-balances")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(73)))
        .await;
    response.assert_status_unauthorized();
    assert_no_store(&response);
    let expired_record = crate::db::load_client_capability(expired_id)
        .unwrap()
        .unwrap();
    assert!(expired_record.wrapped_dek.is_none());
    assert!(expired_record.wrap_nonce.is_none());
    assert!(expired_record.last_used_at.is_none());
}

#[tokio::test]
async fn client_key_cannot_select_another_user_or_unlock_private_routes() {
    let server = setup_test_server();
    super::fixtures::register_user_with_prefix(&server, "client_key_owner").await;
    let owner_id = super::fixtures::current_user_id(&server).await;
    add_ethereum_wallet(&server, "Owner Wallet").await;
    activate_client(&server, 74, "owner balance reader").await;
    server.post("/_app/auth/logout").await.assert_status_ok();

    let pending = start_pairing(&server, 75, "pending powerless key").await;
    server
        .get("/_app/auth/me")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(75)))
        .await
        .assert_status_unauthorized();
    assert!(pending["pairing_id"].is_string());
    server
        .get("/_app/auth/me")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(74)))
        .await
        .assert_status_unauthorized();

    super::fixtures::register_user_with_prefix(&server, "other_user").await;
    let other_id = super::fixtures::current_user_id(&server).await;
    add_ethereum_wallet(&server, "Other Wallet").await;
    let response = server
        .get(&format!("/api/v1/wallet-balances?user_id={other_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(74)))
        .await;
    response.assert_status_ok();
    assert_no_store(&response);
    let body: Value = response.json();
    assert_eq!(body["wallets"][0]["name"], "Owner Wallet");
    assert!(!body.to_string().contains(&owner_id.to_string()));
    assert!(!body.to_string().contains("Other Wallet"));

    server
        .get("/api/v1/build")
        .add_header(header::AUTHORIZATION, format!("Bearer {}", client_key(74)))
        .await
        .assert_status_ok();
}

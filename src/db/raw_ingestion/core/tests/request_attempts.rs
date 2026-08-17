use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::user_db::with_user_db;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::Utc;

#[test]
fn record_request_attempt_persists_transport_and_deserialize_outcomes() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let sync_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect("sync run should insert");

    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse(
                    "connection reset".to_string(),
                )
                .expect("transport error message"),
            },
        },
    )
    .expect("transport request attempt should insert");

    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsAfterConfirmed,
            request_url: RequestUrl::parse(
                "https://mempool.space/api/address/bc1qtest/txs/chain/txid",
            )
            .expect("request url"),
            scope_address_id: address_id,
            page_cursor: Some(PageCursor::parse("txid").expect("page cursor")),
            page_kind: MempoolPageKind::PaginatedAfterConfirmed,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::DeserializeError(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429).expect("http status code"),
                response_headers_json: Some(
                    ResponseHeadersJson::parse("{\"retry-after\":\"30\"}".to_string())
                        .expect("response headers json"),
                ),
                response_body: CapturedResponseBody::truncate(vec![1_u8, 2, 3, 4], 3),
            }),
        },
    )
    .expect("deserialize request attempt should insert");

    type RequestAttemptRow = (
        String,
        Option<i64>,
        Option<String>,
        Option<Vec<u8>>,
        i64,
        Option<String>,
    );

    let rows: Vec<RequestAttemptRow> = with_user_db(user_id, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT outcome_kind, http_status_code, response_headers_json, response_body_truncated, response_body_was_truncated, transport_error_message
                     FROM request_attempts
                     ORDER BY created_at ASC",
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to prepare request attempt query", err)
                })?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|err| DbError::from_rusqlite_error("Failed to query request attempts", err))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| DbError::from_rusqlite_error("Failed to read request attempts", err))
        })
        .expect("request attempts should load");

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (
            "transport_error".to_string(),
            None,
            None,
            None,
            0,
            Some("connection reset".to_string()),
        )
    );
    assert_eq!(
        rows[1],
        (
            "deserialize_error".to_string(),
            Some(429),
            Some("{\"retry-after\":\"30\"}".to_string()),
            Some(vec![1_u8, 2, 3]),
            1,
            None,
        )
    );
}

#[test]
fn record_request_attempt_rejects_success_path_http_response() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let sync_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect("sync run should insert");

    // 2xx HttpResponse should be rejected
    let success_error = record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect_err("success-path HttpResponse should be rejected");
    assert!(
        success_error
            .to_string()
            .contains("only retain failure diagnostics"),
        "expected failure-diagnostics rejection error, got: {}",
        success_error
    );

    // 201 Created should also be rejected
    let created_error = record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(201).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect_err("201 Created should be rejected");
    assert!(
        created_error
            .to_string()
            .contains("only retain failure diagnostics"),
        "expected failure-diagnostics rejection error, got: {}",
        created_error
    );
}

#[test]
fn record_request_attempt_accepts_failure_outcomes() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let sync_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect("sync run should insert");

    // TransportError should be accepted
    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse(
                    "connection reset".to_string(),
                )
                .expect("transport error message"),
            },
        },
    )
    .expect("TransportError should be accepted");

    // DeserializeError should be accepted
    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::DeserializeError(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("DeserializeError should be accepted");

    // Non-2xx HttpResponse should be accepted (e.g., 500, 429)
    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(500).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("500 Internal Server Error should be accepted");

    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/bc1qtest/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("429 Too Many Requests should be accepted");
}

#[test]
fn record_etherscan_request_attempt_rejects_success_path_http_response() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("99");
    insert_test_eth_address(user_id, address_id, &watched_address);

    let sync_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Etherscan,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect("sync run should insert");

    // 2xx HttpResponse should be rejected
    let success_error = record_etherscan_request_attempt(
        user_id,
        RecordEtherscanRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: EtherscanRequestKind::NormalTransactionsPage,
            request_url: RequestUrl::parse(
                "https://api.etherscan.io/api?module=account&action=txlist",
            )
            .expect("request url"),
            scope_address_id: address_id,
            request_query_json: EtherscanQueryJson::parse("{\"module\":\"account\"}".to_string())
                .expect("query json"),
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect_err("success-path HttpResponse should be rejected");
    assert!(
        success_error
            .to_string()
            .contains("only retain failure diagnostics"),
        "expected failure-diagnostics rejection error, got: {}",
        success_error
    );
}

#[test]
fn record_etherscan_request_attempt_accepts_failure_outcomes() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("aa");
    insert_test_eth_address(user_id, address_id, &watched_address);

    let sync_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Etherscan,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect("sync run should insert");

    // TransportError should be accepted
    record_etherscan_request_attempt(
        user_id,
        RecordEtherscanRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: EtherscanRequestKind::NormalTransactionsPage,
            request_url: RequestUrl::parse(
                "https://api.etherscan.io/api?module=account&action=txlist",
            )
            .expect("request url"),
            scope_address_id: address_id,
            request_query_json: EtherscanQueryJson::parse("{\"module\":\"account\"}".to_string())
                .expect("query json"),
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse(
                    "connection timeout".to_string(),
                )
                .expect("transport error message"),
            },
        },
    )
    .expect("TransportError should be accepted");

    // Non-2xx HttpResponse should be accepted
    record_etherscan_request_attempt(
        user_id,
        RecordEtherscanRequestAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            request_kind: EtherscanRequestKind::NormalTransactionsPage,
            request_url: RequestUrl::parse(
                "https://api.etherscan.io/api?module=account&action=txlist",
            )
            .expect("request url"),
            scope_address_id: address_id,
            request_query_json: EtherscanQueryJson::parse("{\"module\":\"account\"}".to_string())
                .expect("query json"),
            attempted_at: Utc::now(),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(503).expect("http status code"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("503 Service Unavailable should be accepted");
}

use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::Utc;
use rusqlite::params;

#[test]
fn insert_raw_mempool_tx_version_reuses_current_head_for_exact_duplicate() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Mempool,
        Network::Mainnet,
        address_id,
    );
    let txid = sample_txid("01");
    let payload_bytes = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01","vin":[],"vout":[],"status":{"confirmed":true}}"#,
    );
    let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
    let first_observed_at = Utc::now();

    let first = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: payload_hash_sha256_hex.clone(),
            payload_bytes: payload_bytes.clone(),
            first_observed_at,
        },
    )
    .expect("first insert should succeed");

    let second = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: payload_hash_sha256_hex.clone(),
            payload_bytes: payload_bytes.clone(),
            first_observed_at: first_observed_at + chrono::Duration::seconds(10),
        },
    )
    .expect("second insert should reuse exact duplicate");

    assert_eq!(first.raw_version_id, second.raw_version_id);
    assert_eq!(
        second.write_outcome,
        RawVersionWriteOutcome::ReusedCurrentHead
    );

    let count: i64 = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM raw_mempool_transaction_versions WHERE network = ?1 AND txid = ?2",
                params![Network::Mainnet.as_str(), txid.as_str()],
                |row| row.get(0),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to count raw mempool transaction versions",
                    err,
                )
            })
        })
        .expect("row count query should succeed");
    assert_eq!(count, 1);
}

#[test]
fn raw_mempool_page_observation_rolls_back_when_membership_insert_fails() {
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
    let payload = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0a","vin":[],"vout":[],"status":{"confirmed":true}}"#,
    );
    let version = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: sync_run.source_connection_id.clone(),
            network: Network::Mainnet,
            txid: sample_txid("0a"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
            payload_bytes: payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("raw version should insert");

    let result = record_raw_mempool_page_observation(
        user_id,
        RecordRawMempoolPageObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: sync_run.source_connection_id,
            metadata: MempoolPageObservationMetadata {
                address_id,
                scan_start_run_id: Some(SyncRunId::new()),
                page_kind: MempoolPageKind::FirstPage,
                requested_cursor: None,
                returned_last_confirmed_cursor: Some(sample_txid("0a").as_str().to_string()),
                item_count: 2,
            },
            raw_version_ids: vec![version.raw_version_id, version.raw_version_id],
            observed_at: Utc::now(),
        },
    );

    assert!(result.is_err());
    let sets = load_raw_mempool_page_observations_for_sync_run(user_id, sync_run.sync_run_id)
        .expect("page observations should load");
    assert!(sets.is_empty());
    let memberships: i64 = with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM raw_mempool_transaction_observations WHERE sync_run_id = ?1",
            [sync_run.sync_run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to count mempool page memberships", err)
        })
    })
    .expect("membership count should load");
    assert_eq!(memberships, 0);
}

#[test]
fn load_observed_raw_mempool_transactions_for_observation_set_orders_by_page_item_index() {
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
    let _request_attempt = record_request_attempt(
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
    .expect("request attempt should insert");
    let source_connection_id = sync_run.source_connection_id.clone();
    let observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        source_connection_id.clone(),
        RawObservationSetGroupingKind::MempoolAddress,
        r#"{"page_kind":"first_page","page_cursor":null}"#,
        Utc::now(),
    );

    let first_txid = sample_txid("0b");
    let second_txid = sample_txid("0c");
    let first_payload = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0b"}"#,
    );
    let second_payload = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0c"}"#,
    );
    let first_version = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: first_txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&first_payload),
            payload_bytes: first_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("first raw version should insert");
    let second_version = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: second_txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&second_payload),
            payload_bytes: second_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("second raw version should insert");

    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: second_version.raw_version_id,
            page_item_index: 1,
            observed_at: Utc::now(),
        },
    )
    .expect("second observation should insert");
    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id,
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: first_version.raw_version_id,
            page_item_index: 0,
            observed_at: Utc::now(),
        },
    )
    .expect("first observation should insert");

    let loaded = load_observed_raw_mempool_transactions_for_observation_set(
        user_id,
        observation_set.raw_observation_set_id,
    )
    .expect("observed raw transactions should load");

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].page_item_index, 0);
    assert_eq!(loaded[0].txid, first_txid);
    assert_eq!(loaded[1].page_item_index, 1);
    assert_eq!(loaded[1].txid, second_txid);
}

#[test]
fn insert_raw_mempool_tx_version_tracks_lineage_and_creates_fresh_head_on_reversion() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Mempool,
        Network::Mainnet,
        address_id,
    );
    let txid = sample_txid("13");
    let payload_a = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa13","status":{"confirmed":true}}"#,
    );
    let payload_b = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa13","status":{"confirmed":false}}"#,
    );
    let observed_at = Utc::now();
    let source_connection_id_for_query = source_connection_id.clone();

    let first = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a.clone(),
            first_observed_at: observed_at,
        },
    )
    .expect("first raw version should insert");
    let second = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_b),
            payload_bytes: payload_b,
            first_observed_at: observed_at + chrono::Duration::seconds(1),
        },
    )
    .expect("second raw version should insert");
    let third = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id,
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a,
            first_observed_at: observed_at + chrono::Duration::seconds(2),
        },
    )
    .expect("third raw version should insert a fresh head");

    assert_ne!(first.raw_version_id, second.raw_version_id);
    assert_ne!(first.raw_version_id, third.raw_version_id);
    assert_eq!(third.write_outcome, RawVersionWriteOutcome::InsertedNewHead);

    let rows: Vec<(String, Option<String>)> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, supersedes_raw_version_id
                     FROM raw_mempool_transaction_versions
                     WHERE source_connection_id = ?1 AND txid = ?2
                     ORDER BY created_at ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare raw mempool lineage query", err)
            })?;
        stmt.query_map(
            params![source_connection_id_for_query.to_string(), txid.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query raw mempool lineage rows", err)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("Failed to read raw mempool lineage rows", err))
    })
    .expect("raw mempool lineage rows should load");

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (first.raw_version_id.to_string(), None));
    assert_eq!(
        rows[1],
        (
            second.raw_version_id.to_string(),
            Some(first.raw_version_id.to_string()),
        )
    );
    assert_eq!(
        rows[2],
        (
            third.raw_version_id.to_string(),
            Some(second.raw_version_id.to_string()),
        )
    );
}

#[test]
fn load_current_raw_mempool_transaction_heads_returns_one_head_per_txid() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Mempool,
        Network::Mainnet,
        address_id,
    );
    let observed_at = Utc::now();
    let txid_a = sample_txid("41");
    let txid_b = sample_txid("42");

    for (txid, payload, seconds) in [
        (
            txid_a.clone(),
            r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa41","status":{"confirmed":true}}"#,
            0,
        ),
        (
            txid_a.clone(),
            r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa41","status":{"confirmed":false}}"#,
            1,
        ),
        (
            txid_b.clone(),
            r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa42","status":{"confirmed":true}}"#,
            2,
        ),
    ] {
        let payload_bytes = sample_payload(payload);
        insert_raw_mempool_tx_version(
            user_id,
            InsertRawMempoolTransactionVersionRequest {
                source_connection_id: source_connection_id.clone(),
                network: Network::Mainnet,
                txid,
                payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_bytes),
                payload_bytes,
                first_observed_at: observed_at + chrono::Duration::seconds(seconds),
            },
        )
        .expect("raw version should insert");
    }

    let heads = load_current_raw_mempool_transaction_heads(user_id, &source_connection_id)
        .expect("current heads should load");

    assert_eq!(heads.len(), 2);
    assert_eq!(heads[0].txid, txid_a);
    assert_eq!(heads[1].txid, txid_b);
    assert!(
        String::from_utf8_lossy(heads[0].payload_bytes.as_slice()).contains("\"confirmed\":false")
    );
}

#[test]
fn repair_legacy_mempool_head_rebuild_contract_promotes_latest_observed_payload_to_head() {
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
            started_at: utc_dt(2026, 4, 1, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("sync run should insert");
    let source_connection_id = sync_run.source_connection_id.clone();
    let txid = sample_txid("71");
    let payload_a = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa71","status":{"confirmed":true}}"#,
    );
    let payload_b = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa71","status":{"confirmed":false}}"#,
    );

    let first_observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        source_connection_id.clone(),
        RawObservationSetGroupingKind::MempoolAddress,
        r#"{"page_kind":"first_page","page_cursor":null}"#,
        utc_dt(2026, 4, 1, 0, 1, 0),
    );
    let second_observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        source_connection_id.clone(),
        RawObservationSetGroupingKind::MempoolAddress,
        r#"{"page_kind":"first_page","page_cursor":null}"#,
        utc_dt(2026, 4, 1, 0, 2, 0),
    );
    let third_observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        source_connection_id.clone(),
        RawObservationSetGroupingKind::MempoolAddress,
        r#"{"page_kind":"first_page","page_cursor":null}"#,
        utc_dt(2026, 4, 1, 0, 3, 0),
    );

    let version_a = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a.clone(),
            first_observed_at: utc_dt(2026, 4, 1, 0, 1, 30),
        },
    )
    .expect("first mempool version should insert");
    let version_b = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            network: Network::Mainnet,
            txid: txid.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_b),
            payload_bytes: payload_b.clone(),
            first_observed_at: utc_dt(2026, 4, 1, 0, 2, 30),
        },
    )
    .expect("second mempool version should insert");

    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: first_observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: version_a.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 4, 1, 0, 1, 0),
        },
    )
    .expect("first observation should persist");
    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: second_observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: version_b.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 4, 1, 0, 2, 0),
        },
    )
    .expect("second observation should persist");
    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: third_observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: version_a.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 4, 1, 0, 3, 0),
        },
    )
    .expect("legacy reverted observation should persist");

    let stale_heads = load_current_raw_mempool_transaction_heads(user_id, &source_connection_id)
        .expect("stale heads should load");
    assert_eq!(stale_heads.len(), 1);
    assert!(
        String::from_utf8_lossy(stale_heads[0].payload_bytes.as_slice())
            .contains("\"confirmed\":false")
    );

    let repaired_count = with_user_db_mut(user_id, |conn| {
        repair_legacy_mempool_head_rebuild_contract(conn)
    })
    .expect("legacy repair should succeed");
    assert_eq!(repaired_count, 1);

    let repaired_heads = load_current_raw_mempool_transaction_heads(user_id, &source_connection_id)
        .expect("repaired heads should load");
    assert_eq!(repaired_heads.len(), 1);
    assert!(
        String::from_utf8_lossy(repaired_heads[0].payload_bytes.as_slice())
            .contains("\"confirmed\":true")
    );

    let repaired_again = with_user_db_mut(user_id, |conn| {
        repair_legacy_mempool_head_rebuild_contract(conn)
    })
    .expect("second legacy repair should succeed");
    assert_eq!(repaired_again, 0);

    let lineage: Vec<(String, Option<String>)> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, supersedes_raw_version_id
                     FROM raw_mempool_transaction_versions
                     WHERE source_connection_id = ?1 AND txid = ?2
                     ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare repaired mempool lineage query",
                    err,
                )
            })?;
        stmt.query_map(
            params![source_connection_id.to_string(), txid.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query repaired mempool lineage rows", err)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to read repaired mempool lineage rows", err)
        })
    })
    .expect("repaired lineage rows should load");

    assert_eq!(lineage.len(), 3);
    assert_eq!(lineage[0], (version_a.raw_version_id.to_string(), None));
    assert_eq!(
        lineage[1],
        (
            version_b.raw_version_id.to_string(),
            Some(version_a.raw_version_id.to_string()),
        )
    );
    assert_eq!(
        lineage[2].1,
        Some(version_b.raw_version_id.to_string()),
        "repaired head should supersede the stale current head"
    );
}

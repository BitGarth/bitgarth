use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::sqlite_config::SqliteAutoVacuumMode;
use crate::db::user_db::with_user_db;
use crate::models::{SyncHistoryRetentionDays, UserId};
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::Utc;

#[test]
fn select_prunable_sync_run_ids_prunes_old_successes_and_uses_strict_failure_ttl() {
    let successful_cutoff = utc_dt(2026, 4, 1, 0, 0, 0);
    let failure_cutoff = utc_dt(2026, 4, 2, 12, 0, 0);
    let candidates = vec![
        cleanup_candidate(
            "run-old-success",
            "source-a",
            SyncRunStatus::CompletedSuccess,
            utc_dt(2026, 3, 1, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-anchor-success",
            "source-a",
            SyncRunStatus::CompletedSuccess,
            utc_dt(2026, 3, 15, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-old-failure",
            "source-a",
            SyncRunStatus::CompletedFailure,
            utc_dt(2026, 3, 10, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-recent-failure",
            "source-a",
            SyncRunStatus::CompletedFailure,
            utc_dt(2026, 4, 2, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-old-source-b-failure",
            "source-b",
            SyncRunStatus::CompletedFailure,
            utc_dt(2026, 3, 5, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-old-started",
            "source-a",
            SyncRunStatus::Started,
            utc_dt(2026, 4, 1, 0, 0, 0),
        ),
        cleanup_candidate(
            "run-recent-started",
            "source-a",
            SyncRunStatus::Started,
            utc_dt(2026, 4, 3, 0, 0, 0),
        ),
    ];

    let prunable = select_prunable_sync_run_ids(&candidates, successful_cutoff, failure_cutoff);

    assert_eq!(
        prunable,
        vec![
            "run-old-success".to_string(),
            "run-anchor-success".to_string(),
            "run-old-failure".to_string(),
            "run-recent-failure".to_string(),
            "run-old-source-b-failure".to_string(),
            "run-old-started".to_string(),
        ]
    );
}

#[test]
fn incremental_vacuum_pages_to_request_only_supports_incremental_mode() {
    assert_eq!(
        incremental_vacuum_pages_to_request(SqliteAutoVacuumMode::None, 17),
        0
    );
    assert_eq!(
        incremental_vacuum_pages_to_request(SqliteAutoVacuumMode::Full, 17),
        0
    );
    assert_eq!(
        incremental_vacuum_pages_to_request(SqliteAutoVacuumMode::Incremental, 17),
        17
    );
}

#[test]
fn cleanup_report_skips_compaction_when_auto_vacuum_is_not_incremental() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test_with_auto_vacuum_mode(
        user_id,
        SqliteAutoVacuumMode::None,
    )
    .expect("seeded user db should initialize");

    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    create_old_failure_run_with_large_request_attempts(user_id, address_id, 12, 65_536);

    let retention = SyncHistoryRetentionDays::try_new(14).expect("retention should validate");
    let cleanup_at = utc_dt(2026, 4, 4, 0, 0, 0);

    let report = cleanup_raw_sync_history_with_compaction(user_id, cleanup_at, retention)
        .expect("cleanup report should load");

    assert_eq!(report.deletion.deleted_sync_runs, 1);
    assert_eq!(
        report.compaction.auto_vacuum_mode,
        SqliteAutoVacuumMode::None
    );
    assert_eq!(report.compaction.incremental_vacuum_pages_requested, 0);
    assert_eq!(report.compaction.pages_reclaimed_by_compaction, 0);
    assert_eq!(
        report.compaction.page_count_after_compaction,
        report.compaction.page_count_before_compaction
    );
    assert_eq!(
        report.compaction.freelist_pages_after_compaction,
        report.compaction.freelist_pages_after_cleanup
    );
}

#[test]
fn cleanup_report_runs_incremental_vacuum_for_incremental_auto_vacuum_dbs() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");

    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    create_old_failure_run_with_large_request_attempts(user_id, address_id, 12, 65_536);

    let retention = SyncHistoryRetentionDays::try_new(14).expect("retention should validate");
    let cleanup_at = utc_dt(2026, 4, 4, 0, 0, 0);

    let report = cleanup_raw_sync_history_with_compaction(user_id, cleanup_at, retention)
        .expect("cleanup report should load");

    assert_eq!(report.deletion.deleted_sync_runs, 1);
    assert_eq!(
        report.compaction.auto_vacuum_mode,
        SqliteAutoVacuumMode::Incremental
    );
    assert!(
        report.compaction.pages_freed_by_cleanup > 0,
        "large deleted blobs should free at least one SQLite page"
    );
    assert_eq!(
        report.compaction.incremental_vacuum_pages_requested,
        report.compaction.pages_freed_by_cleanup
    );
    assert!(
        report.compaction.page_count_after_compaction
            < report.compaction.page_count_before_compaction,
        "incremental vacuum should reclaim on-disk pages for compatible databases"
    );
    assert!(
        report.compaction.freelist_pages_after_compaction
            <= report.compaction.freelist_pages_after_cleanup,
        "incremental vacuum should not increase the freelist"
    );
    assert!(
        report.compaction.pages_reclaimed_by_compaction > 0,
        "incremental vacuum should reclaim pages when cleanup freed pages"
    );
}

#[test]
fn cleanup_raw_sync_history_prunes_stale_started_runs_using_started_at() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);

    let stale_started_run = start_sync_run(
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
    .expect("stale started run should insert");

    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: stale_started_run.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/test/txs")
                .expect("request url"),
            scope_address_id: address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: utc_dt(2026, 4, 1, 0, 5, 0),
            outcome: RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse(
                    "temporary upstream failure".to_string(),
                )
                .expect("transport error"),
            },
        },
    )
    .expect("stale started run request attempt should persist");
    record_raw_parse_attempt(
        user_id,
        RecordRawParseAttemptRequest {
            sync_run_id: stale_started_run.sync_run_id,
            integration: IntegrationKind::Mempool,
            raw_object_key: RawObjectKey::Mempool {
                txid: sample_txid("61"),
            },
            raw_version_id: RawVersionId::Mempool(RawMempoolTransactionVersionId::new()),
            parser_kind: RawParserKind::Mempool,
            parser_version: ParserVersion::parse("mempool-v1").expect("parser version"),
            status: RawParseAttemptStatus::Failure,
            error_message: Some(
                ParseFailureMessage::parse("parser failed".to_string())
                    .expect("parse failure message"),
            ),
            attempted_at: utc_dt(2026, 4, 1, 0, 6, 0),
        },
    )
    .expect("stale started run parse attempt should persist");

    let recent_started_run = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 4, 4, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("recent started run should insert");

    let retention = SyncHistoryRetentionDays::try_new(14).expect("retention should validate");
    let cleanup_at = utc_dt(2026, 4, 4, 12, 0, 1);

    let stats =
        cleanup_raw_sync_history(user_id, cleanup_at, retention).expect("cleanup should work");

    assert_eq!(
        stats,
        RawSyncHistoryCleanupStats {
            deleted_sync_runs: 1,
            deleted_request_attempts: 1,
            deleted_raw_observation_sets: 0,
            deleted_raw_parse_attempts: 1,
            deleted_raw_mempool_transaction_observations: 0,
            deleted_raw_etherscan_normal_transaction_observations: 0,
            deleted_raw_etherscan_internal_transaction_observations: 0,
        }
    );

    let remaining_runs: Vec<String> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM sync_runs ORDER BY started_at ASC")
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare remaining started sync runs query",
                    err,
                )
            })?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query remaining started sync runs", err)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to read remaining started sync runs", err)
            })
    })
    .expect("remaining runs should load");

    assert_eq!(
        remaining_runs,
        vec![recent_started_run.sync_run_id.to_string()]
    );
}

#[test]
fn cleanup_raw_sync_history_prunes_old_runs_cascades_dependents_and_keeps_raw_versions() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");

    let mempool_address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, mempool_address_id);

    let etherscan_address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("55");
    insert_test_eth_address(user_id, etherscan_address_id, &watched_address);

    let old_mempool_failure = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: mempool_address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 2, 28, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old mempool failure should start");
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: old_mempool_failure.sync_run_id,
            status: SyncRunStatus::CompletedFailure,
            completed_at: utc_dt(2026, 3, 1, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old mempool failure should complete");

    record_request_attempt(
        user_id,
        RecordRequestAttemptRequest {
            sync_run_id: old_mempool_failure.sync_run_id,
            request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
            request_url: RequestUrl::parse("https://mempool.space/api/address/test/txs")
                .expect("request url"),
            scope_address_id: mempool_address_id,
            page_cursor: None,
            page_kind: MempoolPageKind::FirstPage,
            attempted_at: utc_dt(2026, 3, 1, 0, 1, 0),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429).expect("http status"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("old mempool request attempt should persist");
    let old_mempool_observation_set = record_test_observation_set(
        user_id,
        old_mempool_failure.sync_run_id,
        old_mempool_failure.source_connection_id.clone(),
        RawObservationSetGroupingKind::MempoolAddress,
        r#"{"page_kind":"first_page","page_cursor":null}"#,
        utc_dt(2026, 3, 1, 0, 2, 0),
    );
    let mempool_payload = sample_payload(
        r#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa41"}"#,
    );
    let old_mempool_version = insert_raw_mempool_tx_version(
        user_id,
        InsertRawMempoolTransactionVersionRequest {
            source_connection_id: old_mempool_failure.source_connection_id.clone(),
            network: Network::Mainnet,
            txid: sample_txid("41"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&mempool_payload),
            payload_bytes: mempool_payload,
            first_observed_at: utc_dt(2026, 3, 1, 0, 2, 30),
        },
    )
    .expect("old mempool raw version should persist");
    record_raw_mempool_tx_observation(
        user_id,
        RecordRawMempoolTransactionObservationRequest {
            sync_run_id: old_mempool_failure.sync_run_id,
            source_connection_id: old_mempool_failure.source_connection_id.clone(),
            raw_observation_set_id: old_mempool_observation_set.raw_observation_set_id,
            raw_mempool_transaction_version_id: old_mempool_version.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 3, 1, 0, 3, 0),
        },
    )
    .expect("old mempool observation should persist");
    record_raw_parse_attempt(
        user_id,
        RecordRawParseAttemptRequest {
            sync_run_id: old_mempool_failure.sync_run_id,
            integration: IntegrationKind::Mempool,
            raw_object_key: RawObjectKey::Mempool {
                txid: sample_txid("41"),
            },
            raw_version_id: RawVersionId::Mempool(old_mempool_version.raw_version_id),
            parser_kind: RawParserKind::Mempool,
            parser_version: ParserVersion::parse("mempool-v1").expect("parser version"),
            status: RawParseAttemptStatus::Failure,
            error_message: Some(
                ParseFailureMessage::parse("legacy parse failure".to_string())
                    .expect("failure message"),
            ),
            attempted_at: utc_dt(2026, 3, 1, 0, 4, 0),
        },
    )
    .expect("old mempool parse attempt should persist");

    let old_etherscan_success = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Etherscan,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: etherscan_address_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 3, 9, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old etherscan success should start");
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: old_etherscan_success.sync_run_id,
            status: SyncRunStatus::CompletedSuccess,
            completed_at: utc_dt(2026, 3, 10, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old etherscan success should complete");

    record_etherscan_request_attempt(
        user_id,
        RecordEtherscanRequestAttemptRequest {
            sync_run_id: old_etherscan_success.sync_run_id,
            request_kind: EtherscanRequestKind::NormalTransactionsPage,
            request_url: RequestUrl::parse("https://api.etherscan.io/v2/api")
                .expect("etherscan request url"),
            scope_address_id: etherscan_address_id,
            request_query_json: EtherscanQueryJson::parse(
                r#"{"module":"account","action":"txlist"}"#.to_string(),
            )
            .expect("etherscan query json"),
            attempted_at: utc_dt(2026, 3, 10, 0, 1, 0),
            outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429).expect("http status"),
                response_headers_json: None,
                response_body: None,
            }),
        },
    )
    .expect("old etherscan request attempt should persist");
    let old_normal_observation_set = record_test_observation_set(
        user_id,
        old_etherscan_success.sync_run_id,
        old_etherscan_success.source_connection_id.clone(),
        RawObservationSetGroupingKind::EtherscanNormal,
        r#"{"endpoint_family":"txlist","page_number":1,"page_size":1000,"start_block":"0","end_block":"99999999","window_index":0}"#,
        utc_dt(2026, 3, 10, 0, 2, 0),
    );
    let old_internal_observation_set = record_test_observation_set(
        user_id,
        old_etherscan_success.sync_run_id,
        old_etherscan_success.source_connection_id.clone(),
        RawObservationSetGroupingKind::EtherscanInternal,
        r#"{"endpoint_family":"txlistinternal","page_number":1,"page_size":1000,"start_block":"0","end_block":"99999999","window_index":0}"#,
        utc_dt(2026, 3, 10, 0, 2, 30),
    );
    let normal_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa51","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#,
    );
    let internal_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa52","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","isError":"0","type":"call","traceId":"0"}"#,
    );
    let old_normal_version = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: old_etherscan_success.source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("51"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&normal_payload),
            payload_bytes: normal_payload,
            first_observed_at: utc_dt(2026, 3, 10, 0, 3, 0),
        },
    )
    .expect("old normal raw version should persist");
    let old_internal_version = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: old_etherscan_success.source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("52"),
            trace_id: EtherscanTraceId::parse("0").expect("trace id"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&internal_payload),
            payload_bytes: internal_payload,
            first_observed_at: utc_dt(2026, 3, 10, 0, 3, 30),
        },
    )
    .expect("old internal raw version should persist");
    record_raw_etherscan_normal_observation(
        user_id,
        RecordRawEtherscanNormalTransactionObservationRequest {
            sync_run_id: old_etherscan_success.sync_run_id,
            source_connection_id: old_etherscan_success.source_connection_id.clone(),
            raw_observation_set_id: old_normal_observation_set.raw_observation_set_id,
            raw_etherscan_normal_transaction_version_id: old_normal_version.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 3, 10, 0, 4, 0),
        },
    )
    .expect("old normal observation should persist");
    record_raw_etherscan_internal_observation(
        user_id,
        RecordRawEtherscanInternalTransactionObservationRequest {
            sync_run_id: old_etherscan_success.sync_run_id,
            source_connection_id: old_etherscan_success.source_connection_id.clone(),
            raw_observation_set_id: old_internal_observation_set.raw_observation_set_id,
            raw_etherscan_internal_transaction_version_id: old_internal_version.raw_version_id,
            page_item_index: 0,
            observed_at: utc_dt(2026, 3, 10, 0, 4, 30),
        },
    )
    .expect("old internal observation should persist");
    record_raw_parse_attempt(
        user_id,
        RecordRawParseAttemptRequest {
            sync_run_id: old_etherscan_success.sync_run_id,
            integration: IntegrationKind::Etherscan,
            raw_object_key: RawObjectKey::EtherscanNormal {
                tx_hash: sample_txid("51"),
            },
            raw_version_id: RawVersionId::EtherscanNormal(old_normal_version.raw_version_id),
            parser_kind: RawParserKind::EtherscanNormal,
            parser_version: ParserVersion::parse("etherscan-v1").expect("parser version"),
            status: RawParseAttemptStatus::Failure,
            error_message: Some(
                ParseFailureMessage::parse("legacy etherscan parse failure".to_string())
                    .expect("failure message"),
            ),
            attempted_at: utc_dt(2026, 3, 10, 0, 5, 0),
        },
    )
    .expect("old etherscan parse attempt should persist");

    let protected_old_success = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Etherscan,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: etherscan_address_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 3, 19, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("protected old success should start");
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: protected_old_success.sync_run_id,
            status: SyncRunStatus::CompletedSuccess,
            completed_at: utc_dt(2026, 3, 20, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("protected old success should complete");

    let recent_mempool_failure = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: mempool_address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 4, 2, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("recent mempool failure should start");
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: recent_mempool_failure.sync_run_id,
            status: SyncRunStatus::CompletedFailure,
            completed_at: utc_dt(2026, 4, 3, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("recent mempool failure should complete");

    let retention = SyncHistoryRetentionDays::try_new(14).expect("retention should validate");
    let cleanup_at = utc_dt(2026, 4, 4, 0, 0, 0);

    let first_stats =
        cleanup_raw_sync_history(user_id, cleanup_at, retention).expect("cleanup should work");

    assert_eq!(
        first_stats,
        RawSyncHistoryCleanupStats {
            deleted_sync_runs: 3,
            deleted_request_attempts: 2,
            deleted_raw_observation_sets: 3,
            deleted_raw_parse_attempts: 2,
            deleted_raw_mempool_transaction_observations: 1,
            deleted_raw_etherscan_normal_transaction_observations: 1,
            deleted_raw_etherscan_internal_transaction_observations: 1,
        }
    );

    let post_cleanup_counts: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) =
        with_user_db(user_id, |conn| {
            let tuple = (
                conn.query_row("SELECT COUNT(*) FROM sync_runs", [], |row| row.get(0))
                    .map_err(|err| {
                        DbError::from_rusqlite_error("Failed to count sync runs", err)
                    })?,
                conn.query_row("SELECT COUNT(*) FROM request_attempts", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to count request attempts", err)
                })?,
                conn.query_row("SELECT COUNT(*) FROM raw_observation_sets", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to count raw observation sets", err)
                })?,
                conn.query_row("SELECT COUNT(*) FROM raw_parse_attempts", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to count raw parse attempts", err)
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_mempool_transaction_observations",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to count raw mempool observations", err)
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_etherscan_normal_transaction_observations",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to count raw etherscan normal observations",
                        err,
                    )
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_etherscan_internal_transaction_observations",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to count raw etherscan internal observations",
                        err,
                    )
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_mempool_transaction_versions",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to count raw mempool versions", err)
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_etherscan_normal_transaction_versions",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to count raw etherscan normal versions",
                        err,
                    )
                })?,
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_etherscan_internal_transaction_versions",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to count raw etherscan internal versions",
                        err,
                    )
                })?,
            );
            Ok::<_, DbError>(tuple)
        })
        .expect("post-cleanup counts should load");

    assert_eq!(post_cleanup_counts.0, 1);
    assert_eq!(post_cleanup_counts.1, 0);
    assert_eq!(post_cleanup_counts.2, 0);
    assert_eq!(post_cleanup_counts.3, 0);
    assert_eq!(post_cleanup_counts.4, 0);
    assert_eq!(post_cleanup_counts.5, 0);
    assert_eq!(post_cleanup_counts.6, 0);
    assert_eq!(post_cleanup_counts.7, 1);
    assert_eq!(post_cleanup_counts.8, 1);
    assert_eq!(post_cleanup_counts.9, 1);

    let remaining_runs: Vec<String> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM sync_runs ORDER BY completed_at ASC")
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare remaining sync runs query", err)
            })?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query remaining sync runs", err)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| DbError::from_rusqlite_error("Failed to read remaining sync runs", err))
    })
    .expect("remaining runs should load");

    assert_eq!(
        remaining_runs,
        vec![recent_mempool_failure.sync_run_id.to_string()]
    );

    let second_stats =
        cleanup_raw_sync_history(user_id, cleanup_at, retention).expect("second cleanup works");
    assert_eq!(second_stats, RawSyncHistoryCleanupStats::default());
}

#[test]
fn start_and_complete_sync_run_persists_expected_state() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let started_at = Utc::now();

    let started = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Scheduled,
            started_at,
            summary_json: Some(
                OpaqueJsonText::parse("{\"phase\":1}".to_string()).expect("json should parse"),
            ),
        },
    )
    .expect("sync run should insert");

    let completed_at = started_at + chrono::Duration::seconds(5);
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: started.sync_run_id,
            status: SyncRunStatus::CompletedSuccess,
            completed_at,
            summary_json: Some(
                OpaqueJsonText::parse("{\"done\":true}".to_string()).expect("json should parse"),
            ),
        },
    )
    .expect("sync run should complete");

    let result: Result<(), DbError> = with_user_db(user_id, |conn| {
        let row = conn
                .query_row(
                    "SELECT integration, scope_kind, asset_id, network, trigger_kind, status, completed_at FROM sync_runs WHERE id = ?1",
                    [started.sync_run_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .map_err(|err| DbError::from_rusqlite_error("Failed to load sync run", err))?;
        assert_eq!(row.0, "mempool");
        assert_eq!(row.1, "address");
        assert_eq!(row.2, "bitcoin");
        assert_eq!(row.3, "mainnet");
        assert_eq!(row.4, "scheduled");
        assert_eq!(row.5, "completed_success");
        assert_eq!(row.6, completed_at.to_rfc3339());
        Ok(())
    });
    result.expect("sync run should be queryable");
}

#[test]
fn start_sync_run_supports_etherscan_integration() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("42");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let started_at = Utc::now();

    let started = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Etherscan,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at,
            summary_json: None,
        },
    )
    .expect("etherscan sync run should insert");

    let integration: String = with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT integration FROM sync_runs WHERE id = ?1",
            [started.sync_run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to load sync run", err))
    })
    .expect("sync run should load");
    assert_eq!(integration, "etherscan");
}

#[test]
fn complete_sync_run_rejects_started_status() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    insert_test_address(user_id, address_id);
    let started = start_sync_run(
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

    let error = complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: started.sync_run_id,
            status: SyncRunStatus::Started,
            completed_at: Utc::now(),
            summary_json: None,
        },
    )
    .expect_err("started status should be rejected");
    assert!(
        error
            .to_string()
            .contains("cannot set sync run status back to started")
    );
}

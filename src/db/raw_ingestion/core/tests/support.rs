use super::super::*;
use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::ethereum::{EthAddress, RawEthAddress};
use crate::models::UserId;
use crate::transactions::TxHash;
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::params;

pub(super) fn sample_txid(suffix_hex: &str) -> TxHash {
    let prefix_len = 64_usize.saturating_sub(suffix_hex.len());
    let txid = format!("{}{}", "a".repeat(prefix_len), suffix_hex);
    TxHash::parse(&txid).expect("sample txid should parse")
}

pub(super) fn sample_payload(raw: &str) -> ExactPayloadBytes {
    ExactPayloadBytes::try_new(raw.as_bytes().to_vec()).expect("payload should be non-empty")
}

pub(super) fn sample_eth_address(last_hex: &str) -> EthAddress {
    let prefix_len = 40_usize.saturating_sub(last_hex.len());
    let raw = RawEthAddress::new(format!("0x{}{}", "1".repeat(prefix_len), last_hex));
    EthAddress::parse(&raw).expect("sample eth address should parse")
}

pub(super) fn insert_test_address(user_id: UserId, address_id: DigitalAssetAddressId) {
    let result: Result<(), DbError> = with_user_db_mut(user_id, |conn| {
        let inserted_at = Utc::now();
        let now = inserted_at.to_rfc3339();
        let tx = conn
            .transaction()
            .map_err(|err| DbError::new(format!("Failed to start test address tx: {err}")))?;
        tx.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    address_id.to_string(),
                    Option::<String>::None,
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    "bc1qrawingestiontest00000000000000000000000000000",
                    "bc1qrawingestiontest00000000000000000000000000000",
                    "native_segwit",
                    Option::<i64>::None,
                    Option::<i64>::None,
                    "observed",
                    now,
                    now,
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("Failed to insert test address", err))?;
        ensure_source_connection_for_address_tx(
            &tx,
            address_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            "bc1qrawingestiontest00000000000000000000000000000",
            inserted_at,
        )?;
        tx.commit()
            .map_err(|err| DbError::new(format!("Failed to commit test address tx: {err}")))?;
        Ok(())
    });
    result.expect("test address insert should succeed");
}

pub(super) fn insert_test_eth_address(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    address: &EthAddress,
) {
    let result: Result<(), DbError> = with_user_db_mut(user_id, |conn| {
        let inserted_at = Utc::now();
        let now = inserted_at.to_rfc3339();
        let tx = conn
            .transaction()
            .map_err(|err| DbError::new(format!("Failed to start test eth address tx: {err}")))?;
        tx.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    address_id.to_string(),
                    Option::<String>::None,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    address.checksummed(),
                    address.normalized(),
                    "standard",
                    Option::<i64>::None,
                    Option::<i64>::None,
                    "observed",
                    now,
                    now,
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("Failed to insert test eth address", err))?;
        ensure_source_connection_for_address_tx(
            &tx,
            address_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &address.normalized(),
            inserted_at,
        )?;
        tx.commit()
            .map_err(|err| DbError::new(format!("Failed to commit test eth address tx: {err}")))?;
        Ok(())
    });
    result.expect("test eth address insert should succeed");
}

pub(super) fn start_etherscan_sync_run(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> StartedSyncRun {
    start_sync_run(
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
    .expect("etherscan sync run should insert")
}

pub(super) fn source_connection_id_for_address(
    user_id: UserId,
    integration: IntegrationKind,
    network: Network,
    address_id: DigitalAssetAddressId,
) -> SourceConnectionId {
    with_user_db(user_id, |conn| {
        load_active_source_connection_id(conn, integration, network, address_id)
    })
    .expect("source connection should load for address")
}

pub(super) fn record_test_observation_set(
    user_id: UserId,
    sync_run_id: SyncRunId,
    source_connection_id: SourceConnectionId,
    grouping_kind: RawObservationSetGroupingKind,
    grouping_metadata_json: &str,
    observed_at: DateTime<Utc>,
) -> RecordedRawObservationSet {
    record_raw_observation_set(
        user_id,
        RecordRawObservationSetRequest {
            sync_run_id,
            source_connection_id,
            grouping_kind,
            grouping_metadata_json: RawObservationMetadataJson::parse(
                grouping_metadata_json.to_string(),
            )
            .expect("grouping metadata"),
            observed_at,
        },
    )
    .expect("raw observation set should insert")
}

pub(super) fn utc_dt(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .expect("valid UTC datetime")
}

pub(super) fn cleanup_candidate(
    sync_run_id: &str,
    source_connection_id: &str,
    status: SyncRunStatus,
    age_anchor: DateTime<Utc>,
) -> SyncRunRetentionCandidate {
    SyncRunRetentionCandidate {
        sync_run_id: sync_run_id.to_string(),
        source_connection_id: source_connection_id.to_string(),
        status,
        age_anchor,
    }
}

pub(super) fn large_captured_response_body(bytes: usize) -> CapturedResponseBody {
    CapturedResponseBody::truncate(vec![b'x'; bytes], bytes).expect("captured body")
}

pub(super) fn create_old_failure_run_with_large_request_attempts(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    request_count: usize,
    response_body_bytes: usize,
) {
    let old_failure = start_sync_run(
        user_id,
        StartSyncRunRequest {
            integration: IntegrationKind::Mempool,
            scope_kind: SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: SyncRunTriggerKind::Manual,
            started_at: utc_dt(2026, 3, 1, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old failure sync run should start");
    complete_sync_run(
        user_id,
        CompleteSyncRunRequest {
            sync_run_id: old_failure.sync_run_id,
            status: SyncRunStatus::CompletedFailure,
            completed_at: utc_dt(2026, 3, 2, 0, 0, 0),
            summary_json: None,
        },
    )
    .expect("old failure sync run should complete");

    for attempt_index in 0..request_count {
        let attempt_offset_seconds =
            i64::try_from(attempt_index).expect("attempt index fits in i64");
        record_request_attempt(
            user_id,
            RecordRequestAttemptRequest {
                sync_run_id: old_failure.sync_run_id,
                request_kind: MempoolRequestKind::AddressTransactionsFirstPage,
                request_url: RequestUrl::parse("https://mempool.space/api/address/test/txs")
                    .expect("request url"),
                scope_address_id: address_id,
                page_cursor: None,
                page_kind: MempoolPageKind::FirstPage,
                attempted_at: utc_dt(2026, 3, 2, 0, 0, 0)
                    + chrono::Duration::seconds(attempt_offset_seconds),
                outcome: RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                    http_status_code: HttpStatusCode::try_new(429).expect("http status"),
                    response_headers_json: None,
                    response_body: Some(large_captured_response_body(response_body_bytes)),
                }),
            },
        )
        .expect("large request attempt should persist");
    }
}

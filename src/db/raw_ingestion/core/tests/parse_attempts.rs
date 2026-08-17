use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::user_db::with_user_db;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{TimeZone, Utc};

#[test]
fn record_raw_parse_attempt_rejects_success_rows_and_persists_failures() {
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
    let parser_version = ParserVersion::parse("mempool-v1").expect("parser version");
    let txid = sample_txid("0a");
    let raw_version_id = RawMempoolTransactionVersionId::new();
    let failure_attempted_at = chrono::Utc
        .timestamp_opt(1_700_000_001, 0)
        .single()
        .expect("valid timestamp");

    let missing_error = record_raw_parse_attempt(
        user_id,
        RecordRawParseAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            integration: IntegrationKind::Mempool,
            raw_object_key: RawObjectKey::Mempool { txid: txid.clone() },
            raw_version_id: RawVersionId::Mempool(raw_version_id),
            parser_kind: RawParserKind::Mempool,
            parser_version: parser_version.clone(),
            status: RawParseAttemptStatus::Failure,
            error_message: None,
            attempted_at: failure_attempted_at,
        },
    )
    .expect_err("failure parse attempt without message should fail");
    assert_eq!(
        missing_error.to_string(),
        "failed raw parse attempt must include an error message"
    );

    record_raw_parse_attempt(
        user_id,
        RecordRawParseAttemptRequest {
            sync_run_id: sync_run.sync_run_id,
            integration: IntegrationKind::Mempool,
            raw_object_key: RawObjectKey::Mempool { txid },
            raw_version_id: RawVersionId::Mempool(raw_version_id),
            parser_kind: RawParserKind::Mempool,
            parser_version,
            status: RawParseAttemptStatus::Failure,
            error_message: Some(
                ParseFailureMessage::parse("parser failed".to_string()).expect("failure message"),
            ),
            attempted_at: failure_attempted_at,
        },
    )
    .expect("failure parse attempt should persist");

    type RawParseAttemptRow = (String, String, String, Option<String>);

    let rows: Vec<RawParseAttemptRow> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT raw_object_kind, raw_object_key_json, status, error_message
                     FROM raw_parse_attempts
                     ORDER BY attempted_at ASC, created_at ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare raw parse attempt query", err)
            })?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|err| DbError::from_rusqlite_error("Failed to query raw parse attempts", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("Failed to read raw parse attempts", err))
    })
    .expect("raw parse attempt query should succeed");

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0],
        (
            "mempool_transaction".to_string(),
            format!(r#"{{"txid":"{}"}}"#, sample_txid("0a").as_str()),
            "failure".to_string(),
            Some("parser failed".to_string()),
        )
    );
}

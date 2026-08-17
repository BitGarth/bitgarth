use crate::db::DbError;
use crate::db::raw_ingestion::{
    IntegrationKind as RawIntegrationKind, SourceConnectionId, SyncRunId,
};
use crate::models::UserId;
use crate::tasks::UserTransactionMonitorError;
use crate::tasks::raw_ingestion_executor::{
    EtherscanCurrentHeadReplayReport, MempoolCurrentHeadReplayReport, MempoolHeadReplayRequest,
    replay_etherscan_current_heads, replay_mempool_current_heads,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::fmt;
use std::str::FromStr;

const RAW_REPLAY_COMMAND: &str = "raw-replay";
const RAW_REPLAY_RUN_COMMAND: &str = "run";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawReplayRequest {
    user_id: UserId,
    scope: RawReplayScope,
    replayed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawReplayScope {
    SyncRun {
        sync_run_id: SyncRunId,
    },
    SourceConnection {
        source_connection_id: SourceConnectionId,
    },
}

#[derive(Debug)]
enum RawReplayCommand {
    Help,
    Run(RawReplayRequest),
}

#[derive(Debug)]
pub(crate) enum RawReplayError {
    Usage(String),
    Db(String),
    Replay(String),
    Json(String),
}

impl RawReplayError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}

impl fmt::Display for RawReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Db(message) => write!(f, "{message}"),
            Self::Replay(message) => write!(f, "{message}"),
            Self::Json(message) => write!(f, "{message}"),
        }
    }
}

impl From<DbError> for RawReplayError {
    fn from(value: DbError) -> Self {
        Self::Db(value.to_string())
    }
}

impl From<UserTransactionMonitorError> for RawReplayError {
    fn from(value: UserTransactionMonitorError) -> Self {
        Self::Replay(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RawReplayReport {
    user_id: String,
    replayed_at: String,
    scope: RawReplayScopeReport,
    observation_set_count: usize,
    observed_item_count: usize,
    attempted_item_count: usize,
    successful_item_count: usize,
    failed_observation_set_count: usize,
    observation_sets: Vec<RawReplayObservationSetReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawReplayScopeReport {
    SyncRun { sync_run_id: String },
    SourceConnection { source_connection_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RawReplayObservationSetReport {
    raw_observation_set_id: String,
    sync_run_id: String,
    source_connection_id: String,
    grouping_kind: String,
    grouping_metadata: Value,
    observed_at: String,
    observed_item_count: usize,
    attempted_item_count: usize,
    successful_item_count: usize,
    outcome: RawReplayObservationOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RawReplayObservationOutcome {
    Success,
    Failure {
        failed_raw_object_key: Value,
        error_message: String,
    },
}

pub(crate) fn maybe_run_from_args() -> Result<bool, RawReplayError> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|arg| arg.to_str()) != Some(RAW_REPLAY_COMMAND) {
        return Ok(false);
    }

    match parse_command(&args[1..])? {
        RawReplayCommand::Help => print_usage(),
        RawReplayCommand::Run(request) => {
            let report = run_raw_replay_report(request)?;
            let serialized = serde_json::to_string_pretty(&report)
                .map_err(|err| RawReplayError::Json(err.to_string()))?;
            println!("{serialized}");
        }
    }

    Ok(true)
}

fn parse_command(args: &[OsString]) -> Result<RawReplayCommand, RawReplayError> {
    let Some(subcommand) = args.first().and_then(|value| value.to_str()) else {
        return Ok(RawReplayCommand::Help);
    };

    match subcommand {
        "help" | "--help" | "-h" => Ok(RawReplayCommand::Help),
        RAW_REPLAY_RUN_COMMAND => parse_run_command(&args[1..]).map(RawReplayCommand::Run),
        other => Err(RawReplayError::usage(format!(
            "unknown raw replay subcommand '{other}'"
        ))),
    }
}

fn parse_run_command(args: &[OsString]) -> Result<RawReplayRequest, RawReplayError> {
    let mut user_id = None;
    let mut sync_run_id = None;
    let mut source_connection_id = None;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| RawReplayError::usage("raw replay arguments must be valid UTF-8"))?;
        match flag {
            "--user-id" => {
                let value = next_flag_value(args, &mut index, "--user-id")?;
                user_id = Some(UserId::from_str(value).map_err(|err| {
                    RawReplayError::usage(format!("invalid --user-id value: {err}"))
                })?);
            }
            "--sync-run-id" => {
                let value = next_flag_value(args, &mut index, "--sync-run-id")?;
                sync_run_id = Some(SyncRunId::from_str(value).map_err(|err| {
                    RawReplayError::usage(format!("invalid --sync-run-id value: {err}"))
                })?);
            }
            "--source-connection-id" => {
                let value = next_flag_value(args, &mut index, "--source-connection-id")?;
                source_connection_id =
                    Some(SourceConnectionId::from_str(value).map_err(|err| {
                        RawReplayError::usage(format!(
                            "invalid --source-connection-id value: {err}"
                        ))
                    })?);
            }
            "--help" | "-h" => return Ok(default_run_request()),
            other => {
                return Err(RawReplayError::usage(format!(
                    "unknown raw replay run flag '{other}'"
                )));
            }
        }
        index += 1;
    }

    let user_id =
        user_id.ok_or_else(|| RawReplayError::usage("missing required --user-id flag"))?;
    let scope = match (sync_run_id, source_connection_id) {
        (Some(sync_run_id), None) => RawReplayScope::SyncRun { sync_run_id },
        (None, Some(source_connection_id)) => RawReplayScope::SourceConnection {
            source_connection_id,
        },
        (None, None) => {
            return Err(RawReplayError::usage(
                "exactly one of --sync-run-id or --source-connection-id is required",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(RawReplayError::usage(
                "use either --sync-run-id or --source-connection-id, not both",
            ));
        }
    };

    Ok(RawReplayRequest {
        user_id,
        scope,
        replayed_at: Utc::now(),
    })
}

fn default_run_request() -> RawReplayRequest {
    RawReplayRequest {
        user_id: UserId::new(),
        scope: RawReplayScope::SyncRun {
            sync_run_id: SyncRunId::new(),
        },
        replayed_at: Utc::now(),
    }
}

fn next_flag_value<'a>(
    args: &'a [OsString],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, RawReplayError> {
    let value = args
        .get(*index + 1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| RawReplayError::usage(format!("missing value for {flag}")))?;
    *index += 1;
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage:\n  raw-replay run --user-id <USER_ID> --sync-run-id <SYNC_RUN_ID>\n  raw-replay run --user-id <USER_ID> --source-connection-id <SOURCE_CONNECTION_ID>\n\nOutputs a deterministic JSON replay report to stdout."
    );
}

fn run_raw_replay_report(request: RawReplayRequest) -> Result<RawReplayReport, RawReplayError> {
    match resolve_scope_integration(request.user_id, &request.scope)? {
        RawIntegrationKind::Mempool => run_mempool_head_replay_report(request),
        RawIntegrationKind::Etherscan => run_etherscan_head_replay_report(request),
    }
}

fn run_mempool_head_replay_report(
    request: RawReplayRequest,
) -> Result<RawReplayReport, RawReplayError> {
    let source_connection_id = resolve_scope_source_connection_id(request.user_id, &request.scope)?;
    let MempoolCurrentHeadReplayReport {
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failure,
    } = replay_mempool_current_heads(MempoolHeadReplayRequest {
        user_id: request.user_id,
        source_connection_id: &source_connection_id,
    })?;
    let outcome = match failure {
        Some(failure) => {
            let raw_object_key_json = failure
                .raw_object_key
                .to_json_string()
                .map_err(RawReplayError::from)?;
            let failed_raw_object_key = serde_json::from_str(&raw_object_key_json)
                .map_err(|err| RawReplayError::Json(err.to_string()))?;
            RawReplayObservationOutcome::Failure {
                failed_raw_object_key,
                error_message: failure.error_message,
            }
        }
        None => RawReplayObservationOutcome::Success,
    };

    Ok(RawReplayReport {
        user_id: request.user_id.to_string(),
        replayed_at: request.replayed_at.to_rfc3339(),
        scope: match &request.scope {
            RawReplayScope::SyncRun { sync_run_id } => RawReplayScopeReport::SyncRun {
                sync_run_id: sync_run_id.to_string(),
            },
            RawReplayScope::SourceConnection {
                source_connection_id,
            } => RawReplayScopeReport::SourceConnection {
                source_connection_id: source_connection_id.to_string(),
            },
        },
        observation_set_count: 1,
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failed_observation_set_count: usize::from(matches!(
            outcome,
            RawReplayObservationOutcome::Failure { .. }
        )),
        observation_sets: vec![RawReplayObservationSetReport {
            raw_observation_set_id: "current_mempool_heads".to_string(),
            sync_run_id: "current_state".to_string(),
            source_connection_id: source_connection_id.to_string(),
            grouping_kind: "mempool_current_heads".to_string(),
            grouping_metadata: serde_json::json!({
                "replay_boundary": "current_raw_mempool_heads",
                "documented_scope": "current_canonical_state_for_source_connection"
            }),
            observed_at: request.replayed_at.to_rfc3339(),
            observed_item_count,
            attempted_item_count,
            successful_item_count,
            outcome,
        }],
    })
}

fn run_etherscan_head_replay_report(
    request: RawReplayRequest,
) -> Result<RawReplayReport, RawReplayError> {
    let source_connection_id = resolve_scope_source_connection_id(request.user_id, &request.scope)?;
    let EtherscanCurrentHeadReplayReport {
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failure,
    } = replay_etherscan_current_heads(MempoolHeadReplayRequest {
        user_id: request.user_id,
        source_connection_id: &source_connection_id,
    })?;
    let outcome = match failure {
        Some(failure) => {
            let raw_object_key_json = failure
                .raw_object_key
                .to_json_string()
                .map_err(RawReplayError::from)?;
            let failed_raw_object_key = serde_json::from_str(&raw_object_key_json)
                .map_err(|err| RawReplayError::Json(err.to_string()))?;
            RawReplayObservationOutcome::Failure {
                failed_raw_object_key,
                error_message: failure.error_message,
            }
        }
        None => RawReplayObservationOutcome::Success,
    };

    Ok(RawReplayReport {
        user_id: request.user_id.to_string(),
        replayed_at: request.replayed_at.to_rfc3339(),
        scope: match &request.scope {
            RawReplayScope::SyncRun { sync_run_id } => RawReplayScopeReport::SyncRun {
                sync_run_id: sync_run_id.to_string(),
            },
            RawReplayScope::SourceConnection {
                source_connection_id,
            } => RawReplayScopeReport::SourceConnection {
                source_connection_id: source_connection_id.to_string(),
            },
        },
        observation_set_count: 1,
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failed_observation_set_count: usize::from(matches!(
            outcome,
            RawReplayObservationOutcome::Failure { .. }
        )),
        observation_sets: vec![RawReplayObservationSetReport {
            raw_observation_set_id: "current_etherscan_heads".to_string(),
            sync_run_id: "current_state".to_string(),
            source_connection_id: source_connection_id.to_string(),
            grouping_kind: "etherscan_current_heads".to_string(),
            grouping_metadata: serde_json::json!({
                "replay_boundary": "current_raw_etherscan_heads",
                "documented_scope": "current_canonical_state_for_source_connection",
                "normal_family": "txlist",
                "internal_family": "txlistinternal"
            }),
            observed_at: request.replayed_at.to_rfc3339(),
            observed_item_count,
            attempted_item_count,
            successful_item_count,
            outcome,
        }],
    })
}

fn resolve_scope_source_connection_id(
    user_id: UserId,
    scope: &RawReplayScope,
) -> Result<SourceConnectionId, RawReplayError> {
    use crate::db::with_user_db;

    match scope {
        RawReplayScope::SourceConnection {
            source_connection_id,
        } => Ok(source_connection_id.clone()),
        RawReplayScope::SyncRun { sync_run_id } => with_user_db(user_id, |conn| {
            let raw = conn
                .query_row(
                    "SELECT source_connection_id FROM sync_runs WHERE id = ?1",
                    [sync_run_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to load source connection id for raw replay sync run",
                        err,
                    )
                })?;
            SourceConnectionId::from_str(&raw).map_err(|err| {
                DbError::new(format!(
                    "Invalid source connection id for raw replay: {err}"
                ))
            })
        })
        .map_err(Into::into),
    }
}

fn resolve_scope_integration(
    user_id: UserId,
    scope: &RawReplayScope,
) -> Result<RawIntegrationKind, RawReplayError> {
    use crate::db::with_user_db;

    let integration_raw = match scope {
        RawReplayScope::SyncRun { sync_run_id } => with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT integration FROM sync_runs WHERE id = ?1",
                [sync_run_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to load integration for raw replay sync run",
                    err,
                )
            })
        })?,
        RawReplayScope::SourceConnection {
            source_connection_id,
        } => with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT integration FROM source_connections WHERE id = ?1",
                [source_connection_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to load integration for raw replay source connection",
                    err,
                )
            })
        })?,
    };

    match integration_raw.as_str() {
        "mempool" => Ok(RawIntegrationKind::Mempool),
        "etherscan" => Ok(RawIntegrationKind::Etherscan),
        _ => Err(RawReplayError::Db(format!(
            "Invalid raw replay integration: {integration_raw}"
        ))),
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::{
        RawReplayObservationOutcome, RawReplayRequest, RawReplayScope, run_raw_replay_report,
    };
    use crate::db::raw_ingestion::{
        EtherscanChainId, EtherscanTraceId, InsertRawEtherscanInternalTransactionVersionRequest,
        InsertRawEtherscanNormalTransactionVersionRequest, IntegrationKind as RawIntegrationKind,
        MempoolPageKind as RawMempoolPageKind, PayloadSha256Hex, StartSyncRunRequest,
        SyncRunScopeKind, SyncRunTriggerKind, insert_raw_etherscan_internal_transaction_version,
        insert_raw_etherscan_normal_transaction_version, start_sync_run,
    };
    use crate::db::test_fixtures::create_eth_wallet_account_fixture;
    use crate::db::{
        acquire_test_runtime, add_bitcoin_address, setup_test_user, unique_user_id, wallet_label,
    };
    use crate::integrations::mempool::{MempoolPageTransaction, MempoolTransactionPage};
    use crate::models::UserId;
    use crate::tasks::raw_ingestion_executor::{MempoolPageIngestionRequest, ingest_mempool_page};
    use crate::transactions::TxHash;
    use crate::wallets::{BtcAddress, Network, RawBtcAddress, SyncedAssetId};
    use crate::{
        db::raw_ingestion::ExactPayloadBytes,
        ethereum::{EthAddress, RawEthAddress},
    };
    use chrono::{TimeZone, Utc};
    use url::Url;

    fn sample_btc_address() -> BtcAddress {
        BtcAddress::parse(
            &RawBtcAddress::new("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string()),
            Network::Mainnet,
        )
        .expect("sample btc address should parse")
    }

    fn genesis_btc_address() -> BtcAddress {
        BtcAddress::parse(
            &RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            Network::Mainnet,
        )
        .expect("genesis btc address should parse")
    }

    fn sample_txid(suffix: &str) -> TxHash {
        let mut value = "a".repeat(62);
        value.push_str(suffix);
        TxHash::parse(&value).expect("sample txid should parse")
    }

    fn start_mempool_sync_run(
        user_id: UserId,
        scope_address_id: crate::wallets::DigitalAssetAddressId,
    ) -> crate::db::raw_ingestion::StartedSyncRun {
        start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: RawIntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Manual,
                started_at: Utc
                    .with_ymd_and_hms(2026, 3, 12, 23, 0, 0)
                    .single()
                    .expect("valid timestamp"),
                summary_json: None,
            },
        )
        .expect("sync run should insert")
    }

    fn sample_eth_address(last_hex: &str) -> EthAddress {
        let prefix_len = 40_usize.saturating_sub(last_hex.len());
        let raw = RawEthAddress::new(format!("0x{}{}", "1".repeat(prefix_len), last_hex));
        EthAddress::parse(&raw).expect("sample eth address should parse")
    }

    fn start_etherscan_sync_run(
        user_id: UserId,
        scope_address_id: crate::wallets::DigitalAssetAddressId,
    ) -> crate::db::raw_ingestion::StartedSyncRun {
        start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: RawIntegrationKind::Etherscan,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id,
                asset_id: SyncedAssetId::Ethereum,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Manual,
                started_at: Utc
                    .with_ymd_and_hms(2026, 3, 12, 23, 20, 0)
                    .single()
                    .expect("valid timestamp"),
                summary_json: None,
            },
        )
        .expect("etherscan sync run should insert")
    }

    #[test]
    fn run_raw_replay_report_for_sync_run_is_deterministic() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = add_bitcoin_address(
            user_id,
            &sample_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Replay Wallet")),
            Utc.with_ymd_and_hms(2026, 3, 12, 23, 1, 0)
                .single()
                .expect("valid timestamp"),
        )
        .expect("bitcoin address should insert");
        let sync_run = start_mempool_sync_run(user_id, inserted.address_id);
        let success_observed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 2, 0)
            .single()
            .expect("valid timestamp");
        let failure_observed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 3, 0)
            .single()
            .expect("valid timestamp");
        let replayed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 4, 0)
            .single()
            .expect("valid timestamp");
        let success_page = MempoolTransactionPage {
            request_url: Url::parse("https://mempool.space/api/address/replay/txs")
                .expect("request url"),
            http_status_code: 200,
            transactions: vec![
                MempoolPageTransaction {
                    txid: sample_txid("01"),
                    payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                },
                MempoolPageTransaction {
                    txid: sample_txid("02"),
                    payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                },
            ],
        };
        let failed_page = MempoolTransactionPage {
            request_url: Url::parse("https://mempool.space/api/address/replay/txs/chain")
                .expect("request url"),
            http_status_code: 200,
            transactions: vec![
                MempoolPageTransaction {
                    txid: sample_txid("03"),
                    payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa03","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                },
                MempoolPageTransaction {
                    txid: sample_txid("04"),
                    payload_bytes: br#"{"txid":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff04","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                },
            ],
        };

        let first_result = ingest_mempool_page(
            MempoolPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                scope_address_id: inserted.address_id,
                page_kind: RawMempoolPageKind::FirstPage,
                page_cursor: None,
                scan_start_run_id: None,
                network: Network::Mainnet,
                observed_at: success_observed_at,
            },
            success_page,
        )
        .expect("success page should ingest");
        assert_eq!(first_result.transactions.len(), 2);

        let failure = ingest_mempool_page(
            MempoolPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                scope_address_id: inserted.address_id,
                page_kind: RawMempoolPageKind::FirstPage,
                page_cursor: None,
                scan_start_run_id: None,
                network: Network::Mainnet,
                observed_at: failure_observed_at,
            },
            failed_page,
        )
        .expect_err("failed page should abort on parse failure");
        assert!(failure.to_string().contains("txid mismatch"));

        let report = run_raw_replay_report(RawReplayRequest {
            user_id,
            scope: RawReplayScope::SyncRun {
                sync_run_id: sync_run.sync_run_id,
            },
            replayed_at,
        })
        .expect("replay report should succeed");

        assert_eq!(report.observation_set_count, 1);
        assert_eq!(report.observed_item_count, 3);
        assert_eq!(report.attempted_item_count, 3);
        assert_eq!(report.successful_item_count, 3);
        assert_eq!(report.failed_observation_set_count, 0);
        assert_eq!(report.observation_sets.len(), 1);
        assert_eq!(
            report.observation_sets[0].grouping_metadata["page_kind"],
            serde_json::Value::Null
        );
        assert_eq!(
            report.observation_sets[0].grouping_metadata["replay_boundary"],
            "current_raw_mempool_heads"
        );
        assert_eq!(
            report.observation_sets[0].grouping_metadata["documented_scope"],
            "current_canonical_state_for_source_connection"
        );
        assert_eq!(report.observation_sets[0].observed_item_count, 3);
        assert_eq!(report.observation_sets[0].attempted_item_count, 3);
        assert_eq!(report.observation_sets[0].successful_item_count, 3);
        assert_eq!(report.replayed_at, replayed_at.to_rfc3339());

        assert_eq!(
            report.observation_sets[0].outcome,
            RawReplayObservationOutcome::Success
        );
    }

    #[test]
    fn run_raw_replay_report_for_source_connection_filters_other_sources() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let first_address = add_bitcoin_address(
            user_id,
            &sample_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Replay Source A")),
            Utc.with_ymd_and_hms(2026, 3, 12, 23, 10, 0)
                .single()
                .expect("valid timestamp"),
        )
        .expect("first bitcoin address should insert");
        let second_address = add_bitcoin_address(
            user_id,
            &genesis_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Replay Source B")),
            Utc.with_ymd_and_hms(2026, 3, 12, 23, 11, 0)
                .single()
                .expect("valid timestamp"),
        )
        .expect("second bitcoin address should insert");

        let first_run = start_mempool_sync_run(user_id, first_address.address_id);
        let second_run = start_mempool_sync_run(user_id, second_address.address_id);
        let observed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 12, 0)
            .single()
            .expect("valid timestamp");
        let replayed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 13, 0)
            .single()
            .expect("valid timestamp");

        for (run, address_id, suffix) in [
            (first_run.clone(), first_address.address_id, "11"),
            (second_run.clone(), second_address.address_id, "12"),
        ] {
            ingest_mempool_page(
                MempoolPageIngestionRequest {
                    user_id,
                    raw_sync_run_id: run.sync_run_id,
                    source_connection_id: &run.source_connection_id,
                    scope_address_id: address_id,
                    page_kind: RawMempoolPageKind::FirstPage,
                    page_cursor: None,
                    scan_start_run_id: None,
                    network: Network::Mainnet,
                    observed_at,
                },
                MempoolTransactionPage {
                    request_url: Url::parse("https://mempool.space/api/address/replay/filter")
                        .expect("request url"),
                    http_status_code: 200,
                    transactions: vec![MempoolPageTransaction {
                        txid: sample_txid(suffix),
                        payload_bytes: format!(
                            r#"{{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa{suffix}","vin":[],"vout":[],"status":{{"confirmed":true}}}}"#
                        )
                        .into_bytes(),
                    }],
                },
            )
            .expect("page should ingest");
        }

        let report = run_raw_replay_report(RawReplayRequest {
            user_id,
            scope: RawReplayScope::SourceConnection {
                source_connection_id: first_run.source_connection_id.clone(),
            },
            replayed_at,
        })
        .expect("source-scoped replay should succeed");

        assert_eq!(report.observation_set_count, 1);
        assert_eq!(report.successful_item_count, 1);
        assert_eq!(
            report.observation_sets[0].source_connection_id,
            first_run.source_connection_id.to_string()
        );
    }

    #[test]
    fn run_raw_replay_report_for_etherscan_sync_run_uses_current_heads() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = create_eth_wallet_account_fixture(
            user_id,
            &sample_eth_address("42"),
            "Replay Ethereum Wallet",
            Utc.with_ymd_and_hms(2026, 3, 12, 23, 21, 0)
                .single()
                .expect("valid timestamp"),
        );
        let sync_run = start_etherscan_sync_run(user_id, inserted.address_id);
        let observed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 22, 0)
            .single()
            .expect("valid timestamp");
        let replayed_at = Utc
            .with_ymd_and_hms(2026, 3, 12, 23, 23, 0)
            .single()
            .expect("valid timestamp");
        let normal_payload = ExactPayloadBytes::try_new(
            br#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#
                .to_vec(),
        )
        .expect("normal payload");
        let internal_payload = ExactPayloadBytes::try_new(
            br#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","isError":"0","type":"call","traceId":"0"}"#
                .to_vec(),
        )
        .expect("internal payload");

        insert_raw_etherscan_normal_transaction_version(
            user_id,
            InsertRawEtherscanNormalTransactionVersionRequest {
                source_connection_id: sync_run.source_connection_id.clone(),
                chain_id: EtherscanChainId::try_new(1).expect("chain id"),
                network: Network::Mainnet,
                tx_hash: sample_txid("21"),
                payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&normal_payload),
                payload_bytes: normal_payload,
                first_observed_at: observed_at,
            },
        )
        .expect("normal version should insert");
        insert_raw_etherscan_internal_transaction_version(
            user_id,
            InsertRawEtherscanInternalTransactionVersionRequest {
                source_connection_id: sync_run.source_connection_id.clone(),
                chain_id: EtherscanChainId::try_new(1).expect("chain id"),
                network: Network::Mainnet,
                tx_hash: sample_txid("21"),
                trace_id: EtherscanTraceId::parse("0").expect("trace id"),
                payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&internal_payload),
                payload_bytes: internal_payload,
                first_observed_at: observed_at,
            },
        )
        .expect("internal version should insert");

        let report = run_raw_replay_report(RawReplayRequest {
            user_id,
            scope: RawReplayScope::SyncRun {
                sync_run_id: sync_run.sync_run_id,
            },
            replayed_at,
        })
        .expect("etherscan replay report should succeed");

        assert_eq!(report.observation_set_count, 1);
        assert_eq!(report.observed_item_count, 2);
        assert_eq!(report.attempted_item_count, 2);
        assert_eq!(report.successful_item_count, 2);
        assert_eq!(report.failed_observation_set_count, 0);
        assert_eq!(
            report.observation_sets[0].grouping_kind,
            "etherscan_current_heads"
        );
        assert_eq!(
            report.observation_sets[0].grouping_metadata["replay_boundary"],
            "current_raw_etherscan_heads"
        );
        assert_eq!(
            report.observation_sets[0].grouping_metadata["normal_family"],
            "txlist"
        );
        assert_eq!(
            report.observation_sets[0].grouping_metadata["internal_family"],
            "txlistinternal"
        );
        assert_eq!(
            report.observation_sets[0].outcome,
            RawReplayObservationOutcome::Success
        );
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::parse_command;
    use crate::models::UserId;
    use std::ffi::OsString;

    #[test]
    fn parse_run_command_rejects_missing_scope_flag() {
        let err = parse_command(&[
            OsString::from("run"),
            OsString::from("--user-id"),
            OsString::from(UserId::new().to_string()),
        ])
        .expect_err("missing scope flag should fail");

        assert_eq!(
            err.to_string(),
            "exactly one of --sync-run-id or --source-connection-id is required"
        );
    }
}

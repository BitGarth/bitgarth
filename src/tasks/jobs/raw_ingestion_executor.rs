use crate::db::raw_ingestion::MempoolPageObservationMetadata;
use crate::db::raw_ingestion::{
    CapturedResponseBody, EtherscanChainId, EtherscanQueryJson, EtherscanRequestKind,
    EtherscanTraceId, ExactPayloadBytes, HttpStatusCode,
    InsertRawEtherscanInternalTransactionVersionRequest,
    InsertRawEtherscanNormalTransactionVersionRequest, InsertRawMempoolTransactionVersionRequest,
    IntegrationKind as RawIntegrationKind, MempoolPageKind as RawMempoolPageKind,
    MempoolRequestKind as RawMempoolRequestKind, PageCursor, ParseFailureMessage, ParserVersion,
    PayloadSha256Hex, RawMempoolTransactionVersionId, RawObjectKey, RawParseAttemptStatus,
    RawParserKind, RawVersionId, RawVersionWriteOutcome, RecordEtherscanRequestAttemptRequest,
    RecordRawMempoolPageObservationRequest, RecordRawParseAttemptRequest,
    RecordRequestAttemptRequest, RequestAttemptHttpResponse, RequestAttemptOutcome, RequestUrl,
    ResponseHeadersJson, SourceConnectionId, SyncRunId, TransportErrorMessage,
    insert_raw_etherscan_internal_transaction_version,
    insert_raw_etherscan_normal_transaction_version, insert_raw_mempool_tx_version,
    load_current_raw_etherscan_internal_transaction_heads,
    load_current_raw_etherscan_normal_transaction_heads,
    load_current_raw_mempool_transaction_heads, record_etherscan_request_attempt,
    record_raw_mempool_page_observation, record_raw_parse_attempt, record_request_attempt,
};
use crate::integrations::etherscan::{
    EtherscanError, EtherscanFetchedPage, EtherscanInternalTx, EtherscanNormalTx,
    EtherscanRequestMetadata,
};
use crate::integrations::mempool::{
    MempoolAddressTransaction, MempoolError, MempoolTransactionPage,
};
use crate::models::UserId;
use crate::tasks::jobs::sync::integrations::etherscan::map_etherscan_transactions;
use crate::tasks::jobs::user_transaction_monitor::UserTransactionMonitorError;
use crate::transactions::TxHash;
use crate::wallets::DigitalAssetAddressId;
use crate::wallets::Network;
use chrono::{DateTime, Utc};

pub(crate) const REQUEST_ATTEMPT_RESPONSE_BODY_LIMIT_BYTES: usize = 64 * 1024;

const MEMPOOL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION: &str =
    "mempool_transaction_to_sync_record:v1";
const ETHERSCAN_NORMAL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION: &str =
    "etherscan_normal_transaction_to_sync_record:v1";
const ETHERSCAN_INTERNAL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION: &str =
    "etherscan_internal_transaction_to_sync_record:v1";

#[derive(Clone, Copy)]
pub(crate) struct MempoolPageIngestionRequest<'a> {
    pub(crate) user_id: UserId,
    pub(crate) raw_sync_run_id: SyncRunId,
    pub(crate) source_connection_id: &'a SourceConnectionId,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) page_kind: RawMempoolPageKind,
    pub(crate) page_cursor: Option<&'a str>,
    pub(crate) scan_start_run_id: Option<SyncRunId>,
    pub(crate) network: Network,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
pub(crate) struct MempoolRequestFailureRecord<'a> {
    pub(crate) user_id: UserId,
    pub(crate) raw_sync_run_id: SyncRunId,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) page_kind: RawMempoolPageKind,
    pub(crate) page_cursor: Option<&'a str>,
    pub(crate) attempted_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EtherscanPageIngestionRequest<'a> {
    pub(crate) user_id: UserId,
    pub(crate) raw_sync_run_id: SyncRunId,
    pub(crate) source_connection_id: &'a SourceConnectionId,
    pub(crate) chain_id: EtherscanChainId,
    pub(crate) network: Network,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
pub(crate) struct EtherscanRequestFailureRecord<'a> {
    pub(crate) user_id: UserId,
    pub(crate) raw_sync_run_id: SyncRunId,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) request_kind: EtherscanRequestKind,
    pub(crate) request_metadata: &'a EtherscanRequestMetadata,
    pub(crate) attempted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservationSetParseFailure {
    pub(crate) raw_object_key: RawObjectKey,
    pub(crate) error_message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MempoolPageIngestionSummary {
    pub(crate) items_seen: u32,
    pub(crate) versions_inserted: u32,
    pub(crate) versions_reused: u32,
    pub(crate) parse_success_count: u32,
    pub(crate) parse_failure_count: u32,
}

#[derive(Debug)]
pub(crate) struct IngestedMempoolPage {
    pub(crate) transactions: Vec<MempoolAddressTransaction>,
    pub(crate) summary: MempoolPageIngestionSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EtherscanPageIngestionSummary {
    pub(crate) items_seen: u32,
    pub(crate) versions_inserted: u32,
    pub(crate) versions_reused: u32,
    pub(crate) parse_success_count: u32,
    pub(crate) parse_failure_count: u32,
}

#[derive(Debug)]
pub(crate) struct IngestedEtherscanPage<T> {
    pub(crate) transactions: Vec<T>,
    pub(crate) summary: EtherscanPageIngestionSummary,
}

#[derive(Clone, Copy)]
pub(crate) struct MempoolHeadReplayRequest<'a> {
    pub(crate) user_id: UserId,
    pub(crate) source_connection_id: &'a SourceConnectionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MempoolCurrentHeadReplayReport {
    pub(crate) observed_item_count: usize,
    pub(crate) attempted_item_count: usize,
    pub(crate) successful_item_count: usize,
    pub(crate) failure: Option<ObservationSetParseFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanCurrentHeadReplayReport {
    pub(crate) observed_item_count: usize,
    pub(crate) attempted_item_count: usize,
    pub(crate) successful_item_count: usize,
    pub(crate) failure: Option<ObservationSetParseFailure>,
}

pub(crate) fn ingest_mempool_page(
    request: MempoolPageIngestionRequest<'_>,
    page: MempoolTransactionPage,
) -> Result<IngestedMempoolPage, UserTransactionMonitorError> {
    let _page_request_url = RequestUrl::parse(page.request_url.as_str())?;
    let _page_http_status_code = HttpStatusCode::try_new(page.http_status_code)?;
    let mut transactions = Vec::with_capacity(page.transactions.len());
    let mut raw_version_ids = Vec::with_capacity(page.transactions.len());
    let mut summary = MempoolPageIngestionSummary {
        items_seen: u32::try_from(page.transactions.len()).unwrap_or(u32::MAX),
        ..MempoolPageIngestionSummary::default()
    };
    let parser_version = ParserVersion::parse(MEMPOOL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION)?;

    for (page_item_index, raw_transaction) in page.transactions.into_iter().enumerate() {
        let payload_bytes = ExactPayloadBytes::try_new(raw_transaction.payload_bytes)?;
        let candidate_raw_version_id = RawMempoolTransactionVersionId::new();
        match parse_persisted_raw_mempool_transaction(
            candidate_raw_version_id,
            &raw_transaction.txid,
            &payload_bytes,
        ) {
            Ok(transaction) => {
                let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
                let inserted = insert_raw_mempool_tx_version(
                    request.user_id,
                    InsertRawMempoolTransactionVersionRequest {
                        source_connection_id: request.source_connection_id.clone(),
                        network: request.network,
                        txid: raw_transaction.txid.clone(),
                        payload_hash_sha256_hex,
                        payload_bytes: payload_bytes.clone(),
                        first_observed_at: request.observed_at,
                    },
                )?;
                match inserted.write_outcome {
                    RawVersionWriteOutcome::InsertedNewHead => {
                        summary.versions_inserted = summary.versions_inserted.saturating_add(1);
                    }
                    RawVersionWriteOutcome::ReusedCurrentHead => {
                        summary.versions_reused = summary.versions_reused.saturating_add(1);
                    }
                }
                summary.parse_success_count = summary.parse_success_count.saturating_add(1);
                raw_version_ids.push(inserted.raw_version_id);
                transactions.push(transaction);
            }
            Err(error_message) => {
                let parse_error =
                    format!("mempool page item {page_item_index} failed parse: {error_message}");
                record_raw_parse_attempt(
                    request.user_id,
                    RecordRawParseAttemptRequest {
                        sync_run_id: request.raw_sync_run_id,
                        integration: RawIntegrationKind::Mempool,
                        raw_object_key: RawObjectKey::Mempool {
                            txid: raw_transaction.txid,
                        },
                        raw_version_id: RawVersionId::Mempool(candidate_raw_version_id),
                        parser_kind: RawParserKind::Mempool,
                        parser_version: parser_version.clone(),
                        status: RawParseAttemptStatus::Failure,
                        error_message: Some(ParseFailureMessage::parse(parse_error.clone())?),
                        attempted_at: request.observed_at,
                    },
                )?;
                return Err(UserTransactionMonitorError::Parse(parse_error));
            }
        }
    }

    let returned_last_confirmed_cursor = transactions
        .iter()
        .rev()
        .find(|transaction| transaction.status.confirmed)
        .map(|transaction| transaction.txid.clone());
    record_raw_mempool_page_observation(
        request.user_id,
        RecordRawMempoolPageObservationRequest {
            sync_run_id: request.raw_sync_run_id,
            source_connection_id: request.source_connection_id.clone(),
            metadata: MempoolPageObservationMetadata {
                address_id: request.scope_address_id,
                scan_start_run_id: request.scan_start_run_id,
                page_kind: request.page_kind,
                requested_cursor: request.page_cursor.map(str::to_owned),
                returned_last_confirmed_cursor,
                item_count: summary.items_seen,
            },
            raw_version_ids,
            observed_at: request.observed_at,
        },
    )?;

    Ok(IngestedMempoolPage {
        transactions,
        summary,
    })
}

pub(crate) fn ingest_etherscan_normal_page(
    request: EtherscanPageIngestionRequest<'_>,
    page: EtherscanFetchedPage<EtherscanNormalTx>,
) -> Result<IngestedEtherscanPage<EtherscanNormalTx>, UserTransactionMonitorError> {
    let mut transactions = Vec::with_capacity(page.items.len());
    let mut summary = EtherscanPageIngestionSummary {
        items_seen: u32::try_from(page.items.len()).unwrap_or(u32::MAX),
        ..EtherscanPageIngestionSummary::default()
    };
    let parser_version =
        ParserVersion::parse(ETHERSCAN_NORMAL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION)?;

    for (page_item_index, item) in page.items.into_iter().enumerate() {
        let tx_hash = parse_etherscan_tx_hash(&item.parsed.hash)?;
        let payload_bytes = ExactPayloadBytes::try_new(item.raw_json_bytes)?;
        let candidate_raw_version_id =
            crate::db::raw_ingestion::RawEtherscanNormalTransactionVersionId::new();
        match parse_persisted_raw_etherscan_normal_transaction(
            candidate_raw_version_id,
            &tx_hash,
            &payload_bytes,
        ) {
            Ok(transaction) => {
                let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
                let inserted = insert_raw_etherscan_normal_transaction_version(
                    request.user_id,
                    InsertRawEtherscanNormalTransactionVersionRequest {
                        source_connection_id: request.source_connection_id.clone(),
                        chain_id: request.chain_id,
                        network: request.network,
                        tx_hash,
                        payload_hash_sha256_hex,
                        payload_bytes,
                        first_observed_at: request.observed_at,
                    },
                )?;
                match inserted.write_outcome {
                    RawVersionWriteOutcome::InsertedNewHead => {
                        summary.versions_inserted = summary.versions_inserted.saturating_add(1);
                    }
                    RawVersionWriteOutcome::ReusedCurrentHead => {
                        summary.versions_reused = summary.versions_reused.saturating_add(1);
                    }
                }
                summary.parse_success_count = summary.parse_success_count.saturating_add(1);
                transactions.push(transaction);
            }
            Err(error_message) => {
                let parse_error = format!(
                    "etherscan normal page item {page_item_index} failed parse: {error_message}"
                );
                record_raw_parse_attempt(
                    request.user_id,
                    RecordRawParseAttemptRequest {
                        sync_run_id: request.raw_sync_run_id,
                        integration: RawIntegrationKind::Etherscan,
                        raw_object_key: RawObjectKey::EtherscanNormal { tx_hash },
                        raw_version_id: RawVersionId::EtherscanNormal(candidate_raw_version_id),
                        parser_kind: RawParserKind::EtherscanNormal,
                        parser_version: parser_version.clone(),
                        status: RawParseAttemptStatus::Failure,
                        error_message: Some(ParseFailureMessage::parse(parse_error.clone())?),
                        attempted_at: request.observed_at,
                    },
                )?;
                return Err(UserTransactionMonitorError::Parse(parse_error));
            }
        }
    }

    Ok(IngestedEtherscanPage {
        transactions,
        summary,
    })
}

pub(crate) fn ingest_etherscan_internal_page(
    request: EtherscanPageIngestionRequest<'_>,
    page: EtherscanFetchedPage<EtherscanInternalTx>,
) -> Result<IngestedEtherscanPage<EtherscanInternalTx>, UserTransactionMonitorError> {
    let mut transactions = Vec::with_capacity(page.items.len());
    let mut summary = EtherscanPageIngestionSummary {
        items_seen: u32::try_from(page.items.len()).unwrap_or(u32::MAX),
        ..EtherscanPageIngestionSummary::default()
    };
    let parser_version =
        ParserVersion::parse(ETHERSCAN_INTERNAL_TRANSACTION_TO_SYNC_RECORD_PARSER_VERSION)?;

    for (page_item_index, item) in page.items.into_iter().enumerate() {
        let tx_hash = parse_etherscan_tx_hash(&item.parsed.hash)?;
        let trace_id = EtherscanTraceId::parse(&item.parsed.trace_id)?;
        let payload_bytes = ExactPayloadBytes::try_new(item.raw_json_bytes)?;
        let candidate_raw_version_id =
            crate::db::raw_ingestion::RawEtherscanInternalTransactionVersionId::new();
        match parse_persisted_raw_etherscan_internal_transaction(
            candidate_raw_version_id,
            &tx_hash,
            &trace_id,
            &payload_bytes,
        ) {
            Ok(transaction) => {
                let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
                let inserted = insert_raw_etherscan_internal_transaction_version(
                    request.user_id,
                    InsertRawEtherscanInternalTransactionVersionRequest {
                        source_connection_id: request.source_connection_id.clone(),
                        chain_id: request.chain_id,
                        network: request.network,
                        tx_hash,
                        trace_id,
                        payload_hash_sha256_hex,
                        payload_bytes,
                        first_observed_at: request.observed_at,
                    },
                )?;
                match inserted.write_outcome {
                    RawVersionWriteOutcome::InsertedNewHead => {
                        summary.versions_inserted = summary.versions_inserted.saturating_add(1);
                    }
                    RawVersionWriteOutcome::ReusedCurrentHead => {
                        summary.versions_reused = summary.versions_reused.saturating_add(1);
                    }
                }
                summary.parse_success_count = summary.parse_success_count.saturating_add(1);
                transactions.push(transaction);
            }
            Err(error_message) => {
                let parse_error = format!(
                    "etherscan internal page item {page_item_index} failed parse: {error_message}"
                );
                record_raw_parse_attempt(
                    request.user_id,
                    RecordRawParseAttemptRequest {
                        sync_run_id: request.raw_sync_run_id,
                        integration: RawIntegrationKind::Etherscan,
                        raw_object_key: RawObjectKey::EtherscanInternal { tx_hash, trace_id },
                        raw_version_id: RawVersionId::EtherscanInternal(candidate_raw_version_id),
                        parser_kind: RawParserKind::EtherscanInternal,
                        parser_version: parser_version.clone(),
                        status: RawParseAttemptStatus::Failure,
                        error_message: Some(ParseFailureMessage::parse(parse_error.clone())?),
                        attempted_at: request.observed_at,
                    },
                )?;
                return Err(UserTransactionMonitorError::Parse(parse_error));
            }
        }
    }

    Ok(IngestedEtherscanPage {
        transactions,
        summary,
    })
}

pub(crate) fn replay_mempool_current_heads(
    request: MempoolHeadReplayRequest<'_>,
) -> Result<MempoolCurrentHeadReplayReport, UserTransactionMonitorError> {
    let persisted_raw_transactions =
        load_current_raw_mempool_transaction_heads(request.user_id, request.source_connection_id)?;
    let observed_item_count = persisted_raw_transactions.len();
    let mut attempted_item_count = 0_usize;
    let mut successful_item_count = 0_usize;

    for raw_transaction in persisted_raw_transactions {
        attempted_item_count = attempted_item_count.saturating_add(1);
        let raw_object_key = RawObjectKey::Mempool {
            txid: raw_transaction.txid.clone(),
        };
        if let Err(error_message) = parse_persisted_raw_mempool_transaction(
            raw_transaction.raw_version_id,
            &raw_transaction.txid,
            &raw_transaction.payload_bytes,
        ) {
            return Ok(MempoolCurrentHeadReplayReport {
                observed_item_count,
                attempted_item_count,
                successful_item_count,
                failure: Some(ObservationSetParseFailure {
                    raw_object_key,
                    error_message,
                }),
            });
        }
        successful_item_count = successful_item_count.saturating_add(1);
    }

    Ok(MempoolCurrentHeadReplayReport {
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failure: None,
    })
}

pub(crate) fn replay_etherscan_current_heads(
    request: MempoolHeadReplayRequest<'_>,
) -> Result<EtherscanCurrentHeadReplayReport, UserTransactionMonitorError> {
    let normal_heads = load_current_raw_etherscan_normal_transaction_heads(
        request.user_id,
        request.source_connection_id,
    )?;
    let internal_heads = load_current_raw_etherscan_internal_transaction_heads(
        request.user_id,
        request.source_connection_id,
    )?;
    let observed_item_count = normal_heads.len().saturating_add(internal_heads.len());
    let mut attempted_item_count = 0_usize;
    let mut successful_item_count = 0_usize;
    let mut normal_transactions = Vec::with_capacity(normal_heads.len());
    let mut internal_transactions = Vec::with_capacity(internal_heads.len());

    for raw_transaction in normal_heads {
        attempted_item_count = attempted_item_count.saturating_add(1);
        let raw_object_key = RawObjectKey::EtherscanNormal {
            tx_hash: raw_transaction.tx_hash.clone(),
        };
        match parse_persisted_raw_etherscan_normal_transaction(
            raw_transaction.raw_version_id,
            &raw_transaction.tx_hash,
            &raw_transaction.payload_bytes,
        ) {
            Ok(transaction) => {
                successful_item_count = successful_item_count.saturating_add(1);
                normal_transactions.push(transaction);
            }
            Err(error_message) => {
                return Ok(EtherscanCurrentHeadReplayReport {
                    observed_item_count,
                    attempted_item_count,
                    successful_item_count,
                    failure: Some(ObservationSetParseFailure {
                        raw_object_key,
                        error_message,
                    }),
                });
            }
        }
    }

    for raw_transaction in internal_heads {
        attempted_item_count = attempted_item_count.saturating_add(1);
        let raw_object_key = RawObjectKey::EtherscanInternal {
            tx_hash: raw_transaction.tx_hash.clone(),
            trace_id: raw_transaction.trace_id.clone(),
        };
        match parse_persisted_raw_etherscan_internal_transaction(
            raw_transaction.raw_version_id,
            &raw_transaction.tx_hash,
            &raw_transaction.trace_id,
            &raw_transaction.payload_bytes,
        ) {
            Ok(transaction) => {
                successful_item_count = successful_item_count.saturating_add(1);
                internal_transactions.push(transaction);
            }
            Err(error_message) => {
                return Ok(EtherscanCurrentHeadReplayReport {
                    observed_item_count,
                    attempted_item_count,
                    successful_item_count,
                    failure: Some(ObservationSetParseFailure {
                        raw_object_key,
                        error_message,
                    }),
                });
            }
        }
    }

    map_etherscan_transactions(normal_transactions, internal_transactions)?;

    Ok(EtherscanCurrentHeadReplayReport {
        observed_item_count,
        attempted_item_count,
        successful_item_count,
        failure: None,
    })
}

pub(crate) fn record_mempool_request_failure(
    request: MempoolRequestFailureRecord<'_>,
    error: &MempoolError,
) -> Result<(), UserTransactionMonitorError> {
    let Some(request_url) = request_url_for_mempool_error(error)? else {
        return Ok(());
    };
    let Some(outcome) = request_attempt_outcome_for_mempool_error(error)? else {
        return Ok(());
    };
    record_request_attempt(
        request.user_id,
        RecordRequestAttemptRequest {
            sync_run_id: request.raw_sync_run_id,
            request_kind: raw_request_kind_for_page_kind(request.page_kind),
            request_url,
            scope_address_id: request.scope_address_id,
            page_cursor: raw_page_cursor(request.page_cursor)?,
            page_kind: request.page_kind,
            attempted_at: request.attempted_at,
            outcome,
        },
    )?;
    Ok(())
}

pub(crate) fn record_etherscan_request_failure(
    request: EtherscanRequestFailureRecord<'_>,
    error: &EtherscanError,
) -> Result<(), UserTransactionMonitorError> {
    record_etherscan_request_attempt(
        request.user_id,
        RecordEtherscanRequestAttemptRequest {
            sync_run_id: request.raw_sync_run_id,
            request_kind: request.request_kind,
            request_url: RequestUrl::parse(&request.request_metadata.request_url_without_api_key)?,
            scope_address_id: request.scope_address_id,
            request_query_json: EtherscanQueryJson::parse(
                request.request_metadata.request_query_json.clone(),
            )?,
            attempted_at: request.attempted_at,
            outcome: request_attempt_outcome_for_etherscan_error(error)?,
        },
    )?;
    Ok(())
}

pub(crate) fn parse_persisted_raw_mempool_transaction(
    raw_version_id: crate::db::raw_ingestion::RawMempoolTransactionVersionId,
    expected_txid: &TxHash,
    payload_bytes: &ExactPayloadBytes,
) -> Result<MempoolAddressTransaction, String> {
    let transaction: MempoolAddressTransaction = serde_json::from_slice(payload_bytes.as_slice())
        .map_err(|err| {
        format!(
            "raw mempool transaction version {} failed JSON deserialization: {}",
            raw_version_id, err
        )
    })?;
    let parsed_txid = TxHash::parse(&transaction.txid).map_err(|err| {
        format!(
            "raw mempool transaction version {} has invalid txid: {}",
            raw_version_id, err
        )
    })?;
    if &parsed_txid != expected_txid {
        return Err(format!(
            "raw mempool transaction version {} txid mismatch: stored {}, parsed {}",
            raw_version_id,
            expected_txid.as_str(),
            parsed_txid.as_str()
        ));
    }
    Ok(transaction)
}

pub(crate) fn parse_persisted_raw_etherscan_normal_transaction(
    raw_version_id: crate::db::raw_ingestion::RawEtherscanNormalTransactionVersionId,
    expected_tx_hash: &TxHash,
    payload_bytes: &ExactPayloadBytes,
) -> Result<EtherscanNormalTx, String> {
    let transaction: EtherscanNormalTx =
        serde_json::from_slice(payload_bytes.as_slice()).map_err(|err| {
            format!(
                "raw etherscan normal transaction version {} failed JSON deserialization: {}",
                raw_version_id, err
            )
        })?;
    let parsed_tx_hash = parse_etherscan_tx_hash(&transaction.hash)
        .map_err(|err| format!("invalid etherscan normal tx hash: {err}"))?;
    if &parsed_tx_hash != expected_tx_hash {
        return Err(format!(
            "raw etherscan normal transaction version {} tx hash mismatch: stored {}, parsed {}",
            raw_version_id,
            expected_tx_hash.as_str(),
            parsed_tx_hash.as_str()
        ));
    }
    Ok(transaction)
}

pub(crate) fn parse_persisted_raw_etherscan_internal_transaction(
    raw_version_id: crate::db::raw_ingestion::RawEtherscanInternalTransactionVersionId,
    expected_tx_hash: &TxHash,
    expected_trace_id: &EtherscanTraceId,
    payload_bytes: &ExactPayloadBytes,
) -> Result<EtherscanInternalTx, String> {
    let transaction: EtherscanInternalTx = serde_json::from_slice(payload_bytes.as_slice())
        .map_err(|err| {
            format!(
                "raw etherscan internal transaction version {} failed JSON deserialization: {}",
                raw_version_id, err
            )
        })?;
    let parsed_tx_hash = parse_etherscan_tx_hash(&transaction.hash)
        .map_err(|err| format!("invalid etherscan internal tx hash: {err}"))?;
    if &parsed_tx_hash != expected_tx_hash {
        return Err(format!(
            "raw etherscan internal transaction version {} tx hash mismatch: stored {}, parsed {}",
            raw_version_id,
            expected_tx_hash.as_str(),
            parsed_tx_hash.as_str()
        ));
    }
    let parsed_trace_id = EtherscanTraceId::parse(&transaction.trace_id)
        .map_err(|err| format!("invalid etherscan internal trace id: {err}"))?;
    if &parsed_trace_id != expected_trace_id {
        return Err(format!(
            "raw etherscan internal transaction version {} trace id mismatch: stored {}, parsed {}",
            raw_version_id,
            expected_trace_id.as_str(),
            parsed_trace_id.as_str()
        ));
    }
    Ok(transaction)
}

fn raw_request_kind_for_page_kind(page_kind: RawMempoolPageKind) -> RawMempoolRequestKind {
    match page_kind {
        RawMempoolPageKind::FirstPage => RawMempoolRequestKind::AddressTransactionsFirstPage,
        RawMempoolPageKind::PaginatedAfterConfirmed => {
            RawMempoolRequestKind::AddressTransactionsAfterConfirmed
        }
    }
}

fn raw_page_cursor(
    page_cursor: Option<&str>,
) -> Result<Option<PageCursor>, UserTransactionMonitorError> {
    page_cursor
        .map(PageCursor::parse)
        .transpose()
        .map_err(UserTransactionMonitorError::from)
}

fn request_attempt_response_headers_json(
    raw_json: Option<&String>,
) -> Result<Option<ResponseHeadersJson>, UserTransactionMonitorError> {
    raw_json
        .cloned()
        .map(ResponseHeadersJson::parse)
        .transpose()
        .map_err(UserTransactionMonitorError::from)
}

fn request_attempt_response_body(bytes: Option<&[u8]>) -> Option<CapturedResponseBody> {
    bytes.and_then(|bytes| {
        CapturedResponseBody::truncate(bytes.to_vec(), REQUEST_ATTEMPT_RESPONSE_BODY_LIMIT_BYTES)
    })
}

pub(crate) fn request_attempt_outcome_for_mempool_error(
    error: &MempoolError,
) -> Result<Option<RequestAttemptOutcome>, UserTransactionMonitorError> {
    match error {
        MempoolError::Http { error, .. } => Ok(Some(RequestAttemptOutcome::TransportError {
            transport_error_message: TransportErrorMessage::parse(error.clone())?,
        })),
        MempoolError::UpstreamStatus {
            status,
            response_headers_json,
            response_body,
            ..
        } => Ok(Some(RequestAttemptOutcome::HttpResponse(
            RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(*status)?,
                response_headers_json: request_attempt_response_headers_json(
                    response_headers_json.as_ref(),
                )?,
                response_body: request_attempt_response_body(Some(response_body.as_slice())),
            },
        ))),
        MempoolError::RateLimited {
            response_headers_json,
            response_body,
            ..
        } => Ok(Some(RequestAttemptOutcome::HttpResponse(
            RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(429)?,
                response_headers_json: request_attempt_response_headers_json(
                    response_headers_json.as_ref(),
                )?,
                response_body: request_attempt_response_body(Some(response_body.as_slice())),
            },
        ))),
        MempoolError::Deserialize {
            http_status_code,
            response_headers_json,
            response_body,
            ..
        } => match http_status_code {
            Some(http_status_code) => Ok(Some(RequestAttemptOutcome::DeserializeError(
                RequestAttemptHttpResponse {
                    http_status_code: HttpStatusCode::try_new(*http_status_code)?,
                    response_headers_json: request_attempt_response_headers_json(
                        response_headers_json.as_ref(),
                    )?,
                    response_body: request_attempt_response_body(response_body.as_deref()),
                },
            ))),
            None => Ok(None),
        },
        MempoolError::UrlJoin(_) => Ok(None),
    }
}

fn request_url_for_mempool_error(
    error: &MempoolError,
) -> Result<Option<RequestUrl>, UserTransactionMonitorError> {
    match error {
        MempoolError::Http { url, .. }
        | MempoolError::UpstreamStatus { url, .. }
        | MempoolError::RateLimited { url, .. }
        | MempoolError::Deserialize { url, .. } => Ok(Some(
            RequestUrl::parse(url).map_err(UserTransactionMonitorError::from)?,
        )),
        MempoolError::UrlJoin(_) => Ok(None),
    }
}

pub(crate) fn request_attempt_outcome_for_etherscan_error(
    error: &EtherscanError,
) -> Result<RequestAttemptOutcome, UserTransactionMonitorError> {
    match error {
        EtherscanError::Http { error, .. } => Ok(RequestAttemptOutcome::TransportError {
            transport_error_message: TransportErrorMessage::parse(error.clone())?,
        }),
        EtherscanError::UpstreamStatus {
            status,
            body_snippet,
            ..
        } => Ok(RequestAttemptOutcome::HttpResponse(
            RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(*status)?,
                response_headers_json: None,
                response_body: request_attempt_response_body(Some(body_snippet.as_bytes())),
            },
        )),
        EtherscanError::Deserialize { .. } => Ok(RequestAttemptOutcome::DeserializeError(
            RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200)?,
                response_headers_json: None,
                response_body: None,
            },
        )),
        EtherscanError::ApiError { message, .. } => Ok(RequestAttemptOutcome::HttpResponse(
            RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200)?,
                response_headers_json: None,
                response_body: request_attempt_response_body(Some(message.as_bytes())),
            },
        )),
    }
}

fn parse_etherscan_tx_hash(raw_hash: &str) -> Result<TxHash, UserTransactionMonitorError> {
    let normalized = raw_hash.strip_prefix("0x").unwrap_or(raw_hash);
    TxHash::parse(normalized)
        .map_err(|err| UserTransactionMonitorError::Parse(format!("invalid tx hash: {err}")))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::*;

    fn sample_persisted_raw_payload(txid: &str, confirmed: bool) -> ExactPayloadBytes {
        let block_height = if confirmed { "123" } else { "null" };
        let block_time = if confirmed { "1700000000" } else { "null" };
        ExactPayloadBytes::try_new(
            format!(
                concat!(
                    "{{",
                    "\"txid\":\"{txid}\",",
                    "\"vin\":[],",
                    "\"vout\":[],",
                    "\"fee\":123,",
                    "\"status\":{{",
                    "\"confirmed\":{confirmed},",
                    "\"block_height\":{block_height},",
                    "\"block_hash\":null,",
                    "\"block_time\":{block_time}",
                    "}}",
                    "}}"
                ),
                txid = txid,
                confirmed = confirmed,
                block_height = block_height,
                block_time = block_time,
            )
            .into_bytes(),
        )
        .expect("payload should be non-empty")
    }

    #[test]
    fn parse_persisted_raw_mempool_transaction_accepts_matching_payload() {
        let raw_version_id = crate::db::raw_ingestion::RawMempoolTransactionVersionId::new();
        let expected_txid =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa10")
                .expect("expected txid should parse");
        let payload = sample_persisted_raw_payload(expected_txid.as_str(), true);

        let transaction =
            parse_persisted_raw_mempool_transaction(raw_version_id, &expected_txid, &payload)
                .expect("persisted raw payload should parse");

        assert_eq!(transaction.txid, expected_txid.as_str());
        assert!(transaction.status.confirmed);
    }

    #[test]
    fn parse_persisted_raw_mempool_transaction_rejects_txid_mismatch() {
        let raw_version_id = crate::db::raw_ingestion::RawMempoolTransactionVersionId::new();
        let expected_txid =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa10")
                .expect("expected txid should parse");
        let payload = sample_persisted_raw_payload(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa11",
            true,
        );

        let error =
            parse_persisted_raw_mempool_transaction(raw_version_id, &expected_txid, &payload)
                .expect_err("mismatched payload txid should fail");

        assert!(error.contains("txid mismatch"));
        assert!(error.contains(expected_txid.as_str()));
        assert!(error.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa11"));
    }

    #[test]
    fn parse_persisted_raw_etherscan_normal_transaction_accepts_matching_payload() {
        let raw_version_id =
            crate::db::raw_ingestion::RawEtherscanNormalTransactionVersionId::new();
        let expected_tx_hash =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa20")
                .expect("expected tx hash should parse");
        let payload = ExactPayloadBytes::try_new(
            br#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa20","blockNumber":"10","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"3"}"#.to_vec(),
        )
        .expect("payload should be non-empty");

        let transaction = parse_persisted_raw_etherscan_normal_transaction(
            raw_version_id,
            &expected_tx_hash,
            &payload,
        )
        .expect("persisted raw etherscan normal payload should parse");

        assert_eq!(
            transaction.hash,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa20"
        );
    }

    #[test]
    fn parse_persisted_raw_etherscan_internal_transaction_rejects_trace_id_mismatch() {
        let raw_version_id =
            crate::db::raw_ingestion::RawEtherscanInternalTransactionVersionId::new();
        let expected_tx_hash =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21")
                .expect("expected tx hash should parse");
        let expected_trace_id = EtherscanTraceId::parse("7").expect("trace id");
        let payload = ExactPayloadBytes::try_new(
            br#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21","blockNumber":"10","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","isError":"0","type":"call","traceId":"8"}"#.to_vec(),
        )
        .expect("payload should be non-empty");

        let error = parse_persisted_raw_etherscan_internal_transaction(
            raw_version_id,
            &expected_tx_hash,
            &expected_trace_id,
            &payload,
        )
        .expect_err("mismatched raw etherscan internal trace id should fail");

        assert!(error.contains("trace id mismatch"));
        assert!(error.contains("stored 7"));
        assert!(error.contains("parsed 8"));
    }

    #[test]
    fn request_attempt_outcome_for_mempool_error_maps_transport_error() {
        let outcome = request_attempt_outcome_for_mempool_error(&MempoolError::Http {
            url: "https://mempool.space/api/address/bc1qtest/txs".to_string(),
            error: "GET failed".to_string(),
        })
        .expect("transport outcome mapping should succeed");

        assert_eq!(
            outcome,
            Some(RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse("GET failed".to_string())
                    .expect("transport error message"),
            })
        );
    }

    #[test]
    fn request_attempt_outcome_for_mempool_error_maps_deserialize_error() {
        let outcome = request_attempt_outcome_for_mempool_error(&MempoolError::Deserialize {
            url: "https://mempool.space/api/address/bc1qtest/txs".to_string(),
            error: "invalid txid".to_string(),
            http_status_code: Some(200),
            response_headers_json: Some("{\"retry-after\":\"30\"}".to_string()),
            response_body: Some(vec![1_u8, 2, 3, 4]),
        })
        .expect("deserialize outcome mapping should succeed");

        assert_eq!(
            outcome,
            Some(RequestAttemptOutcome::DeserializeError(
                RequestAttemptHttpResponse {
                    http_status_code: HttpStatusCode::try_new(200).expect("http status"),
                    response_headers_json: Some(
                        ResponseHeadersJson::parse("{\"retry-after\":\"30\"}".to_string())
                            .expect("response headers json"),
                    ),
                    response_body: Some(
                        CapturedResponseBody::truncate(
                            vec![1_u8, 2, 3, 4],
                            REQUEST_ATTEMPT_RESPONSE_BODY_LIMIT_BYTES,
                        )
                        .expect("captured response body"),
                    ),
                }
            ))
        );
    }

    #[test]
    fn request_attempt_outcome_for_etherscan_error_maps_transport_error() {
        let outcome = request_attempt_outcome_for_etherscan_error(&EtherscanError::Http {
            url: "https://api.etherscan.io/v2/api".to_string(),
            error: "send_failed: timeout".to_string(),
        })
        .expect("etherscan transport outcome mapping should succeed");

        assert_eq!(
            outcome,
            RequestAttemptOutcome::TransportError {
                transport_error_message: TransportErrorMessage::parse(
                    "send_failed: timeout".to_string()
                )
                .expect("transport error message"),
            }
        );
    }

    #[test]
    fn request_attempt_outcome_for_etherscan_error_maps_deserialize_error() {
        let outcome = request_attempt_outcome_for_etherscan_error(&EtherscanError::Deserialize {
            url: "https://api.etherscan.io/v2/api?module=account".to_string(),
            error: "expected value at line 1 column 1".to_string(),
        })
        .expect("etherscan deserialize outcome mapping should succeed");

        assert_eq!(
            outcome,
            RequestAttemptOutcome::DeserializeError(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200).expect("http status"),
                response_headers_json: None,
                response_body: None,
            })
        );
    }

    #[test]
    fn request_attempt_outcome_for_etherscan_error_maps_api_error_to_http_response() {
        let outcome = request_attempt_outcome_for_etherscan_error(&EtherscanError::ApiError {
            status: "0".to_string(),
            message: "Max rate limit reached".to_string(),
        })
        .expect("etherscan api error outcome mapping should succeed");

        assert_eq!(
            outcome,
            RequestAttemptOutcome::HttpResponse(RequestAttemptHttpResponse {
                http_status_code: HttpStatusCode::try_new(200).expect("http status"),
                response_headers_json: None,
                response_body: CapturedResponseBody::truncate(
                    b"Max rate limit reached".to_vec(),
                    REQUEST_ATTEMPT_RESPONSE_BODY_LIMIT_BYTES,
                ),
            })
        );
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::raw_ingestion::{
        StartSyncRunRequest, SyncRunScopeKind, SyncRunTriggerKind,
        load_current_raw_mempool_transaction_heads,
        load_observed_raw_mempool_transactions_for_observation_set,
        load_raw_mempool_page_observations_for_sync_run, start_sync_run,
    };
    use crate::db::test_fixtures::{
        create_eth_wallet_account_fixture, setup_test_user, unique_user_id, wallet_label,
    };
    use crate::db::{acquire_test_runtime, add_bitcoin_address};
    use crate::ethereum::{EthAddress, RawEthAddress};
    use crate::integrations::etherscan::{EtherscanFetchedItem, EtherscanRequestMetadata};
    use crate::integrations::mempool::MempoolPageTransaction;
    use crate::models::UserId;
    use crate::wallets::{BtcAddress, Network, RawBtcAddress, SyncedAssetId};
    use url::Url;

    fn sample_btc_address() -> BtcAddress {
        BtcAddress::parse(
            &RawBtcAddress::new("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string()),
            Network::Mainnet,
        )
        .expect("sample btc address should parse")
    }

    fn sample_eth_address(last_hex: &str) -> EthAddress {
        let prefix_len = 40_usize.saturating_sub(last_hex.len());
        let raw = RawEthAddress::new(format!("0x{}{}", "1".repeat(prefix_len), last_hex));
        EthAddress::parse(&raw).expect("sample eth address should parse")
    }

    fn sample_txid(suffix_hex: &str) -> TxHash {
        let prefix_len = 64_usize.saturating_sub(suffix_hex.len());
        let txid = format!("{}{}", "a".repeat(prefix_len), suffix_hex);
        TxHash::parse(&txid).expect("sample txid should parse")
    }

    fn start_mempool_sync_run(
        user_id: UserId,
        address_id: DigitalAssetAddressId,
    ) -> crate::db::raw_ingestion::StartedSyncRun {
        start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: RawIntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: address_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Manual,
                started_at: Utc::now(),
                summary_json: None,
            },
        )
        .expect("mempool sync run should insert")
    }

    fn start_etherscan_sync_run(
        user_id: UserId,
        address_id: DigitalAssetAddressId,
    ) -> crate::db::raw_ingestion::StartedSyncRun {
        start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: RawIntegrationKind::Etherscan,
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

    #[test]
    fn ingest_mempool_page_aborts_on_first_parse_failure() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = add_bitcoin_address(
            user_id,
            &sample_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("mempool raw executor")),
            Utc::now(),
        )
        .expect("bitcoin address should insert");
        let sync_run = start_mempool_sync_run(user_id, inserted.address_id);
        let observed_at = Utc::now();

        let error = ingest_mempool_page(
            MempoolPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                scope_address_id: inserted.address_id,
                page_kind: RawMempoolPageKind::FirstPage,
                page_cursor: None,
                scan_start_run_id: None,
                network: Network::Mainnet,
                observed_at,
            },
            MempoolTransactionPage {
                request_url: Url::parse("https://mempool.space/api/address/bc1qtest/txs")
                    .expect("request url"),
                http_status_code: 200,
                transactions: vec![
                    MempoolPageTransaction {
                        txid: sample_txid("01"),
                        payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                    },
                    MempoolPageTransaction {
                        txid: sample_txid("02"),
                        payload_bytes: br#"{"txid":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff02","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                    },
                    MempoolPageTransaction {
                        txid: sample_txid("03"),
                        payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa03","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                    },
                ],
            },
        )
        .expect_err("mempool ingestion should fail on mismatched persisted payload");

        assert!(error.to_string().contains("page item 1"));
        assert!(error.to_string().contains("txid mismatch"));

        let heads =
            load_current_raw_mempool_transaction_heads(user_id, &sync_run.source_connection_id)
                .expect("current mempool heads should load after parse failure");
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].txid, sample_txid("01"));
        let sets = load_raw_mempool_page_observations_for_sync_run(user_id, sync_run.sync_run_id)
            .expect("page observations should load");
        assert!(sets.is_empty());
    }

    #[test]
    fn raw_mempool_page_observation_records_complete_page() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = add_bitcoin_address(
            user_id,
            &sample_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("mempool page observation")),
            Utc::now(),
        )
        .expect("bitcoin address should insert");
        let sync_run = start_mempool_sync_run(user_id, inserted.address_id);
        let scan_start_run_id = SyncRunId::new();
        let previous_cursor = sample_txid("10").as_str().to_string();
        let next_cursor = sample_txid("12").as_str().to_string();

        ingest_mempool_page(
            MempoolPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                scope_address_id: inserted.address_id,
                page_kind: RawMempoolPageKind::PaginatedAfterConfirmed,
                page_cursor: Some(&previous_cursor),
                scan_start_run_id: Some(scan_start_run_id),
                network: Network::Mainnet,
                observed_at: Utc::now(),
            },
            MempoolTransactionPage {
                request_url: Url::parse("https://mempool.space/api/address/bc1qtest/txs/chain/previous")
                    .expect("request url"),
                http_status_code: 200,
                transactions: vec![
                    MempoolPageTransaction {
                        txid: sample_txid("11"),
                        payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa11","vin":[],"vout":[],"status":{"confirmed":false}}"#.to_vec(),
                    },
                    MempoolPageTransaction {
                        txid: sample_txid("12"),
                        payload_bytes: br#"{"txid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa12","vin":[],"vout":[],"status":{"confirmed":true}}"#.to_vec(),
                    },
                ],
            },
        )
        .expect("page should ingest");

        let sets = load_raw_mempool_page_observations_for_sync_run(user_id, sync_run.sync_run_id)
            .expect("page observations should load");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].metadata.scan_start_run_id, Some(scan_start_run_id));
        assert_eq!(sets[0].metadata.requested_cursor, Some(previous_cursor));
        assert_eq!(
            sets[0].metadata.returned_last_confirmed_cursor,
            Some(next_cursor)
        );
        assert_eq!(sets[0].metadata.item_count, 2);
        let members = load_observed_raw_mempool_transactions_for_observation_set(
            user_id,
            sets[0].raw_observation_set_id,
        )
        .expect("page memberships should load");
        assert_eq!(
            members
                .iter()
                .map(|row| row.page_item_index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn raw_mempool_page_observation_records_empty_terminal_page() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = add_bitcoin_address(
            user_id,
            &sample_btc_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("empty mempool page observation")),
            Utc::now(),
        )
        .expect("bitcoin address should insert");
        let sync_run = start_mempool_sync_run(user_id, inserted.address_id);

        ingest_mempool_page(
            MempoolPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                scope_address_id: inserted.address_id,
                page_kind: RawMempoolPageKind::PaginatedAfterConfirmed,
                page_cursor: Some("previous"),
                scan_start_run_id: None,
                network: Network::Mainnet,
                observed_at: Utc::now(),
            },
            MempoolTransactionPage {
                request_url: Url::parse(
                    "https://mempool.space/api/address/bc1qtest/txs/chain/previous",
                )
                .expect("request url"),
                http_status_code: 200,
                transactions: Vec::new(),
            },
        )
        .expect("empty terminal page should ingest");

        let sets = load_raw_mempool_page_observations_for_sync_run(user_id, sync_run.sync_run_id)
            .expect("page observations should load");
        assert_eq!(sets.len(), 1);
        let members = load_observed_raw_mempool_transactions_for_observation_set(
            user_id,
            sets[0].raw_observation_set_id,
        )
        .expect("page memberships should load");
        assert!(members.is_empty());
    }

    #[test]
    fn ingest_etherscan_normal_page_returns_parsed_items_in_order() {
        let _guard = acquire_test_runtime();
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let inserted = create_eth_wallet_account_fixture(
            user_id,
            &sample_eth_address("42"),
            "etherscan raw executor",
            Utc::now(),
        );
        let sync_run = start_etherscan_sync_run(user_id, inserted.address_id);
        let observed_at = Utc::now();
        let first_raw = r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#;
        let second_raw = r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa22","blockNumber":"101","timeStamp":"2","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","gasPrice":"8","gasUsed":"10","isError":"0","txreceipt_status":"1","nonce":"2"}"#;

        let parsed = ingest_etherscan_normal_page(
            EtherscanPageIngestionRequest {
                user_id,
                raw_sync_run_id: sync_run.sync_run_id,
                source_connection_id: &sync_run.source_connection_id,
                chain_id: EtherscanChainId::try_new(1).expect("chain id"),
                network: Network::Mainnet,
                observed_at,
            },
            EtherscanFetchedPage {
                request: EtherscanRequestMetadata {
                    request_url_without_api_key: "https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address=0x1111111111111111111111111111111111111111&startblock=0&endblock=99999999&sort=asc&page=1&offset=1000".to_string(),
                    request_query_json: "{\"chainid\":\"1\",\"module\":\"account\",\"action\":\"txlist\",\"address\":\"0x1111111111111111111111111111111111111111\",\"startblock\":\"0\",\"endblock\":\"99999999\",\"sort\":\"asc\",\"page\":\"1\",\"offset\":\"1000\"}".to_string(),
                },
                items: vec![
                    EtherscanFetchedItem {
                        parsed: EtherscanNormalTx {
                            hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21".to_string(),
                            block_number: "100".to_string(),
                            time_stamp: "1".to_string(),
                            from: "0x1111111111111111111111111111111111111111".to_string(),
                            to: "0x2222222222222222222222222222222222222222".to_string(),
                            value: "5".to_string(),
                            gas_price: "7".to_string(),
                            gas_used: "9".to_string(),
                            is_error: "0".to_string(),
                            txreceipt_status: "1".to_string(),
                            nonce: "1".to_string(),
                        },
                        raw_json_bytes: first_raw.as_bytes().to_vec(),
                    },
                    EtherscanFetchedItem {
                        parsed: EtherscanNormalTx {
                            hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa22".to_string(),
                            block_number: "101".to_string(),
                            time_stamp: "2".to_string(),
                            from: "0x1111111111111111111111111111111111111111".to_string(),
                            to: "0x3333333333333333333333333333333333333333".to_string(),
                            value: "6".to_string(),
                            gas_price: "8".to_string(),
                            gas_used: "10".to_string(),
                            is_error: "0".to_string(),
                            txreceipt_status: "1".to_string(),
                            nonce: "2".to_string(),
                        },
                        raw_json_bytes: second_raw.as_bytes().to_vec(),
                    },
                ],
            },
        )
        .expect("etherscan normal ingestion should succeed");

        assert_eq!(parsed.transactions.len(), 2);
        assert_eq!(
            parsed.transactions[0].hash,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21"
        );
        assert_eq!(
            parsed.transactions[1].hash,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa22"
        );
    }
}

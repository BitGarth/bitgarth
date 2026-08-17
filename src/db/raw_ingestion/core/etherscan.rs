use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::transactions::TxHash;
use crate::wallets::Network;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[cfg(all(test, feature = "db-tests"))]
use super::ids::{
    RawEtherscanInternalTransactionObservationId, RawEtherscanNormalTransactionObservationId,
    RawObservationSetId, SyncRunId,
};
use super::ids::{
    RawEtherscanInternalTransactionVersionId, RawEtherscanNormalTransactionVersionId,
    SourceConnectionId,
};
use super::payload::{ExactPayloadBytes, PayloadSha256Hex};
use super::shared::{EtherscanChainId, EtherscanTraceId, RawVersionWriteOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentRawEtherscanNormalTransactionHeadRow {
    pub(crate) raw_version_id: RawEtherscanNormalTransactionVersionId,
    pub(crate) tx_hash: TxHash,
    pub(crate) payload_bytes: ExactPayloadBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentRawEtherscanInternalTransactionHeadRow {
    pub(crate) raw_version_id: RawEtherscanInternalTransactionVersionId,
    pub(crate) tx_hash: TxHash,
    pub(crate) trace_id: EtherscanTraceId,
    pub(crate) payload_bytes: ExactPayloadBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AllCurrentRawEtherscanNormalTransactionHeadRow {
    pub(crate) network: Network,
    pub(crate) raw_version_id: RawEtherscanNormalTransactionVersionId,
    pub(crate) tx_hash: TxHash,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AllCurrentRawEtherscanInternalTransactionHeadRow {
    pub(crate) network: Network,
    pub(crate) raw_version_id: RawEtherscanInternalTransactionVersionId,
    pub(crate) tx_hash: TxHash,
    pub(crate) trace_id: EtherscanTraceId,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InsertRawEtherscanNormalTransactionVersionRequest {
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) chain_id: EtherscanChainId,
    pub(crate) network: Network,
    pub(crate) tx_hash: TxHash,
    pub(crate) payload_hash_sha256_hex: PayloadSha256Hex,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) first_observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InsertedRawEtherscanNormalTransactionVersion {
    pub(crate) raw_version_id: RawEtherscanNormalTransactionVersionId,
    pub(crate) write_outcome: RawVersionWriteOutcome,
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRawEtherscanNormalTransactionObservationRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) raw_observation_set_id: RawObservationSetId,
    pub(crate) raw_etherscan_normal_transaction_version_id: RawEtherscanNormalTransactionVersionId,
    pub(crate) page_item_index: i64,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InsertRawEtherscanInternalTransactionVersionRequest {
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) chain_id: EtherscanChainId,
    pub(crate) network: Network,
    pub(crate) tx_hash: TxHash,
    pub(crate) trace_id: EtherscanTraceId,
    pub(crate) payload_hash_sha256_hex: PayloadSha256Hex,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) first_observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InsertedRawEtherscanInternalTransactionVersion {
    pub(crate) raw_version_id: RawEtherscanInternalTransactionVersionId,
    pub(crate) write_outcome: RawVersionWriteOutcome,
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRawEtherscanInternalTransactionObservationRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) raw_observation_set_id: RawObservationSetId,
    pub(crate) raw_etherscan_internal_transaction_version_id:
        RawEtherscanInternalTransactionVersionId,
    pub(crate) page_item_index: i64,
    pub(crate) observed_at: DateTime<Utc>,
}
fn load_current_raw_etherscan_normal_tx_head(
    conn: &rusqlite::Connection,
    source_connection_id: &SourceConnectionId,
    tx_hash: &TxHash,
) -> Result<Option<CurrentRawEtherscanNormalTransactionHeadRow>, DbError> {
    conn.query_row(
            "SELECT id, tx_hash, payload_bytes
             FROM raw_etherscan_normal_transaction_versions
             WHERE source_connection_id = ?1
               AND tx_hash = ?2
               AND NOT EXISTS (
                   SELECT 1
                   FROM raw_etherscan_normal_transaction_versions newer
                   WHERE newer.supersedes_raw_version_id = raw_etherscan_normal_transaction_versions.id
               )",
            params![source_connection_id.to_string(), tx_hash.as_str()],
            |row| {
                let raw_version_id_raw: String = row.get(0)?;
                let tx_hash_raw: String = row.get(1)?;
                let payload_bytes_raw: Vec<u8> = row.get(2)?;
                let raw_version_id =
                    RawEtherscanNormalTransactionVersionId::from_str(&raw_version_id_raw).map_err(
                        |err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(err),
                            )
                        },
                    )?;
                let tx_hash = TxHash::parse(&tx_hash_raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(DbError::new(format!(
                            "Invalid etherscan normal tx_hash in DB: {err}"
                        ))),
                    )
                })?;
                let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Blob,
                        Box::new(err),
                    )
                })?;
                Ok(CurrentRawEtherscanNormalTransactionHeadRow {
                    raw_version_id,
                    tx_hash,
                    payload_bytes,
                })
            },
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to load current raw etherscan normal head",
                err,
            )
        })
}

pub(crate) fn load_current_raw_etherscan_normal_transaction_heads(
    user_id: UserId,
    source_connection_id: &SourceConnectionId,
) -> Result<Vec<CurrentRawEtherscanNormalTransactionHeadRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT v.id, v.tx_hash, v.payload_bytes
                 FROM raw_etherscan_normal_transaction_versions v
                 WHERE v.source_connection_id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM raw_etherscan_normal_transaction_versions newer
                       WHERE newer.supersedes_raw_version_id = v.id
                   )
                 ORDER BY v.tx_hash ASC, v.created_at ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare current raw etherscan normal head lookup",
                    err,
                )
            })?;
        stmt.query_map([source_connection_id.to_string()], |row| {
            let raw_version_id_raw: String = row.get(0)?;
            let tx_hash_raw: String = row.get(1)?;
            let payload_bytes_raw: Vec<u8> = row.get(2)?;
            let raw_version_id = RawEtherscanNormalTransactionVersionId::from_str(
                &raw_version_id_raw,
            )
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            let tx_hash = TxHash::parse(&tx_hash_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(DbError::new(format!(
                        "Invalid etherscan normal tx_hash in DB: {err}"
                    ))),
                )
            })?;
            let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Blob,
                    Box::new(err),
                )
            })?;
            Ok(CurrentRawEtherscanNormalTransactionHeadRow {
                raw_version_id,
                tx_hash,
                payload_bytes,
            })
        })
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query current raw etherscan normal heads", err)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to read current raw etherscan normal heads", err)
        })
    })
}

pub(crate) fn load_all_current_raw_etherscan_normal_transaction_heads_conn(
    conn: &rusqlite::Connection,
) -> Result<Vec<AllCurrentRawEtherscanNormalTransactionHeadRow>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT v.network, v.id, v.tx_hash, v.payload_bytes, v.created_at
             FROM raw_etherscan_normal_transaction_versions v
             WHERE NOT EXISTS (
                 SELECT 1 FROM raw_etherscan_normal_transaction_versions newer
                 WHERE newer.supersedes_raw_version_id = v.id
             )
             ORDER BY v.network, v.tx_hash, v.created_at, v.id",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to prepare all-current Etherscan normal head lookup",
                err,
            )
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query all-current Etherscan normal heads", err)
        })?;

    let mut heads = Vec::new();
    for row in rows {
        let (network, raw_version_id, tx_hash, payload_bytes, created_at) = row.map_err(|err| {
            DbError::from_rusqlite_error("Failed to read all-current Etherscan normal head", err)
        })?;
        heads.push(AllCurrentRawEtherscanNormalTransactionHeadRow {
            network: Network::from_str(&network)
                .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network}")))?,
            raw_version_id: RawEtherscanNormalTransactionVersionId::from_str(&raw_version_id)
                .map_err(|err| DbError::new(format!("Invalid raw normal version ID: {err}")))?,
            tx_hash: TxHash::parse(&tx_hash)
                .map_err(|err| DbError::new(format!("Invalid normal tx hash in DB: {err}")))?,
            payload_bytes: ExactPayloadBytes::try_new(payload_bytes)?,
            created_at: crate::models::parse_datetime(&created_at)
                .map_err(|err| DbError::new(format!("Invalid raw normal created_at: {err}")))?,
        });
    }
    Ok(heads)
}

pub(crate) fn insert_raw_etherscan_normal_transaction_version(
    user_id: UserId,
    request: InsertRawEtherscanNormalTransactionVersionRequest,
) -> Result<InsertedRawEtherscanNormalTransactionVersion, DbError> {
    let raw_version_id = RawEtherscanNormalTransactionVersionId::new();
    with_user_db_mut(user_id, |conn| {
        let current_head = load_current_raw_etherscan_normal_tx_head(
            conn,
            &request.source_connection_id,
            &request.tx_hash,
        )?;
        if let Some(current_head) = current_head.as_ref()
            && current_head.payload_bytes == request.payload_bytes
        {
            return Ok(InsertedRawEtherscanNormalTransactionVersion {
                raw_version_id: current_head.raw_version_id,
                write_outcome: RawVersionWriteOutcome::ReusedCurrentHead,
            });
        }
        conn.execute(
                "INSERT INTO raw_etherscan_normal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    raw_version_id.to_string(),
                    request.source_connection_id.to_string(),
                    request.chain_id.value(),
                    request.network.as_str(),
                    request.tx_hash.as_str(),
                    request.payload_hash_sha256_hex.as_str(),
                    request.payload_bytes.as_slice(),
                    request.first_observed_at.to_rfc3339(),
                    current_head.map(|head| head.raw_version_id.to_string()),
                    request.first_observed_at.to_rfc3339(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to insert raw etherscan normal transaction version",
                    err,
                )
            })?;
        Ok(InsertedRawEtherscanNormalTransactionVersion {
            raw_version_id,
            write_outcome: RawVersionWriteOutcome::InsertedNewHead,
        })
    })
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn record_raw_etherscan_normal_observation(
    user_id: UserId,
    request: RecordRawEtherscanNormalTransactionObservationRequest,
) -> Result<(), DbError> {
    if request.page_item_index < 0 {
        return Err(DbError::new("page item index cannot be negative"));
    }

    let observation_id = RawEtherscanNormalTransactionObservationId::new();
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO raw_etherscan_normal_transaction_observations
             (id, sync_run_id, source_connection_id, raw_observation_set_id, raw_etherscan_normal_transaction_version_id, page_item_index, observed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                observation_id.to_string(),
                request.sync_run_id.to_string(),
                request.source_connection_id.to_string(),
                request.raw_observation_set_id.to_string(),
                request
                    .raw_etherscan_normal_transaction_version_id
                    .to_string(),
                request.page_item_index,
                request.observed_at.to_rfc3339(),
                request.observed_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to insert raw etherscan normal transaction observation",
                err,
            )
        })?;
        Ok(())
    })
}

fn load_current_raw_etherscan_internal_tx_head(
    conn: &rusqlite::Connection,
    source_connection_id: &SourceConnectionId,
    tx_hash: &TxHash,
    trace_id: &EtherscanTraceId,
) -> Result<Option<CurrentRawEtherscanInternalTransactionHeadRow>, DbError> {
    conn.query_row(
            "SELECT id, tx_hash, trace_id, payload_bytes
             FROM raw_etherscan_internal_transaction_versions
             WHERE source_connection_id = ?1
               AND tx_hash = ?2
               AND trace_id = ?3
               AND NOT EXISTS (
                   SELECT 1
                   FROM raw_etherscan_internal_transaction_versions newer
                   WHERE newer.supersedes_raw_version_id = raw_etherscan_internal_transaction_versions.id
               )",
            params![
                source_connection_id.to_string(),
                tx_hash.as_str(),
                trace_id.as_str(),
            ],
            |row| {
                let raw_version_id_raw: String = row.get(0)?;
                let tx_hash_raw: String = row.get(1)?;
                let trace_id_raw: String = row.get(2)?;
                let payload_bytes_raw: Vec<u8> = row.get(3)?;
                let raw_version_id = RawEtherscanInternalTransactionVersionId::from_str(
                    &raw_version_id_raw,
                )
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
                let tx_hash = TxHash::parse(&tx_hash_raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(DbError::new(format!(
                            "Invalid etherscan internal tx_hash in DB: {err}"
                        ))),
                    )
                })?;
                let trace_id = EtherscanTraceId::parse(&trace_id_raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
                let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Blob,
                        Box::new(err),
                    )
                })?;
                Ok(CurrentRawEtherscanInternalTransactionHeadRow {
                    raw_version_id,
                    tx_hash,
                    trace_id,
                    payload_bytes,
                })
            },
        )
        .optional()
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to load current raw etherscan internal head",
                err,
            )
        })
}

pub(crate) fn load_current_raw_etherscan_internal_transaction_heads(
    user_id: UserId,
    source_connection_id: &SourceConnectionId,
) -> Result<Vec<CurrentRawEtherscanInternalTransactionHeadRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT v.id, v.tx_hash, v.trace_id, v.payload_bytes
                 FROM raw_etherscan_internal_transaction_versions v
                 WHERE v.source_connection_id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM raw_etherscan_internal_transaction_versions newer
                       WHERE newer.supersedes_raw_version_id = v.id
                   )
                 ORDER BY v.tx_hash ASC, v.trace_id ASC, v.created_at ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare current raw etherscan internal head lookup",
                    err,
                )
            })?;
        stmt.query_map([source_connection_id.to_string()], |row| {
            let raw_version_id_raw: String = row.get(0)?;
            let tx_hash_raw: String = row.get(1)?;
            let trace_id_raw: String = row.get(2)?;
            let payload_bytes_raw: Vec<u8> = row.get(3)?;
            let raw_version_id = RawEtherscanInternalTransactionVersionId::from_str(
                &raw_version_id_raw,
            )
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            let tx_hash = TxHash::parse(&tx_hash_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(DbError::new(format!(
                        "Invalid etherscan internal tx_hash in DB: {err}"
                    ))),
                )
            })?;
            let trace_id = EtherscanTraceId::parse(&trace_id_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Blob,
                    Box::new(err),
                )
            })?;
            Ok(CurrentRawEtherscanInternalTransactionHeadRow {
                raw_version_id,
                tx_hash,
                trace_id,
                payload_bytes,
            })
        })
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to query current raw etherscan internal heads",
                err,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to read current raw etherscan internal heads", err)
        })
    })
}

pub(crate) fn load_all_current_raw_etherscan_internal_transaction_heads_conn(
    conn: &rusqlite::Connection,
) -> Result<Vec<AllCurrentRawEtherscanInternalTransactionHeadRow>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT v.network, v.id, v.tx_hash, v.trace_id, v.payload_bytes, v.created_at
             FROM raw_etherscan_internal_transaction_versions v
             WHERE NOT EXISTS (
                 SELECT 1 FROM raw_etherscan_internal_transaction_versions newer
                 WHERE newer.supersedes_raw_version_id = v.id
             )
             ORDER BY v.network, v.tx_hash, v.trace_id, v.created_at, v.id",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to prepare all-current Etherscan internal head lookup",
                err,
            )
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to query all-current Etherscan internal heads",
                err,
            )
        })?;

    let mut heads = Vec::new();
    for row in rows {
        let (network, raw_version_id, tx_hash, trace_id, payload_bytes, created_at) =
            row.map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to read all-current Etherscan internal head",
                    err,
                )
            })?;
        heads.push(AllCurrentRawEtherscanInternalTransactionHeadRow {
            network: Network::from_str(&network)
                .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network}")))?,
            raw_version_id: RawEtherscanInternalTransactionVersionId::from_str(&raw_version_id)
                .map_err(|err| DbError::new(format!("Invalid raw internal version ID: {err}")))?,
            tx_hash: TxHash::parse(&tx_hash)
                .map_err(|err| DbError::new(format!("Invalid internal tx hash in DB: {err}")))?,
            trace_id: EtherscanTraceId::parse(&trace_id)?,
            payload_bytes: ExactPayloadBytes::try_new(payload_bytes)?,
            created_at: crate::models::parse_datetime(&created_at)
                .map_err(|err| DbError::new(format!("Invalid raw internal created_at: {err}")))?,
        });
    }
    Ok(heads)
}

pub(crate) fn insert_raw_etherscan_internal_transaction_version(
    user_id: UserId,
    request: InsertRawEtherscanInternalTransactionVersionRequest,
) -> Result<InsertedRawEtherscanInternalTransactionVersion, DbError> {
    let raw_version_id = RawEtherscanInternalTransactionVersionId::new();
    with_user_db_mut(user_id, |conn| {
        let current_head = load_current_raw_etherscan_internal_tx_head(
            conn,
            &request.source_connection_id,
            &request.tx_hash,
            &request.trace_id,
        )?;
        if let Some(current_head) = current_head.as_ref()
            && current_head.payload_bytes == request.payload_bytes
        {
            return Ok(InsertedRawEtherscanInternalTransactionVersion {
                raw_version_id: current_head.raw_version_id,
                write_outcome: RawVersionWriteOutcome::ReusedCurrentHead,
            });
        }
        conn.execute(
                "INSERT INTO raw_etherscan_internal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash, trace_id, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    raw_version_id.to_string(),
                    request.source_connection_id.to_string(),
                    request.chain_id.value(),
                    request.network.as_str(),
                    request.tx_hash.as_str(),
                    request.trace_id.as_str(),
                    request.payload_hash_sha256_hex.as_str(),
                    request.payload_bytes.as_slice(),
                    request.first_observed_at.to_rfc3339(),
                    current_head.map(|head| head.raw_version_id.to_string()),
                    request.first_observed_at.to_rfc3339(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to insert raw etherscan internal transaction version",
                    err,
                )
            })?;
        Ok(InsertedRawEtherscanInternalTransactionVersion {
            raw_version_id,
            write_outcome: RawVersionWriteOutcome::InsertedNewHead,
        })
    })
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn record_raw_etherscan_internal_observation(
    user_id: UserId,
    request: RecordRawEtherscanInternalTransactionObservationRequest,
) -> Result<(), DbError> {
    if request.page_item_index < 0 {
        return Err(DbError::new("page item index cannot be negative"));
    }

    let observation_id = RawEtherscanInternalTransactionObservationId::new();
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO raw_etherscan_internal_transaction_observations
             (id, sync_run_id, source_connection_id, raw_observation_set_id, raw_etherscan_internal_transaction_version_id, page_item_index, observed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                observation_id.to_string(),
                request.sync_run_id.to_string(),
                request.source_connection_id.to_string(),
                request.raw_observation_set_id.to_string(),
                request
                    .raw_etherscan_internal_transaction_version_id
                    .to_string(),
                request.page_item_index,
                request.observed_at.to_rfc3339(),
                request.observed_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to insert raw etherscan internal transaction observation",
                err,
            )
        })?;
        Ok(())
    })
}

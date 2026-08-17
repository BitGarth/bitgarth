use crate::db::error::DbError;
use crate::db::user_db::with_user_db;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::transactions::TxHash;
use crate::wallets::Network;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

use super::ids::{RawMempoolTransactionObservationId, RawObservationSetId, SyncRunId};
use super::ids::{RawMempoolTransactionVersionId, SourceConnectionId};
use super::payload::{ExactPayloadBytes, PayloadSha256Hex};
use super::shared::RawVersionWriteOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentRawMempoolTransactionHeadRow {
    pub(crate) raw_version_id: RawMempoolTransactionVersionId,
    pub(crate) txid: TxHash,
    pub(crate) payload_bytes: ExactPayloadBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InsertRawMempoolTransactionVersionRequest {
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) network: Network,
    pub(crate) txid: TxHash,
    pub(crate) payload_hash_sha256_hex: PayloadSha256Hex,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) first_observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InsertedRawMempoolTransactionVersion {
    pub(crate) raw_version_id: RawMempoolTransactionVersionId,
    pub(crate) write_outcome: RawVersionWriteOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRawMempoolTransactionObservationRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) raw_observation_set_id: RawObservationSetId,
    pub(crate) raw_mempool_transaction_version_id: RawMempoolTransactionVersionId,
    pub(crate) page_item_index: i64,
    pub(crate) observed_at: DateTime<Utc>,
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedRawMempoolTransactionForRequestRow {
    pub(crate) raw_version_id: RawMempoolTransactionVersionId,
    pub(crate) txid: TxHash,
    pub(crate) payload_bytes: ExactPayloadBytes,
    pub(crate) page_item_index: i64,
}
fn load_current_raw_mempool_tx_head(
    conn: &rusqlite::Connection,
    source_connection_id: &SourceConnectionId,
    txid: &TxHash,
) -> Result<Option<CurrentRawMempoolTransactionHeadRow>, DbError> {
    conn.query_row(
        "SELECT id, txid, payload_bytes
             FROM raw_mempool_transaction_versions
             WHERE source_connection_id = ?1
               AND txid = ?2
               AND NOT EXISTS (
                   SELECT 1
                   FROM raw_mempool_transaction_versions newer
                   WHERE newer.supersedes_raw_version_id = raw_mempool_transaction_versions.id
               )",
        params![source_connection_id.to_string(), txid.as_str()],
        |row| {
            let raw_version_id_raw: String = row.get(0)?;
            let txid_raw: String = row.get(1)?;
            let payload_bytes_raw: Vec<u8> = row.get(2)?;
            let raw_version_id = RawMempoolTransactionVersionId::from_str(&raw_version_id_raw)
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            let txid = TxHash::parse(&txid_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(DbError::new(format!("Invalid txid in DB: {err}"))),
                )
            })?;
            let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Blob,
                    Box::new(err),
                )
            })?;
            Ok(CurrentRawMempoolTransactionHeadRow {
                raw_version_id,
                txid,
                payload_bytes,
            })
        },
    )
    .optional()
    .map_err(|err| DbError::from_rusqlite_error("Failed to load current raw mempool head", err))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyMempoolHeadRepairCandidate {
    source_connection_id: SourceConnectionId,
    network: Network,
    txid: TxHash,
    payload_bytes: ExactPayloadBytes,
    observed_at: DateTime<Utc>,
    current_head_raw_version_id: RawMempoolTransactionVersionId,
}

fn load_legacy_mempool_head_repair_candidates_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<Vec<LegacyMempoolHeadRepairCandidate>, DbError> {
    let mut stmt = tx
        .prepare(
            "WITH current_heads AS (
                 SELECT v.id, v.source_connection_id, v.txid, v.payload_bytes
                 FROM raw_mempool_transaction_versions v
                 WHERE NOT EXISTS (
                     SELECT 1
                     FROM raw_mempool_transaction_versions newer
                     WHERE newer.supersedes_raw_version_id = v.id
                 )
             ),
             latest_observed AS (
                 SELECT
                     o.source_connection_id,
                     observed_v.network,
                     observed_v.txid,
                     observed_v.payload_bytes AS observed_payload_bytes,
                     o.observed_at,
                     current_heads.id AS current_head_raw_version_id,
                     current_heads.payload_bytes AS current_head_payload_bytes,
                     ROW_NUMBER() OVER (
                         PARTITION BY o.source_connection_id, observed_v.txid
                         ORDER BY o.observed_at DESC, o.created_at DESC, o.id DESC
                     ) AS latest_rank
                 FROM raw_mempool_transaction_observations o
                 INNER JOIN raw_mempool_transaction_versions observed_v
                     ON observed_v.id = o.raw_mempool_transaction_version_id
                 INNER JOIN current_heads
                     ON current_heads.source_connection_id = o.source_connection_id
                    AND current_heads.txid = observed_v.txid
             )
             SELECT
                 source_connection_id,
                 network,
                 txid,
                 observed_payload_bytes,
                 observed_at,
                 current_head_raw_version_id
             FROM latest_observed
             WHERE latest_rank = 1
               AND observed_payload_bytes != current_head_payload_bytes",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to prepare legacy mempool head repair candidate query",
                err,
            )
        })?;

    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })
    .map_err(|err| {
        DbError::from_rusqlite_error("Failed to query legacy mempool head repair candidates", err)
    })?
    .map(|row| {
        let (
            source_connection_id_raw,
            network_raw,
            txid_raw,
            payload_bytes_raw,
            observed_at_raw,
            current_head_raw_version_id_raw,
        ) = row.map_err(|err| {
            DbError::from_rusqlite_error("Failed to read legacy mempool head repair candidate", err)
        })?;
        let source_connection_id = SourceConnectionId::parse(&source_connection_id_raw)?;
        let network = Network::from_str(&network_raw)
            .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network_raw}")))?;
        let txid = TxHash::parse(&txid_raw)
            .map_err(|err| DbError::new(format!("Invalid txid in DB: {err}")))?;
        let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw)?;
        let observed_at = crate::models::parse_datetime(&observed_at_raw).map_err(|err| {
            DbError::new(format!(
                "Failed to parse observed_at for legacy mempool head repair candidate: {err}"
            ))
        })?;
        let current_head_raw_version_id = RawMempoolTransactionVersionId::from_str(
            &current_head_raw_version_id_raw,
        )
        .map_err(|err| {
            DbError::new(format!(
                "Invalid current head raw version id in DB during mempool repair: {err}"
            ))
        })?;

        Ok(LegacyMempoolHeadRepairCandidate {
            source_connection_id,
            network,
            txid,
            payload_bytes,
            observed_at,
            current_head_raw_version_id,
        })
    })
    .collect()
}

pub(crate) fn repair_legacy_mempool_head_rebuild_contract(
    conn: &mut rusqlite::Connection,
) -> Result<u32, DbError> {
    let tx = conn.transaction().map_err(|err| {
        DbError::new(format!(
            "Failed to start legacy mempool head rebuild contract repair: {err}"
        ))
    })?;
    let candidates = load_legacy_mempool_head_repair_candidates_tx(&tx)?;

    for candidate in &candidates {
        let repaired_raw_version_id = RawMempoolTransactionVersionId::new();
        let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&candidate.payload_bytes);
        tx.execute(
            "INSERT INTO raw_mempool_transaction_versions
             (id, source_connection_id, network, txid, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                repaired_raw_version_id.to_string(),
                candidate.source_connection_id.to_string(),
                candidate.network.as_str(),
                candidate.txid.as_str(),
                payload_hash_sha256_hex.as_str(),
                candidate.payload_bytes.as_slice(),
                candidate.observed_at.to_rfc3339(),
                candidate.current_head_raw_version_id.to_string(),
                candidate.observed_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to insert repaired legacy mempool head version",
                err,
            )
        })?;
    }

    tx.commit().map_err(|err| {
        DbError::new(format!(
            "Failed to commit legacy mempool head rebuild contract repair: {err}"
        ))
    })?;

    u32::try_from(candidates.len())
        .map_err(|_| DbError::new("legacy mempool head repair count out of range"))
}

pub(crate) fn load_current_raw_mempool_transaction_heads(
    user_id: UserId,
    source_connection_id: &SourceConnectionId,
) -> Result<Vec<CurrentRawMempoolTransactionHeadRow>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT v.id, v.txid, v.payload_bytes
                 FROM raw_mempool_transaction_versions v
                 WHERE v.source_connection_id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM raw_mempool_transaction_versions newer
                       WHERE newer.supersedes_raw_version_id = v.id
                   )
                 ORDER BY v.txid ASC, v.created_at ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare current raw mempool head lookup",
                    err,
                )
            })?;
        stmt.query_map([source_connection_id.to_string()], |row| {
            let raw_version_id_raw: String = row.get(0)?;
            let txid_raw: String = row.get(1)?;
            let payload_bytes_raw: Vec<u8> = row.get(2)?;
            let raw_version_id = RawMempoolTransactionVersionId::from_str(&raw_version_id_raw)
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            let txid = TxHash::parse(&txid_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(DbError::new(format!("Invalid txid in DB: {err}"))),
                )
            })?;
            let payload_bytes = ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Blob,
                    Box::new(err),
                )
            })?;
            Ok(CurrentRawMempoolTransactionHeadRow {
                raw_version_id,
                txid,
                payload_bytes,
            })
        })
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query current raw mempool heads", err)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to read current raw mempool heads", err)
        })
    })
}

pub(crate) fn insert_raw_mempool_tx_version(
    user_id: UserId,
    request: InsertRawMempoolTransactionVersionRequest,
) -> Result<InsertedRawMempoolTransactionVersion, DbError> {
    let raw_version_id = RawMempoolTransactionVersionId::new();
    with_user_db_mut(user_id, |conn| {
        let current_head =
            load_current_raw_mempool_tx_head(conn, &request.source_connection_id, &request.txid)?;
        if let Some(current_head) = current_head.as_ref()
            && current_head.payload_bytes == request.payload_bytes
        {
            return Ok(InsertedRawMempoolTransactionVersion {
                raw_version_id: current_head.raw_version_id,
                write_outcome: RawVersionWriteOutcome::ReusedCurrentHead,
            });
        }

        conn.execute(
                "INSERT INTO raw_mempool_transaction_versions
                 (id, source_connection_id, network, txid, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    raw_version_id.to_string(),
                    request.source_connection_id.to_string(),
                    request.network.as_str(),
                    request.txid.as_str(),
                    request.payload_hash_sha256_hex.as_str(),
                    request.payload_bytes.as_slice(),
                    request.first_observed_at.to_rfc3339(),
                    current_head.map(|head| head.raw_version_id.to_string()),
                    request.first_observed_at.to_rfc3339(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to insert raw mempool transaction version", err)
            })?;

        Ok(InsertedRawMempoolTransactionVersion {
            raw_version_id,
            write_outcome: RawVersionWriteOutcome::InsertedNewHead,
        })
    })
}

pub(crate) fn record_raw_mempool_tx_observation_tx(
    tx: &rusqlite::Transaction<'_>,
    request: &RecordRawMempoolTransactionObservationRequest,
) -> Result<(), DbError> {
    if request.page_item_index < 0 {
        return Err(DbError::new("page item index cannot be negative"));
    }

    let observation_id = RawMempoolTransactionObservationId::new();
    tx.execute(
        "INSERT INTO raw_mempool_transaction_observations
         (id, sync_run_id, source_connection_id, raw_observation_set_id, raw_mempool_transaction_version_id, page_item_index, observed_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            observation_id.to_string(),
            request.sync_run_id.to_string(),
            request.source_connection_id.to_string(),
            request.raw_observation_set_id.to_string(),
            request.raw_mempool_transaction_version_id.to_string(),
            request.page_item_index,
            request.observed_at.to_rfc3339(),
            request.observed_at.to_rfc3339(),
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("Failed to insert raw mempool transaction observation", err))?;
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn record_raw_mempool_tx_observation(
    user_id: UserId,
    request: RecordRawMempoolTransactionObservationRequest,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::from_rusqlite_error("Failed to start raw mempool observation transaction", err)
        })?;
        record_raw_mempool_tx_observation_tx(&tx, &request)?;
        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to commit raw mempool observation transaction",
                err,
            )
        })
    })
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn load_observed_raw_mempool_transactions_for_observation_set(
    user_id: UserId,
    raw_observation_set_id: RawObservationSetId,
) -> Result<Vec<ObservedRawMempoolTransactionForRequestRow>, DbError> {
    let result: Result<Vec<ObservedRawMempoolTransactionForRequestRow>, DbError> =
        with_user_db(user_id, |conn: &rusqlite::Connection| {
            let mut stmt = conn
                .prepare(
                    "SELECT v.id, v.txid, v.payload_bytes, o.page_item_index
                     FROM raw_mempool_transaction_observations o
                     INNER JOIN raw_mempool_transaction_versions v
                       ON v.id = o.raw_mempool_transaction_version_id
                     WHERE o.raw_observation_set_id = ?1
                     ORDER BY o.page_item_index ASC, o.created_at ASC",
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to prepare observed raw mempool transaction lookup",
                        err,
                    )
                })?;
            let rows = stmt
                .query_map([raw_observation_set_id.to_string()], |row| {
                    let raw_version_id_raw: String = row.get(0)?;
                    let txid_raw: String = row.get(1)?;
                    let payload_bytes_raw: Vec<u8> = row.get(2)?;
                    let page_item_index: i64 = row.get(3)?;

                    let raw_version_id = RawMempoolTransactionVersionId::from_str(
                        &raw_version_id_raw,
                    )
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?;
                    let txid = TxHash::parse(&txid_raw).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(DbError::new(format!("Invalid txid in DB: {err}"))),
                        )
                    })?;
                    let payload_bytes =
                        ExactPayloadBytes::try_new(payload_bytes_raw).map_err(|err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Blob,
                                Box::new(err),
                            )
                        })?;

                    Ok(ObservedRawMempoolTransactionForRequestRow {
                        raw_version_id,
                        txid,
                        payload_bytes,
                        page_item_index,
                    })
                })
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to query observed raw mempool transactions",
                        err,
                    )
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| {
                    DbError::from_rusqlite_error(
                        "Failed to read observed raw mempool transactions",
                        err,
                    )
                })?;
            Ok(rows)
        });
    result
}

use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::wallets::DigitalAssetAddressId;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ids::{
    RawMempoolTransactionVersionId, RawObservationSetId, SourceConnectionId, SyncRunId,
};
use super::mempool::{
    RecordRawMempoolTransactionObservationRequest, record_raw_mempool_tx_observation_tx,
};
use super::shared::MempoolPageKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawObservationSetGroupingKind {
    MempoolAddress,
    #[cfg(all(test, feature = "db-tests"))]
    EtherscanNormal,
    #[cfg(all(test, feature = "db-tests"))]
    EtherscanInternal,
}

impl RawObservationSetGroupingKind {
    fn as_db_value(self) -> &'static str {
        match self {
            Self::MempoolAddress => "mempool_address_transactions_page",
            #[cfg(all(test, feature = "db-tests"))]
            Self::EtherscanNormal => "etherscan_normal_transactions_page",
            #[cfg(all(test, feature = "db-tests"))]
            Self::EtherscanInternal => "etherscan_internal_transactions_page",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawObservationMetadataJson(String);

impl RawObservationMetadataJson {
    pub(crate) fn parse(raw: String) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new(
                "raw observation metadata json cannot be empty",
            ));
        }
        let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
            DbError::new(format!(
                "raw observation metadata json must be valid JSON: {err}"
            ))
        })?;
        if !parsed.is_object() {
            return Err(DbError::new(
                "raw observation metadata json must be a JSON object",
            ));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRawObservationSetRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) grouping_kind: RawObservationSetGroupingKind,
    pub(crate) grouping_metadata_json: RawObservationMetadataJson,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(all(test, feature = "db-tests"))]
pub(crate) struct RecordedRawObservationSet {
    pub(crate) raw_observation_set_id: RawObservationSetId,
}

fn insert_raw_observation_set_tx(
    tx: &rusqlite::Transaction<'_>,
    raw_observation_set_id: RawObservationSetId,
    request: &RecordRawObservationSetRequest,
) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO raw_observation_sets
         (id, sync_run_id, source_connection_id, grouping_kind, grouping_metadata_json, observed_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            raw_observation_set_id.to_string(),
            request.sync_run_id.to_string(),
            request.source_connection_id.to_string(),
            request.grouping_kind.as_db_value(),
            request.grouping_metadata_json.as_str(),
            request.observed_at.to_rfc3339(),
            request.observed_at.to_rfc3339(),
        ],
    )
    .map_err(|err| DbError::from_rusqlite_error("Failed to insert raw observation set", err))?;
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn record_raw_observation_set(
    user_id: UserId,
    request: RecordRawObservationSetRequest,
) -> Result<RecordedRawObservationSet, DbError> {
    let raw_observation_set_id = RawObservationSetId::new();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::from_rusqlite_error("Failed to start raw observation set transaction", err)
        })?;
        insert_raw_observation_set_tx(&tx, raw_observation_set_id, &request)?;
        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error("Failed to commit raw observation set transaction", err)
        })?;
        Ok(RecordedRawObservationSet {
            raw_observation_set_id,
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MempoolPageObservationMetadata {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) scan_start_run_id: Option<SyncRunId>,
    pub(crate) page_kind: MempoolPageKind,
    pub(crate) requested_cursor: Option<String>,
    pub(crate) returned_last_confirmed_cursor: Option<String>,
    pub(crate) item_count: u32,
}

pub(crate) struct RecordRawMempoolPageObservationRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
    pub(crate) metadata: MempoolPageObservationMetadata,
    pub(crate) raw_version_ids: Vec<RawMempoolTransactionVersionId>,
    pub(crate) observed_at: DateTime<Utc>,
}

pub(crate) fn record_raw_mempool_page_observation(
    user_id: UserId,
    request: RecordRawMempoolPageObservationRequest,
) -> Result<RawObservationSetId, DbError> {
    let grouping_metadata_json =
        RawObservationMetadataJson::parse(serde_json::to_string(&request.metadata).map_err(
            |err| DbError::new(format!("Failed to serialize mempool page metadata: {err}")),
        )?)?;
    let raw_observation_set_id = RawObservationSetId::new();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to start mempool page observation transaction",
                err,
            )
        })?;
        insert_raw_observation_set_tx(
            &tx,
            raw_observation_set_id,
            &RecordRawObservationSetRequest {
                sync_run_id: request.sync_run_id,
                source_connection_id: request.source_connection_id.clone(),
                grouping_kind: RawObservationSetGroupingKind::MempoolAddress,
                grouping_metadata_json,
                observed_at: request.observed_at,
            },
        )?;
        for (page_item_index, raw_version_id) in request.raw_version_ids.iter().enumerate() {
            let page_item_index = i64::try_from(page_item_index)
                .map_err(|_| DbError::new("mempool page item index out of range"))?;
            record_raw_mempool_tx_observation_tx(
                &tx,
                &RecordRawMempoolTransactionObservationRequest {
                    sync_run_id: request.sync_run_id,
                    source_connection_id: request.source_connection_id.clone(),
                    raw_observation_set_id,
                    raw_mempool_transaction_version_id: *raw_version_id,
                    page_item_index,
                    observed_at: request.observed_at,
                },
            )?;
        }
        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to commit mempool page observation transaction",
                err,
            )
        })?;
        Ok(raw_observation_set_id)
    })
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawMempoolPageObservationSetRow {
    pub(crate) raw_observation_set_id: RawObservationSetId,
    pub(crate) metadata: MempoolPageObservationMetadata,
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn load_raw_mempool_page_observations_for_sync_run(
    user_id: UserId,
    sync_run_id: SyncRunId,
) -> Result<Vec<RawMempoolPageObservationSetRow>, DbError> {
    crate::db::user_db::with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT id, grouping_metadata_json
                 FROM raw_observation_sets
                 WHERE sync_run_id = ?1
                   AND grouping_kind = 'mempool_address_transactions_page'
                 ORDER BY observed_at ASC, created_at ASC, id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare mempool page observation lookup",
                    err,
                )
            })?;
        statement
            .query_map([sync_run_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query mempool page observations", err)
            })?
            .map(|row| {
                let (raw_observation_set_id, metadata_json) = row.map_err(|err| {
                    DbError::from_rusqlite_error("Failed to read mempool page observation", err)
                })?;
                Ok(RawMempoolPageObservationSetRow {
                    raw_observation_set_id: raw_observation_set_id.parse().map_err(|err| {
                        DbError::new(format!("Invalid raw observation set id: {err}"))
                    })?,
                    metadata: serde_json::from_str(&metadata_json).map_err(|err| {
                        DbError::new(format!("Invalid mempool page observation metadata: {err}"))
                    })?,
                })
            })
            .collect()
    })
}

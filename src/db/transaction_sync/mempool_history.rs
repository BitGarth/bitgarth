use super::address_loading::load_confirmed_bitcoin_tx_hashes_for_address_conn;
use crate::db::error::DbError;
use crate::db::raw_ingestion::{
    MempoolPageKind, MempoolPageObservationMetadata, RawObservationSetId, SyncRunId,
};
use crate::db::user_db::with_user_db;
use crate::integrations::mempool::MempoolAddressTransaction;
use crate::models::UserId;
use crate::transactions::{ChainTipHeight, SyncErrorMessage, TransactionCount, TxHash};
use crate::wallets::DigitalAssetAddressId;
use chrono::Utc;
use rusqlite::params;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StrictMempoolScanValidation {
    Exact,
    Restart { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitcoinAccountHistoryCoverage {
    Unscanned,
    Syncing,
    Limited,
    Complete { coverage_height: ChainTipHeight },
}

struct PageObservation {
    metadata: MempoolPageObservationMetadata,
    requested_cursor: Option<TxHash>,
    returned_cursor: Option<TxHash>,
    confirmed_txids: Vec<TxHash>,
    membership_count: usize,
}

fn restart(reason: &str) -> StrictMempoolScanValidation {
    StrictMempoolScanValidation::Restart {
        reason: SyncErrorMessage::sanitize(reason).as_str().to_string(),
    }
}

fn parse_cursor(raw: Option<&str>) -> Result<Option<TxHash>, StrictMempoolScanValidation> {
    raw.map(|value| {
        TxHash::parse(value).map_err(|_| restart("Mempool scan evidence has an invalid cursor"))
    })
    .transpose()
}

fn load_page_observation(
    conn: &rusqlite::Connection,
    id_raw: String,
    metadata_raw: String,
    address_id: DigitalAssetAddressId,
    start_run_id: SyncRunId,
) -> Result<Result<PageObservation, StrictMempoolScanValidation>, DbError> {
    let id = match RawObservationSetId::from_str(&id_raw) {
        Ok(id) => id,
        Err(_) => return Ok(Err(restart("Mempool scan evidence has an invalid page id"))),
    };
    let metadata: MempoolPageObservationMetadata = match serde_json::from_str(&metadata_raw) {
        Ok(metadata) => metadata,
        Err(_) => {
            return Ok(Err(restart(
                "Mempool scan evidence has invalid page metadata",
            )));
        }
    };
    if metadata.address_id != address_id || metadata.scan_start_run_id != Some(start_run_id) {
        return Ok(Err(restart(
            "Mempool scan evidence does not match the requested scan",
        )));
    }
    let requested_cursor = match parse_cursor(metadata.requested_cursor.as_deref()) {
        Ok(cursor) => cursor,
        Err(validation) => return Ok(Err(validation)),
    };
    let returned_cursor = match parse_cursor(metadata.returned_last_confirmed_cursor.as_deref()) {
        Ok(cursor) => cursor,
        Err(validation) => return Ok(Err(validation)),
    };

    let mut statement = conn
        .prepare(
            "SELECT v.txid, v.payload_bytes, o.page_item_index
             FROM raw_mempool_transaction_observations o
             JOIN raw_mempool_transaction_versions v
               ON v.id = o.raw_mempool_transaction_version_id
             WHERE o.raw_observation_set_id = ?1
             ORDER BY o.page_item_index ASC",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to prepare mempool page membership lookup", err)
        })?;
    let rows = statement
        .query_map([id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to query mempool page memberships", err)
        })?;

    let mut confirmed_txids = Vec::new();
    let mut last_confirmed_txid = None;
    let mut membership_count = 0_usize;
    for row in rows {
        let (txid_raw, payload, page_item_index) = row.map_err(|err| {
            DbError::from_rusqlite_error("Failed to read mempool page membership", err)
        })?;
        if page_item_index != i64::try_from(membership_count).unwrap_or(i64::MAX) {
            return Ok(Err(restart(
                "Mempool scan evidence has non-contiguous page membership",
            )));
        }
        let txid = match TxHash::parse(&txid_raw) {
            Ok(txid) => txid,
            Err(_) => {
                return Ok(Err(restart(
                    "Mempool scan evidence has an invalid transaction id",
                )));
            }
        };
        let transaction: MempoolAddressTransaction = match serde_json::from_slice(&payload) {
            Ok(transaction) => transaction,
            Err(_) => {
                return Ok(Err(restart(
                    "Mempool scan evidence has an invalid transaction payload",
                )));
            }
        };
        let payload_txid = match TxHash::parse(&transaction.txid) {
            Ok(payload_txid) => payload_txid,
            Err(_) => {
                return Ok(Err(restart(
                    "Mempool scan evidence payload has an invalid transaction id",
                )));
            }
        };
        if payload_txid != txid {
            return Ok(Err(restart(
                "Mempool scan evidence transaction ids do not match",
            )));
        }
        if transaction.status.confirmed {
            last_confirmed_txid = Some(txid.clone());
            confirmed_txids.push(txid);
        }
        membership_count += 1;
    }

    if usize::try_from(metadata.item_count).ok() != Some(membership_count) {
        return Ok(Err(restart(
            "Mempool scan evidence page count does not match its memberships",
        )));
    }
    if returned_cursor != last_confirmed_txid {
        return Ok(Err(restart(
            "Mempool scan evidence cursor does not match the last confirmed transaction",
        )));
    }

    Ok(Ok(PageObservation {
        metadata,
        requested_cursor,
        returned_cursor,
        confirmed_txids,
        membership_count,
    }))
}

pub(crate) fn validate_strict_mempool_history_scan(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    start_run_id: SyncRunId,
    expected_confirmed_count: TransactionCount,
) -> Result<StrictMempoolScanValidation, DbError> {
    with_user_db(user_id, |conn| {
        let start_run_exists = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_runs WHERE id = ?1)",
                [start_run_id.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to check mempool scan start run", err)
            })?;
        if !start_run_exists {
            return Ok(restart("Mempool scan start evidence no longer exists"));
        }

        let mut statement = conn
            .prepare(
                "SELECT id, grouping_metadata_json
                 FROM raw_observation_sets
                 WHERE grouping_kind = 'mempool_address_transactions_page'
                   AND json_extract(grouping_metadata_json, '$.address_id') = ?1
                   AND json_extract(grouping_metadata_json, '$.scan_start_run_id') = ?2
                 ORDER BY observed_at ASC, created_at ASC, id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare strict mempool scan page lookup",
                    err,
                )
            })?;
        let rows = statement
            .query_map(
                params![address_id.to_string(), start_run_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query strict mempool scan pages", err)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to read strict mempool scan page", err)
            })?;

        let mut pages = Vec::with_capacity(rows.len());
        for (id, metadata) in rows {
            match load_page_observation(conn, id, metadata, address_id, start_run_id)? {
                Ok(page) => pages.push(page),
                Err(validation) => return Ok(validation),
            }
        }

        let first_pages = pages
            .iter()
            .enumerate()
            .filter(|(_, page)| {
                page.metadata.page_kind == MempoolPageKind::FirstPage
                    && page.requested_cursor.is_none()
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if first_pages.len() != 1 {
            return Ok(restart(
                "Mempool scan evidence must contain exactly one first page",
            ));
        }

        let mut successors = HashMap::new();
        for (index, page) in pages.iter().enumerate() {
            if index == first_pages[0] {
                continue;
            }
            if page.metadata.page_kind != MempoolPageKind::PaginatedAfterConfirmed {
                return Ok(restart(
                    "Mempool scan evidence contains an invalid page kind",
                ));
            }
            let Some(requested_cursor) = page.requested_cursor.as_ref() else {
                return Ok(restart(
                    "Mempool scan evidence contains a page without its requested cursor",
                ));
            };
            if successors
                .insert(requested_cursor.as_str().to_string(), index)
                .is_some()
            {
                return Ok(restart(
                    "Mempool scan evidence contains duplicate cursor successors",
                ));
            }
        }

        let mut observed_confirmed_txids = BTreeSet::new();
        let mut visited = HashSet::new();
        let mut current = first_pages[0];
        loop {
            if !visited.insert(current) {
                return Ok(restart("Mempool scan evidence contains a cursor cycle"));
            }
            let page = &pages[current];
            for txid in &page.confirmed_txids {
                observed_confirmed_txids.insert(txid.clone());
            }

            let Some(returned_cursor) = page.returned_cursor.as_ref() else {
                if page.metadata.item_count != 0 || page.membership_count != 0 {
                    return Ok(restart("Mempool scan evidence ended on a nonempty page"));
                }
                break;
            };
            if page.requested_cursor.as_ref() == Some(returned_cursor) {
                return Ok(restart(
                    "Mempool scan evidence contains a non-advancing cursor",
                ));
            }
            let Some(next) = successors.get(returned_cursor.as_str()) else {
                return Ok(restart("Mempool scan evidence has a missing cursor link"));
            };
            current = *next;
        }

        if visited.len() != pages.len() {
            return Ok(restart("Mempool scan evidence contains an unlinked page"));
        }
        if observed_confirmed_txids.len()
            != usize::try_from(expected_confirmed_count.value()).unwrap_or(usize::MAX)
        {
            return Ok(restart(
                "Mempool scan confirmed transaction count does not match the provider count",
            ));
        }

        let canonical_txids = load_confirmed_bitcoin_tx_hashes_for_address_conn(conn, address_id)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        if observed_confirmed_txids != canonical_txids {
            return Ok(restart(
                "Mempool scan transactions do not exactly match canonical history",
            ));
        }
        Ok(StrictMempoolScanValidation::Exact)
    })
}

pub(crate) fn restart_strict_mempool_history_scan(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> Result<(), DbError> {
    super::super::user_db::with_user_db_mut(user_id, |conn| {
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET mempool_history_complete_tx_count = NULL,
                     mempool_history_complete_height = NULL,
                     mempool_backfill_cursor_txid = NULL,
                     mempool_history_scan_start_run_id = NULL,
                     updated_at = ?1
                 WHERE scope = 'address' AND address_id = ?2",
                params![Utc::now().to_rfc3339(), address_id.to_string()],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to restart strict Mempool history scan: {err}"
                ))
            })?;
        if changed == 0 {
            return Err(DbError::new(
                "Failed to restart strict Mempool history scan: sync state row missing",
            ));
        }
        Ok(())
    })
}

use super::super::error::DbError;
use super::super::user_db::with_user_db;
use super::parsers::*;
use super::types::*;
use crate::models::UserId;
use crate::transactions::{TransactionCount, TxHash};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};

#[allow(clippy::too_many_arguments)]
pub(super) fn map_sync_address_row(
    id: String,
    account_id: Option<String>,
    address: String,
    asset_id: String,
    network: String,
    address_scheme: Option<String>,
    derivation_change: Option<i64>,
    derivation_index: Option<i64>,
    last_completed_at: Option<String>,
    last_result: Option<String>,
    last_tip_height: Option<i64>,
    mempool_backfill_cursor_txid: Option<String>,
    mempool_expected_tx_count: Option<i64>,
    mempool_history_complete_tx_count: Option<i64>,
    mempool_history_complete_height: Option<i64>,
    mempool_history_scan_start_run_id: Option<String>,
    etherscan_backfill_end_block: Option<i64>,
    etherscan_history_checkpoint_version: Option<i64>,
    has_api_confirmed_balance: bool,
    consecutive_failure_count: i64,
) -> Result<SyncAddress, DbError> {
    let address_scheme = address_scheme
        .as_deref()
        .map(parse_address_scheme)
        .transpose()?;
    let account_id = account_id.as_deref().map(parse_account_id).transpose()?;
    let last_completed_at = parse_optional_time(last_completed_at, "last_completed_at")?;
    let last_result = parse_optional_sync_result(last_result)?;

    Ok(SyncAddress {
        address_id: parse_address_id(&id)?,
        account_id,
        address: parse_tracked_address(&address)?,
        asset_id: parse_asset_id(&asset_id)?,
        network: parse_network(&network)?,
        derivation_change: parse_optional_u32(derivation_change, "derivation_change")?,
        derivation_index: parse_optional_u32(derivation_index, "derivation_index")?,
        address_scheme,
        last_completed_at,
        last_result,
        last_tip_height: parse_optional_tip_height(last_tip_height)?,
        mempool_backfill_cursor_txid: parse_optional_mempool_cursor_txid(
            mempool_backfill_cursor_txid,
        )?,
        mempool_expected_tx_count: parse_optional_transaction_count(
            mempool_expected_tx_count,
            "mempool_expected_tx_count",
        )?,
        mempool_history_proof: parse_optional_mempool_history_proof(
            mempool_history_complete_tx_count,
            mempool_history_complete_height,
        )?,
        mempool_history_scan_start_run_id: parse_optional_sync_run_id(
            mempool_history_scan_start_run_id,
            "mempool_history_scan_start_run_id",
        )?,
        etherscan_backfill_end_block: parse_optional_ethereum_block_number(
            etherscan_backfill_end_block,
        )?,
        etherscan_history_checkpoint_verified: parse_etherscan_history_checkpoint_verified(
            etherscan_history_checkpoint_version,
        )?,
        has_api_confirmed_balance,
        consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::try_new(
            consecutive_failure_count,
        )
        .map_err(|err| DbError::new(format!("Invalid consecutive_failure_count in DB: {err}")))?,
    })
}

pub(crate) fn get_non_hd_sync_addresses(user_id: UserId) -> Result<Vec<SyncAddress>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    da.id,
                    da.account_id,
                    da.address,
                    da.asset_id,
                    da.network,
                    da.address_scheme,
                    da.derivation_change,
                    da.derivation_index,
                    t.last_completed_at,
                    t.last_result,
                    t.last_tip_height,
                    t.mempool_backfill_cursor_txid,
                    t.mempool_expected_tx_count,
                    t.mempool_history_complete_tx_count,
                    t.mempool_history_complete_height,
                    t.mempool_history_scan_start_run_id,
                    t.etherscan_backfill_end_block,
                    t.etherscan_history_checkpoint_version,
                    t.api_confirmed_balance_hi IS NOT NULL
                        AND t.api_confirmed_balance_lo IS NOT NULL,
                    COALESCE(t.consecutive_failure_count, 0)
                 FROM digital_asset_addresses da
                 JOIN source_connections sc
                   ON sc.current_digital_asset_address_id = da.id
                  AND sc.status = 'active'
                  AND sc.network = da.network
                  AND (
                      (da.asset_id = 'bitcoin' AND sc.integration = 'mempool')
                      OR
                      (da.asset_id = 'ethereum' AND sc.integration = 'etherscan')
                  )
                 LEFT JOIN digital_asset_accounts a ON a.id = da.account_id
                 LEFT JOIN transaction_sync_state t ON t.scope = ?1 AND t.address_id = da.id
                 WHERE a.account_kind IS NULL OR a.account_kind != 'hd_pubkey'
                 ORDER BY da.created_at ASC, da.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!("Failed to prepare sync addresses query: {err}"))
            })?;

        let rows = stmt
            .query_map(params![super::ADDRESS_SYNC_SCOPE], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, Option<i64>>(17)?,
                    row.get::<_, bool>(18)?,
                    row.get::<_, i64>(19)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!("Failed to execute sync addresses query: {err}"))
            })?;

        let mut addresses = Vec::new();
        for row_result in rows {
            let (
                id,
                account_id,
                address,
                asset_id,
                network,
                address_scheme,
                derivation_change,
                derivation_index,
                last_completed_at,
                last_result,
                last_tip_height,
                mempool_backfill_cursor_txid,
                mempool_expected_tx_count,
                mempool_history_complete_tx_count,
                mempool_history_complete_height,
                mempool_history_scan_start_run_id,
                etherscan_backfill_end_block,
                etherscan_history_checkpoint_version,
                has_api_confirmed_balance,
                consecutive_failure_count,
            ) = row_result
                .map_err(|err| DbError::new(format!("Failed to map sync address row: {err}")))?;
            addresses.push(map_sync_address_row(
                id,
                account_id,
                address,
                asset_id,
                network,
                address_scheme,
                derivation_change,
                derivation_index,
                last_completed_at,
                last_result,
                last_tip_height,
                mempool_backfill_cursor_txid,
                mempool_expected_tx_count,
                mempool_history_complete_tx_count,
                mempool_history_complete_height,
                mempool_history_scan_start_run_id,
                etherscan_backfill_end_block,
                etherscan_history_checkpoint_version,
                has_api_confirmed_balance,
                consecutive_failure_count,
            )?);
        }

        Ok(addresses)
    })
}

pub(crate) fn get_sync_addresses_for_account(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<Vec<SyncAddress>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT
                    da.id,
                    da.account_id,
                    da.address,
                    da.asset_id,
                    da.network,
                    da.address_scheme,
                    da.derivation_change,
                    da.derivation_index,
                    t.last_completed_at,
                    t.last_result,
                    t.last_tip_height,
                    t.mempool_backfill_cursor_txid,
                    t.mempool_expected_tx_count,
                    t.mempool_history_complete_tx_count,
                    t.mempool_history_complete_height,
                    t.mempool_history_scan_start_run_id,
                    t.etherscan_backfill_end_block,
                    t.etherscan_history_checkpoint_version,
                    t.api_confirmed_balance_hi IS NOT NULL
                        AND t.api_confirmed_balance_lo IS NOT NULL,
                    COALESCE(t.consecutive_failure_count, 0)
                 FROM digital_asset_addresses da
                 JOIN source_connections sc
                   ON sc.current_digital_asset_address_id = da.id
                  AND sc.status = 'active'
                  AND sc.network = da.network
                  AND (
                      (da.asset_id = 'bitcoin' AND sc.integration = 'mempool')
                      OR
                      (da.asset_id = 'ethereum' AND sc.integration = 'etherscan')
                  )
                 LEFT JOIN transaction_sync_state t ON t.scope = ?1 AND t.address_id = da.id
                 WHERE da.account_id = ?2
                 ORDER BY da.created_at ASC, da.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare account sync addresses query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map(
                params![super::ADDRESS_SYNC_SCOPE, account_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<i64>>(12)?,
                        row.get::<_, Option<i64>>(13)?,
                        row.get::<_, Option<i64>>(14)?,
                        row.get::<_, Option<String>>(15)?,
                        row.get::<_, Option<i64>>(16)?,
                        row.get::<_, Option<i64>>(17)?,
                        row.get::<_, bool>(18)?,
                        row.get::<_, i64>(19)?,
                    ))
                },
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute account sync addresses query: {err}"
                ))
            })?;

        let mut addresses = Vec::new();
        for row_result in rows {
            let (
                id,
                account_id,
                address,
                asset_id,
                network,
                address_scheme,
                derivation_change,
                derivation_index,
                last_completed_at,
                last_result,
                last_tip_height,
                mempool_backfill_cursor_txid,
                mempool_expected_tx_count,
                mempool_history_complete_tx_count,
                mempool_history_complete_height,
                mempool_history_scan_start_run_id,
                etherscan_backfill_end_block,
                etherscan_history_checkpoint_version,
                has_api_confirmed_balance,
                consecutive_failure_count,
            ) = row_result.map_err(|err| {
                DbError::new(format!("Failed to map account sync address row: {err}"))
            })?;
            addresses.push(map_sync_address_row(
                id,
                account_id,
                address,
                asset_id,
                network,
                address_scheme,
                derivation_change,
                derivation_index,
                last_completed_at,
                last_result,
                last_tip_height,
                mempool_backfill_cursor_txid,
                mempool_expected_tx_count,
                mempool_history_complete_tx_count,
                mempool_history_complete_height,
                mempool_history_scan_start_run_id,
                etherscan_backfill_end_block,
                etherscan_history_checkpoint_version,
                has_api_confirmed_balance,
                consecutive_failure_count,
            )?);
        }

        Ok(addresses)
    })
}

pub(crate) fn load_account_labels(
    user_id: UserId,
) -> Result<HashMap<DigitalAssetAccountId, String>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare("SELECT id, label FROM digital_asset_accounts")
            .map_err(|err| {
                DbError::new(format!("Failed to prepare account labels query: {err}"))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|err| {
                DbError::new(format!("Failed to execute account labels query: {err}"))
            })?;

        let mut labels = HashMap::new();
        for row_result in rows {
            let (account_id_raw, label) = row_result
                .map_err(|err| DbError::new(format!("Failed to map account label row: {err}")))?;
            let account_id = parse_account_id(&account_id_raw)?;
            labels.insert(account_id, label);
        }
        Ok(labels)
    })
}

pub(super) fn load_account_sync_state_row(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<AccountSyncStateRow>, DbError> {
    let row = conn
        .query_row(
            "SELECT
                account_id,
                last_scanned_time,
                gap_limit,
                last_derived_external_index,
                last_derived_internal_index,
                mempool_history_next_address_id
             FROM account_sync_state
             WHERE account_id = ?1",
            params![account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load account_sync_state row: {err}")))?;

    let Some((
        account_id_raw,
        last_scanned_time_raw,
        gap_limit_raw,
        last_derived_external_raw,
        last_derived_internal_raw,
        mempool_history_next_address_id_raw,
    )) = row
    else {
        return Ok(None);
    };

    if gap_limit_raw < 0 {
        return Err(DbError::new("Invalid gap_limit in account_sync_state"));
    }
    let gap_limit = u32::try_from(gap_limit_raw)
        .map_err(|_| DbError::new("gap_limit out of u32 range in account_sync_state"))?;

    Ok(Some(AccountSyncStateRow {
        account_id: parse_account_id(&account_id_raw)?,
        last_scanned_time: parse_optional_time(last_scanned_time_raw, "last_scanned_time")?,
        gap_limit,
        last_derived_external_index: parse_optional_u32(
            last_derived_external_raw,
            "last_derived_external_index",
        )?,
        last_derived_internal_index: parse_optional_u32(
            last_derived_internal_raw,
            "last_derived_internal_index",
        )?,
        mempool_history_next_address_id: mempool_history_next_address_id_raw
            .as_deref()
            .map(parse_address_id)
            .transpose()?,
    }))
}

pub(crate) fn load_address_ids_with_activity(
    user_id: UserId,
) -> Result<HashSet<DigitalAssetAddressId>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT address_id FROM transaction_inputs WHERE address_id IS NOT NULL
                 UNION
                 SELECT address_id FROM transaction_outputs WHERE address_id IS NOT NULL
                 UNION
                 SELECT from_address_id AS address_id FROM account_transfers WHERE from_address_id IS NOT NULL
                 UNION
                 SELECT to_address_id AS address_id FROM account_transfers WHERE to_address_id IS NOT NULL",
            )
            .map_err(|err| DbError::new(format!("Failed to prepare address activity query: {err}")))?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!("Failed to execute address activity query: {err}"))
            })?;

        let mut result = HashSet::new();
        for row in rows {
            let id_raw =
                row.map_err(|err| DbError::new(format!("Failed to map activity row: {err}")))?;
            result.insert(parse_address_id(&id_raw)?);
        }
        Ok(result)
    })
}

pub(crate) fn load_address_ids_with_pending_txs(
    user_id: UserId,
) -> Result<HashSet<DigitalAssetAddressId>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT address_id
                 FROM (
                    SELECT o.address_id AS address_id
                    FROM chain_transactions ct
                    JOIN transaction_outputs o ON o.tx_id = ct.id
                    WHERE o.address_id IS NOT NULL
                      AND ct.status = 'pending'
                    UNION
                    SELECT i.address_id AS address_id
                    FROM chain_transactions ct
                    JOIN transaction_inputs i ON i.tx_id = ct.id
                    WHERE i.address_id IS NOT NULL
                      AND ct.status = 'pending'
                 )",
            )
            .map_err(|err| {
                DbError::new(format!("Failed to prepare pending address query: {err}"))
            })?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!("Failed to execute pending address query: {err}"))
            })?;

        let mut result = HashSet::new();
        for row in rows {
            let id_raw = row
                .map_err(|err| DbError::new(format!("Failed to map pending address row: {err}")))?;
            result.insert(parse_address_id(&id_raw)?);
        }

        Ok(result)
    })
}

pub(crate) fn load_account_ids_with_pending_txs(
    user_id: UserId,
) -> Result<HashSet<DigitalAssetAccountId>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT da.account_id
                 FROM digital_asset_addresses da
                 JOIN (
                    SELECT o.address_id AS address_id
                    FROM chain_transactions ct
                    JOIN transaction_outputs o ON o.tx_id = ct.id
                    WHERE o.address_id IS NOT NULL
                      AND ct.status = 'pending'
                    UNION
                    SELECT i.address_id AS address_id
                    FROM chain_transactions ct
                    JOIN transaction_inputs i ON i.tx_id = ct.id
                    WHERE i.address_id IS NOT NULL
                      AND ct.status = 'pending'
                 ) pending ON pending.address_id = da.id",
            )
            .map_err(|err| {
                DbError::new(format!("Failed to prepare pending account query: {err}"))
            })?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!("Failed to execute pending account query: {err}"))
            })?;

        let mut result = HashSet::new();
        for row in rows {
            let id_raw = row
                .map_err(|err| DbError::new(format!("Failed to map pending account row: {err}")))?;
            result.insert(parse_account_id(&id_raw)?);
        }

        Ok(result)
    })
}

pub(crate) fn load_api_confirmed_balances_for_account_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Vec<AddressApiConfirmedBalanceRow>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT da.id,
                    tss.last_completed_at,
                    tss.api_confirmed_balance_hi,
                    tss.api_confirmed_balance_lo
             FROM digital_asset_addresses da
             LEFT JOIN transaction_sync_state tss
               ON tss.address_id = da.id
              AND tss.scope = ?2
             WHERE da.account_id = ?1
             ORDER BY da.id ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare api confirmed balances query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map(
            params![account_id.to_string(), super::ADDRESS_SYNC_SCOPE],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute api confirmed balances query: {err}"
            ))
        })?;

    let mut result = Vec::new();
    for row in rows {
        let (address_id_raw, last_completed_at_raw, balance_hi, balance_lo) =
            row.map_err(|err| {
                DbError::new(format!("Failed to map api confirmed balance row: {err}"))
            })?;
        let address_id = parse_address_id(&address_id_raw)?;
        let last_completed_at = parse_optional_time(last_completed_at_raw, "last_completed_at")?;
        let api_confirmed_balance = match (balance_hi, balance_lo) {
            (Some(hi), Some(lo)) => parse_optional_api_confirmed_balance(Some(hi), Some(lo))?,
            _ => None,
        };
        result.push(AddressApiConfirmedBalanceRow {
            address_id,
            last_completed_at,
            api_confirmed_balance,
        });
    }

    Ok(result)
}

pub(crate) fn load_account_mempool_expected_tx_count(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<Option<TransactionCount>, DbError> {
    with_user_db(user_id, |conn| {
        load_account_mempool_expected_tx_count_with_conn(conn, account_id)
    })
}

pub(crate) fn load_account_reported_tx_counts(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<Vec<TransactionCount>, DbError> {
    with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT tss.reported_tx_count
                 FROM transaction_sync_state tss
                 JOIN digital_asset_addresses da ON da.id = tss.address_id
                 WHERE da.account_id = ?1
                   AND tss.scope = ?2
                   AND tss.reported_tx_count IS NOT NULL",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare account reported transaction counts query: {err}"
                ))
            })?;
        let rows = statement
            .query_map(
                params![account_id.to_string(), super::ADDRESS_SYNC_SCOPE],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to query account reported transaction counts: {err}"
                ))
            })?;
        rows.map(|row| {
            let count = row.map_err(|err| {
                DbError::new(format!(
                    "Failed to map account reported transaction count: {err}"
                ))
            })?;
            parse_transaction_count(count, "reported_tx_count")
        })
        .collect()
    })
}

pub(crate) fn load_account_mempool_expected_tx_count_with_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<TransactionCount>, DbError> {
    let raw = conn
        .query_row(
            "SELECT SUM(tss.mempool_expected_tx_count)
             FROM transaction_sync_state tss
             JOIN digital_asset_addresses da ON da.id = tss.address_id
             WHERE da.account_id = ?1
               AND tss.scope = ?2
               AND tss.mempool_expected_tx_count IS NOT NULL",
            params![account_id.to_string(), super::ADDRESS_SYNC_SCOPE],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load account mempool expected transaction count: {err}"
            ))
        })?;

    parse_optional_transaction_count(raw, "account mempool_expected_tx_count")
}

pub(crate) fn account_has_incomplete_mempool_history_with_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM transaction_sync_state tss
             JOIN digital_asset_addresses da ON da.id = tss.address_id
             WHERE da.account_id = ?1
               AND tss.scope = ?2
               AND tss.mempool_expected_tx_count IS NOT NULL
               AND tss.mempool_expected_tx_count > (
                   SELECT COUNT(*)
                   FROM (
                       SELECT ct.id
                       FROM chain_transactions ct
                       JOIN transaction_inputs i ON i.tx_id = ct.id
                       WHERE ct.status = 'confirmed' AND i.address_id = da.id
                       UNION
                       SELECT ct.id
                       FROM chain_transactions ct
                       JOIN transaction_outputs o ON o.tx_id = ct.id
                       WHERE ct.status = 'confirmed' AND o.address_id = da.id
                     )
               )
         )",
        params![account_id.to_string(), super::ADDRESS_SYNC_SCOPE],
        |row| row.get(0),
    )
    .map_err(|err| {
        DbError::new(format!(
            "Failed to check account mempool transaction history completeness: {err}"
        ))
    })
}

pub(crate) fn load_canonical_confirmed_account_transaction_count(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<TransactionCount, DbError> {
    with_user_db(user_id, |conn| {
        let count = conn
            .query_row(
                "SELECT COUNT(DISTINCT linked.tx_id)
                 FROM (
                    SELECT i.tx_id AS tx_id
                    FROM transaction_inputs i
                    JOIN digital_asset_addresses da ON da.id = i.address_id
                    WHERE da.account_id = ?1
                    UNION
                    SELECT o.tx_id AS tx_id
                    FROM transaction_outputs o
                    JOIN digital_asset_addresses da ON da.id = o.address_id
                    WHERE da.account_id = ?1
                    UNION
                    SELECT at.chain_transaction_id AS tx_id
                    FROM account_transfers at
                    JOIN digital_asset_addresses da
                      ON da.id = at.from_address_id
                      OR da.id = at.to_address_id
                    WHERE da.account_id = ?1
                 ) linked
                 JOIN chain_transactions ct ON ct.id = linked.tx_id
                 WHERE ct.status = 'confirmed'",
                params![account_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to load canonical confirmed account transaction count: {err}"
                ))
            })?;

        parse_transaction_count(count, "canonical confirmed account transaction count")
    })
}

pub(crate) fn load_canonical_account_transaction_count_bounded(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    stop_at: TransactionCount,
) -> Result<TransactionCount, DbError> {
    with_user_db(user_id, |conn| {
        load_canonical_account_transaction_count_bounded_conn(conn, account_id, stop_at)
    })
}

pub(in crate::db) fn load_canonical_account_transaction_count_bounded_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    stop_at: TransactionCount,
) -> Result<TransactionCount, DbError> {
    let count = conn
        .query_row(
            "SELECT COUNT(*)
                 FROM (
                    SELECT DISTINCT tx_id
                    FROM (
                        SELECT i.tx_id AS tx_id
                        FROM transaction_inputs i
                        JOIN digital_asset_addresses da ON da.id = i.address_id
                        WHERE da.account_id = ?1
                        UNION
                        SELECT o.tx_id AS tx_id
                        FROM transaction_outputs o
                        JOIN digital_asset_addresses da ON da.id = o.address_id
                        WHERE da.account_id = ?1
                        UNION
                        SELECT at.chain_transaction_id AS tx_id
                        FROM account_transfers at
                        JOIN digital_asset_addresses da
                          ON da.id = at.from_address_id
                          OR da.id = at.to_address_id
                        WHERE da.account_id = ?1
                    )
                    LIMIT ?2
                 )",
            params![account_id.to_string(), stop_at.value()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load bounded canonical account transaction count: {err}"
            ))
        })?;

    parse_transaction_count(count, "bounded canonical account transaction count")
}

fn load_tx_hashes_for_address_with_status(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
    status: Option<&str>,
) -> Result<HashSet<TxHash>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT ct.tx_hash
                 FROM chain_transactions ct
                 JOIN transaction_outputs o ON o.tx_id = ct.id
                 WHERE o.address_id = ?1
                   AND ct.asset_id = ?2
                   AND ct.network = ?3
                   AND (?4 IS NULL OR ct.status = ?4)
                 UNION
                 SELECT ct.tx_hash
                 FROM chain_transactions ct
                 JOIN transaction_inputs i ON i.tx_id = ct.id
                 WHERE i.address_id = ?1
                   AND ct.asset_id = ?2
                   AND ct.network = ?3
                   AND (?4 IS NULL OR ct.status = ?4)
                 UNION
                 SELECT ct.tx_hash
                 FROM chain_transactions ct
                 JOIN account_transfers t ON t.chain_transaction_id = ct.id
                 WHERE (t.from_address_id = ?1 OR t.to_address_id = ?1)
                   AND ct.asset_id = ?2
                   AND ct.network = ?3
                   AND (?4 IS NULL OR ct.status = ?4)",
            )
            .map_err(|err| DbError::new(format!("Failed to prepare known tx hash query: {err}")))?;

        let rows = stmt
            .query_map(
                params![
                    address_id.to_string(),
                    asset_id.as_str(),
                    network.as_str(),
                    status,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| DbError::new(format!("Failed to execute known tx hash query: {err}")))?;

        let mut hashes = HashSet::new();
        for row in rows {
            let tx_hash_raw = row
                .map_err(|err| DbError::new(format!("Failed to read known tx hash row: {err}")))?;
            let tx_hash = TxHash::parse(&tx_hash_raw)
                .map_err(|err| DbError::new(format!("Invalid tx hash in DB: {err}")))?;
            hashes.insert(tx_hash);
        }
        Ok(hashes)
    })
}

pub(crate) fn load_known_tx_hashes_for_address(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<HashSet<TxHash>, DbError> {
    load_tx_hashes_for_address_with_status(user_id, address_id, asset_id, network, None)
}

pub(crate) fn load_confirmed_tx_hashes_for_address(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<HashSet<TxHash>, DbError> {
    load_tx_hashes_for_address_with_status(
        user_id,
        address_id,
        asset_id,
        network,
        Some("confirmed"),
    )
}

pub(super) fn load_confirmed_bitcoin_tx_hashes_for_address_conn(
    conn: &rusqlite::Connection,
    address_id: DigitalAssetAddressId,
) -> Result<HashSet<TxHash>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT ct.tx_hash
             FROM chain_transactions ct
             JOIN transaction_outputs o ON o.tx_id = ct.id
             WHERE o.address_id = ?1
               AND ct.asset_id = 'bitcoin'
               AND ct.status = 'confirmed'
             UNION
             SELECT ct.tx_hash
             FROM chain_transactions ct
             JOIN transaction_inputs i ON i.tx_id = ct.id
             WHERE i.address_id = ?1
               AND ct.asset_id = 'bitcoin'
               AND ct.status = 'confirmed'",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare confirmed bitcoin tx hash query: {err}"
            ))
        })?;
    let rows = stmt
        .query_map([address_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute confirmed bitcoin tx hash query: {err}"
            ))
        })?;
    let mut hashes = HashSet::new();
    for row in rows {
        let raw = row.map_err(|err| {
            DbError::new(format!(
                "Failed to read confirmed bitcoin tx hash row: {err}"
            ))
        })?;
        hashes.insert(
            TxHash::parse(&raw)
                .map_err(|err| DbError::new(format!("Invalid bitcoin tx hash in DB: {err}")))?,
        );
    }
    Ok(hashes)
}

pub(crate) fn address_has_pending_txs(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<bool, DbError> {
    with_user_db(user_id, |conn| {
        let has_pending = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM chain_transactions ct
                     JOIN transaction_outputs o ON o.tx_id = ct.id
                     WHERE o.address_id = ?1
                       AND ct.asset_id = ?2
                       AND ct.network = ?3
                       AND ct.status = 'pending'
                 ) OR EXISTS(
                     SELECT 1
                     FROM chain_transactions ct
                     JOIN transaction_inputs i ON i.tx_id = ct.id
                     WHERE i.address_id = ?1
                       AND ct.asset_id = ?2
                       AND ct.network = ?3
                       AND ct.status = 'pending'
                 )",
                params![address_id.to_string(), asset_id.as_str(), network.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| DbError::new(format!("Failed to check pending tx state: {err}")))?;
        Ok(has_pending != 0)
    })
}

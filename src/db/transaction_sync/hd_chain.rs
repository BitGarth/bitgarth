use super::super::error::DbError;
use super::super::user_db::{with_user_db, with_user_db_mut};
use super::address_loading::{load_account_sync_state_row, map_sync_address_row};
use super::parsers::*;
use super::types::*;
use crate::models::{UserId, parse_datetime};
use crate::transactions::ChainTipHeight;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use ulid::Ulid;

pub(crate) fn load_hd_account_chain_sync_state(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    derivation_change: u32,
) -> Result<Option<HdAccountChainSyncStateRow>, DbError> {
    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT
                    account_id,
                    derivation_change,
                    frontier_phase,
                    next_index_to_scan,
                    consecutive_unused,
                    active_rescan_from_index,
                    updated_at
                 FROM hd_account_chain_sync_state
                 WHERE account_id = ?1
                   AND derivation_change = ?2",
                params![account_id.to_string(), i64::from(derivation_change)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to load hd_account_chain_sync_state row: {err}"
                ))
            })?;

        let Some((
            account_id_raw,
            derivation_change_raw,
            frontier_phase_raw,
            next_index_raw,
            consecutive_unused_raw,
            active_rescan_from_index_raw,
            updated_at_raw,
        )) = row
        else {
            return Ok(None);
        };

        let frontier_phase = parse_hd_account_chain_frontier_phase(&frontier_phase_raw)?;
        let next_index_to_scan = parse_required_u32(next_index_raw, "next_index_to_scan")?;
        let consecutive_unused = parse_required_u32(consecutive_unused_raw, "consecutive_unused")?;
        let active_rescan_from_index =
            parse_optional_u32(active_rescan_from_index_raw, "active_rescan_from_index")?;
        let frontier_state = match frontier_phase {
            HdAccountChainFrontierPhase::ExistingAddresses => {
                if active_rescan_from_index.is_some() {
                    return Err(DbError::new(
                        "active_rescan_from_index must be NULL for existing_addresses frontier state",
                    ));
                }
                HdAccountChainSyncState::ExistingAddresses {
                    next_index_to_scan,
                    consecutive_unused,
                }
            }
            HdAccountChainFrontierPhase::DerivedAddresses => {
                if active_rescan_from_index.is_some() {
                    return Err(DbError::new(
                        "active_rescan_from_index must be NULL for derived_addresses frontier state",
                    ));
                }
                HdAccountChainSyncState::DerivedAddresses {
                    next_index_to_scan,
                    consecutive_unused,
                }
            }
            HdAccountChainFrontierPhase::ActiveRescan => {
                let active_rescan_from_index = active_rescan_from_index.ok_or_else(|| {
                    DbError::new(
                        "active_rescan_from_index is required for active_rescan frontier state",
                    )
                })?;
                HdAccountChainSyncState::ActiveRescan {
                    next_index_to_scan,
                    consecutive_unused,
                    active_rescan_from_index,
                }
            }
        };

        Ok(Some(HdAccountChainSyncStateRow {
            account_id: parse_account_id(&account_id_raw)?,
            derivation_change: parse_required_u32(derivation_change_raw, "derivation_change")?,
            frontier_state,
            updated_at: parse_datetime(&updated_at_raw)
                .map_err(|err| DbError::new(format!("Invalid updated_at in DB: {err}")))?,
        }))
    })
}

pub(crate) fn get_hd_account_sync_bundles(
    user_id: UserId,
) -> Result<Vec<AccountSyncBundle>, DbError> {
    with_user_db(user_id, |conn| {
        let mut bundles = Vec::new();
        let mut account_stmt = conn
            .prepare(
                "SELECT
                    a.id,
                    a.asset_id,
                    a.network,
                    k.address_scheme,
                    k.extended_pubkey
                 FROM digital_asset_accounts a
                 JOIN digital_asset_account_hd_keys k ON k.account_id = a.id
                 WHERE a.account_kind = 'hd_pubkey'
                   AND k.key_role = 'primary'
                 ORDER BY a.created_at ASC, a.id ASC, k.created_at ASC, k.id ASC",
            )
            .map_err(|err| DbError::new(format!("Failed to prepare HD bundle query: {err}")))?;

        let account_rows = account_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|err| DbError::new(format!("Failed to execute HD bundle query: {err}")))?;

        for row_result in account_rows {
            let (account_id_raw, asset_id_raw, network_raw, address_scheme_raw, extended_pubkey) =
                row_result
                    .map_err(|err| DbError::new(format!("Failed to map HD bundle row: {err}")))?;

            let account_id = parse_account_id(&account_id_raw)?;
            let asset_id = parse_asset_id(&asset_id_raw)?;
            let network = parse_network(&network_raw)?;
            let address_scheme = parse_address_scheme(&address_scheme_raw)?;
            let sync_state = load_account_sync_state_row(conn, account_id)?;

            let mut address_stmt = conn
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
                       AND da.address_scheme = ?3
                       AND da.source_type = 'derived'
                     ORDER BY da.derivation_change ASC, da.derivation_index ASC, da.created_at ASC, da.id ASC",
                )
                .map_err(|err| DbError::new(format!("Failed to prepare account addresses query: {err}")))?;

            let address_rows = address_stmt
                .query_map(
                    params![
                        super::ADDRESS_SYNC_SCOPE,
                        account_id.to_string(),
                        address_scheme.as_str(),
                    ],
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
                    DbError::new(format!("Failed to execute account addresses query: {err}"))
                })?;

            let mut external_addresses = Vec::new();
            let mut internal_addresses = Vec::new();
            for address_row in address_rows {
                let (
                    id,
                    address_account_id,
                    address,
                    address_asset_id,
                    address_network,
                    address_scheme_raw,
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
                ) = address_row.map_err(|err| {
                    DbError::new(format!("Failed to map account address row: {err}"))
                })?;

                let sync_address = map_sync_address_row(
                    id,
                    address_account_id,
                    address,
                    address_asset_id,
                    address_network,
                    address_scheme_raw,
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
                )?;

                match sync_address.derivation_change {
                    Some(1) => internal_addresses.push(sync_address),
                    _ => external_addresses.push(sync_address),
                }
            }

            bundles.push(AccountSyncBundle {
                account_id,
                asset_id,
                network,
                hd_key_extended_pubkey: extended_pubkey,
                address_scheme,
                sync_state,
                external_addresses,
                internal_addresses,
            });
        }

        Ok(bundles)
    })
}

pub(crate) fn upsert_hd_account_chain_sync_state(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    derivation_change: u32,
    frontier_state: &HdAccountChainSyncState,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let now_raw = now.to_rfc3339();
        conn.execute(
            "INSERT INTO hd_account_chain_sync_state
             (id, account_id, derivation_change, frontier_phase, next_index_to_scan, consecutive_unused, active_rescan_from_index, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(account_id, derivation_change) DO UPDATE SET
               frontier_phase = excluded.frontier_phase,
               next_index_to_scan = excluded.next_index_to_scan,
               consecutive_unused = excluded.consecutive_unused,
               active_rescan_from_index = excluded.active_rescan_from_index,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                account_id.to_string(),
                i64::from(derivation_change),
                frontier_state.frontier_phase().as_str(),
                i64::from(frontier_state.next_index_to_scan()),
                i64::from(frontier_state.consecutive_unused()),
                frontier_state.active_rescan_from_index().map(i64::from),
                now_raw,
                now_raw,
            ],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to upsert hd_account_chain_sync_state: {err}"
            ))
        })?;
        Ok(())
    })
}

pub(crate) fn delete_hd_account_chain_sync_state(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    derivation_change: u32,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "DELETE FROM hd_account_chain_sync_state
             WHERE account_id = ?1
               AND derivation_change = ?2",
            params![account_id.to_string(), i64::from(derivation_change)],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to delete hd_account_chain_sync_state: {err}"
            ))
        })?;
        Ok(())
    })
}

pub(crate) fn complete_hd_account_discovery(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    external_last_index: Option<u32>,
    internal_last_index: Option<u32>,
    completed_tip: ChainTipHeight,
    completed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let transaction = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start HD discovery completion: {err}"))
        })?;
        complete_hd_account_discovery_conn(
            &transaction,
            account_id,
            external_last_index,
            internal_last_index,
            completed_tip,
            completed_at,
        )?;
        transaction
            .commit()
            .map_err(|err| DbError::new(format!("Failed to commit HD discovery completion: {err}")))
    })
}

pub(in crate::db) fn complete_hd_account_discovery_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    external_last_index: Option<u32>,
    internal_last_index: Option<u32>,
    completed_tip: ChainTipHeight,
    completed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    let remaining_frontiers: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM hd_account_chain_sync_state
             WHERE account_id = ?1
               AND derivation_change IN (0, 1)",
            [account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| DbError::new(format!("Failed to check HD discovery frontiers: {err}")))?;
    if remaining_frontiers != 0 {
        return Err(DbError::new(
            "Cannot complete HD discovery while a branch frontier remains",
        ));
    }

    let updated = conn
        .execute(
            "UPDATE account_sync_state
             SET last_scanned_height = ?1,
                 last_scanned_time = ?2,
                 last_derived_external_index = ?3,
                 last_derived_internal_index = ?4,
                 updated_at = ?2
             WHERE account_id = ?5",
            params![
                completed_tip.value(),
                completed_at.to_rfc3339(),
                external_last_index.map(i64::from),
                internal_last_index.map(i64::from),
                account_id.to_string(),
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to complete HD discovery: {err}")))?;
    if updated != 1 {
        return Err(DbError::new(
            "Cannot complete HD discovery without an account sync state",
        ));
    }
    Ok(())
}

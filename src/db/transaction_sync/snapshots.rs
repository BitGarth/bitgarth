use super::super::error::DbError;
use super::super::user_db::with_user_db;
use super::parsers::*;
use super::types::*;
use crate::models::{UserId, parse_datetime};
use crate::transactions::{
    AccountBackfillProgress, AccountIntegrationSyncSnapshot, AccountSyncResult,
    AccountSyncSnapshot, AddressBackfillCursor, AddressBackfillState, AddressCount,
    AggregateSyncResult, AggregateSyncSnapshot, ConsecutiveFailureCount, EtherscanHistoryStatus,
    SyncErrorMessage, SyncIntegrationId, TransactionCount, TransactionSyncResult,
    compute_aggregate_sync_result, derive_account_sync_result,
};
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use rusqlite::params;
use std::collections::HashMap;

pub(super) fn aggregate_sync_result_from_account_sync_result(
    value: Option<AccountSyncResult>,
) -> Option<AggregateSyncResult> {
    match value {
        Some(AccountSyncResult::Success) => Some(AggregateSyncResult::Success),
        Some(AccountSyncResult::Partial) => Some(AggregateSyncResult::Partial),
        Some(AccountSyncResult::Failure) => Some(AggregateSyncResult::Failure),
        Some(AccountSyncResult::InProgress) | None => None,
    }
}

fn parse_optional_etherscan_history_status_rank(
    rank: Option<i64>,
) -> Result<Option<EtherscanHistoryStatus>, DbError> {
    match rank {
        Some(2) => Ok(Some(EtherscanHistoryStatus::Gap)),
        Some(1) => Ok(Some(EtherscanHistoryStatus::RecentOnly)),
        Some(0) => Ok(Some(EtherscanHistoryStatus::Continuous)),
        Some(other) => Err(DbError::new(format!(
            "Invalid etherscan_history_status rank in DB: {other}"
        ))),
        None => Ok(None),
    }
}

pub(super) struct AccountIntegrationSyncSnapshotFallback {
    last_completed_at: Option<DateTime<Utc>>,
    last_result: Option<AccountSyncResult>,
    last_error: Option<SyncErrorMessage>,
    backfill_progress: Option<AccountBackfillProgress>,
    etherscan_history_status: Option<EtherscanHistoryStatus>,
}

pub(super) fn build_account_integration_sync_snapshot(
    integration_id: SyncIntegrationId,
    row: Option<&AccountIntegrationSyncStateRow>,
    is_active: bool,
    fallback: AccountIntegrationSyncSnapshotFallback,
) -> AccountIntegrationSyncSnapshot {
    AccountIntegrationSyncSnapshot {
        integration_id,
        is_active,
        last_started_at: row.and_then(|state| state.last_started_at.as_ref().cloned()),
        last_completed_at: row
            .and_then(|state| state.last_completed_at.as_ref().cloned())
            .or(fallback.last_completed_at),
        last_result: row
            .and_then(|state| state.last_result)
            .or_else(|| aggregate_sync_result_from_account_sync_result(fallback.last_result)),
        last_error: row
            .and_then(|state| state.last_error.clone())
            .or(fallback.last_error),
        backfill_progress: fallback.backfill_progress,
        etherscan_history_status: fallback.etherscan_history_status,
    }
}

pub(super) fn load_account_integration_sync_state_rows(
    conn: &rusqlite::Connection,
) -> Result<
    HashMap<DigitalAssetAccountId, HashMap<SyncIntegrationId, AccountIntegrationSyncStateRow>>,
    DbError,
> {
    let mut stmt = conn
        .prepare(
            "SELECT
                account_id,
                integration_id,
                last_started_at,
                last_completed_at,
                last_result,
                last_error
             FROM account_integration_sync_state",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare account integration sync state query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute account integration sync state query: {err}"
            ))
        })?;

    let mut by_account = HashMap::<
        DigitalAssetAccountId,
        HashMap<SyncIntegrationId, AccountIntegrationSyncStateRow>,
    >::new();
    for row in rows {
        let (
            account_id_raw,
            integration_id_raw,
            last_started_at_raw,
            last_completed_at_raw,
            last_result_raw,
            last_error_raw,
        ) = row.map_err(|err| {
            DbError::new(format!(
                "Failed to map account integration sync state row: {err}"
            ))
        })?;
        let account_id = parse_account_id(&account_id_raw)?;
        let integration_id =
            SyncIntegrationId::from_db_value(&integration_id_raw).ok_or_else(|| {
                DbError::new(format!(
                    "Invalid integration_id in account_integration_sync_state: {integration_id_raw}"
                ))
            })?;
        let row = AccountIntegrationSyncStateRow {
            account_id,
            integration_id,
            last_started_at: parse_optional_time(last_started_at_raw, "last_started_at")?,
            last_completed_at: parse_optional_time(last_completed_at_raw, "last_completed_at")?,
            last_result: parse_optional_aggregate_sync_result(last_result_raw)?,
            last_error: last_error_raw
                .as_deref()
                .map(str::trim)
                .filter(|raw| !raw.is_empty())
                .map(SyncErrorMessage::sanitize),
        };
        by_account
            .entry(account_id)
            .or_default()
            .insert(integration_id, row);
    }

    Ok(by_account)
}

pub(crate) fn load_account_sync_snapshots(
    user_id: UserId,
) -> Result<Vec<AccountSyncSnapshot>, DbError> {
    with_user_db(user_id, |conn| {
        let integration_state_rows = load_account_integration_sync_state_rows(conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    a.id AS account_id,
                    a.asset_id AS asset_id,
                    COUNT(DISTINCT da.id) AS addresses_total,
                    COUNT(DISTINCT CASE
                        WHEN da.id IS NOT NULL AND tss.id IS NULL THEN da.id
                    END) AS addresses_never_synced,
                    COUNT(DISTINCT CASE
                        WHEN da.id IS NOT NULL
                         AND tss.last_started_at IS NOT NULL
                         AND (
                            tss.last_completed_at IS NULL
                            OR tss.last_started_at > tss.last_completed_at
                         )
                        THEN da.id
                    END) AS addresses_in_progress,
                    COUNT(DISTINCT CASE
                        WHEN da.id IS NOT NULL
                         AND tss.last_completed_at IS NOT NULL
                         AND tss.last_result = 'success'
                        THEN da.id
                    END) AS addresses_synced,
                    COUNT(DISTINCT CASE
                        WHEN da.id IS NOT NULL
                         AND tss.last_completed_at IS NOT NULL
                         AND tss.last_result = 'failure'
                        THEN da.id
                    END) AS addresses_failed,
                    MAX(CASE WHEN tss.last_result = 'success' THEN tss.last_completed_at END) AS last_success_at,
                    MAX(tss.last_completed_at) AS last_completed_at,
                    SUM(CASE
                        WHEN da.id IS NOT NULL
                         AND (
                            tss.mempool_backfill_cursor_txid IS NOT NULL
                            OR tss.etherscan_backfill_end_block IS NOT NULL
                         )
                        THEN COALESCE(atc.fetched_tx_count, 0)
                        ELSE 0
                    END) AS active_backfill_fetched_tx_count,
                    SUM(CASE
                        WHEN da.id IS NOT NULL
                         AND (
                            tss.mempool_backfill_cursor_txid IS NOT NULL
                            OR tss.etherscan_backfill_end_block IS NOT NULL
                         )
                        THEN COALESCE(tss.mempool_expected_tx_count, 0)
                        ELSE 0
                    END) AS active_backfill_expected_tx_count,
                    MAX(CASE
                        WHEN da.id IS NOT NULL
                         AND tss.mempool_backfill_cursor_txid IS NOT NULL
                        THEN tss.mempool_backfill_cursor_txid
                    END) AS active_mempool_backfill_cursor_txid,
                    MAX(CASE
                        WHEN da.id IS NOT NULL
                         AND tss.etherscan_backfill_end_block IS NOT NULL
                        THEN tss.etherscan_backfill_end_block
                    END) AS active_etherscan_backfill_end_block,
                    MAX(CASE tss.etherscan_history_status
                        WHEN 'gap' THEN 2
                        WHEN 'recent_only' THEN 1
                        WHEN 'continuous' THEN 0
                        ELSE NULL
                    END) AS etherscan_history_status_rank,
                    (
                        SELECT t2.last_error
                        FROM transaction_sync_state t2
                        JOIN digital_asset_addresses da2 ON da2.id = t2.address_id
                        WHERE da2.account_id = a.id
                          AND t2.scope = ?1
                          AND t2.last_result = 'failure'
                          AND t2.last_completed_at IS NOT NULL
                        ORDER BY t2.last_completed_at DESC, t2.id DESC
                        LIMIT 1
                    ) AS last_error,
                    MAX(tss.consecutive_failure_count) AS max_consecutive_failures
                 FROM digital_asset_accounts a
                 LEFT JOIN digital_asset_addresses da ON da.account_id = a.id
                 LEFT JOIN transaction_sync_state tss
                    ON tss.scope = ?1
                   AND tss.address_id = da.id
                 LEFT JOIN (
                    SELECT
                        merged.address_id,
                        COUNT(DISTINCT merged.tx_id) AS fetched_tx_count
                    FROM (
                        SELECT address_id, tx_id FROM transaction_outputs
                        UNION
                        SELECT address_id, tx_id FROM transaction_inputs
                    ) AS merged
                    GROUP BY merged.address_id
                 ) AS atc
                    ON atc.address_id = da.id
                 GROUP BY a.id
                 ORDER BY a.created_at ASC, a.id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare account sync snapshot query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map(params![super::ADDRESS_SYNC_SCOPE], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute account sync snapshot query: {err}"
                ))
            })?;

        let mut snapshots = Vec::new();
        for row_result in rows {
            let (
                account_id_raw,
                asset_id_raw,
                addresses_total_raw,
                addresses_never_synced_raw,
                addresses_in_progress_raw,
                addresses_synced_raw,
                addresses_failed_raw,
                last_success_at_raw,
                last_completed_at_raw,
                active_backfill_fetched_tx_count_raw,
                active_backfill_expected_tx_count_raw,
                active_mempool_backfill_cursor_txid_raw,
                active_etherscan_backfill_end_block_raw,
                etherscan_history_status_rank,
                last_error_raw,
                max_consecutive_failures_raw,
            ) = row_result.map_err(|err| {
                DbError::new(format!("Failed to map account sync snapshot row: {err}"))
            })?;

            let account_id = parse_account_id(&account_id_raw)?;
            let asset_id = parse_asset_id(&asset_id_raw)?;
            let addresses_total = parse_address_count(addresses_total_raw, "addresses_total")?;
            let active_backfill_fetched_tx_count = if addresses_total.value() == 1
                && (active_mempool_backfill_cursor_txid_raw.is_some()
                    || active_etherscan_backfill_end_block_raw.is_some())
            {
                Some(parse_transaction_count(
                    active_backfill_fetched_tx_count_raw,
                    "active_backfill_fetched_tx_count",
                )?)
            } else {
                None
            };
            let active_backfill_expected_tx_count =
                if addresses_total.value() == 1 && active_backfill_expected_tx_count_raw > 0 {
                    Some(parse_transaction_count(
                        active_backfill_expected_tx_count_raw,
                        "active_backfill_expected_tx_count",
                    )?)
                } else {
                    None
                };
            let backfill_progress = if addresses_total.value() == 1 {
                match (
                    parse_optional_mempool_cursor_txid(active_mempool_backfill_cursor_txid_raw)?,
                    parse_optional_ethereum_block_number(active_etherscan_backfill_end_block_raw)?,
                ) {
                    (Some(cursor_txid), _) => Some(AccountBackfillProgress::new(
                        AddressBackfillState::new(
                            AddressBackfillCursor::Mempool { cursor_txid },
                            active_backfill_expected_tx_count,
                        ),
                        active_backfill_fetched_tx_count,
                        false,
                    )),
                    (None, Some(end_block)) => Some(AccountBackfillProgress::new(
                        AddressBackfillState::new(
                            AddressBackfillCursor::Etherscan { end_block },
                            None,
                        ),
                        active_backfill_fetched_tx_count,
                        false,
                    )),
                    (None, None) => None,
                }
            } else {
                None
            };
            let etherscan_history_status =
                parse_optional_etherscan_history_status_rank(etherscan_history_status_rank)?;
            let last_error = last_error_raw
                .as_deref()
                .map(str::trim)
                .filter(|raw| !raw.is_empty())
                .map(SyncErrorMessage::sanitize);
            let mut snapshot = AccountSyncSnapshot {
                account_id,
                sync_integration_id: Some(SyncIntegrationId::for_asset(asset_id)),
                addresses_total,
                addresses_never_synced: parse_address_count(
                    addresses_never_synced_raw,
                    "addresses_never_synced",
                )?,
                addresses_synced: parse_address_count(addresses_synced_raw, "addresses_synced")?,
                addresses_failed: parse_address_count(addresses_failed_raw, "addresses_failed")?,
                addresses_in_progress: parse_address_count(
                    addresses_in_progress_raw,
                    "addresses_in_progress",
                )?,
                max_consecutive_failures: ConsecutiveFailureCount::try_new(
                    max_consecutive_failures_raw.unwrap_or(0),
                )
                .map_err(|err| {
                    DbError::new(format!("Invalid consecutive_failure_count in DB: {err}"))
                })?,
                last_success_at: last_success_at_raw
                    .as_deref()
                    .map(parse_datetime)
                    .transpose()
                    .map_err(|err| DbError::new(format!("Invalid last_success_at in DB: {err}")))?,
                last_completed_at: last_completed_at_raw
                    .as_deref()
                    .map(parse_datetime)
                    .transpose()
                    .map_err(|err| {
                        DbError::new(format!("Invalid last_completed_at in DB: {err}"))
                    })?,
                last_result: None,
                last_error: last_error.clone(),
                backfill_progress: backfill_progress.clone(),
                etherscan_history_status,
                integration_states: Vec::new(),
            };
            let legacy_last_result = derive_account_sync_result(&snapshot);
            let integration_id = SyncIntegrationId::for_asset(asset_id);
            let integration_state = build_account_integration_sync_snapshot(
                integration_id,
                integration_state_rows
                    .get(&account_id)
                    .and_then(|by_integration| by_integration.get(&integration_id)),
                snapshot.is_running(),
                AccountIntegrationSyncSnapshotFallback {
                    last_completed_at: snapshot.last_completed_at,
                    last_result: legacy_last_result,
                    last_error,
                    backfill_progress,
                    etherscan_history_status,
                },
            );
            snapshot.integration_states.push(integration_state);
            snapshot.last_result = derive_account_sync_result(&snapshot);
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    })
}

pub(crate) fn load_aggregate_sync_snapshot(
    user_id: UserId,
) -> Result<AggregateSyncSnapshot, DbError> {
    with_user_db(user_id, |conn| {
        // Count total addresses
        let addresses_total: i64 = conn
            .query_row("SELECT COUNT(*) FROM digital_asset_addresses", [], |row| {
                row.get(0)
            })
            .map_err(|err| DbError::new(format!("Failed to count addresses: {err}")))?;

        // Query all sync state rows
        let mut stmt = conn
            .prepare(
                "SELECT
                    last_started_at,
                    last_completed_at,
                    last_result,
                    last_error,
                    new_tx_count,
                    updated_tx_count
                 FROM transaction_sync_state
                 WHERE scope = ?1",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare aggregate sync snapshot query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map(params![super::ADDRESS_SYNC_SCOPE], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute aggregate sync snapshot query: {err}"
                ))
            })?;

        let mut any_running = false;
        let mut success_count: u32 = 0;
        let mut failure_count: u32 = 0;
        let mut total_new_tx = TransactionCount::zero();
        let mut total_updated_tx = TransactionCount::zero();
        let mut max_completed_at: Option<DateTime<Utc>> = None;
        let mut latest_failure_with_error: Option<(DateTime<Utc>, SyncErrorMessage)> = None;
        let mut latest_failure_without_time_error: Option<SyncErrorMessage> = None;

        for row_result in rows {
            let (
                started_at_raw,
                completed_at_raw,
                result_raw,
                last_error_raw,
                new_tx_raw,
                updated_tx_raw,
            ) = row_result
                .map_err(|err| DbError::new(format!("Failed to map sync state row: {err}")))?;

            let started_at = started_at_raw
                .as_deref()
                .map(parse_datetime)
                .transpose()
                .map_err(|err| DbError::new(format!("Invalid last_started_at in DB: {err}")))?;
            let completed_at = completed_at_raw
                .as_deref()
                .map(parse_datetime)
                .transpose()
                .map_err(|err| DbError::new(format!("Invalid last_completed_at in DB: {err}")))?;

            let is_row_running = match (started_at, completed_at) {
                (Some(started), Some(completed)) => started > completed,
                (Some(_), None) => true,
                _ => false,
            };
            if is_row_running {
                any_running = true;
            }

            if let Some(result_str) = result_raw.as_deref() {
                match TransactionSyncResult::from_db_value(result_str) {
                    Some(TransactionSyncResult::Success) => {
                        success_count = success_count.saturating_add(1);
                    }
                    Some(TransactionSyncResult::Failure) => {
                        failure_count = failure_count.saturating_add(1);
                        if let Some(raw_error) = last_error_raw
                            .as_deref()
                            .map(str::trim)
                            .filter(|raw| !raw.is_empty())
                        {
                            let sanitized = SyncErrorMessage::sanitize(raw_error);
                            if let Some(completed) = completed_at {
                                let should_replace = match latest_failure_with_error.as_ref() {
                                    Some((existing_completed, _)) => {
                                        completed > *existing_completed
                                    }
                                    None => true,
                                };
                                if should_replace {
                                    latest_failure_with_error = Some((completed, sanitized));
                                }
                            } else if latest_failure_without_time_error.is_none() {
                                latest_failure_without_time_error = Some(sanitized);
                            }
                        }
                    }
                    None => {}
                }
            }

            let new_tx = parse_transaction_count(new_tx_raw, "new_tx_count")?;
            let updated_tx = parse_transaction_count(updated_tx_raw, "updated_tx_count")?;
            total_new_tx = total_new_tx.saturating_add(new_tx);
            total_updated_tx = total_updated_tx.saturating_add(updated_tx);

            if let Some(completed) = completed_at {
                max_completed_at = Some(match max_completed_at {
                    Some(existing) if existing >= completed => existing,
                    _ => completed,
                });
            }
        }

        let addresses_synced = AddressCount::from_u32(success_count);
        let addresses_failed = AddressCount::from_u32(failure_count);
        let last_result = if success_count > 0 || failure_count > 0 {
            Some(compute_aggregate_sync_result(
                addresses_synced,
                addresses_failed,
            ))
        } else {
            None
        };
        let last_error = latest_failure_with_error
            .map(|(_, error)| error)
            .or(latest_failure_without_time_error);

        Ok(AggregateSyncSnapshot {
            is_running: any_running,
            addresses_total: AddressCount::from_u32(
                u32::try_from(addresses_total).unwrap_or(u32::MAX),
            ),
            addresses_synced,
            addresses_failed,
            last_completed_at: max_completed_at,
            last_result,
            last_error,
            new_tx_count: total_new_tx,
            updated_tx_count: total_updated_tx,
        })
    })
}

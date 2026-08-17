use super::super::error::DbError;
use super::super::raw_ingestion::SyncRunId;
use super::super::user_db::with_user_db_mut;
use super::parsers::*;
use super::types::*;
use crate::models::UserId;
use crate::transactions::{
    AggregateSyncResult, ApiConfirmedBalance, ChainTipHeight, EthereumBlockNumber,
    EtherscanHistoryStatus, MempoolCursorTxid, SyncErrorMessage, SyncIntegrationId,
    TransactionCount, TransactionSyncResult, TransactionSyncRunId, compute_aggregate_sync_result,
};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use ulid::Ulid;

pub(crate) struct MempoolAddressObservationSuccess {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) confirmed_tx_count: TransactionCount,
    pub(crate) confirmed_balance: Option<ApiConfirmedBalance>,
    pub(crate) tip_height: ChainTipHeight,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddressSyncCompletion {
    address_id: DigitalAssetAddressId,
    run_id: TransactionSyncRunId,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    result: TransactionSyncResult,
    last_error: Option<SyncErrorMessage>,
    last_tip_height: Option<ChainTipHeight>,
    new_tx_count: TransactionCount,
    updated_tx_count: TransactionCount,
    api_confirmed_balance: Option<ApiConfirmedBalance>,
}

impl AddressSyncCompletion {
    fn from_success(success: &AddressSyncSuccess) -> Self {
        Self {
            address_id: success.address_id,
            run_id: success.run_id,
            started_at: success.started_at,
            completed_at: success.completed_at,
            result: TransactionSyncResult::Success,
            last_error: None,
            last_tip_height: Some(success.last_tip_height),
            new_tx_count: success.new_tx_count,
            updated_tx_count: success.updated_tx_count,
            api_confirmed_balance: success.api_confirmed_balance,
        }
    }

    fn from_failure(
        address_id: DigitalAssetAddressId,
        run_id: TransactionSyncRunId,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        error: &SyncErrorMessage,
    ) -> Self {
        Self {
            address_id,
            run_id,
            started_at,
            completed_at,
            result: TransactionSyncResult::Failure,
            last_error: Some(error.clone()),
            last_tip_height: None,
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: None,
        }
    }
}

fn account_exists_for_integration_state(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM digital_asset_accounts
            WHERE id = ?1
        )",
        params![account_id.to_string()],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|err| {
        DbError::new(format!(
            "Failed to check account existence for integration sync state: {err}"
        ))
    })
}

struct UpsertAccountIntegrationSyncStateRequest<'a> {
    account_id: DigitalAssetAccountId,
    integration_id: SyncIntegrationId,
    last_started_at: Option<DateTime<Utc>>,
    last_completed_at: Option<DateTime<Utc>>,
    last_result: Option<AggregateSyncResult>,
    last_error: Option<&'a SyncErrorMessage>,
    updated_at: DateTime<Utc>,
}

fn upsert_account_integration_sync_state_row(
    conn: &rusqlite::Connection,
    request: UpsertAccountIntegrationSyncStateRequest<'_>,
) -> Result<(), DbError> {
    let updated_at_raw = request.updated_at.to_rfc3339();
    conn.execute(
        "INSERT INTO account_integration_sync_state
         (id, account_id, integration_id, last_started_at, last_completed_at, last_result, last_error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(account_id, integration_id) DO UPDATE SET
           last_started_at = excluded.last_started_at,
           last_completed_at = excluded.last_completed_at,
           last_result = excluded.last_result,
           last_error = excluded.last_error,
           updated_at = excluded.updated_at",
        params![
            Ulid::new().to_string(),
            request.account_id.to_string(),
            request.integration_id.as_db_value(),
            request.last_started_at.map(|value| value.to_rfc3339()),
            request.last_completed_at.map(|value| value.to_rfc3339()),
            request.last_result.map(AggregateSyncResult::as_db_value),
            request.last_error.map(SyncErrorMessage::as_str),
            updated_at_raw,
            updated_at_raw,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to upsert account integration sync state: {err}")))?;

    Ok(())
}

pub(crate) fn mark_account_integration_sync_started(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    integration_id: SyncIntegrationId,
    started_at: DateTime<Utc>,
) -> Result<AccountIntegrationSyncStart, DbError> {
    with_user_db_mut(user_id, |conn| {
        if !account_exists_for_integration_state(conn, account_id)? {
            return Ok(AccountIntegrationSyncStart {
                was_interrupted: false,
            });
        }

        let previous = conn
            .query_row(
                "SELECT last_started_at, last_completed_at
                 FROM account_integration_sync_state
                 WHERE account_id = ?1 AND integration_id = ?2",
                params![account_id.to_string(), integration_id.as_db_value()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to load previous account integration sync state: {err}"
                ))
            })?;
        let was_interrupted = previous
            .map(|(last_started_at, last_completed_at)| {
                Ok::<bool, DbError>(
                    match (
                        parse_optional_time(last_started_at, "last_started_at")?,
                        parse_optional_time(last_completed_at, "last_completed_at")?,
                    ) {
                        (Some(_), None) => true,
                        (Some(last_started_at), Some(last_completed_at)) => {
                            last_started_at > last_completed_at
                        }
                        (None, _) => false,
                    },
                )
            })
            .transpose()?
            .unwrap_or(false);
        let started_at_raw = started_at.to_rfc3339();
        conn.execute(
            "INSERT INTO account_integration_sync_state
             (id, account_id, integration_id, last_started_at, last_completed_at, last_result, last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(account_id, integration_id) DO UPDATE SET
               last_started_at = excluded.last_started_at,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                account_id.to_string(),
                integration_id.as_db_value(),
                started_at_raw,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                started_at.to_rfc3339(),
                started_at.to_rfc3339(),
            ],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to mark account integration sync start: {err}"
            ))
        })?;

        Ok(AccountIntegrationSyncStart { was_interrupted })
    })
}

pub(crate) fn refresh_account_integration_sync_state(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    integration_id: SyncIntegrationId,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        if !account_exists_for_integration_state(conn, account_id)? {
            return Ok(());
        }

        let (
            addresses_in_progress_raw,
            addresses_never_synced_raw,
            addresses_synced_raw,
            addresses_failed_raw,
            last_started_at_raw,
            last_completed_at_raw,
            last_error_raw,
        ) = conn
            .query_row(
                "SELECT
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
                        WHEN da.id IS NOT NULL AND tss.id IS NULL THEN da.id
                    END) AS addresses_never_synced,
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
                    MAX(tss.last_started_at) AS last_started_at,
                    MAX(tss.last_completed_at) AS last_completed_at,
                    (
                        SELECT t2.last_error
                        FROM transaction_sync_state t2
                        JOIN digital_asset_addresses da2 ON da2.id = t2.address_id
                        WHERE da2.account_id = ?2
                          AND t2.scope = ?1
                          AND t2.last_result = 'failure'
                          AND t2.last_completed_at IS NOT NULL
                        ORDER BY t2.last_completed_at DESC, t2.id DESC
                        LIMIT 1
                    ) AS last_error
                 FROM digital_asset_addresses da
                 LEFT JOIN transaction_sync_state tss
                    ON tss.scope = ?1
                   AND tss.address_id = da.id
                 WHERE da.account_id = ?2",
                params![super::ADDRESS_SYNC_SCOPE, account_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to load account integration sync aggregate state: {err}"
                ))
            })?;

        let addresses_in_progress =
            parse_address_count(addresses_in_progress_raw, "addresses_in_progress")?;
        if addresses_in_progress.value() > 0 {
            return Ok(());
        }

        let addresses_never_synced =
            parse_address_count(addresses_never_synced_raw, "addresses_never_synced")?;
        let addresses_synced = parse_address_count(addresses_synced_raw, "addresses_synced")?;
        let addresses_failed = parse_address_count(addresses_failed_raw, "addresses_failed")?;
        let last_result = if addresses_failed.value() == 0
            && (addresses_synced.value() == 0 || addresses_never_synced.value() > 0)
        {
            None
        } else if addresses_never_synced.value() > 0 {
            Some(AggregateSyncResult::Partial)
        } else {
            Some(compute_aggregate_sync_result(
                addresses_synced,
                addresses_failed,
            ))
        };
        let last_error = match last_result {
            Some(AggregateSyncResult::Success) | None => None,
            Some(AggregateSyncResult::Partial) | Some(AggregateSyncResult::Failure) => {
                last_error_raw
                    .as_deref()
                    .map(str::trim)
                    .filter(|raw| !raw.is_empty())
                    .map(SyncErrorMessage::sanitize)
            }
        };

        upsert_account_integration_sync_state_row(
            conn,
            UpsertAccountIntegrationSyncStateRequest {
                account_id,
                integration_id,
                last_started_at: parse_optional_time(last_started_at_raw, "last_started_at")?,
                last_completed_at: parse_optional_time(last_completed_at_raw, "last_completed_at")?,
                last_result,
                last_error: last_error.as_ref(),
                updated_at: observed_at,
            },
        )
    })
}

pub(crate) fn mark_address_sync_started(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    run_id: TransactionSyncRunId,
    started_at: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let started_at = started_at.to_rfc3339();
        conn.execute(
            "INSERT INTO transaction_sync_state
             (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result, last_error, last_tip_height, new_tx_count, updated_tx_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(scope, address_id) DO UPDATE SET
               last_run_id = excluded.last_run_id,
               last_started_at = excluded.last_started_at,
               last_completed_at = excluded.last_completed_at,
               last_error = excluded.last_error,
               last_tip_height = excluded.last_tip_height,
               new_tx_count = excluded.new_tx_count,
               updated_tx_count = excluded.updated_tx_count,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                super::ADDRESS_SYNC_SCOPE,
                address_id.to_string(),
                run_id.to_string(),
                started_at,
                Option::<String>::None,
                TransactionSyncResult::Success.as_db_value(),
                Option::<String>::None,
                Option::<i64>::None,
                0_i64,
                0_i64,
                started_at,
                started_at,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to mark sync start: {err}")))?;

        Ok(())
    })
}

pub(crate) fn persist_mempool_address_observation_success(
    user_id: UserId,
    observation: MempoolAddressObservationSuccess,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let observed_at = observation.observed_at.to_rfc3339();
        let reported_tx_count = observation.confirmed_tx_count.value();
        let (balance_hi, balance_lo) = observation
            .confirmed_balance
            .map(split_api_confirmed_balance)
            .transpose()?
            .map_or((None, None), |(hi, lo)| (Some(hi), Some(lo)));
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET last_completed_at = ?1,
                     last_result = ?2,
                     last_error = NULL,
                     last_tip_height = ?3,
                     reported_tx_count = ?4,
                     api_confirmed_balance_hi = ?5,
                     api_confirmed_balance_lo = ?6,
                     consecutive_failure_count = 0,
                     updated_at = ?1
                 WHERE scope = ?7
                   AND address_id = ?8",
                params![
                    observed_at,
                    TransactionSyncResult::Success.as_db_value(),
                    observation.tip_height.value(),
                    i64::from(reported_tx_count),
                    balance_hi,
                    balance_lo,
                    super::ADDRESS_SYNC_SCOPE,
                    observation.address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to persist successful mempool address observation: {err}"
                ))
            })?;
        if changed == 0 {
            return Err(DbError::new(
                "Failed to persist successful mempool address observation: sync state row missing",
            ));
        }
        Ok(())
    })
}

fn insert_address_sync_completion(
    conn: &rusqlite::Connection,
    completion: &AddressSyncCompletion,
) -> Result<(), DbError> {
    let started_at = completion.started_at.to_rfc3339();
    let completed_at = completion.completed_at.to_rfc3339();
    let (balance_hi, balance_lo) = completion
        .api_confirmed_balance
        .map(split_api_confirmed_balance)
        .transpose()?
        .map_or((None, None), |(hi, lo)| (Some(hi), Some(lo)));
    conn.execute(
        "INSERT INTO transaction_sync_state
         (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result, last_error, last_tip_height, new_tx_count, updated_tx_count, api_confirmed_balance_hi, api_confirmed_balance_lo, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            Ulid::new().to_string(),
            super::ADDRESS_SYNC_SCOPE,
            completion.address_id.to_string(),
            completion.run_id.to_string(),
            started_at,
            completed_at,
            completion.result.as_db_value(),
            completion.last_error.as_ref().map(SyncErrorMessage::as_str),
            completion.last_tip_height.map(ChainTipHeight::value),
            i64::from(completion.new_tx_count.value()),
            i64::from(completion.updated_tx_count.value()),
            balance_hi,
            balance_lo,
            started_at,
            completed_at,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert sync completion: {err}")))?;

    Ok(())
}

pub(crate) fn mark_address_sync_completed_success(
    user_id: UserId,
    success: &AddressSyncSuccess,
) -> Result<(), DbError> {
    let completion = AddressSyncCompletion::from_success(success);
    with_user_db_mut(user_id, |conn| {
        let started_at_raw = completion.started_at.to_rfc3339();
        let completed_at_raw = completion.completed_at.to_rfc3339();
        let (balance_hi, balance_lo) = completion
            .api_confirmed_balance
            .map(split_api_confirmed_balance)
            .transpose()?
            .map_or((None, None), |(hi, lo)| (Some(hi), Some(lo)));
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET last_run_id = ?1,
                     last_started_at = ?2,
                     last_completed_at = ?3,
                     last_result = ?4,
                     last_error = ?5,
                     last_tip_height = ?6,
                     new_tx_count = ?7,
                     updated_tx_count = ?8,
                     api_confirmed_balance_hi = COALESCE(?9, api_confirmed_balance_hi),
                     api_confirmed_balance_lo = COALESCE(?10, api_confirmed_balance_lo),
                     consecutive_failure_count = 0,
                     updated_at = ?11
                 WHERE scope = ?12
                   AND address_id = ?13",
                params![
                    completion.run_id.to_string(),
                    started_at_raw,
                    completed_at_raw,
                    TransactionSyncResult::Success.as_db_value(),
                    Option::<String>::None,
                    success.last_tip_height.value(),
                    i64::from(completion.new_tx_count.value()),
                    i64::from(completion.updated_tx_count.value()),
                    balance_hi,
                    balance_lo,
                    completed_at_raw,
                    super::ADDRESS_SYNC_SCOPE,
                    completion.address_id.to_string(),
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to mark sync success: {err}")))?;

        if changed == 0 {
            insert_address_sync_completion(conn, &completion)?;
        }

        Ok(())
    })
}

pub(crate) fn mark_address_sync_completed_failure(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    run_id: TransactionSyncRunId,
    started_at: DateTime<Utc>,
    failed_at: DateTime<Utc>,
    error: &SyncErrorMessage,
    count_as_address_failure: bool,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let started_at_raw = started_at.to_rfc3339();
        let failed_at_raw = failed_at.to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET last_run_id = ?1,
                     last_started_at = ?2,
                     last_completed_at = ?3,
                     last_result = ?4,
                     last_error = ?5,
                     new_tx_count = ?6,
                     updated_tx_count = ?7,
                     consecutive_failure_count = CASE
                        WHEN ?8 THEN consecutive_failure_count + 1
                        ELSE consecutive_failure_count
                     END,
                     updated_at = ?9
                 WHERE scope = ?10
                   AND address_id = ?11",
                params![
                    run_id.to_string(),
                    started_at_raw,
                    failed_at_raw,
                    TransactionSyncResult::Failure.as_db_value(),
                    error.as_str(),
                    0_i64,
                    0_i64,
                    count_as_address_failure,
                    failed_at_raw,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to mark sync failure: {err}")))?;

        if changed == 0 {
            let completion = AddressSyncCompletion::from_failure(
                address_id, run_id, started_at, failed_at, error,
            );
            insert_address_sync_completion(conn, &completion)?;
            conn.execute(
                "UPDATE transaction_sync_state
                 SET consecutive_failure_count = ?1
                 WHERE scope = ?2
                   AND address_id = ?3",
                params![
                    if count_as_address_failure {
                        1_i64
                    } else {
                        0_i64
                    },
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to initialize address failure count state: {err}"
                ))
            })?;
        }

        Ok(())
    })
}

pub(crate) fn update_address_mempool_backfill_cursor(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    cursor_txid: Option<&MempoolCursorTxid>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET mempool_backfill_cursor_txid = ?1,
                     updated_at = ?2
                 WHERE scope = ?3
                   AND address_id = ?4",
                params![
                    cursor_txid.map(MempoolCursorTxid::as_str),
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to update mempool backfill cursor in sync state: {err}"
                ))
            })?;

        if changed == 0 {
            return Err(DbError::new(
                "Failed to update mempool backfill cursor: sync state row missing",
            ));
        }

        Ok(())
    })
}

pub(crate) fn begin_mempool_history_scan(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    start_run_id: SyncRunId,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET mempool_history_scan_start_run_id = ?1,
                     mempool_backfill_cursor_txid = NULL,
                     updated_at = ?2
                 WHERE scope = ?3
                   AND address_id = ?4",
                params![
                    start_run_id.to_string(),
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to begin mempool history scan: {err}")))?;

        if changed == 0 {
            return Err(DbError::new(
                "Failed to begin mempool history scan: sync state row missing",
            ));
        }
        Ok(())
    })
}

pub(crate) fn commit_mempool_history_page_work(
    user_id: UserId,
    update: MempoolHistoryPageWorkUpdate,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!(
                "Failed to start mempool history page work transaction: {err}"
            ))
        })?;
        let updated_at = Utc::now().to_rfc3339();

        if let Some(frontier) = &update.hd_frontier {
            let next_address_id = frontier.next_address_id.map(|id| id.to_string());
            let addresses_belong_to_account = tx
                .query_row(
                    "SELECT
                        EXISTS(
                            SELECT 1 FROM digital_asset_addresses
                            WHERE id = ?1 AND account_id = ?2
                        )
                        AND (
                            ?3 IS NULL
                            OR EXISTS(
                                SELECT 1 FROM digital_asset_addresses
                                WHERE id = ?3 AND account_id = ?2
                            )
                        )",
                    params![
                        update.address_id.to_string(),
                        frontier.account_id.to_string(),
                        next_address_id,
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|err| {
                    DbError::new(format!(
                        "Failed to validate mempool history frontier ownership: {err}"
                    ))
                })?;
            if !addresses_belong_to_account {
                return Err(DbError::new(
                    "Mempool history frontier address must belong to the account",
                ));
            }
        }

        let changed = tx
            .execute(
                "UPDATE transaction_sync_state
                 SET mempool_backfill_cursor_txid = ?1,
                     updated_at = ?2
                 WHERE scope = ?3
                   AND address_id = ?4",
                params![
                    update.next_cursor.as_ref().map(MempoolCursorTxid::as_str),
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    update.address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to commit mempool history page cursor: {err}"
                ))
            })?;
        if changed == 0 {
            return Err(DbError::new(
                "Failed to commit mempool history page cursor: sync state row missing",
            ));
        }

        if let Some(frontier) = update.hd_frontier {
            let changed = tx
                .execute(
                    "UPDATE account_sync_state
                     SET mempool_history_next_address_id = ?1,
                         updated_at = ?2
                     WHERE account_id = ?3",
                    params![
                        frontier.next_address_id.map(|id| id.to_string()),
                        updated_at,
                        frontier.account_id.to_string(),
                    ],
                )
                .map_err(|err| {
                    DbError::new(format!(
                        "Failed to commit mempool history account frontier: {err}"
                    ))
                })?;
            if changed == 0 {
                return Err(DbError::new(
                    "Failed to commit mempool history account frontier: account sync state row missing",
                ));
            }
        }

        tx.commit().map_err(|err| {
            DbError::new(format!(
                "Failed to commit mempool history page work transaction: {err}"
            ))
        })
    })
}

pub(crate) fn publish_mempool_history_proof(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    proof: MempoolHistoryProof,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        publish_mempool_history_proof_conn(conn, address_id, proof, &updated_at)
    })
}

pub(in crate::db) fn publish_mempool_history_proof_conn(
    conn: &rusqlite::Connection,
    address_id: DigitalAssetAddressId,
    proof: MempoolHistoryProof,
    updated_at: &str,
) -> Result<(), DbError> {
    let changed = conn
        .execute(
            "UPDATE transaction_sync_state
             SET mempool_history_complete_tx_count = ?1,
                 mempool_history_complete_height = ?2,
                 updated_at = ?3
             WHERE scope = ?4
               AND address_id = ?5",
            params![
                i64::from(proof.confirmed_tx_count.value()),
                proof.complete_height.value(),
                updated_at,
                super::ADDRESS_SYNC_SCOPE,
                address_id.to_string(),
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to publish mempool history proof: {err}")))?;
    if changed == 0 {
        return Err(DbError::new(
            "Failed to publish mempool history proof: sync state row missing",
        ));
    }
    Ok(())
}

pub(crate) fn publish_strict_mempool_history_proof(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    scan_start_run_id: SyncRunId,
    proof: MempoolHistoryProof,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        publish_strict_mempool_history_proof_conn(
            conn,
            address_id,
            scan_start_run_id,
            proof,
            &updated_at,
        )
    })
}

pub(in crate::db) fn publish_strict_mempool_history_proof_conn(
    conn: &rusqlite::Connection,
    address_id: DigitalAssetAddressId,
    scan_start_run_id: SyncRunId,
    proof: MempoolHistoryProof,
    updated_at: &str,
) -> Result<(), DbError> {
    let changed = conn
        .execute(
            "UPDATE transaction_sync_state
             SET mempool_history_complete_tx_count = ?1,
                 mempool_history_complete_height = ?2,
                 mempool_history_scan_start_run_id = NULL,
                 updated_at = ?3
             WHERE scope = ?4
               AND address_id = ?5
               AND mempool_history_scan_start_run_id = ?6",
            params![
                i64::from(proof.confirmed_tx_count.value()),
                proof.complete_height.value(),
                updated_at,
                super::ADDRESS_SYNC_SCOPE,
                address_id.to_string(),
                scan_start_run_id.to_string(),
            ],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to publish strict mempool history proof: {err}"
            ))
        })?;
    if changed == 0 {
        return Err(DbError::new(
            "Failed to publish strict mempool history proof: scan start did not match",
        ));
    }
    Ok(())
}

pub(crate) fn invalidate_mempool_history_proof(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!(
                "Failed to start mempool history invalidation transaction: {err}"
            ))
        })?;
        let account_id = tx
            .query_row(
                "SELECT account_id FROM digital_asset_addresses WHERE id = ?1",
                [address_id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to resolve invalidated mempool account: {err}"
                ))
            })?
            .as_deref()
            .map(parse_account_id)
            .transpose()?;
        let mut targets = CoverageInvalidationTargets {
            address_ids: std::collections::HashSet::from([address_id]),
            account_ids: std::collections::HashSet::new(),
        };
        targets.account_ids.extend(account_id);
        let changed =
            invalidate_mempool_history_coverage_tx(&tx, &targets, &Utc::now().to_rfc3339())?;
        if changed == 0 {
            return Err(DbError::new(
                "Failed to invalidate mempool history proof: sync state row missing",
            ));
        }
        tx.commit().map_err(|err| {
            DbError::new(format!(
                "Failed to commit mempool history invalidation transaction: {err}"
            ))
        })
    })
}

fn invalidate_mempool_history_coverage_tx(
    tx: &rusqlite::Transaction<'_>,
    targets: &CoverageInvalidationTargets,
    updated_at: &str,
) -> Result<usize, DbError> {
    let mut changed = 0_usize;
    for address_id in &targets.address_ids {
        changed = changed.saturating_add(
            tx.execute(
                "UPDATE transaction_sync_state
                 SET mempool_history_complete_tx_count = NULL,
                     mempool_history_complete_height = NULL,
                     updated_at = ?1
                 WHERE scope = ?2 AND address_id = ?3",
                params![
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!("Failed to invalidate mempool history proof: {err}"))
            })?,
        );
    }
    for account_id in &targets.account_ids {
        tx.execute(
            "UPDATE account_transaction_ledger
             SET closing_balance_hi = NULL,
                 closing_balance_lo = NULL,
                 updated_at = ?1
             WHERE account_id = ?2",
            params![updated_at, account_id.to_string()],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to clear invalidated mempool account closing balances: {err}"
            ))
        })?;
    }
    Ok(changed)
}

pub(crate) fn invalidate_mempool_history_coverage(
    user_id: UserId,
    targets: &CoverageInvalidationTargets,
) -> Result<(), DbError> {
    if targets.address_ids.is_empty() && targets.account_ids.is_empty() {
        return Ok(());
    }
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!(
                "Failed to start mempool coverage invalidation transaction: {err}"
            ))
        })?;
        invalidate_mempool_history_coverage_tx(&tx, targets, &Utc::now().to_rfc3339())?;
        tx.commit().map_err(|err| {
            DbError::new(format!(
                "Failed to commit mempool coverage invalidation transaction: {err}"
            ))
        })
    })
}

pub(crate) fn invalidate_mempool_account_history_coverage(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<CoverageInvalidationTargets, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!(
                "Failed to start interrupted account invalidation transaction: {err}"
            ))
        })?;
        let mut statement = tx
            .prepare("SELECT id FROM digital_asset_addresses WHERE account_id = ?1")
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare interrupted account addresses: {err}"
                ))
            })?;
        let address_ids = statement
            .query_map([account_id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to query interrupted account addresses: {err}"
                ))
            })?
            .map(|row| {
                parse_address_id(&row.map_err(|err| {
                    DbError::new(format!("Failed to read interrupted account address: {err}"))
                })?)
            })
            .collect::<Result<std::collections::HashSet<_>, DbError>>()?;
        drop(statement);
        let targets = CoverageInvalidationTargets {
            address_ids,
            account_ids: std::collections::HashSet::from([account_id]),
        };
        invalidate_mempool_history_coverage_tx(&tx, &targets, &Utc::now().to_rfc3339())?;
        tx.commit().map_err(|err| {
            DbError::new(format!(
                "Failed to commit interrupted account invalidation transaction: {err}"
            ))
        })?;
        Ok(targets)
    })
}

pub(crate) fn update_address_etherscan_backfill_cursor(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    end_block: Option<EthereumBlockNumber>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET etherscan_backfill_end_block = ?1,
                     etherscan_backfill_start_block = NULL,
                     updated_at = ?2
                WHERE scope = ?3
                   AND address_id = ?4",
                params![
                    end_block.map(EthereumBlockNumber::value),
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to update etherscan backfill cursor in sync state: {err}"
                ))
            })?;

        if changed == 0 {
            return Err(DbError::new(
                "Failed to update etherscan backfill cursor: sync state row missing",
            ));
        }

        Ok(())
    })
}

pub(crate) fn update_address_etherscan_history_status(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    status: EtherscanHistoryStatus,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        let checkpoint_version = (status == EtherscanHistoryStatus::Continuous).then_some(1_i64);
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET etherscan_history_status = ?1,
                     etherscan_history_checkpoint_version = ?2,
                     updated_at = ?3
                 WHERE scope = ?4
                   AND address_id = ?5",
                params![
                    status.as_db_value(),
                    checkpoint_version,
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to update etherscan history status in sync state: {err}"
                ))
            })?;

        if changed == 0 {
            return Err(DbError::new(
                "Failed to update etherscan history status: sync state row missing",
            ));
        }

        Ok(())
    })
}

pub(crate) fn update_address_mempool_expected_tx_count(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    expected_tx_count: Option<TransactionCount>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let updated_at = Utc::now().to_rfc3339();
        let expected_tx_count_raw = expected_tx_count.map(|value| i64::from(value.value()));
        let changed = conn
            .execute(
                "UPDATE transaction_sync_state
                 SET mempool_expected_tx_count = ?1,
                     updated_at = ?2
                 WHERE scope = ?3
                   AND address_id = ?4",
                params![
                    expected_tx_count_raw,
                    updated_at,
                    super::ADDRESS_SYNC_SCOPE,
                    address_id.to_string(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to update mempool expected tx count in sync state: {err}"
                ))
            })?;

        if changed == 0 {
            return Err(DbError::new(
                "Failed to update mempool expected tx count: sync state row missing",
            ));
        }

        Ok(())
    })
}

pub(crate) fn upsert_account_sync_state(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    gap_limit: u32,
    last_derived_external_index: Option<u32>,
    last_derived_internal_index: Option<u32>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let now_raw = now.to_rfc3339();
        conn.execute(
            "INSERT INTO account_sync_state
             (id, account_id, last_scanned_height, last_scanned_time, gap_limit, last_derived_external_index, last_derived_internal_index, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
               gap_limit = excluded.gap_limit,
               last_derived_external_index = excluded.last_derived_external_index,
               last_derived_internal_index = excluded.last_derived_internal_index,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                account_id.to_string(),
                i64::from(gap_limit),
                last_derived_external_index.map(i64::from),
                last_derived_internal_index.map(i64::from),
                now_raw,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert account_sync_state: {err}")))?;
        Ok(())
    })
}

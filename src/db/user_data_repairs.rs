use super::error::DbError;
use crate::db::account_admission::backfill_account_admission_conn;
use crate::db::account_transactions::rebuild_account_transaction_ledger_conn;
use crate::db::chain_cleanup::{
    begin_chain_cleanup_scope, execute_chain_cleanup_for_marked_candidates,
    mark_chain_cleanup_candidate,
};
use crate::db::raw_ingestion::{
    AllCurrentRawEtherscanInternalTransactionHeadRow,
    AllCurrentRawEtherscanNormalTransactionHeadRow,
    load_all_current_raw_etherscan_internal_transaction_heads_conn,
    load_all_current_raw_etherscan_normal_transaction_heads_conn,
};
use crate::db::transaction_sync::reconcile_account_transactions_conn;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAccountId, Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Instant;

pub(crate) const BTC_LEDGER_VALUE_EXCLUDES_FEE_REPAIR: &str = "btc_ledger_value_excludes_fee_v1";
pub(crate) const BTC_LEDGER_SELF_TRANSFER_DIRECTION_REPAIR: &str =
    "btc_ledger_self_transfer_direction_v1";
pub(crate) const NATIVE_LEDGER_BALANCE_DELTA_REPAIR: &str = "native_ledger_balance_delta_v1";
pub(crate) const ETHERSCAN_PROVIDER_TRANSFER_KEY_REPAIR: &str =
    "etherscan_provider_transfer_key_v1";
pub(crate) const FAILED_TRANSACTION_ACCOUNTING_REPAIR: &str = "failed_transaction_accounting_v1";
pub(crate) const BITCOIN_HISTORY_FULL_RESYNC_REPAIR: &str = "bitcoin_history_full_resync_v1";
pub(crate) const ACCOUNT_ADMISSION_ORDER_REPAIR: &str = "account_admission_order_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserDataRepairStatus {
    Pending,
    Completed,
}

impl UserDataRepairStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
        }
    }
}

fn parse_status(raw: &str) -> Result<UserDataRepairStatus, DbError> {
    match raw {
        "pending" => Ok(UserDataRepairStatus::Pending),
        "completed" => Ok(UserDataRepairStatus::Completed),
        _ => Err(DbError::new(format!(
            "Invalid user_data_repairs.status value: {raw}"
        ))),
    }
}

fn truncate_repair_error(error: &str) -> String {
    const MAX_LEN: usize = 500;
    error.chars().take(MAX_LEN).collect()
}

fn register_user_data_repair_conn(
    conn: &rusqlite::Connection,
    repair_key: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let now_raw = now.to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO user_data_repairs
             (repair_key, status, last_attempted_at, completed_at, last_error, created_at, updated_at)
         VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?3)",
        params![repair_key, UserDataRepairStatus::Pending.as_str(), now_raw],
    )
    .map_err(|err| DbError::new(format!("Failed to register user data repair: {err}")))?;
    Ok(())
}

pub(crate) fn load_user_data_repair_status_conn(
    conn: &rusqlite::Connection,
    repair_key: &str,
) -> Result<Option<UserDataRepairStatus>, DbError> {
    let raw = conn
        .query_row(
            "SELECT status FROM user_data_repairs WHERE repair_key = ?1",
            params![repair_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load user data repair status: {err}")))?;

    raw.as_deref().map(parse_status).transpose()
}

pub(crate) fn load_user_data_repair_status(
    user_id: UserId,
    repair_key: &str,
) -> Result<Option<UserDataRepairStatus>, DbError> {
    super::user_db::with_user_db(user_id, |conn| {
        load_user_data_repair_status_conn(conn, repair_key)
    })
}

fn mark_user_data_repair_completed_conn(
    conn: &rusqlite::Connection,
    repair_key: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let now_raw = now.to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE user_data_repairs
            SET status = ?2,
                last_attempted_at = ?3,
                completed_at = ?3,
                last_error = NULL,
                updated_at = ?3
          WHERE repair_key = ?1",
            params![
                repair_key,
                UserDataRepairStatus::Completed.as_str(),
                now_raw
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to mark user data repair completed: {err}")))?;
    if rows == 0 {
        return Err(DbError::new(format!(
            "Cannot mark unknown user data repair completed: {repair_key}"
        )));
    }
    Ok(())
}

fn record_user_data_repair_failure_conn(
    conn: &rusqlite::Connection,
    repair_key: &str,
    now: DateTime<Utc>,
    error: &str,
) -> Result<(), DbError> {
    let now_raw = now.to_rfc3339();
    let truncated = truncate_repair_error(error);
    let rows = conn
        .execute(
            "UPDATE user_data_repairs
            SET status = ?2,
                last_attempted_at = ?3,
                completed_at = NULL,
                last_error = ?4,
                updated_at = ?3
          WHERE repair_key = ?1",
            params![
                repair_key,
                UserDataRepairStatus::Pending.as_str(),
                now_raw,
                truncated
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to record user data repair failure: {err}")))?;
    if rows == 0 {
        return Err(DbError::new(format!(
            "Cannot record failure for unknown user data repair: {repair_key}"
        )));
    }
    Ok(())
}

pub(crate) fn record_user_data_repair_failure(
    user_id: UserId,
    repair_key: &str,
    now: DateTime<Utc>,
    error: &str,
) -> Result<(), DbError> {
    super::user_db::with_user_db_mut(user_id, |conn| {
        record_user_data_repair_failure_conn(conn, repair_key, now, error)
    })
}

fn load_eligible_bitcoin_history_repair_account_ids(
    user_id: UserId,
) -> Result<Vec<DigitalAssetAccountId>, DbError> {
    super::user_db::with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT da.account_id
                 FROM digital_asset_addresses da
                 JOIN digital_asset_accounts daa ON daa.id = da.account_id
                 JOIN (
                     SELECT tx_id, address_id FROM transaction_inputs
                     UNION
                     SELECT tx_id, address_id FROM transaction_outputs
                 ) owned ON owned.address_id = da.id
                 JOIN chain_transactions ct ON ct.id = owned.tx_id
                 WHERE daa.asset_id = 'bitcoin'
                   AND ct.asset_id = 'bitcoin'
                   AND ct.status = 'confirmed'
                 ORDER BY da.account_id",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare Bitcoin history repair eligibility query: {err}"
                ))
            })?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to query Bitcoin history repair eligibility: {err}"
                ))
            })?;
        rows.map(|row| {
            let raw = row.map_err(|err| {
                DbError::new(format!(
                    "Failed to read Bitcoin history repair account ID: {err}"
                ))
            })?;
            DigitalAssetAccountId::from_str(&raw).map_err(|err| {
                DbError::new(format!(
                    "Invalid Bitcoin history repair account ID {raw}: {err}"
                ))
            })
        })
        .collect()
    })
}

pub(crate) fn load_unverified_bitcoin_history_repair_account_ids(
    user_id: UserId,
) -> Result<Vec<DigitalAssetAccountId>, DbError> {
    let eligible = load_eligible_bitcoin_history_repair_account_ids(user_id)?;
    super::user_db::with_user_db(user_id, |conn| {
        eligible
            .into_iter()
            .filter_map(|account_id| {
                match crate::db::account_transactions::bitcoin_account_has_complete_history_proof_for_repair(conn, account_id) {
                    Ok(true) => None,
                    Ok(false) => Some(Ok(account_id)),
                    Err(err) => Some(Err(err)),
                }
            })
            .collect()
    })
}

pub(crate) fn bitcoin_history_repair_owns_account(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    if load_user_data_repair_status(user_id, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)?
        != Some(UserDataRepairStatus::Pending)
    {
        return Ok(false);
    }
    Ok(load_unverified_bitcoin_history_repair_account_ids(user_id)?.contains(&account_id))
}

pub(crate) fn complete_bitcoin_history_full_resync_if_satisfied(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    match load_user_data_repair_status(user_id, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)? {
        Some(UserDataRepairStatus::Completed) => return Ok(true),
        Some(UserDataRepairStatus::Pending) => {}
        None => return Ok(false),
    }
    if !load_unverified_bitcoin_history_repair_account_ids(user_id)?.is_empty() {
        return Ok(false);
    }
    super::user_db::with_user_db_mut(user_id, |conn| {
        mark_user_data_repair_completed_conn(conn, BITCOIN_HISTORY_FULL_RESYNC_REPAIR, now)
    })?;
    Ok(true)
}

fn load_bitcoin_account_ids_conn(
    conn: &rusqlite::Connection,
) -> Result<Vec<DigitalAssetAccountId>, DbError> {
    let mut stmt = conn
        .prepare("SELECT id FROM digital_asset_accounts WHERE asset_id = 'bitcoin' ORDER BY id")
        .map_err(|err| {
            DbError::new(format!("Failed to prepare bitcoin account id query: {err}"))
        })?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| DbError::new(format!("Failed to query bitcoin account ids: {err}")))?;

    let mut account_ids = Vec::new();
    for row in rows {
        let raw =
            row.map_err(|err| DbError::new(format!("Failed to read bitcoin account id: {err}")))?;
        let account_id = DigitalAssetAccountId::from_str(&raw).map_err(|err| {
            DbError::new(format!(
                "Invalid digital_asset_accounts.id value {raw}: {err}"
            ))
        })?;
        account_ids.push(account_id);
    }

    Ok(account_ids)
}

fn load_native_account_ids_conn(
    conn: &rusqlite::Connection,
) -> Result<Vec<DigitalAssetAccountId>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM digital_asset_accounts
             WHERE asset_id IN ('bitcoin', 'ethereum')
             ORDER BY id",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare native account id query: {err}")))?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| DbError::new(format!("Failed to query native account ids: {err}")))?;

    let mut account_ids = Vec::new();
    for row in rows {
        let raw =
            row.map_err(|err| DbError::new(format!("Failed to read native account id: {err}")))?;
        let account_id = DigitalAssetAccountId::from_str(&raw).map_err(|err| {
            DbError::new(format!(
                "Invalid digital_asset_accounts.id value {raw}: {err}"
            ))
        })?;
        account_ids.push(account_id);
    }

    Ok(account_ids)
}

pub(crate) fn run_pending_user_data_repairs_conn(
    conn: &mut rusqlite::Connection,
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        BTC_LEDGER_VALUE_EXCLUDES_FEE_REPAIR,
        now,
        repair_btc_ledger_value_excludes_fee_conn,
    )?;
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        BTC_LEDGER_SELF_TRANSFER_DIRECTION_REPAIR,
        now,
        repair_btc_ledger_value_excludes_fee_conn,
    )?;
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        NATIVE_LEDGER_BALANCE_DELTA_REPAIR,
        now,
        repair_native_ledger_balance_delta_conn,
    )?;
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        ETHERSCAN_PROVIDER_TRANSFER_KEY_REPAIR,
        now,
        repair_etherscan_provider_transfer_keys_conn,
    )?;
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        FAILED_TRANSACTION_ACCOUNTING_REPAIR,
        now,
        repair_failed_transaction_accounting_conn,
    )?;
    run_registered_user_data_repair_conn(
        conn,
        user_id,
        ACCOUNT_ADMISSION_ORDER_REPAIR,
        now,
        backfill_account_admission_conn,
    )
}

fn elapsed_millis(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn run_registered_user_data_repair_conn(
    conn: &mut rusqlite::Connection,
    user_id: UserId,
    repair_key: &str,
    now: DateTime<Utc>,
    repair: fn(&mut rusqlite::Connection, DateTime<Utc>) -> Result<(), DbError>,
) -> Result<(), DbError> {
    register_user_data_repair_conn(conn, repair_key, now)?;
    if load_user_data_repair_status_conn(conn, repair_key)? == Some(UserDataRepairStatus::Completed)
    {
        return Ok(());
    }

    tracing::info!(user_id = %user_id, repair_key, "user data repair started");
    let started_at = Instant::now();
    match repair(conn, now)
        .and_then(|()| mark_user_data_repair_completed_conn(conn, repair_key, now))
    {
        Ok(()) => {
            tracing::info!(
                user_id = %user_id,
                repair_key,
                duration_ms = elapsed_millis(started_at),
                "user data repair completed"
            );
            Ok(())
        }
        Err(err) => {
            let repair_error = truncate_repair_error(&err.to_string());
            let _ = record_user_data_repair_failure_conn(conn, repair_key, now, &repair_error);
            tracing::error!(
                user_id = %user_id,
                repair_key,
                error = %repair_error,
                duration_ms = elapsed_millis(started_at),
                "user data repair failed"
            );
            Err(err)
        }
    }
}

fn repair_btc_ledger_value_excludes_fee_conn(
    conn: &mut rusqlite::Connection,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    for account_id in load_bitcoin_account_ids_conn(conn)? {
        rebuild_account_transaction_ledger_conn(conn, account_id, now)?;
    }
    Ok(())
}

fn repair_native_ledger_balance_delta_conn(
    conn: &mut rusqlite::Connection,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    for account_id in load_native_account_ids_conn(conn)? {
        rebuild_account_transaction_ledger_conn(conn, account_id, now)?;
    }
    Ok(())
}

fn repair_failed_transaction_accounting_conn(
    conn: &mut rusqlite::Connection,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    // Do not expose parser, amount, address, or SQLite trigger details through repair logs.
    let repair_error = |_| DbError::new("Failed to repair stored transaction accounting");
    let mut normal_heads =
        load_all_current_raw_etherscan_normal_transaction_heads_conn(conn).map_err(repair_error)?;
    let mut internal_heads = load_all_current_raw_etherscan_internal_transaction_heads_conn(conn)
        .map_err(repair_error)?;
    // Same newest-head ordering as live retained-evidence replay and transfer-key repair.
    normal_heads.sort_by_key(|head| (head.created_at, head.raw_version_id.to_string()));
    internal_heads.sort_by_key(|head| (head.created_at, head.raw_version_id.to_string()));
    let mut replay = Vec::new();
    for network in [Network::Mainnet, Network::Testnet] {
        let normals = normal_heads
            .iter()
            .filter(|head| head.network == network)
            .map(|head| (head.tx_hash.as_str(), head))
            .collect::<BTreeMap<_, _>>();
        let internals = internal_heads
            .iter()
            .filter(|head| head.network == network)
            .map(|head| ((head.tx_hash.as_str(), head.trace_id.as_str()), head))
            .collect::<BTreeMap<_, _>>();
        let normal_transactions = normals.values().map(|head| {
            crate::tasks::raw_ingestion_executor::parse_persisted_raw_etherscan_normal_transaction(
                head.raw_version_id, &head.tx_hash, &head.payload_bytes,
            ).map_err(|_| DbError::new("Failed to parse stored transaction accounting evidence"))
        }).collect::<Result<Vec<_>, _>>()?;
        let internal_transactions = internals.values().map(|head| {
            crate::tasks::raw_ingestion_executor::parse_persisted_raw_etherscan_internal_transaction(
                head.raw_version_id, &head.tx_hash, &head.trace_id, &head.payload_bytes,
            ).map_err(|_| DbError::new("Failed to parse stored transaction accounting evidence"))
        }).collect::<Result<Vec<_>, _>>()?;
        let mut records =
            crate::tasks::map_etherscan_transactions(normal_transactions, internal_transactions)
                .map_err(|_| {
                    DbError::new("Failed to map stored transaction accounting evidence")
                })?;
        let source_rows = {
            let mut statement = conn
                .prepare(
                    "SELECT tx_hash, status, created_at, updated_at FROM chain_transactions
                 WHERE asset_id = 'ethereum' AND network = ?1",
                )
                .map_err(|_| {
                    DbError::new("Failed to load stored transaction accounting history")
                })?;
            statement
                .query_map([network.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ),
                    ))
                })
                .map_err(|_| DbError::new("Failed to load stored transaction accounting history"))?
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map_err(|_| DbError::new("Failed to load stored transaction accounting history"))?
        };
        // Correct stored history only. Raw-only observations belong to normal sync/replay.
        records.retain(|record| source_rows.contains_key(record.tx_hash.as_str()));
        let successful_hashes = records
            .iter()
            .filter(|record| {
                record.status == crate::transactions::ChainTransactionStatus::Confirmed
            })
            .map(|record| record.tx_hash.as_str())
            .collect::<std::collections::HashSet<_>>();
        for (hash, (status, _, _)) in &source_rows {
            if status == "failed"
                && !normals.contains_key(hash.as_str())
                && !successful_hashes.contains(hash.as_str())
            {
                return Err(DbError::new(
                    "Failed transaction repair requires recovery of source history",
                ));
            }
        }
        // Do not turn an ambiguous old success into an unrepairable failure on retry.
        if records.iter().any(|record| {
            record.status == crate::transactions::ChainTransactionStatus::Failed
                && !normals.contains_key(record.tx_hash.as_str())
        }) {
            return Err(DbError::new(
                "Failed transaction repair requires recovery of source history",
            ));
        }
        replay.push((network, records, source_rows));
    }

    // Validate both networks before changing any derived history. Each replacement and
    // its source timestamps commit together; completed replacements are safe to replay.
    for (network, records, source_rows) in replay {
        for record in records {
            let tx = conn
                .transaction()
                .map_err(|_| DbError::new("Failed to start transaction accounting repair"))?;
            let (_, created_at, updated_at) = source_rows
                .get(record.tx_hash.as_str())
                .ok_or_else(|| DbError::new("Failed to load transaction accounting timestamps"))?;
            // Internal evidence cannot reconstruct the retained normal sender's fee/nonce.
            // Keep known metadata only where replay has no normal transfer of its own.
            let retained_normal_metadata = if record
                .transfers
                .iter()
                .any(|transfer| transfer.provider_transfer_key.as_str() == "normal")
            {
                None
            } else {
                tx.query_row(
                    "SELECT ct.block_height, ct.block_hash, ct.block_time,
                            ct.fee_amount_hi, ct.fee_amount_lo, ct.nonce
                     FROM chain_transactions ct
                     WHERE ct.asset_id = 'ethereum' AND ct.network = ?1 AND ct.tx_hash = ?2
                       AND EXISTS (SELECT 1 FROM account_transfers t
                                   WHERE t.chain_transaction_id = ct.id
                                     AND t.provider_transfer_key IN ('normal', 'legacy:0')
                                     AND t.transfer_kind = 'normal' AND t.from_address IS NOT NULL)",
                    params![network.as_str(), record.tx_hash.as_str()],
                    |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?, row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?, row.get::<_, Option<i64>>(5)?)),
                ).optional().map_err(|_| DbError::new("Failed to load retained transaction accounting metadata"))?
            };
            crate::db::transaction_sync::reconcile_account_transaction_tx(
                &tx,
                SyncedAssetId::Ethereum,
                network,
                &record,
                updated_at,
            )
            .map_err(repair_error)?;
            if let Some((height, hash, time, fee_hi, fee_lo, nonce)) = retained_normal_metadata {
                tx.execute(
                    "UPDATE chain_transactions SET
                         block_height = COALESCE(block_height, ?1),
                         block_hash = COALESCE(block_hash, ?2),
                         block_time = COALESCE(block_time, ?3),
                         fee_amount_hi = COALESCE(fee_amount_hi, ?4),
                         fee_amount_lo = COALESCE(fee_amount_lo, ?5),
                         nonce = COALESCE(nonce, ?6)
                     WHERE asset_id = 'ethereum' AND network = ?7 AND tx_hash = ?8",
                    params![
                        height,
                        hash,
                        time,
                        fee_hi,
                        fee_lo,
                        nonce,
                        network.as_str(),
                        record.tx_hash.as_str()
                    ],
                )
                .map_err(|_| {
                    DbError::new("Failed to preserve retained transaction accounting metadata")
                })?;
            }
            tx.execute(
                "UPDATE chain_transactions SET created_at = ?1, updated_at = ?2
                     WHERE asset_id = 'ethereum' AND network = ?3 AND tx_hash = ?4",
                params![
                    created_at,
                    updated_at,
                    network.as_str(),
                    record.tx_hash.as_str()
                ],
            )
            .map_err(|_| DbError::new("Failed to preserve transaction accounting timestamps"))?;
            tx.commit()
                .map_err(|_| DbError::new("Failed to commit transaction accounting repair"))?;
        }
    }
    // Includes native accounts that lost their final transfer during replay.
    repair_native_ledger_balance_delta_conn(conn, now).map_err(repair_error)
}

fn head_is_newer(
    created_at: DateTime<Utc>,
    raw_version_id: &str,
    current_created_at: DateTime<Utc>,
    current_raw_version_id: &str,
) -> bool {
    created_at > current_created_at
        || (created_at == current_created_at && raw_version_id > current_raw_version_id)
}

fn repair_etherscan_provider_transfer_keys_conn(
    conn: &mut rusqlite::Connection,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let normal_heads = load_all_current_raw_etherscan_normal_transaction_heads_conn(conn)?;
    let internal_heads = load_all_current_raw_etherscan_internal_transaction_heads_conn(conn)?;

    for network in [Network::Mainnet, Network::Testnet] {
        let mut normal_by_hash =
            BTreeMap::<String, &AllCurrentRawEtherscanNormalTransactionHeadRow>::new();
        for head in normal_heads.iter().filter(|head| head.network == network) {
            let key = head.tx_hash.as_str().to_string();
            let replace = normal_by_hash.get(&key).is_none_or(|current| {
                head_is_newer(
                    head.created_at,
                    &head.raw_version_id.to_string(),
                    current.created_at,
                    &current.raw_version_id.to_string(),
                )
            });
            if replace {
                normal_by_hash.insert(key, head);
            }
        }

        let mut internal_by_identity =
            BTreeMap::<(String, String), &AllCurrentRawEtherscanInternalTransactionHeadRow>::new();
        for head in internal_heads.iter().filter(|head| head.network == network) {
            let key = (
                head.tx_hash.as_str().to_string(),
                head.trace_id.as_str().to_string(),
            );
            let replace = internal_by_identity.get(&key).is_none_or(|current| {
                head_is_newer(
                    head.created_at,
                    &head.raw_version_id.to_string(),
                    current.created_at,
                    &current.raw_version_id.to_string(),
                )
            });
            if replace {
                internal_by_identity.insert(key, head);
            }
        }

        let raw_head_count = normal_by_hash
            .len()
            .saturating_add(internal_by_identity.len());
        if raw_head_count == 0 {
            let existing_transfer_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM account_transfers
                     WHERE asset_id = 'ethereum' AND network = ?1",
                    [network.as_str()],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    DbError::new(format!("Failed to count Ethereum transfers: {err}"))
                })?;
            if existing_transfer_count > 0 {
                conn.execute(
                    "UPDATE transaction_sync_state
                     SET etherscan_backfill_start_block = NULL,
                         etherscan_backfill_end_block = NULL,
                         etherscan_history_status = NULL,
                         updated_at = ?2
                     WHERE address_id IN (
                         SELECT da.id
                         FROM digital_asset_addresses da
                         JOIN digital_asset_accounts a ON a.id = da.account_id
                         WHERE a.asset_id = 'ethereum' AND a.network = ?1
                     )",
                    params![network.as_str(), now.to_rfc3339()],
                )
                .map_err(|err| {
                    DbError::new(format!("Failed to reset Etherscan refetch state: {err}"))
                })?;
            }
            continue;
        }

        let normal_transactions = normal_by_hash
            .into_values()
            .map(|head| {
                crate::tasks::raw_ingestion_executor::parse_persisted_raw_etherscan_normal_transaction(
                    head.raw_version_id,
                    &head.tx_hash,
                    &head.payload_bytes,
                )
                .map_err(|_| {
                    DbError::new(format!(
                        "Failed to parse raw Etherscan normal version {}",
                        head.raw_version_id,
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let internal_transactions = internal_by_identity
            .into_values()
            .map(|head| {
                crate::tasks::raw_ingestion_executor::parse_persisted_raw_etherscan_internal_transaction(
                    head.raw_version_id,
                    &head.tx_hash,
                    &head.trace_id,
                    &head.payload_bytes,
                )
                .map_err(|_| {
                    DbError::new(format!(
                        "Failed to parse raw Etherscan internal version {}",
                        head.raw_version_id,
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let records =
            crate::tasks::map_etherscan_transactions(normal_transactions, internal_transactions)
                .map_err(|_| DbError::new("Failed to map current raw Etherscan heads"))?;

        conn.execute(
            "DELETE FROM account_transfers WHERE asset_id = 'ethereum' AND network = ?1",
            [network.as_str()],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to clear Ethereum transfers for repair: {err}"
            ))
        })?;
        reconcile_account_transactions_conn(conn, SyncedAssetId::Ethereum, network, &records, now)?;

        let chain_transaction_ids = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM chain_transactions
                     WHERE asset_id = 'ethereum' AND network = ?1 ORDER BY id",
                )
                .map_err(|err| {
                    DbError::new(format!("Failed to prepare repair cleanup IDs: {err}"))
                })?;
            stmt.query_map([network.as_str()], |row| row.get::<_, String>(0))
                .map_err(|err| DbError::new(format!("Failed to query repair cleanup IDs: {err}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| DbError::new(format!("Failed to read repair cleanup IDs: {err}")))?
        };
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start Etherscan repair cleanup: {err}"))
        })?;
        begin_chain_cleanup_scope(&tx)?;
        for chain_transaction_id in &chain_transaction_ids {
            mark_chain_cleanup_candidate(&tx, chain_transaction_id)?;
        }
        execute_chain_cleanup_for_marked_candidates(&tx)?;
        tx.commit().map_err(|err| {
            DbError::new(format!("Failed to commit Etherscan repair cleanup: {err}"))
        })?;

        let account_ids = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM digital_asset_accounts
                     WHERE asset_id = 'ethereum' AND network = ?1 ORDER BY id",
                )
                .map_err(|err| {
                    DbError::new(format!("Failed to prepare Ethereum accounts: {err}"))
                })?;
            stmt.query_map([network.as_str()], |row| row.get::<_, String>(0))
                .map_err(|err| DbError::new(format!("Failed to query Ethereum accounts: {err}")))?
                .map(|row| {
                    let raw = row.map_err(|err| {
                        DbError::new(format!("Failed to read Ethereum account: {err}"))
                    })?;
                    DigitalAssetAccountId::from_str(&raw)
                        .map_err(|err| DbError::new(format!("Invalid Ethereum account ID: {err}")))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        for account_id in account_ids {
            rebuild_account_transaction_ledger_conn(conn, account_id, now)?;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{
        AddressSyncSuccess, MempoolHistoryProof, SyncTransactionInputRecord,
        SyncTransactionOutputRecord, SyncTransactionRecord, acquire_test_runtime,
        add_bitcoin_address, create_eth_wallet_account_fixture, initialize_user_db_for_test,
        mark_account_integration_sync_started, mark_address_sync_completed_success,
        mark_address_sync_started, rebuild_account_transaction_ledger,
        reconcile_address_transactions, with_user_db, with_user_db_mut,
    };
    use crate::ethereum::{EthAddress, RawEthAddress};
    use crate::models::{UserId, parse_datetime};
    use crate::transactions::{
        ChainTipHeight, ChainTransactionStatus, SyncIntegrationId, TrackedAddress,
        TransactionCount, TransactionSyncRunId, TxHash,
    };
    use crate::wallets::{
        BtcAddress, Label, Network, RawBtcAddress, SyncedAssetId, WALLET_LABEL_MAX_LENGTH,
    };
    use chrono::{DateTime, Duration, Utc};
    use rusqlite::params;

    fn dt(s: &str) -> DateTime<Utc> {
        parse_datetime(s).expect("valid test datetime")
    }

    fn parse_btc_address(value: &str) -> BtcAddress {
        let raw = RawBtcAddress::new(value.to_string());
        BtcAddress::parse(&raw, Network::Mainnet).expect("test btc address should parse")
    }

    fn parse_wallet_label(value: &str) -> Label {
        Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("test label should parse")
    }

    fn parse_eth_address(value: &str) -> EthAddress {
        let raw = RawEthAddress::new(value.to_string());
        EthAddress::parse(&raw).expect("test Ethereum address should parse")
    }

    fn raw_normal_payload(tx_hash: &str, to: &str, value: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "hash": format!("0x{tx_hash}"),
            "blockNumber": "10",
            "timeStamp": "1700000000",
            "from": "0x1111111111111111111111111111111111111111",
            "to": to,
            "value": value,
            "gasPrice": "1",
            "gasUsed": "21000",
            "isError": "0",
            "txreceiptStatus": "1",
            "nonce": "1"
        }))
        .expect("normal raw JSON should serialize")
    }

    fn raw_internal_payload(tx_hash: &str, trace_id: &str, to: &str, value: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "hash": format!("0x{tx_hash}"),
            "blockNumber": "10",
            "timeStamp": "1700000000",
            "from": "0x1111111111111111111111111111111111111111",
            "to": to,
            "value": value,
            "isError": "0",
            "type": "call",
            "traceId": trace_id
        }))
        .expect("internal raw JSON should serialize")
    }

    fn source_connection_id(
        conn: &rusqlite::Connection,
        address_id: crate::wallets::DigitalAssetAddressId,
    ) -> String {
        conn.query_row(
            "SELECT id FROM source_connections
             WHERE integration = 'etherscan'
               AND current_digital_asset_address_id = ?1",
            [address_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .expect("Ethereum address should have an active Etherscan source connection")
    }

    fn seed_internal_head(
        conn: &rusqlite::Connection,
        source_connection_id: &str,
        network: Network,
        tx_hash: &str,
        trace_id: &str,
        payload_bytes: Vec<u8>,
        created_at: DateTime<Utc>,
    ) {
        conn.execute(
            "INSERT INTO raw_etherscan_internal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash, trace_id,
                  payload_hash_sha256_hex, payload_bytes, first_observed_at,
                  supersedes_raw_version_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?9)",
            params![
                ulid::Ulid::new().to_string(),
                source_connection_id,
                if network == Network::Mainnet {
                    1_i64
                } else {
                    11_155_111_i64
                },
                network.as_str(),
                tx_hash,
                trace_id,
                "a".repeat(64),
                payload_bytes,
                created_at.to_rfc3339(),
            ],
        )
        .expect("internal raw head should insert");
    }

    fn seed_normal_head(
        conn: &rusqlite::Connection,
        source_connection_id: &str,
        network: Network,
        tx_hash: &str,
        to: &str,
        value: &str,
        created_at: DateTime<Utc>,
    ) {
        conn.execute(
            "INSERT INTO raw_etherscan_normal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash,
                  payload_hash_sha256_hex, payload_bytes, first_observed_at,
                  supersedes_raw_version_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?8)",
            params![
                ulid::Ulid::new().to_string(),
                source_connection_id,
                if network == Network::Mainnet {
                    1_i64
                } else {
                    11_155_111_i64
                },
                network.as_str(),
                tx_hash,
                "b".repeat(64),
                raw_normal_payload(tx_hash, to, value),
                created_at.to_rfc3339(),
            ],
        )
        .expect("normal raw head should insert");
    }

    #[derive(Debug, PartialEq, Eq)]
    struct EtherscanRepairSnapshot {
        provider_keys: Vec<String>,
        values: Vec<i64>,
        orphan_count: i64,
        ledger_values: Vec<i64>,
    }

    fn load_etherscan_repair_snapshot(
        conn: &rusqlite::Connection,
        orphan_hash: &str,
        repaired_hash: &str,
    ) -> Result<EtherscanRepairSnapshot, DbError> {
        let pairs = {
            let mut stmt = conn
                .prepare(
                    "SELECT provider_transfer_key, value_amount_lo
                     FROM account_transfers
                     WHERE tx_hash = ?1
                     ORDER BY provider_transfer_key",
                )
                .map_err(|err| DbError::new(format!("Failed to prepare repair snapshot: {err}")))?;
            stmt.query_map([repaired_hash], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|err| DbError::new(format!("Failed to query repair snapshot: {err}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| DbError::new(format!("Failed to read repair snapshot: {err}")))?
        };
        let (provider_keys, values) = pairs.into_iter().unzip();
        let orphan_count = conn
            .query_row(
                "SELECT COUNT(*) FROM chain_transactions WHERE tx_hash = ?1",
                [orphan_hash],
                |row| row.get(0),
            )
            .map_err(|err| DbError::new(format!("Failed to count repair orphan: {err}")))?;
        let ledger_values = conn
            .prepare(
                "SELECT value_amount_lo
                 FROM account_transaction_ledger
                 WHERE tx_hash = ?1
                 ORDER BY value_amount_lo",
            )
            .map_err(|err| DbError::new(format!("Failed to prepare repaired ledger query: {err}")))?
            .query_map([repaired_hash], |row| row.get::<_, i64>(0))
            .map_err(|err| DbError::new(format!("Failed to query repaired ledger: {err}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| DbError::new(format!("Failed to read repaired ledger: {err}")))?;
        Ok(EtherscanRepairSnapshot {
            provider_keys,
            values,
            orphan_count,
            ledger_values,
        })
    }

    fn failing_user_data_repair_conn(
        _conn: &mut rusqlite::Connection,
        _now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::new("synthetic repair failure"))
    }

    fn accounting_snapshot(
        conn: &rusqlite::Connection,
        tables: &[&str],
    ) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
        tables
            .iter()
            .map(|table| {
                let mut statement = conn
                    .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
                    .unwrap();
                let columns = statement.column_count();
                statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| row.get(column))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            })
            .collect()
    }

    fn reset_accounting_repair(conn: &rusqlite::Connection) {
        conn.execute(
            "DELETE FROM user_data_repairs WHERE repair_key = 'failed_transaction_accounting_v1'",
            [],
        )
        .unwrap();
    }

    fn seed_accounting_history(
        conn: &mut rusqlite::Connection,
        source: &str,
        network: Network,
        address: &str,
        now: DateTime<Utc>,
    ) {
        let hash = "78".repeat(32);
        seed_normal_head(conn, source, network, &hash, address, "33", now);
        seed_internal_head(
            conn,
            source,
            network,
            &hash,
            "1",
            raw_internal_payload(&hash, "1", address, "11"),
            now,
        );
        repair_etherscan_provider_transfer_keys_conn(conn, now).unwrap();
        conn.execute(
            "UPDATE chain_transactions SET status = 'failed' WHERE tx_hash = ?1",
            [&hash],
        )
        .unwrap();
        conn.execute(
            "UPDATE account_transfers SET value_amount_lo = 999 WHERE tx_hash = ?1",
            [&hash],
        )
        .unwrap();
        conn.execute("UPDATE account_transaction_ledger SET status = 'failed', occurred_at = ?1, balance_delta_lo = 999, closing_balance_lo = 999 WHERE tx_hash = ?2", params![now.to_rfc3339(), hash]).unwrap();
        reset_accounting_repair(conn);
    }

    #[test]
    fn failed_accounting_repair_replays_newest_heads_preserves_history_and_is_idempotent() {
        let _runtime = acquire_test_runtime().unwrap();
        for network in [Network::Mainnet, Network::Testnet] {
            let user = UserId::new();
            initialize_user_db_for_test(user).unwrap();
            let now = dt("2026-09-20T10:00:00Z");
            let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
            let account = crate::db::add_ethereum_address(
                user,
                &address,
                network,
                None,
                Some(&parse_wallet_label("Repair")),
                now,
            )
            .unwrap();
            let competitor = crate::db::add_ethereum_address(
                user,
                &parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D"),
                network,
                None,
                Some(&parse_wallet_label("Competing source")),
                now,
            )
            .unwrap();
            with_user_db_mut(user, |conn| {
                let source = source_connection_id(conn, account.address_id);
                seed_accounting_history(conn, &source, network, &address.checksummed(), now);
                let hash = "78".repeat(32);
                let mut failed: serde_json::Value = serde_json::from_slice(&raw_internal_payload(&hash, "1", &address.checksummed(), "11")).unwrap();
                failed["isError"] = "1".into();
                seed_internal_head(conn, &source, network, &hash, "1", serde_json::to_vec(&failed).unwrap(), now + Duration::seconds(1));
                // An older competing version must not restore the reverted value.
                let competing_source = source_connection_id(conn, competitor.address_id);
                seed_internal_head(conn, &competing_source, network, &hash, "1", raw_internal_payload(&hash, "1", &address.checksummed(), "888"), now - Duration::seconds(1));
                conn.execute("INSERT INTO chain_transactions (id, asset_id, network, tx_hash, status, created_at, updated_at) VALUES ('unrelated', 'ethereum', ?1, ?2, 'confirmed', ?3, ?3)", params![network.as_str(), "89".repeat(32), now.to_rfc3339()]).unwrap();
                let raw_before = accounting_snapshot(conn, &["raw_etherscan_normal_transaction_versions", "raw_etherscan_internal_transaction_versions"]);
                run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
                let row: (String, i64, i64, String, String) = conn.query_row("SELECT status, value_amount_lo, closing_balance_lo, occurred_at, (SELECT updated_at FROM chain_transactions WHERE tx_hash = ?1) FROM account_transaction_ledger WHERE tx_hash = ?1", [&hash], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).unwrap();
                assert_eq!(row, ("confirmed".into(), 33, 33, dt("2023-11-14T22:13:20Z").to_rfc3339(), now.to_rfc3339()));
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM chain_transactions WHERE id = 'unrelated'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
                assert_eq!(raw_before, accounting_snapshot(conn, &["raw_etherscan_normal_transaction_versions", "raw_etherscan_internal_transaction_versions"]));
                assert_eq!(load_user_data_repair_status_conn(conn, "failed_transaction_accounting_v1")?, Some(UserDataRepairStatus::Completed));
                let tables = ["chain_transactions", "account_transfers", "account_transaction_ledger", "user_data_repairs", "wallets", "digital_asset_accounts"];
                let first = accounting_snapshot(conn, &tables);
                run_pending_user_data_repairs_conn(conn, user, now + Duration::days(2))?;
                assert_eq!(first, accounting_snapshot(conn, &tables));
                Ok::<(), DbError>(())
            }).unwrap();
        }
    }

    #[derive(Clone)]
    struct RepairLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for RepairLogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_accounting_repair_failure_is_redacted_atomic_and_retryable() {
        let _runtime = acquire_test_runtime().unwrap();
        for failure in ["missing", "ambiguous", "downgrade", "malformed", "sql"] {
            let user = UserId::new();
            initialize_user_db_for_test(user).unwrap();
            let now = dt("2026-09-20T10:00:00Z");
            let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
            let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
            with_user_db_mut(user, |conn| {
                let source = source_connection_id(conn, account.address_id);
                seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
                match failure {
                    "missing" => { conn.execute("DELETE FROM raw_etherscan_normal_transaction_versions", []).unwrap(); conn.execute("DELETE FROM raw_etherscan_internal_transaction_versions", []).unwrap(); }
                    "ambiguous" | "downgrade" => {
                        conn.execute("DELETE FROM raw_etherscan_normal_transaction_versions", []).unwrap();
                        let mut payload: serde_json::Value = serde_json::from_slice(&raw_internal_payload(&"78".repeat(32), "1", &address.checksummed(), "11")).unwrap();
                        payload["isError"] = "1".into();
                        conn.execute("UPDATE raw_etherscan_internal_transaction_versions SET payload_bytes = ?1", [serde_json::to_vec(&payload).unwrap()]).unwrap();
                        if failure == "downgrade" {
                            conn.execute("UPDATE chain_transactions SET status = 'confirmed'", []).unwrap();
                        }
                    }
                    "malformed" => { conn.execute("UPDATE raw_etherscan_normal_transaction_versions SET payload_bytes = X'736563726574'", []).unwrap(); }
                    _ => conn.execute_batch("CREATE TEMP TRIGGER fail_replay BEFORE UPDATE ON account_transfers BEGIN SELECT RAISE(FAIL, 'secret financial identifier'); END;").unwrap(),
                }
                let before = accounting_snapshot(conn, &["chain_transactions", "account_transfers", "account_transaction_ledger"]);
                let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let writer = RepairLogWriter(logs.clone());
                let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move || writer.clone()).finish();
                let error = tracing::subscriber::with_default(subscriber, || run_pending_user_data_repairs_conn(conn, user, now)).unwrap_err().to_string();
                let logged = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
                assert!(logged.contains("user data repair failed"));
                assert!(!logged.contains("secret"));
                assert!(!logged.contains(&address.checksummed()));
                assert!(!logged.contains(&"78".repeat(32)));
                assert!(!error.contains("secret"));
                assert!(!error.contains(&address.checksummed()));
                assert!(!error.contains(&"78".repeat(32)));
                assert_eq!(before, accounting_snapshot(conn, &["chain_transactions", "account_transfers", "account_transaction_ledger"]));
                assert_eq!(load_user_data_repair_status_conn(conn, "failed_transaction_accounting_v1")?, Some(UserDataRepairStatus::Pending));
                let stored: String = conn.query_row("SELECT last_error FROM user_data_repairs WHERE repair_key = 'failed_transaction_accounting_v1'", [], |row| row.get(0)).unwrap();
                assert_eq!(stored, error);
                if matches!(failure, "missing" | "ambiguous" | "downgrade") { assert_eq!(error, "Failed transaction repair requires recovery of source history"); }
                conn.execute_batch("DROP TRIGGER IF EXISTS fail_replay").unwrap();
                conn.execute("DELETE FROM raw_etherscan_normal_transaction_versions", []).unwrap();
                seed_normal_head(conn, &source, Network::Mainnet, &"78".repeat(32), &address.checksummed(), "33", now);
                run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
                assert_eq!(load_user_data_repair_status_conn(conn, "failed_transaction_accounting_v1")?, Some(UserDataRepairStatus::Completed));
                Ok::<(), DbError>(())
            }).unwrap();
        }
    }

    #[test]
    fn failed_accounting_repair_missing_evidence_blocks_database_open() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        with_user_db_mut(user, |conn| {
            conn.execute("INSERT INTO chain_transactions (id, asset_id, network, tx_hash, status, created_at, updated_at) VALUES ('missing', 'ethereum', 'mainnet', ?1, 'failed', ?2, ?2)", params!["78".repeat(32), dt("2026-09-20T10:00:00Z").to_rfc3339()]).unwrap();
            reset_accounting_repair(conn);
            Ok::<(), DbError>(())
        }).unwrap();
        crate::db::user_db::close_user_db(user).unwrap();
        let error = crate::db::user_db::initialize_user_db(
            user,
            crate::db::encryption::UserDbOpenMode::PlaintextTest,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed transaction repair requires recovery of source history"
        );
    }

    #[test]
    fn failed_accounting_repair_preserves_fallback_time_and_applies_paid_fee() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            let source = source_connection_id(conn, account.address_id);
            let hash = "78".repeat(32);
            seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
            // Existing funding has no raw coverage and must survive replay.
            conn.execute("INSERT INTO chain_transactions (id, asset_id, network, tx_hash, status, block_time, created_at, updated_at) VALUES ('funding', 'ethereum', 'mainnet', ?1, 'confirmed', ?2, ?2, ?2)", params!["67".repeat(32), (now - Duration::days(1)).to_rfc3339()]).unwrap();
            conn.execute("INSERT INTO account_transfers (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at) VALUES ('funding', 'funding', 'ethereum', 'mainnet', ?1, 0, 'normal', 'normal', ?2, ?3, 0, 50000, ?4, ?4)", params!["67".repeat(32), address.checksummed(), account.address_id.to_string(), now.to_rfc3339()]).unwrap();
            let mut payload: serde_json::Value = serde_json::from_slice(&raw_normal_payload(&hash, "0x1111111111111111111111111111111111111111", "33")).unwrap();
            payload["from"] = address.checksummed().into();
            payload["isError"] = "1".into();
            payload["txreceiptStatus"] = "0".into();
            payload["timeStamp"] = "".into();
            conn.execute("UPDATE raw_etherscan_normal_transaction_versions SET payload_bytes = ?1", [serde_json::to_vec(&payload).unwrap()]).unwrap();
            conn.execute("DELETE FROM raw_etherscan_internal_transaction_versions", []).unwrap();
            conn.execute("DELETE FROM account_transfers WHERE provider_transfer_key = 'internal:1'", []).unwrap();
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
            let row: (String, String, i64, i64, i64, i64) = conn.query_row("SELECT status, occurred_at, value_amount_lo, balance_delta_lo, balance_delta_negative, closing_balance_lo FROM account_transaction_ledger WHERE tx_hash = ?1", [hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
            assert_eq!(row, ("failed".into(), now.to_rfc3339(), 33, 21000, 1, 29000));
            Ok::<(), DbError>(())
        }).unwrap();
    }

    #[test]
    fn failed_accounting_repair_internal_success_resolves_old_failure() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            let source = source_connection_id(conn, account.address_id);
            seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
            conn.execute("DELETE FROM raw_etherscan_normal_transaction_versions", [])
                .unwrap();
            conn.execute(
                "DELETE FROM account_transfers WHERE provider_transfer_key = 'normal'",
                [],
            )
            .unwrap();
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
            let row: (String, i64) = conn
                .query_row(
                    "SELECT status, value_amount_lo FROM account_transaction_ledger",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(row, ("confirmed".into(), 11));
            Ok::<(), DbError>(())
        })
        .unwrap();
    }

    #[test]
    fn failed_accounting_repair_rebuilds_account_losing_final_transfer() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            let source = source_connection_id(conn, account.address_id);
            seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
            let hash = "78".repeat(32);
            let external = "0x1111111111111111111111111111111111111111";
            conn.execute(
                "UPDATE raw_etherscan_normal_transaction_versions SET payload_bytes = ?1",
                [raw_normal_payload(&hash, external, "33")],
            )
            .unwrap();
            conn.execute(
                "DELETE FROM account_transfers WHERE provider_transfer_key = 'normal'",
                [],
            )
            .unwrap();
            let mut payload: serde_json::Value = serde_json::from_slice(&raw_internal_payload(
                &hash,
                "1",
                &address.checksummed(),
                "11",
            ))
            .unwrap();
            payload["isError"] = "1".into();
            conn.execute(
                "UPDATE raw_etherscan_internal_transaction_versions SET payload_bytes = ?1",
                [serde_json::to_vec(&payload).unwrap()],
            )
            .unwrap();
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM account_transaction_ledger WHERE account_id = ?1",
                    [account.account_id.to_string()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM account_transfers", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM raw_etherscan_internal_transaction_versions",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            Ok::<(), DbError>(())
        })
        .unwrap();
    }

    #[test]
    fn failed_accounting_repair_leaves_raw_only_failure_unmaterialized_on_retry() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            let source = source_connection_id(conn, account.address_id);
            seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
            let hash = "01".repeat(32);
            let mut payload: serde_json::Value = serde_json::from_slice(&raw_internal_payload(&hash, "1", &address.checksummed(), "11")).unwrap();
            payload["isError"] = "1".into();
            seed_internal_head(conn, &source, Network::Mainnet, &hash, "1", serde_json::to_vec(&payload).unwrap(), now);
            conn.execute_batch("CREATE TEMP TRIGGER fail_later BEFORE UPDATE ON account_transfers BEGIN SELECT RAISE(FAIL, 'synthetic failure'); END;").unwrap();
            assert!(run_pending_user_data_repairs_conn(conn, user, now).is_err());
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM chain_transactions WHERE tx_hash = ?1", [&hash], |r| r.get::<_, i64>(0)).unwrap(), 0);
            conn.execute_batch("DROP TRIGGER fail_later").unwrap();
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM chain_transactions WHERE tx_hash = ?1", [&hash], |r| r.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM raw_etherscan_internal_transaction_versions WHERE tx_hash = ?1", [&hash], |r| r.get::<_, i64>(0)).unwrap(), 1);
            Ok::<(), DbError>(())
        }).unwrap();
    }

    #[test]
    fn failed_accounting_repair_internal_evidence_preserves_legacy_sender_metadata() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            let source = source_connection_id(conn, account.address_id);
            let hash = "78".repeat(32);
            seed_accounting_history(conn, &source, Network::Mainnet, &address.checksummed(), now);
            conn.execute("INSERT INTO chain_transactions (id, asset_id, network, tx_hash, status, block_time, created_at, updated_at) VALUES ('funding', 'ethereum', 'mainnet', ?1, 'confirmed', ?2, ?2, ?2)", params!["67".repeat(32), (now - Duration::days(1)).to_rfc3339()]).unwrap();
            conn.execute("INSERT INTO account_transfers (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at) VALUES ('funding', 'funding', 'ethereum', 'mainnet', ?1, 0, 'normal', 'normal', ?2, ?3, 0, 50000, ?4, ?4)", params!["67".repeat(32), address.checksummed(), account.address_id.to_string(), now.to_rfc3339()]).unwrap();
            conn.execute("UPDATE account_transfers SET from_address = ?1, from_address_id = ?2, to_address = '0x1111111111111111111111111111111111111111', to_address_id = NULL, value_amount_lo = 33 WHERE tx_hash = ?3 AND provider_transfer_key = 'normal'", params![address.checksummed(), account.address_id.to_string(), hash]).unwrap();
            conn.execute("UPDATE chain_transactions SET block_hash = 'retained-block', block_time = ?1, nonce = 7 WHERE tx_hash = ?2", params![now.to_rfc3339(), hash]).unwrap();
            conn.execute("DELETE FROM raw_etherscan_normal_transaction_versions", []).unwrap();
            conn.execute("UPDATE account_transfers SET provider_transfer_key = 'legacy:0' WHERE tx_hash = ?1 AND provider_transfer_key = 'normal'", [&hash]).unwrap();
            let mut payload: serde_json::Value = serde_json::from_slice(&raw_internal_payload(&hash, "1", &address.checksummed(), "11")).unwrap();
            payload["timeStamp"] = "".into();
            conn.execute("UPDATE raw_etherscan_internal_transaction_versions SET payload_bytes = ?1", [serde_json::to_vec(&payload).unwrap()]).unwrap();
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(1))?;
            let metadata = conn.query_row("SELECT status, fee_amount_hi, fee_amount_lo, nonce, block_height, block_hash, block_time FROM chain_transactions WHERE tx_hash = ?1", [&hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?))).unwrap();
            assert_eq!(metadata, ("confirmed".into(), Some(0), Some(21000), Some(7), Some(10), Some("retained-block".into()), Some(now.to_rfc3339())));
            let ledger: (Option<i64>, i64, i64, i64) = conn.query_row("SELECT fee_amount_lo, balance_delta_lo, balance_delta_negative, closing_balance_lo FROM account_transaction_ledger WHERE tx_hash = ?1", [&hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
            assert_eq!(ledger, (Some(21000), 21022, 1, 28978));
            let tables = ["chain_transactions", "account_transfers", "account_transaction_ledger", "user_data_repairs"];
            let snapshot = accounting_snapshot(conn, &tables);
            run_pending_user_data_repairs_conn(conn, user, now + Duration::days(2))?;
            assert_eq!(snapshot, accounting_snapshot(conn, &tables));
            Ok::<(), DbError>(())
        }).unwrap();
    }

    #[test]
    fn failed_accounting_repair_preserves_legacy_normal_fee_without_raw_history() {
        let _runtime = acquire_test_runtime().unwrap();
        let user = UserId::new();
        initialize_user_db_for_test(user).unwrap();
        let now = dt("2026-09-20T10:00:00Z");
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let account = create_eth_wallet_account_fixture(user, &address, "Repair", now);
        with_user_db_mut(user, |conn| {
            for (id, hash, amount, sender, time) in [
                ("funding", "67".repeat(32), 2000, false, now - Duration::days(1)),
                ("send", "78".repeat(32), 400, true, now),
            ] {
                conn.execute("INSERT INTO chain_transactions (id, asset_id, network, tx_hash, status, block_time, fee_amount_hi, fee_amount_lo, created_at, updated_at) VALUES (?1, 'ethereum', 'mainnet', ?2, 'confirmed', ?3, 0, 1, ?3, ?3)", params![id, hash, time.to_rfc3339()]).unwrap();
                conn.execute("INSERT INTO account_transfers (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address_id, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at) VALUES (?1, ?1, 'ethereum', 'mainnet', ?2, 0, 'legacy:0', 'normal', ?3, ?4, 0, ?5, ?6, ?6)", params![id, hash, sender.then(|| account.address_id.to_string()), (!sender).then(|| account.address_id.to_string()), amount, time.to_rfc3339()]).unwrap();
            }
            reset_accounting_repair(conn);
            run_pending_user_data_repairs_conn(conn, user, now)?;
            let ledger = conn.query_row("SELECT fee_amount_lo, balance_delta_lo, balance_delta_negative, closing_balance_lo FROM account_transaction_ledger WHERE chain_transaction_id = 'send'", [], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))).unwrap();
            assert_eq!(ledger, (Some(1), 401, 1, 1599));
            conn.execute("UPDATE account_transfers SET transfer_kind = 'internal' WHERE id = 'send'", []).unwrap();
            reset_accounting_repair(conn);
            run_pending_user_data_repairs_conn(conn, user, now)?;
            let ledger = conn.query_row("SELECT fee_amount_lo, balance_delta_lo, closing_balance_lo FROM account_transaction_ledger WHERE chain_transaction_id = 'send'", [], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).unwrap();
            assert_eq!(ledger, (None, 400, 1600), "legacy internal participation must not pay normal gas");
            Ok::<(), DbError>(())
        }).unwrap();
    }

    #[derive(Debug, PartialEq, Eq)]
    struct LedgerSnapshot {
        tx_type: String,
        value_amount_lo: i64,
        fee_amount_lo: Option<i64>,
        closing_balance_lo: Option<i64>,
    }

    #[test]
    fn repair_dispatcher_completes_for_user_without_bitcoin_accounts() -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();

        initialize_user_db_for_test(user_id)?;

        with_user_db(user_id, |conn| {
            assert_eq!(
                load_user_data_repair_status_conn(conn, BTC_LEDGER_VALUE_EXCLUDES_FEE_REPAIR)?,
                Some(UserDataRepairStatus::Completed)
            );
            assert_eq!(
                load_user_data_repair_status_conn(conn, BTC_LEDGER_SELF_TRANSFER_DIRECTION_REPAIR)?,
                Some(UserDataRepairStatus::Completed)
            );
            assert_eq!(
                load_user_data_repair_status_conn(conn, NATIVE_LEDGER_BALANCE_DELTA_REPAIR)?,
                Some(UserDataRepairStatus::Completed)
            );
            assert_eq!(
                load_user_data_repair_status_conn(conn, FAILED_TRANSACTION_ACCOUNTING_REPAIR)?,
                Some(UserDataRepairStatus::Completed)
            );
            Ok(())
        })
    }

    #[test]
    fn repair_runner_keeps_status_pending_after_failed_repair() -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;

        let fixed_now = dt("2026-06-12T10:05:00Z");
        let repair_key = "failing_repair_for_status_test";

        let result = with_user_db_mut(user_id, |conn| {
            run_registered_user_data_repair_conn(
                conn,
                user_id,
                repair_key,
                fixed_now,
                failing_user_data_repair_conn,
            )
        });

        assert!(result.is_err());
        with_user_db(user_id, |conn| {
            assert_eq!(
                load_user_data_repair_status_conn(conn, repair_key)?,
                Some(UserDataRepairStatus::Pending)
            );
            Ok::<(), DbError>(())
        })
    }

    #[test]
    fn bitcoin_history_full_resync_eligibility_requires_owned_confirmed_history()
    -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;
        let observed_at = dt("2026-07-24T10:00:00Z");
        let eligible = add_bitcoin_address(
            user_id,
            &parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Eligible repair")),
            observed_at,
        )?;
        let skipped = add_bitcoin_address(
            user_id,
            &parse_btc_address("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"),
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Skipped repair")),
            observed_at,
        )?;
        let owned = TrackedAddress::parse("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
            .expect("owned address should parse");
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[SyncTransactionRecord {
                tx_hash: TxHash::parse(
                    "abababababababababababababababababababababababababababababababab",
                )
                .expect("hash should parse"),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(100),
                block_hash: Some("repair-block".to_string()),
                block_time: Some(observed_at),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(owned),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 50_000,
                }],
            }],
            observed_at,
        )?;

        let accounts = load_eligible_bitcoin_history_repair_account_ids(user_id)?;
        assert_eq!(accounts, vec![eligible.account_id]);
        assert!(!accounts.contains(&skipped.account_id));
        Ok(())
    }

    #[test]
    fn bitcoin_history_full_resync_normal_proofs_release_accounts_and_complete_dynamically()
    -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;
        let observed_at = dt("2026-07-24T10:00:00Z");
        let first_address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let second_address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let first = add_bitcoin_address(
            user_id,
            &parse_btc_address(first_address),
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("First repair")),
            observed_at,
        )?;
        let second = add_bitcoin_address(
            user_id,
            &parse_btc_address(second_address),
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Second repair")),
            observed_at,
        )?;
        for (tx_hash, raw_address) in [
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                first_address,
            ),
            (
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                second_address,
            ),
        ] {
            reconcile_address_transactions(
                user_id,
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                &[SyncTransactionRecord {
                    tx_hash: TxHash::parse(tx_hash).expect("hash should parse"),
                    status: ChainTransactionStatus::Confirmed,
                    block_height: Some(100),
                    block_hash: Some(format!("block-{tx_hash}")),
                    block_time: Some(observed_at),
                    fee_amount: None,
                    inputs: Vec::new(),
                    outputs: vec![SyncTransactionOutputRecord {
                        output_index: 0,
                        raw_address: Some(
                            TrackedAddress::parse(raw_address).expect("address should parse"),
                        ),
                        script_pubkey_hex: "0014deadbeef".to_string(),
                        value_amount: 1,
                    }],
                }],
                observed_at,
            )?;
        }
        let proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(1),
            complete_height: ChainTipHeight::try_new(800_001).expect("height should parse"),
        };
        rebuild_account_transaction_ledger(user_id, first.account_id, observed_at)?;
        rebuild_account_transaction_ledger(user_id, second.account_id, observed_at)?;
        let first_run_id = TransactionSyncRunId::new();
        let second_run_id = TransactionSyncRunId::new();
        let completed_at = observed_at + Duration::seconds(1);
        mark_address_sync_started(user_id, first.address_id, first_run_id, observed_at)?;
        mark_address_sync_started(user_id, second.address_id, second_run_id, observed_at)?;
        mark_account_integration_sync_started(
            user_id,
            first.account_id,
            SyncIntegrationId::Mempool,
            observed_at,
        )?;
        mark_account_integration_sync_started(
            user_id,
            second.account_id,
            SyncIntegrationId::Mempool,
            observed_at,
        )?;
        crate::db::publish_mempool_history_proof(user_id, first.address_id, proof)?;
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: first.address_id,
                run_id: first_run_id,
                started_at: observed_at,
                completed_at,
                last_tip_height: proof.complete_height,
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )?;
        crate::db::refresh_account_integration_sync_state(
            user_id,
            first.account_id,
            SyncIntegrationId::Mempool,
            completed_at,
        )?;

        assert!(!bitcoin_history_repair_owns_account(
            user_id,
            first.account_id
        )?);
        assert!(bitcoin_history_repair_owns_account(
            user_id,
            second.account_id
        )?);
        assert_eq!(
            load_unverified_bitcoin_history_repair_account_ids(user_id)?,
            vec![second.account_id]
        );
        assert!(!complete_bitcoin_history_full_resync_if_satisfied(
            user_id,
            observed_at
        )?);

        crate::db::publish_mempool_history_proof(user_id, second.address_id, proof)?;
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: second.address_id,
                run_id: second_run_id,
                started_at: observed_at,
                completed_at,
                last_tip_height: proof.complete_height,
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )?;
        crate::db::refresh_account_integration_sync_state(
            user_id,
            second.account_id,
            SyncIntegrationId::Mempool,
            completed_at,
        )?;
        assert!(complete_bitcoin_history_full_resync_if_satisfied(
            user_id,
            observed_at
        )?);
        assert_eq!(
            load_user_data_repair_status(user_id, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)?,
            Some(UserDataRepairStatus::Completed)
        );
        Ok(())
    }

    #[test]
    fn bitcoin_history_full_resync_empty_database_completes_without_work() -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;
        let now = dt("2026-07-24T10:00:00Z");

        assert!(complete_bitcoin_history_full_resync_if_satisfied(
            user_id, now
        )?);
        assert_eq!(
            load_user_data_repair_status(user_id, BITCOIN_HISTORY_FULL_RESYNC_REPAIR)?,
            Some(UserDataRepairStatus::Completed)
        );
        Ok(())
    }

    #[test]
    fn btc_repair_rebuilds_existing_bad_ledger_value() -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;

        let seeded_at = dt("2026-06-12T10:00:00Z");
        let fixed_now = dt("2026-06-12T10:05:00Z");
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("BTC Repair Test");
        let add_result = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&label),
            seeded_at,
        )?;

        let owned_tracked =
            TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
        let receive_hash =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("receive hash should parse");
        let send_hash =
            TxHash::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .expect("send hash should parse");
        let records = vec![
            SyncTransactionRecord {
                tx_hash: receive_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(1),
                block_hash: Some("blockhash-1".to_string()),
                block_time: Some(dt("2026-06-12T10:01:00Z")),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(owned_tracked.clone()),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 10_000_000,
                }],
            },
            SyncTransactionRecord {
                tx_hash: send_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(2),
                block_hash: Some("blockhash-2".to_string()),
                block_time: Some(dt("2026-06-12T10:02:00Z")),
                fee_amount: Some(3_172),
                inputs: vec![SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: receive_hash,
                    prev_output_index: 0,
                    prev_address: Some(owned_tracked),
                    value_amount: Some(10_000_000),
                }],
                outputs: Vec::new(),
            },
        ];
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &records,
            seeded_at,
        )?;

        let send_row = with_user_db_mut(user_id, |conn| {
            rebuild_account_transaction_ledger_conn(conn, add_result.account_id, seeded_at)?;

            let rows = conn
                .execute(
                    "UPDATE account_transaction_ledger
                     SET value_amount_lo = ?3,
                         updated_at = ?4
                     WHERE account_id = ?1
                       AND tx_hash = ?2",
                    params![
                        add_result.account_id.to_string(),
                        send_hash.as_str(),
                        10_000_000_i64,
                        seeded_at.to_rfc3339()
                    ],
                )
                .map_err(|err| DbError::new(format!("Failed to corrupt send ledger row: {err}")))?;
            assert_eq!(rows, 1, "send ledger row should be corrupted");

            let rows = conn
                .execute(
                    "UPDATE user_data_repairs
                     SET status = ?2,
                         completed_at = NULL,
                         updated_at = ?3
                     WHERE repair_key = ?1",
                    params![
                        BTC_LEDGER_VALUE_EXCLUDES_FEE_REPAIR,
                        UserDataRepairStatus::Pending.as_str(),
                        seeded_at.to_rfc3339()
                    ],
                )
                .map_err(|err| DbError::new(format!("Failed to reset repair status: {err}")))?;
            assert_eq!(rows, 1, "repair status should be reset");

            run_pending_user_data_repairs_conn(conn, user_id, fixed_now)?;

            conn.query_row(
                "SELECT tx_type, value_amount_lo, fee_amount_lo, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                   AND tx_hash = ?2",
                params![add_result.account_id.to_string(), send_hash.as_str()],
                |row| {
                    Ok(LedgerSnapshot {
                        tx_type: row.get(0)?,
                        value_amount_lo: row.get(1)?,
                        fee_amount_lo: row.get(2)?,
                        closing_balance_lo: row.get(3)?,
                    })
                },
            )
            .map_err(|err| DbError::new(format!("Failed to load repaired send row: {err}")))
        })?;

        assert_eq!(
            send_row,
            LedgerSnapshot {
                tx_type: "send".to_string(),
                value_amount_lo: 9_996_828,
                fee_amount_lo: Some(3_172),
                closing_balance_lo: None,
            }
        );

        with_user_db(user_id, |conn| {
            assert_eq!(
                load_user_data_repair_status_conn(conn, BTC_LEDGER_VALUE_EXCLUDES_FEE_REPAIR)?,
                Some(UserDataRepairStatus::Completed)
            );
            Ok(())
        })
    }

    #[test]
    fn native_balance_delta_repair_backfills_btc_and_eth() -> Result<(), DbError> {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id)?;

        let seeded_at = dt("2026-06-13T09:00:00Z");
        let fixed_now = dt("2026-06-13T09:05:00Z");

        let btc_address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let btc_label = parse_wallet_label("Native Delta BTC Repair");
        let btc = add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&btc_label),
            seeded_at,
        )?;
        let btc_tracked = TrackedAddress::parse(btc_address.canonical())
            .expect("owned tracked address should parse");
        let btc_hash =
            TxHash::parse("4444444444444444444444444444444444444444444444444444444444444444")
                .expect("btc hash should parse");
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[SyncTransactionRecord {
                tx_hash: btc_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(1),
                block_hash: Some("blockhash-btc".to_string()),
                block_time: Some(seeded_at),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(btc_tracked),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 50_000,
                }],
            }],
            seeded_at,
        )?;

        let eth_address = {
            let raw = RawEthAddress::new("0x52908400098527886E0F7030069857D2E4169EE7".to_string());
            EthAddress::parse(&raw).expect("test eth address should parse")
        };
        let eth = create_eth_wallet_account_fixture(
            user_id,
            &eth_address,
            "Native Delta ETH Repair",
            seeded_at,
        );
        let eth_hash = "5555555555555555555555555555555555555555555555555555555555555555";

        with_user_db_mut(user_id, |conn| {
            rebuild_account_transaction_ledger_conn(conn, btc.account_id, seeded_at)?;

            let eth_chain_transaction_id = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                 VALUES (?1, 'ethereum', 'mainnet', ?2, 'confirmed', 1, 'blockhash-eth', ?3, NULL, NULL, NULL, ?3, ?3)",
                params![eth_chain_transaction_id, eth_hash, seeded_at.to_rfc3339()],
            )
            .map_err(|err| DbError::new(format!("Failed to insert eth chain tx fixture: {err}")))?;
            let rows = conn.execute(
                "INSERT INTO account_transfers
                     (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, to_address, from_address_id, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 SELECT ?1, ?2, 'ethereum', 'mainnet', ?3, 0, 'legacy:0', 'normal', '0x0000000000000000000000000000000000000001', ?4, NULL, da.id, 0, 42, ?5, ?5
                   FROM digital_asset_addresses da
                  WHERE da.account_id = ?6
                  LIMIT 1",
                params![
                    ulid::Ulid::new().to_string(),
                    eth_chain_transaction_id,
                    eth_hash,
                    eth_address.checksummed(),
                    seeded_at.to_rfc3339(),
                    eth.account_id.to_string()
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to insert eth transfer fixture: {err}")))?;
            assert_eq!(rows, 1, "eth transfer fixture should insert");
            rebuild_account_transaction_ledger_conn(conn, eth.account_id, seeded_at)?;

            let rows = conn
                .execute(
                    "UPDATE account_transaction_ledger
                    SET balance_delta_hi = 0,
                        balance_delta_lo = 0,
                        balance_delta_negative = 0
                  WHERE tx_hash IN (?1, ?2)",
                    params![btc_hash.as_str(), eth_hash],
                )
                .map_err(|err| DbError::new(format!("Failed to zero delta fixtures: {err}")))?;
            assert_eq!(rows, 2, "both native ledger rows should be zeroed");

            conn.execute(
                "UPDATE user_data_repairs
                    SET status = ?2,
                        completed_at = NULL,
                        updated_at = ?3
                  WHERE repair_key = ?1",
                params![
                    NATIVE_LEDGER_BALANCE_DELTA_REPAIR,
                    UserDataRepairStatus::Pending.as_str(),
                    seeded_at.to_rfc3339()
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to reset native repair status: {err}")))?;

            run_pending_user_data_repairs_conn(conn, user_id, fixed_now)
        })?;

        let zero_delta_count = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT COUNT(*)
                   FROM account_transaction_ledger
                  WHERE status = 'confirmed'
                    AND balance_delta_hi = 0
                    AND balance_delta_lo = 0
                    AND tx_hash IN (?1, ?2)",
                params![btc_hash.as_str(), eth_hash],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| DbError::new(format!("Failed to count zero native deltas: {err}")))
        })?;
        assert_eq!(
            zero_delta_count, 0,
            "repair must backfill all native deltas"
        );
        Ok(())
    }

    #[test]
    fn etherscan_provider_key_repair_rebuilds_collision_cleans_orphan_and_is_idempotent() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let now = dt("2026-07-19T10:00:00Z");
        let address_a = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let address_b = parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D");
        let account_a = create_eth_wallet_account_fixture(user_id, &address_a, "Repair A", now);
        let account_b = create_eth_wallet_account_fixture(user_id, &address_b, "Repair B", now);
        let tx_hash = "3434343434343434343434343434343434343434343434343434343434343434";
        let orphan_hash = "4545454545454545454545454545454545454545454545454545454545454545";

        with_user_db_mut(user_id, |conn| {
            let source_a = source_connection_id(conn, account_a.address_id);
            let source_b = source_connection_id(conn, account_b.address_id);
            seed_normal_head(
                conn,
                &source_a,
                Network::Mainnet,
                tx_hash,
                &address_a.checksummed(),
                "33",
                now,
            );
            seed_internal_head(
                conn,
                &source_a,
                Network::Mainnet,
                tx_hash,
                "1",
                raw_internal_payload(tx_hash, "1", &address_a.checksummed(), "99"),
                now - chrono::Duration::minutes(1),
            );
            seed_internal_head(
                conn,
                &source_a,
                Network::Mainnet,
                tx_hash,
                "1",
                raw_internal_payload(tx_hash, "1", &address_a.checksummed(), "11"),
                now,
            );
            seed_internal_head(
                conn,
                &source_b,
                Network::Mainnet,
                tx_hash,
                "0_1",
                raw_internal_payload(tx_hash, "0_1", &address_b.checksummed(), "22"),
                now,
            );
            conn.execute(
                "INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, created_at, updated_at)
                 VALUES ('stale-chain', 'ethereum', 'mainnet', ?1, 'confirmed', 10, ?2, ?2),
                        ('orphan-chain', 'ethereum', 'mainnet', ?3, 'confirmed', 9, ?2, ?2)",
                params![tx_hash, now.to_rfc3339(), orphan_hash],
            )
            .map_err(|err| DbError::new(format!("Failed to seed chain rows: {err}")))?;
            conn.execute(
                "INSERT INTO account_transfers
                     (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
                      transfer_index, transfer_kind, to_address, to_address_id,
                      value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES ('stale-transfer', 'stale-chain', 'ethereum', 'mainnet', ?1,
                         'legacy:2', 2, 'internal', ?2, ?3, 0, 999, ?4, ?4)",
                params![
                    tx_hash,
                    address_a.checksummed(),
                    account_a.address_id.to_string(),
                    now.to_rfc3339()
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to seed stale transfer: {err}")))?;

            repair_etherscan_provider_transfer_keys_conn(conn, now)?;
            let first = load_etherscan_repair_snapshot(conn, orphan_hash, tx_hash)?;
            repair_etherscan_provider_transfer_keys_conn(conn, now)?;
            let second = load_etherscan_repair_snapshot(conn, orphan_hash, tx_hash)?;
            assert_eq!(first, second);
            assert_eq!(
                first.provider_keys,
                vec!["internal:0_1", "internal:1", "normal"]
            );
            assert_eq!(first.values, vec![22, 11, 33]);
            assert_eq!(first.orphan_count, 0);
            assert_eq!(first.ledger_values, vec![22, 44]);
            Ok::<(), DbError>(())
        })
        .expect("repair should succeed twice");
    }

    #[test]
    fn etherscan_provider_key_repair_partitions_identical_identity_by_network() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let now = dt("2026-07-19T11:00:00Z");
        let main_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let test_address = parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D");
        let main = create_eth_wallet_account_fixture(user_id, &main_address, "Repair Main", now);
        let testnet = crate::db::add_ethereum_address(
            user_id,
            &test_address,
            Network::Testnet,
            None,
            Some(&parse_wallet_label("Repair Testnet")),
            now,
        )
        .expect("testnet Ethereum account should insert");
        let tx_hash = "5656565656565656565656565656565656565656565656565656565656565656";

        with_user_db_mut(user_id, |conn| {
            let main_source = source_connection_id(conn, main.address_id);
            let test_source = source_connection_id(conn, testnet.address_id);
            seed_internal_head(
                conn,
                &main_source,
                Network::Mainnet,
                tx_hash,
                "1",
                raw_internal_payload(tx_hash, "1", &main_address.checksummed(), "11"),
                now,
            );
            seed_internal_head(
                conn,
                &test_source,
                Network::Testnet,
                tx_hash,
                "1",
                raw_internal_payload(tx_hash, "1", &test_address.checksummed(), "22"),
                now,
            );
            repair_etherscan_provider_transfer_keys_conn(conn, now)?;

            let mut stmt = conn
                .prepare(
                    "SELECT network, provider_transfer_key
                     FROM account_transfers
                     WHERE tx_hash = ?1
                     ORDER BY network",
                )
                .map_err(|err| DbError::new(format!("Failed to prepare network query: {err}")))?;
            let rows = stmt
                .query_map([tx_hash], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|err| DbError::new(format!("Failed to query networks: {err}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| DbError::new(format!("Failed to read networks: {err}")))?;
            assert_eq!(
                rows,
                vec![
                    ("mainnet".to_string(), "internal:1".to_string()),
                    ("testnet".to_string(), "internal:1".to_string()),
                ]
            );
            Ok::<(), DbError>(())
        })
        .expect("network-partitioned repair should succeed");
    }

    #[test]
    fn etherscan_provider_key_repair_without_raw_heads_preserves_rows_and_resets_all_addresses() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let now = dt("2026-07-19T12:00:00Z");
        let address_a = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let address_b = parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D");
        let account_a = create_eth_wallet_account_fixture(user_id, &address_a, "Fallback A", now);
        let account_b = create_eth_wallet_account_fixture(user_id, &address_b, "Fallback B", now);
        for address_id in [account_a.address_id, account_b.address_id] {
            crate::db::mark_address_sync_started(
                user_id,
                address_id,
                crate::transactions::TransactionSyncRunId::new(),
                now,
            )
            .expect("sync state should insert");
        }

        with_user_db_mut(user_id, |conn| {
            let tx_hash =
                "6767676767676767676767676767676767676767676767676767676767676767";
            conn.execute(
                "INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, created_at, updated_at)
                 VALUES ('fallback-chain', 'ethereum', 'mainnet', ?1, 'confirmed', 10, ?2, ?2)",
                params![tx_hash, now.to_rfc3339()],
            )
            .map_err(|err| DbError::new(format!("Failed to seed fallback chain row: {err}")))?;
            conn.execute(
                "INSERT INTO account_transfers
                     (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
                      transfer_index, transfer_kind, to_address, to_address_id,
                      value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES ('fallback-transfer', 'fallback-chain', 'ethereum', 'mainnet', ?1,
                         'legacy:0', 0, 'normal', ?2, ?3, 0, 42, ?4, ?4)",
                params![
                    tx_hash,
                    address_a.checksummed(),
                    account_a.address_id.to_string(),
                    now.to_rfc3339()
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to seed fallback transfer: {err}")))?;
            conn.execute(
                "UPDATE transaction_sync_state
                 SET etherscan_backfill_start_block = 1,
                     etherscan_backfill_end_block = 10,
                     etherscan_history_status = 'gap'",
                [],
            )
            .map_err(|err| DbError::new(format!("Failed to seed fallback sync state: {err}")))?;

            repair_etherscan_provider_transfer_keys_conn(conn, now)?;
            let transfer_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM account_transfers WHERE provider_transfer_key = 'legacy:0'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| DbError::new(format!("Failed to count fallback transfer: {err}")))?;
            let reset_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM digital_asset_addresses da
                     JOIN digital_asset_accounts a ON a.id = da.account_id
                     JOIN transaction_sync_state tss ON tss.address_id = da.id
                     WHERE a.asset_id = 'ethereum'
                       AND a.network = 'mainnet'
                       AND tss.etherscan_backfill_start_block IS NULL
                       AND tss.etherscan_backfill_end_block IS NULL
                       AND tss.etherscan_history_status IS NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(|err| DbError::new(format!("Failed to count reset addresses: {err}")))?;
            assert_eq!(transfer_count, 1);
            assert_eq!(reset_count, 2);
            Ok::<(), DbError>(())
        })
        .expect("no-raw fallback should succeed");
    }
}

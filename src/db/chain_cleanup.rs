use super::error::DbError;
use crate::wallets::{DigitalAssetAccountId, WalletId};
use rusqlite::params;

const CANDIDATE_TABLE_NAME: &str = "temp_chain_tx_cleanup_candidates";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainCleanupStats {
    pub(crate) candidate_chain_tx_count: u64,
    pub(crate) deleted_unowned_transfers: u64,
    pub(crate) deleted_orphan_chain_transactions: u64,
}

fn ensure_candidate_table(tx: &rusqlite::Transaction<'_>) -> Result<(), DbError> {
    tx.execute_batch(&format!(
        "CREATE TEMP TABLE IF NOT EXISTS {CANDIDATE_TABLE_NAME} (
             tx_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;"
    ))
    .map_err(|err| DbError::new(format!("Failed to ensure chain cleanup temp table: {err}")))
}

pub(crate) fn begin_chain_cleanup_scope(tx: &rusqlite::Transaction<'_>) -> Result<(), DbError> {
    ensure_candidate_table(tx)?;
    tx.execute(&format!("DELETE FROM {CANDIDATE_TABLE_NAME}"), [])
        .map_err(|err| DbError::new(format!("Failed to reset chain cleanup candidates: {err}")))?;
    Ok(())
}

pub(crate) fn mark_chain_cleanup_candidate(
    tx: &rusqlite::Transaction<'_>,
    chain_tx_id: &str,
) -> Result<(), DbError> {
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO {CANDIDATE_TABLE_NAME} (tx_id)
             VALUES (?1)"
        ),
        params![chain_tx_id],
    )
    .map_err(|err| DbError::new(format!("Failed to mark chain cleanup candidate: {err}")))?;
    Ok(())
}

pub(crate) fn mark_chain_cleanup_candidates_for_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
) -> Result<(), DbError> {
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO {CANDIDATE_TABLE_NAME} (tx_id)
             SELECT candidate.tx_id
             FROM (
                 SELECT ti.tx_id AS tx_id
                   FROM transaction_inputs ti
                   JOIN digital_asset_addresses da ON da.id = ti.address_id
                  WHERE da.account_id = ?1
                 UNION
                 SELECT to2.tx_id AS tx_id
                   FROM transaction_outputs to2
                   JOIN digital_asset_addresses da ON da.id = to2.address_id
                  WHERE da.account_id = ?1
                 UNION
                 SELECT at.chain_transaction_id AS tx_id
                   FROM account_transfers at
                   JOIN digital_asset_addresses da ON da.id = at.from_address_id
                  WHERE da.account_id = ?1
                 UNION
                 SELECT at.chain_transaction_id AS tx_id
                   FROM account_transfers at
                   JOIN digital_asset_addresses da ON da.id = at.to_address_id
                  WHERE da.account_id = ?1
             ) candidate"
        ),
        params![account_id.to_string()],
    )
    .map_err(|err| {
        DbError::new(format!(
            "Failed to mark chain cleanup candidates for account delete: {err}"
        ))
    })?;
    Ok(())
}

pub(crate) fn mark_chain_cleanup_candidates_for_wallet(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
) -> Result<(), DbError> {
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO {CANDIDATE_TABLE_NAME} (tx_id)
             SELECT candidate.tx_id
             FROM (
                 SELECT ti.tx_id AS tx_id
                   FROM transaction_inputs ti
                   JOIN digital_asset_addresses da ON da.id = ti.address_id
                   JOIN digital_asset_accounts a ON a.id = da.account_id
                  WHERE a.wallet_id = ?1
                 UNION
                 SELECT to2.tx_id AS tx_id
                   FROM transaction_outputs to2
                   JOIN digital_asset_addresses da ON da.id = to2.address_id
                   JOIN digital_asset_accounts a ON a.id = da.account_id
                  WHERE a.wallet_id = ?1
                 UNION
                 SELECT at.chain_transaction_id AS tx_id
                   FROM account_transfers at
                   JOIN digital_asset_addresses da ON da.id = at.from_address_id
                   JOIN digital_asset_accounts a ON a.id = da.account_id
                  WHERE a.wallet_id = ?1
                 UNION
                 SELECT at.chain_transaction_id AS tx_id
                   FROM account_transfers at
                   JOIN digital_asset_addresses da ON da.id = at.to_address_id
                   JOIN digital_asset_accounts a ON a.id = da.account_id
                  WHERE a.wallet_id = ?1
             ) candidate"
        ),
        params![wallet_id.to_string()],
    )
    .map_err(|err| {
        DbError::new(format!(
            "Failed to mark chain cleanup candidates for wallet delete: {err}"
        ))
    })?;
    Ok(())
}

pub(crate) fn execute_chain_cleanup_for_marked_candidates(
    tx: &rusqlite::Transaction<'_>,
) -> Result<ChainCleanupStats, DbError> {
    let candidate_chain_tx_count: i64 = tx
        .query_row(
            &format!("SELECT COUNT(*) FROM {CANDIDATE_TABLE_NAME}"),
            [],
            |row| row.get(0),
        )
        .map_err(|err| DbError::new(format!("Failed to count cleanup candidates: {err}")))?;

    let deleted_unowned_transfers = tx
        .execute(
            &format!(
                "DELETE FROM account_transfers
                 WHERE chain_transaction_id IN (
                     SELECT tx_id FROM {CANDIDATE_TABLE_NAME}
                 )
                   AND from_address_id IS NULL
                   AND to_address_id IS NULL"
            ),
            [],
        )
        .map_err(|err| DbError::new(format!("Failed to delete unowned transfer rows: {err}")))?;

    let deleted_orphan_chain_transactions = tx
        .execute(
            &format!(
                "DELETE FROM chain_transactions
                 WHERE id IN (SELECT tx_id FROM {CANDIDATE_TABLE_NAME})
                   AND NOT EXISTS (
                       SELECT 1 FROM transaction_inputs ti
                        WHERE ti.tx_id = chain_transactions.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM transaction_outputs to2
                        WHERE to2.tx_id = chain_transactions.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM account_transfers at2
                        WHERE at2.chain_transaction_id = chain_transactions.id
                   )"
            ),
            [],
        )
        .map_err(|err| {
            DbError::new(format!("Failed to delete orphan chain transactions: {err}"))
        })?;

    Ok(ChainCleanupStats {
        candidate_chain_tx_count: u64::try_from(candidate_chain_tx_count).unwrap_or(0),
        deleted_unowned_transfers: deleted_unowned_transfers as u64,
        deleted_orphan_chain_transactions: deleted_orphan_chain_transactions as u64,
    })
}

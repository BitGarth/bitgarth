use super::super::account_transactions::types::split_unsigned_amount;
use super::super::chain_cleanup::{
    begin_chain_cleanup_scope, execute_chain_cleanup_for_marked_candidates,
    mark_chain_cleanup_candidate,
};
use super::super::error::DbError;
use super::super::user_db::with_user_db_mut;
use super::RECONCILE_LOCK_BATCH_SIZE;
use super::parsers::{parse_account_id, parse_address_id};
use super::types::*;
use crate::amounts::AmountSplitParts;
use crate::amounts::UnsignedAmount;
use crate::models::UserId;
use crate::transactions::{ChainTransactionStatus, TrackedAddress, TransactionCount, TxHash};
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use ulid::Ulid;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingChainTransactionRow {
    status: ChainTransactionStatus,
    block_height: Option<i64>,
    block_hash: Option<String>,
    block_time: Option<String>,
    fee_amount_lo: Option<i64>,
    fee_amount_hi: Option<i64>,
    nonce: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingOwnedInputRow {
    input_index: i64,
    prev_tx_hash: String,
    prev_output_index: i64,
    address_id: String,
    account_id: String,
    value_amount_lo: Option<i64>,
    value_amount_hi: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingOwnedOutputRow {
    output_index: i64,
    address_id: String,
    account_id: String,
    raw_address: Option<String>,
    script_pubkey_hex: String,
    value_amount_lo: i64,
    value_amount_hi: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingTransactionSnapshot {
    chain: ExistingChainTransactionRow,
    inputs: Vec<ExistingOwnedInputRow>,
    outputs: Vec<ExistingOwnedOutputRow>,
}

impl ExistingTransactionSnapshot {
    fn add_owned_targets(&self, targets: &mut CoverageInvalidationTargets) -> Result<(), DbError> {
        for (address_id, account_id) in self
            .inputs
            .iter()
            .map(|row| (&row.address_id, &row.account_id))
            .chain(
                self.outputs
                    .iter()
                    .map(|row| (&row.address_id, &row.account_id)),
            )
        {
            targets.address_ids.insert(parse_address_id(address_id)?);
            targets.account_ids.insert(parse_account_id(account_id)?);
        }
        Ok(())
    }
}

fn load_existing_chain_transaction_row(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    tx_hash: &TxHash,
) -> Result<Option<ExistingChainTransactionRow>, DbError> {
    tx.query_row(
        "SELECT status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce
         FROM chain_transactions
         WHERE asset_id = ?1
           AND network = ?2
           AND tx_hash = ?3",
        params![asset_id.as_str(), network.as_str(), tx_hash.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        },
    )
    .optional()
    .map_err(|err| {
        DbError::new(format!(
            "Failed to query existing chain transaction row: {err}"
        ))
    })?
    .map(
        |(
            status_raw,
            block_height,
            block_hash,
            block_time,
            fee_amount_lo,
            fee_amount_hi,
            nonce,
        )| {
            let status = ChainTransactionStatus::from_db_value(&status_raw).ok_or_else(|| {
                DbError::new(format!(
                    "Invalid chain transaction status in DB: {status_raw}"
                ))
            })?;
            Ok(ExistingChainTransactionRow {
                status,
                block_height,
                block_hash,
                block_time,
                fee_amount_lo,
                fee_amount_hi,
                nonce,
            })
        },
    )
    .transpose()
}

fn load_existing_transaction_snapshot(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    tx_hash: &TxHash,
) -> Result<Option<ExistingTransactionSnapshot>, DbError> {
    let Some(chain) = load_existing_chain_transaction_row(tx, asset_id, network, tx_hash)? else {
        return Ok(None);
    };
    let chain_tx_id = resolve_chain_transaction_id(tx, asset_id, network, tx_hash)?;

    let mut input_statement = tx
        .prepare(
            "SELECT ti.input_index, ti.prev_tx_hash, ti.prev_output_index,
                    ti.address_id, da.account_id, ti.value_amount_lo, ti.value_amount_hi
             FROM transaction_inputs ti
             JOIN digital_asset_addresses da ON da.id = ti.address_id
             WHERE ti.tx_id = ?1
             ORDER BY ti.input_index",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare owned input snapshot: {err}")))?;
    let inputs = input_statement
        .query_map([chain_tx_id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query owned input snapshot: {err}")))?
        .map(|row| {
            let (
                input_index,
                prev_tx_hash,
                prev_output_index,
                address_id,
                account_id,
                value_amount_lo,
                value_amount_hi,
            ) = row.map_err(|err| {
                DbError::new(format!("Failed to read owned input snapshot: {err}"))
            })?;
            Ok(ExistingOwnedInputRow {
                input_index,
                prev_tx_hash,
                prev_output_index,
                address_id,
                account_id,
                value_amount_lo,
                value_amount_hi,
            })
        })
        .collect::<Result<Vec<_>, DbError>>()?;
    drop(input_statement);

    let mut output_statement = tx
        .prepare(
            "SELECT txo.output_index, txo.address_id, da.account_id, txo.raw_address,
                    txo.script_pubkey_hex, txo.value_amount_lo, txo.value_amount_hi
             FROM transaction_outputs txo
             JOIN digital_asset_addresses da ON da.id = txo.address_id
             WHERE txo.tx_id = ?1
             ORDER BY txo.output_index",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare owned output snapshot: {err}")))?;
    let outputs = output_statement
        .query_map([chain_tx_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query owned output snapshot: {err}")))?
        .map(|row| {
            let (
                output_index,
                address_id,
                account_id,
                raw_address,
                script_pubkey_hex,
                value_amount_lo,
                value_amount_hi,
            ) = row.map_err(|err| {
                DbError::new(format!("Failed to read owned output snapshot: {err}"))
            })?;
            Ok(ExistingOwnedOutputRow {
                output_index,
                address_id,
                account_id,
                raw_address,
                script_pubkey_hex,
                value_amount_lo,
                value_amount_hi,
            })
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    Ok(Some(ExistingTransactionSnapshot {
        chain,
        inputs,
        outputs,
    }))
}

fn coverage_invalidation_for_change(
    before: Option<&ExistingTransactionSnapshot>,
    after: Option<&ExistingTransactionSnapshot>,
) -> Result<CoverageInvalidationTargets, DbError> {
    let Some(before) =
        before.filter(|snapshot| snapshot.chain.status == ChainTransactionStatus::Confirmed)
    else {
        return Ok(CoverageInvalidationTargets::default());
    };
    let contradiction = match after {
        None => true,
        Some(after) => {
            after.chain.status != ChainTransactionStatus::Confirmed
                || after.chain.block_height.is_none()
                || before.chain.block_height != after.chain.block_height
                || before.chain.block_hash != after.chain.block_hash
                || before.chain.block_time != after.chain.block_time
                || before.chain.fee_amount_lo != after.chain.fee_amount_lo
                || before.chain.fee_amount_hi != after.chain.fee_amount_hi
                || before.inputs != after.inputs
                || before.outputs != after.outputs
        }
    };
    if !contradiction {
        return Ok(CoverageInvalidationTargets::default());
    }

    let mut targets = CoverageInvalidationTargets::default();
    before.add_owned_targets(&mut targets)?;
    if let Some(after) = after {
        after.add_owned_targets(&mut targets)?;
    }
    Ok(targets)
}

fn resolve_chain_transaction_id(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    tx_hash: &TxHash,
) -> Result<String, DbError> {
    tx.query_row(
        "SELECT id
         FROM chain_transactions
         WHERE asset_id = ?1
           AND network = ?2
           AND tx_hash = ?3",
        params![asset_id.as_str(), network.as_str(), tx_hash.as_str()],
        |row| row.get::<_, String>(0),
    )
    .map_err(|err| DbError::new(format!("Failed to resolve chain transaction id: {err}")))
}

fn upsert_chain_transaction(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    record: &SyncTransactionRecord,
    now_raw: &str,
) -> Result<(), DbError> {
    let block_time = record.block_time.map(|value| value.to_rfc3339());
    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(asset_id, network, tx_hash) DO UPDATE SET
           status = excluded.status,
           block_height = excluded.block_height,
           block_hash = excluded.block_hash,
           block_time = excluded.block_time,
           fee_amount_lo = excluded.fee_amount_lo,
           fee_amount_hi = excluded.fee_amount_hi,
           nonce = excluded.nonce,
           updated_at = excluded.updated_at",
        params![
            Ulid::new().to_string(),
            asset_id.as_str(),
            network.as_str(),
            record.tx_hash.as_str(),
            record.status.as_db_value(),
            record.block_height,
            record.block_hash,
            block_time,
            record.fee_amount,
            0_i64,
            Option::<i64>::None,
            now_raw,
            now_raw,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to upsert chain transaction: {err}")))?;

    Ok(())
}

fn split_i64_amount(value: i64, field_name: &'static str) -> Result<AmountSplitParts, DbError> {
    let amount = UnsignedAmount::try_from_i64(value)
        .map_err(|err| DbError::new(format!("Invalid {field_name}: {err}")))?;
    split_unsigned_amount(amount, field_name)
}

fn split_optional_i64_amount(
    value: Option<i64>,
    field_name: &'static str,
) -> Result<(Option<i64>, Option<i64>), DbError> {
    match value {
        None => Ok((None, None)),
        Some(value) => {
            let parts = split_i64_amount(value, field_name)?;
            Ok((Some(parts.hi), Some(parts.lo)))
        }
    }
}

fn upsert_account_chain_transaction(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    record: &SyncAccountTransactionRecord,
    now_raw: &str,
) -> Result<(), DbError> {
    let block_time = record.block_time.map(|value| value.to_rfc3339());
    let (fee_amount_hi, fee_amount_lo) = match record.fee_amount {
        Some(value) => {
            let parts = split_unsigned_amount(value, "fee_amount")?;
            (Some(parts.hi), Some(parts.lo))
        }
        None => (None, None),
    };

    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(asset_id, network, tx_hash) DO UPDATE SET
           status = excluded.status,
           block_height = excluded.block_height,
           block_hash = excluded.block_hash,
           block_time = excluded.block_time,
           fee_amount_lo = excluded.fee_amount_lo,
           fee_amount_hi = excluded.fee_amount_hi,
           nonce = excluded.nonce,
           updated_at = excluded.updated_at",
        params![
            Ulid::new().to_string(),
            asset_id.as_str(),
            network.as_str(),
            record.tx_hash.as_str(),
            record.status.as_db_value(),
            record.block_height,
            record.block_hash,
            block_time,
            fee_amount_lo,
            fee_amount_hi,
            record.nonce,
            now_raw,
            now_raw,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to upsert account chain transaction: {err}")))?;

    Ok(())
}

fn resolve_owned_address_id(
    tx: &rusqlite::Transaction<'_>,
    address_cache: &mut HashMap<String, Option<DigitalAssetAddressId>>,
    raw_address: Option<&TrackedAddress>,
) -> Result<Option<DigitalAssetAddressId>, DbError> {
    let Some(raw_address) = raw_address else {
        return Ok(None);
    };

    let key = raw_address.as_str().to_string();
    if let Some(cached) = address_cache.get(&key) {
        return Ok(*cached);
    }

    let row = tx
        .query_row(
            "SELECT id
             FROM digital_asset_addresses
             WHERE address = ?1
             LIMIT 1",
            params![raw_address.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to resolve owned address id: {err}")))?;

    let parsed = row.as_deref().map(parse_address_id).transpose()?;
    address_cache.insert(key, parsed);
    Ok(parsed)
}

fn legacy_provider_transfer_key(provider_transfer_key: &ProviderTransferKey) -> Option<String> {
    if provider_transfer_key.is_normal() {
        return Some("legacy:0".to_string());
    }
    let trace_id = provider_transfer_key.internal_trace_id()?;
    let last_segment = trace_id.rsplit('_').next().unwrap_or(trace_id);
    let old_index = last_segment.parse::<i64>().ok()?;
    let old_index = (old_index >= 0).then_some(old_index)?.checked_add(1)?;
    Some(format!("legacy:{old_index}"))
}

fn upsert_account_transfers(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    chain_tx_id: &str,
    record: &SyncAccountTransactionRecord,
    address_cache: &mut HashMap<String, Option<DigitalAssetAddressId>>,
    now_raw: &str,
) -> Result<(), DbError> {
    for transfer in &record.transfers {
        if let Some(legacy_key) = legacy_provider_transfer_key(&transfer.provider_transfer_key) {
            tx.execute(
                "DELETE FROM account_transfers
                 WHERE asset_id = ?1 AND network = ?2 AND tx_hash = ?3
                   AND provider_transfer_key = ?4",
                params![
                    asset_id.as_str(),
                    network.as_str(),
                    record.tx_hash.as_str(),
                    legacy_key,
                ],
            )
            .map_err(|err| {
                DbError::new(format!("Failed to retire legacy account transfer: {err}"))
            })?;
        }
        let from_address_id =
            resolve_owned_address_id(tx, address_cache, transfer.from_address.as_ref())?;
        let to_address_id =
            resolve_owned_address_id(tx, address_cache, transfer.to_address.as_ref())?;
        if from_address_id.is_none() && to_address_id.is_none() {
            tx.execute(
                "DELETE FROM account_transfers
                 WHERE asset_id = ?1
                   AND network = ?2
                   AND tx_hash = ?3
                   AND provider_transfer_key = ?4",
                params![
                    asset_id.as_str(),
                    network.as_str(),
                    record.tx_hash.as_str(),
                    transfer.provider_transfer_key.as_str(),
                ],
            )
            .map_err(|err| {
                DbError::new(format!("Failed to delete unowned account transfer: {err}"))
            })?;
            continue;
        }
        let value_parts = split_unsigned_amount(transfer.value_amount, "value_amount")?;

        tx.execute(
            "INSERT INTO account_transfers
             (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key, transfer_index, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(asset_id, network, tx_hash, provider_transfer_key) DO UPDATE SET
               chain_transaction_id = excluded.chain_transaction_id,
               transfer_index = excluded.transfer_index,
               transfer_kind = excluded.transfer_kind,
               from_address = excluded.from_address,
               from_address_id = excluded.from_address_id,
               to_address = excluded.to_address,
               to_address_id = excluded.to_address_id,
               value_amount_hi = excluded.value_amount_hi,
               value_amount_lo = excluded.value_amount_lo,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                chain_tx_id,
                asset_id.as_str(),
                network.as_str(),
                record.tx_hash.as_str(),
                transfer.provider_transfer_key.as_str(),
                transfer.transfer_index,
                transfer.transfer_kind.as_str(),
                transfer.from_address.as_ref().map(TrackedAddress::as_str),
                from_address_id.map(|value| value.to_string()),
                transfer.to_address.as_ref().map(TrackedAddress::as_str),
                to_address_id.map(|value| value.to_string()),
                value_parts.hi,
                value_parts.lo,
                now_raw,
                now_raw,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert account transfer: {err}")))?;
    }

    Ok(())
}

fn upsert_transaction_inputs(
    tx: &rusqlite::Transaction<'_>,
    chain_tx_id: &str,
    inputs: &[SyncTransactionInputRecord],
    address_cache: &mut HashMap<String, Option<DigitalAssetAddressId>>,
    now_raw: &str,
) -> Result<(), DbError> {
    let incoming_indices = inputs
        .iter()
        .map(|input| input.input_index)
        .collect::<HashSet<_>>();
    let mut statement = tx
        .prepare("SELECT input_index FROM transaction_inputs WHERE tx_id = ?1")
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare existing transaction input indices: {err}"
            ))
        })?;
    let existing_indices = statement
        .query_map([chain_tx_id], |row| row.get::<_, i64>(0))
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query existing transaction input indices: {err}"
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to read existing transaction input index: {err}"
            ))
        })?;
    drop(statement);
    for input_index in existing_indices {
        if !incoming_indices.contains(&input_index) {
            tx.execute(
                "DELETE FROM transaction_inputs WHERE tx_id = ?1 AND input_index = ?2",
                params![chain_tx_id, input_index],
            )
            .map_err(|err| DbError::new(format!("Failed to prune transaction input row: {err}")))?;
        }
    }

    for input in inputs {
        let address_id = resolve_owned_address_id(tx, address_cache, input.prev_address.as_ref())?;
        let Some(address_id) = address_id else {
            tx.execute(
                "DELETE FROM transaction_inputs
                 WHERE tx_id = ?1 AND input_index = ?2",
                params![chain_tx_id, input.input_index],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to delete unowned transaction input row: {err}"
                ))
            })?;
            continue;
        };
        let (value_hi, value_lo) = split_optional_i64_amount(input.value_amount, "value_amount")?;
        tx.execute(
            "INSERT INTO transaction_inputs
             (id, tx_id, input_index, prev_tx_hash, prev_output_index, address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(tx_id, input_index) DO UPDATE SET
               prev_tx_hash = excluded.prev_tx_hash,
               prev_output_index = excluded.prev_output_index,
               address_id = excluded.address_id,
               value_amount_hi = excluded.value_amount_hi,
               value_amount_lo = excluded.value_amount_lo,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                chain_tx_id,
                input.input_index,
                input.prev_tx_hash.as_str(),
                input.prev_output_index,
                address_id.to_string(),
                value_hi,
                value_lo,
                now_raw,
                now_raw,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert transaction input: {err}")))?;
    }

    Ok(())
}

fn upsert_transaction_outputs(
    tx: &rusqlite::Transaction<'_>,
    chain_tx_id: &str,
    outputs: &[SyncTransactionOutputRecord],
    address_cache: &mut HashMap<String, Option<DigitalAssetAddressId>>,
    now_raw: &str,
) -> Result<(), DbError> {
    let incoming_indices = outputs
        .iter()
        .map(|output| output.output_index)
        .collect::<HashSet<_>>();
    let mut statement = tx
        .prepare("SELECT output_index FROM transaction_outputs WHERE tx_id = ?1")
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare existing transaction output indices: {err}"
            ))
        })?;
    let existing_indices = statement
        .query_map([chain_tx_id], |row| row.get::<_, i64>(0))
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query existing transaction output indices: {err}"
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to read existing transaction output index: {err}"
            ))
        })?;
    drop(statement);
    for output_index in existing_indices {
        if !incoming_indices.contains(&output_index) {
            tx.execute(
                "DELETE FROM transaction_outputs WHERE tx_id = ?1 AND output_index = ?2",
                params![chain_tx_id, output_index],
            )
            .map_err(|err| {
                DbError::new(format!("Failed to prune transaction output row: {err}"))
            })?;
        }
    }

    for output in outputs {
        let address_id = resolve_owned_address_id(tx, address_cache, output.raw_address.as_ref())?;
        let Some(address_id) = address_id else {
            tx.execute(
                "DELETE FROM transaction_outputs
                 WHERE tx_id = ?1 AND output_index = ?2",
                params![chain_tx_id, output.output_index],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to delete unowned transaction output row: {err}"
                ))
            })?;
            continue;
        };
        let value_parts = split_i64_amount(output.value_amount, "value_amount")?;
        tx.execute(
            "INSERT INTO transaction_outputs
             (id, tx_id, output_index, address_id, raw_address, script_pubkey_hex, value_amount_hi, value_amount_lo, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(tx_id, output_index) DO UPDATE SET
               address_id = excluded.address_id,
               raw_address = excluded.raw_address,
               script_pubkey_hex = excluded.script_pubkey_hex,
               value_amount_hi = excluded.value_amount_hi,
               value_amount_lo = excluded.value_amount_lo,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                chain_tx_id,
                output.output_index,
                address_id.to_string(),
                output.raw_address.as_ref().map(TrackedAddress::as_str),
                output.script_pubkey_hex,
                value_parts.hi,
                value_parts.lo,
                now_raw,
                now_raw,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert transaction output: {err}")))?;
    }

    Ok(())
}

fn reconcile_utxos_for_transaction(
    tx: &rusqlite::Transaction<'_>,
    asset_id: SyncedAssetId,
    network: Network,
    record: &SyncTransactionRecord,
    outputs: &[SyncTransactionOutputRecord],
    address_cache: &mut HashMap<String, Option<DigitalAssetAddressId>>,
    now_raw: &str,
) -> Result<(), DbError> {
    for output in outputs {
        let address_id = resolve_owned_address_id(tx, address_cache, output.raw_address.as_ref())?;
        let Some(address_id) = address_id else {
            continue;
        };
        let value_parts = split_i64_amount(output.value_amount, "value_amount")?;

        tx.execute(
            "INSERT INTO utxos
             (id, asset_id, network, tx_hash, output_index, address_id, value_amount_hi, value_amount_lo, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(asset_id, network, tx_hash, output_index) DO UPDATE SET
               address_id = excluded.address_id,
               value_amount_hi = excluded.value_amount_hi,
               value_amount_lo = excluded.value_amount_lo,
               status = excluded.status,
               replaced_by_tx_hash = NULL,
               updated_at = excluded.updated_at",
            params![
                Ulid::new().to_string(),
                asset_id.as_str(),
                network.as_str(),
                record.tx_hash.as_str(),
                output.output_index,
                address_id.to_string(),
                value_parts.hi,
                value_parts.lo,
                record.status.as_db_value(),
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                now_raw,
                now_raw,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to upsert UTXO: {err}")))?;

        // Check if a previously-synced transaction already spent this UTXO.
        // This handles the reverse-order case where the spending transaction
        // was synced before the producing transaction (e.g., mempool API
        // returns newest-first for Bitcoin).
        let spending_tx_hash: Option<String> = tx
            .query_row(
                "SELECT ct.tx_hash
                 FROM transaction_inputs ti
                 JOIN chain_transactions ct ON ct.id = ti.tx_id
                 WHERE ti.prev_tx_hash = ?1
                   AND ti.prev_output_index = ?2
                   AND ct.asset_id = ?3
                   AND ct.network = ?4
                 LIMIT 1",
                params![
                    record.tx_hash.as_str(),
                    output.output_index,
                    asset_id.as_str(),
                    network.as_str(),
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to check existing spending input for UTXO: {err}"
                ))
            })?;

        if let Some(spending_hash) = spending_tx_hash {
            tx.execute(
                "UPDATE utxos
                 SET spent_by_tx_hash = ?1,
                     spent_at = ?2,
                     updated_at = ?3
                 WHERE asset_id = ?4
                   AND network = ?5
                   AND tx_hash = ?6
                   AND output_index = ?7
                   AND spent_by_tx_hash IS NULL",
                params![
                    spending_hash,
                    now_raw,
                    now_raw,
                    asset_id.as_str(),
                    network.as_str(),
                    record.tx_hash.as_str(),
                    output.output_index,
                ],
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to link UTXO to existing spending transaction: {err}"
                ))
            })?;
        }
    }

    for input in &record.inputs {
        tx.execute(
            "UPDATE utxos
             SET spent_by_tx_hash = ?1,
                 spent_at = ?2,
                 updated_at = ?3
             WHERE asset_id = ?4
               AND network = ?5
               AND tx_hash = ?6
               AND output_index = ?7",
            params![
                record.tx_hash.as_str(),
                now_raw,
                now_raw,
                asset_id.as_str(),
                network.as_str(),
                input.prev_tx_hash.as_str(),
                input.prev_output_index,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to update spent UTXO linkage: {err}")))?;
    }

    if record.status == ChainTransactionStatus::Dropped {
        tx.execute(
            "UPDATE utxos
             SET status = ?1,
                 updated_at = ?2
             WHERE asset_id = ?3
               AND network = ?4
               AND tx_hash = ?5",
            params![
                ChainTransactionStatus::Dropped.as_db_value(),
                now_raw,
                asset_id.as_str(),
                network.as_str(),
                record.tx_hash.as_str(),
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to mark dropped UTXOs: {err}")))?;
    }

    Ok(())
}

fn apply_transaction_reconciliation(
    conn: &mut rusqlite::Connection,
    asset_id: SyncedAssetId,
    network: Network,
    record: &SyncTransactionRecord,
    now_raw: &str,
) -> Result<(bool, bool, CoverageInvalidationTargets), DbError> {
    let sql_tx = conn
        .transaction()
        .map_err(|err| DbError::new(format!("Failed to start SQL transaction: {err}")))?;

    let existing = load_existing_transaction_snapshot(&sql_tx, asset_id, network, &record.tx_hash)?;
    upsert_chain_transaction(&sql_tx, asset_id, network, record, now_raw)?;
    let chain_tx_id = resolve_chain_transaction_id(&sql_tx, asset_id, network, &record.tx_hash)?;
    let mut address_cache = HashMap::<String, Option<DigitalAssetAddressId>>::new();
    upsert_transaction_inputs(
        &sql_tx,
        &chain_tx_id,
        &record.inputs,
        &mut address_cache,
        now_raw,
    )?;
    upsert_transaction_outputs(
        &sql_tx,
        &chain_tx_id,
        &record.outputs,
        &mut address_cache,
        now_raw,
    )?;
    begin_chain_cleanup_scope(&sql_tx)?;
    mark_chain_cleanup_candidate(&sql_tx, &chain_tx_id)?;
    let cleanup_stats = execute_chain_cleanup_for_marked_candidates(&sql_tx)?;
    reconcile_utxos_for_transaction(
        &sql_tx,
        asset_id,
        network,
        record,
        &record.outputs,
        &mut address_cache,
        now_raw,
    )?;
    let current = load_existing_transaction_snapshot(&sql_tx, asset_id, network, &record.tx_hash)?;
    let coverage_invalidation =
        coverage_invalidation_for_change(existing.as_ref(), current.as_ref())?;

    sql_tx
        .commit()
        .map_err(|err| DbError::new(format!("Failed to commit SQL transaction: {err}")))?;

    let inserted = existing.is_none();
    let updated = existing
        .as_ref()
        .zip(current.as_ref())
        .is_some_and(|(before, after)| before != after);
    if cleanup_stats.deleted_orphan_chain_transactions > 0 {
        return Ok((false, false, coverage_invalidation));
    }
    Ok((inserted, updated, coverage_invalidation))
}

fn apply_account_transaction_reconciliation(
    conn: &mut rusqlite::Connection,
    asset_id: SyncedAssetId,
    network: Network,
    record: &SyncAccountTransactionRecord,
    now_raw: &str,
) -> Result<(bool, bool), DbError> {
    let sql_tx = conn
        .transaction()
        .map_err(|err| DbError::new(format!("Failed to start SQL transaction: {err}")))?;

    let existing =
        load_existing_chain_transaction_row(&sql_tx, asset_id, network, &record.tx_hash)?;
    upsert_account_chain_transaction(&sql_tx, asset_id, network, record, now_raw)?;
    let chain_tx_id = resolve_chain_transaction_id(&sql_tx, asset_id, network, &record.tx_hash)?;
    let mut address_cache = HashMap::<String, Option<DigitalAssetAddressId>>::new();
    upsert_account_transfers(
        &sql_tx,
        asset_id,
        network,
        &chain_tx_id,
        record,
        &mut address_cache,
        now_raw,
    )?;
    begin_chain_cleanup_scope(&sql_tx)?;
    mark_chain_cleanup_candidate(&sql_tx, &chain_tx_id)?;
    let cleanup_stats = execute_chain_cleanup_for_marked_candidates(&sql_tx)?;

    sql_tx
        .commit()
        .map_err(|err| DbError::new(format!("Failed to commit SQL transaction: {err}")))?;

    let block_time = record.block_time.map(|value| value.to_rfc3339());
    let (fee_amount_hi, fee_amount_lo) = match record.fee_amount {
        Some(value) => {
            let parts = split_unsigned_amount(value, "fee_amount")?;
            (Some(parts.hi), Some(parts.lo))
        }
        None => (None, None),
    };
    let current = ExistingChainTransactionRow {
        status: record.status,
        block_height: record.block_height,
        block_hash: record.block_hash.clone(),
        block_time,
        fee_amount_lo,
        fee_amount_hi,
        nonce: record.nonce,
    };

    let inserted = existing.is_none();
    let updated = existing.is_some_and(|value| value != current);
    if cleanup_stats.deleted_orphan_chain_transactions > 0 {
        return Ok((false, false));
    }
    Ok((inserted, updated))
}

#[derive(Debug)]
pub(crate) struct TransactionSyncReconcileFailure {
    pub(crate) error: Box<DbError>,
    pub(crate) summary: TransactionSyncReconcileSummary,
}

fn merge_reconcile_summary(
    total: &mut TransactionSyncReconcileSummary,
    summary: TransactionSyncReconcileSummary,
) {
    total.new_tx_count = total.new_tx_count.saturating_add(summary.new_tx_count);
    total.updated_tx_count = total
        .updated_tx_count
        .saturating_add(summary.updated_tx_count);
    total
        .coverage_invalidation
        .union_with(summary.coverage_invalidation);
}

pub(crate) fn reconcile_address_transactions_preserving_invalidation(
    user_id: UserId,
    asset_id: SyncedAssetId,
    network: Network,
    records: &[SyncTransactionRecord],
    observed_at: DateTime<Utc>,
) -> Result<TransactionSyncReconcileSummary, TransactionSyncReconcileFailure> {
    let now_raw = observed_at.to_rfc3339();
    let mut total = TransactionSyncReconcileSummary::default();

    for batch in records.chunks(RECONCILE_LOCK_BATCH_SIZE) {
        let outcome = match with_user_db_mut(user_id, |conn| {
            let mut summary = TransactionSyncReconcileSummary::default();
            for record in batch {
                let result =
                    apply_transaction_reconciliation(conn, asset_id, network, record, &now_raw);
                let (was_inserted, was_updated, record_invalidation) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        return Ok(Err(TransactionSyncReconcileFailure {
                            error: Box::new(error),
                            summary,
                        }));
                    }
                };
                if was_inserted {
                    summary.new_tx_count = summary
                        .new_tx_count
                        .saturating_add(TransactionCount::from_u32(1));
                }
                if was_updated {
                    summary.updated_tx_count = summary
                        .updated_tx_count
                        .saturating_add(TransactionCount::from_u32(1));
                }
                summary
                    .coverage_invalidation
                    .union_with(record_invalidation);
            }
            Ok::<Result<_, TransactionSyncReconcileFailure>, DbError>(Ok(summary))
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(TransactionSyncReconcileFailure {
                    error: Box::new(error),
                    summary: total,
                });
            }
        };

        match outcome {
            Ok(summary) => merge_reconcile_summary(&mut total, summary),
            Err(failure) => {
                merge_reconcile_summary(&mut total, failure.summary);
                return Err(TransactionSyncReconcileFailure {
                    error: failure.error,
                    summary: total,
                });
            }
        }
    }

    Ok(total)
}

#[cfg(any(
    all(feature = "server", feature = "dev-config", not(test)),
    all(test, feature = "db-tests")
))]
pub(crate) fn reconcile_address_transactions(
    user_id: UserId,
    asset_id: SyncedAssetId,
    network: Network,
    records: &[SyncTransactionRecord],
    observed_at: DateTime<Utc>,
) -> Result<TransactionSyncReconcileSummary, DbError> {
    reconcile_address_transactions_preserving_invalidation(
        user_id,
        asset_id,
        network,
        records,
        observed_at,
    )
    .map_err(|failure| *failure.error)
}

pub(crate) fn reconcile_account_transactions(
    user_id: UserId,
    asset_id: SyncedAssetId,
    network: Network,
    records: &[SyncAccountTransactionRecord],
    observed_at: DateTime<Utc>,
) -> Result<TransactionSyncReconcileSummary, DbError> {
    let mut inserted = 0_u32;
    let mut updated = 0_u32;

    for batch in records.chunks(RECONCILE_LOCK_BATCH_SIZE) {
        let summary = with_user_db_mut(user_id, |conn| {
            reconcile_account_transactions_conn(conn, asset_id, network, batch, observed_at)
        })?;
        inserted = inserted.saturating_add(summary.new_tx_count.value());
        updated = updated.saturating_add(summary.updated_tx_count.value());
    }

    Ok(TransactionSyncReconcileSummary {
        new_tx_count: TransactionCount::from_u32(inserted),
        updated_tx_count: TransactionCount::from_u32(updated),
        coverage_invalidation: CoverageInvalidationTargets::default(),
    })
}

pub(crate) fn reconcile_account_transactions_conn(
    conn: &mut rusqlite::Connection,
    asset_id: SyncedAssetId,
    network: Network,
    records: &[SyncAccountTransactionRecord],
    observed_at: DateTime<Utc>,
) -> Result<TransactionSyncReconcileSummary, DbError> {
    let now_raw = observed_at.to_rfc3339();
    let mut inserted = 0_u32;
    let mut updated = 0_u32;
    for record in records {
        let (was_inserted, was_updated) =
            apply_account_transaction_reconciliation(conn, asset_id, network, record, &now_raw)?;
        inserted = inserted.saturating_add(u32::from(was_inserted));
        updated = updated.saturating_add(u32::from(was_updated));
    }
    Ok(TransactionSyncReconcileSummary {
        new_tx_count: TransactionCount::from_u32(inserted),
        updated_tx_count: TransactionCount::from_u32(updated),
        coverage_invalidation: CoverageInvalidationTargets::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_retirement_skips_unrepresentable_internal_trace_ids() {
        for trace_id in ["0_bad", "-1", "9223372036854775807"] {
            let key = ProviderTransferKey::from_internal_trace_id(trace_id).unwrap();
            assert_eq!(legacy_provider_transfer_key(&key), None, "{trace_id}");
        }

        assert_eq!(
            legacy_provider_transfer_key(&ProviderTransferKey::normal()).as_deref(),
            Some("legacy:0")
        );
        assert_eq!(
            legacy_provider_transfer_key(
                &ProviderTransferKey::from_internal_trace_id("0_1").unwrap()
            )
            .as_deref(),
            Some("legacy:2")
        );
    }
}

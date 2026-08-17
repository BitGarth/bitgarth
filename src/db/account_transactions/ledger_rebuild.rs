use super::balance::load_account_meta;
use super::types::*;
use crate::account_model::AccountModel;
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::account_model_for;
use crate::db::error::DbError;
use crate::db::raw_ingestion::SyncRunId;
use crate::db::transaction_sync::BitcoinAccountHistoryCoverage;
use crate::db::transaction_sync::MempoolHistoryProof;
use crate::db::transaction_sync::load_api_confirmed_balances_for_account_conn;
use crate::db::user_db::with_user_db_mut;
use crate::models::{UserId, parse_datetime};
use crate::transactions::{
    AggregateSyncResult, ChainTipHeight, ChainTransactionStatus, NativeBalanceState,
    SyncIntegrationId,
};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use ulid::Ulid;

#[derive(Debug, Clone, Copy)]
pub(crate) struct BitcoinAddressProofPublication {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) proof: MempoolHistoryProof,
    pub(crate) scan_start_run_id: Option<SyncRunId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BitcoinHdDiscoveryPublication {
    pub(crate) external_last_index: Option<u32>,
    pub(crate) internal_last_index: Option<u32>,
    pub(crate) completed_tip: ChainTipHeight,
    pub(crate) completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BitcoinAccountCompletionPublication {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) final_address_proof: Option<BitcoinAddressProofPublication>,
    pub(crate) completed_hd_discovery: Option<BitcoinHdDiscoveryPublication>,
    pub(crate) observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LedgerBuildEntry {
    pub(super) chain_transaction_id: String,
    pub(super) tx_hash: String,
    pub(super) status: ChainTransactionStatus,
    pub(super) occurred_at: DateTime<Utc>,
    pub(super) first_seen_at: DateTime<Utc>,
    pub(super) block_height: Option<i64>,
    pub(super) nonce: Option<i64>,
    pub(super) min_transfer_index: Option<i64>,
    pub(super) direction: crate::transactions::AccountTransactionDirection,
    pub(super) value: UnsignedAmount,
    pub(super) fee: Option<UnsignedAmount>,
    pub(super) balance_delta: i128,
    pub(super) closing_balance: Option<UnsignedAmount>,
    pub(super) same_block_parent_hashes: Vec<String>,
    pub(super) from_addresses: Vec<String>,
    pub(super) to_addresses: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ClassifiedLedgerFlow {
    pub(super) direction: crate::transactions::AccountTransactionDirection,
    pub(super) value: UnsignedAmount,
}

pub(super) fn classify_utxo_ledger_flow(
    owned_input_total: UnsignedAmount,
    owned_output_total: UnsignedAmount,
    fee: Option<UnsignedAmount>,
) -> ClassifiedLedgerFlow {
    let has_owned_input = owned_input_total.value() > 0;
    let has_owned_output = owned_output_total.value() > 0;
    let account_outflow_value = owned_input_total
        .value()
        .saturating_sub(owned_output_total.value());
    let fee_value = fee.map(|value| value.value()).unwrap_or(0_u128);
    let external_outflow_value = account_outflow_value.saturating_sub(fee_value);

    if has_owned_input && (external_outflow_value > 0 || !has_owned_output) {
        return ClassifiedLedgerFlow {
            direction: crate::transactions::AccountTransactionDirection::Outgoing,
            value: UnsignedAmount::from_u128(external_outflow_value),
        };
    }

    if has_owned_input && has_owned_output {
        return ClassifiedLedgerFlow {
            direction: crate::transactions::AccountTransactionDirection::SelfTransfer,
            value: owned_output_total,
        };
    }

    ClassifiedLedgerFlow {
        direction: crate::transactions::AccountTransactionDirection::Incoming,
        value: owned_output_total,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AggregatedAccountTx {
    chain_transaction_id: String,
    tx_hash: String,
    status: ChainTransactionStatus,
    block_height: Option<i64>,
    block_time: Option<DateTime<Utc>>,
    nonce: Option<i64>,
    first_seen_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    min_transfer_index: Option<i64>,
    has_from_owned: bool,
    has_to_owned: bool,
    incoming_total: UnsignedAmount,
    outgoing_total: UnsignedAmount,
    self_transfer_total: UnsignedAmount,
    fee: Option<UnsignedAmount>,
    from_addresses: BTreeSet<String>,
    to_addresses: BTreeSet<String>,
}

pub(super) fn account_model_entries(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<Vec<LedgerBuildEntry>, DbError> {
    let account_id_raw = account_id.to_string();
    let mut stmt = conn
        .prepare(
            "SELECT
                ct.id,
                ct.tx_hash,
                ct.status,
                ct.block_height,
                ct.block_time,
                ct.nonce,
                ct.created_at,
                ct.updated_at,
                at.transfer_index,
                at.from_address,
                at.to_address,
                at.value_amount_hi,
                at.value_amount_lo,
                ct.fee_amount_hi,
                ct.fee_amount_lo,
                EXISTS(
                    SELECT 1
                    FROM digital_asset_addresses da
                    WHERE da.account_id = ?1 AND da.id = at.from_address_id
                ),
                EXISTS(
                    SELECT 1
                    FROM digital_asset_addresses da
                    WHERE da.account_id = ?1 AND da.id = at.to_address_id
                )
             FROM account_transfers at
             JOIN chain_transactions ct ON ct.id = at.chain_transaction_id
             WHERE at.asset_id = ?2
               AND at.network = ?3
               AND (
                    EXISTS(
                        SELECT 1
                        FROM digital_asset_addresses da
                        WHERE da.account_id = ?1 AND da.id = at.from_address_id
                    )
                    OR EXISTS(
                        SELECT 1
                        FROM digital_asset_addresses da
                        WHERE da.account_id = ?1 AND da.id = at.to_address_id
                    )
               )
             ORDER BY ct.created_at ASC, ct.tx_hash ASC, at.transfer_index ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare account model ledger query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map(
            params![account_id_raw, asset_id.as_str(), network.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, i64>(16)?,
                ))
            },
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute account model ledger query: {err}"
            ))
        })?;

    let mut by_hash: HashMap<String, AggregatedAccountTx> = HashMap::new();

    for row in rows {
        let (
            chain_transaction_id,
            tx_hash,
            status_raw,
            block_height,
            block_time_raw,
            nonce,
            first_seen_raw,
            updated_at_raw,
            transfer_index,
            from_address_raw,
            to_address_raw,
            value_hi,
            value_lo,
            fee_hi,
            fee_lo,
            from_owned_raw,
            to_owned_raw,
        ) = row.map_err(|err| {
            DbError::new(format!("Failed to map account model ledger row: {err}"))
        })?;

        if tx_hash.trim().is_empty() {
            return Err(DbError::new(
                "Invalid account model ledger row: empty tx_hash",
            ));
        }

        let status = parse_chain_status(&status_raw)?;
        let block_time = block_time_raw
            .as_deref()
            .map(parse_datetime)
            .transpose()
            .map_err(|err| DbError::new(format!("Invalid block_time in DB: {err}")))?;
        let first_seen_at = parse_datetime(&first_seen_raw)
            .map_err(|err| DbError::new(format!("Invalid created_at in DB: {err}")))?;
        let updated_at = parse_datetime(&updated_at_raw)
            .map_err(|err| DbError::new(format!("Invalid updated_at in DB: {err}")))?;
        let value = parse_split_amount(value_hi, value_lo, "value_amount")?;
        let fee = parse_optional_split_amount(fee_hi, fee_lo, "fee_amount")?;
        let from_owned = from_owned_raw != 0_i64;
        let to_owned = to_owned_raw != 0_i64;

        let aggregated = by_hash
            .entry(tx_hash.clone())
            .or_insert_with(|| AggregatedAccountTx {
                chain_transaction_id: chain_transaction_id.clone(),
                tx_hash: tx_hash.clone(),
                status,
                block_height,
                block_time,
                nonce,
                first_seen_at,
                updated_at,
                min_transfer_index: Some(transfer_index),
                has_from_owned: false,
                has_to_owned: false,
                incoming_total: UnsignedAmount::zero(),
                outgoing_total: UnsignedAmount::zero(),
                self_transfer_total: UnsignedAmount::zero(),
                fee,
                from_addresses: BTreeSet::new(),
                to_addresses: BTreeSet::new(),
            });

        aggregated.min_transfer_index = Some(match aggregated.min_transfer_index {
            Some(existing) => existing.min(transfer_index),
            None => transfer_index,
        });
        aggregated.has_from_owned |= from_owned;
        aggregated.has_to_owned |= to_owned;

        if to_owned {
            aggregated.incoming_total =
                add_amount(aggregated.incoming_total, value, "incoming_total")?;
        }
        if from_owned {
            aggregated.outgoing_total =
                add_amount(aggregated.outgoing_total, value, "outgoing_total")?;
        }
        if from_owned && to_owned {
            aggregated.self_transfer_total =
                add_amount(aggregated.self_transfer_total, value, "self_transfer_total")?;
        }

        if let Some(from_address) = from_address_raw
            && !from_address.trim().is_empty()
        {
            aggregated.from_addresses.insert(from_address);
        }
        if let Some(to_address) = to_address_raw
            && !to_address.trim().is_empty()
        {
            aggregated.to_addresses.insert(to_address);
        }
    }

    let mut entries = Vec::with_capacity(by_hash.len());
    for aggregated in by_hash.into_values() {
        let direction = if aggregated.has_from_owned && aggregated.has_to_owned {
            crate::transactions::AccountTransactionDirection::SelfTransfer
        } else if aggregated.has_from_owned {
            crate::transactions::AccountTransactionDirection::Outgoing
        } else {
            crate::transactions::AccountTransactionDirection::Incoming
        };

        let value = match direction {
            crate::transactions::AccountTransactionDirection::Incoming => aggregated.incoming_total,
            crate::transactions::AccountTransactionDirection::Outgoing => aggregated.outgoing_total,
            crate::transactions::AccountTransactionDirection::SelfTransfer => {
                aggregated.self_transfer_total
            }
        };
        let fee = if aggregated.has_from_owned {
            aggregated.fee
        } else {
            None
        };

        let incoming_signed = to_signed_amount(aggregated.incoming_total, "incoming_total")?;
        let outgoing_signed = to_signed_amount(aggregated.outgoing_total, "outgoing_total")?;
        let fee_signed = fee
            .map(|value| to_signed_amount(value, "fee_amount"))
            .transpose()?
            .unwrap_or(0_i128);
        let mut balance_delta = incoming_signed - outgoing_signed;
        if aggregated.has_from_owned {
            balance_delta = balance_delta
                .checked_sub(fee_signed)
                .ok_or_else(|| DbError::new("Signed underflow while applying account fee"))?;
        }

        let occurred_at = if aggregated.status == ChainTransactionStatus::Confirmed {
            aggregated.block_time.unwrap_or(aggregated.updated_at)
        } else {
            aggregated.first_seen_at
        };

        entries.push(LedgerBuildEntry {
            chain_transaction_id: aggregated.chain_transaction_id,
            tx_hash: aggregated.tx_hash,
            status: aggregated.status,
            occurred_at,
            first_seen_at: aggregated.first_seen_at,
            block_height: aggregated.block_height,
            nonce: aggregated.nonce,
            min_transfer_index: aggregated.min_transfer_index,
            direction,
            value,
            fee,
            balance_delta,
            closing_balance: None,
            same_block_parent_hashes: Vec::new(),
            from_addresses: aggregated.from_addresses.into_iter().collect(),
            to_addresses: aggregated.to_addresses.into_iter().collect(),
        });
    }

    Ok(entries)
}

pub(super) fn load_utxo_owned_addresses_for_tx(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    chain_transaction_id: &str,
    source_table: &str,
) -> Result<Vec<String>, DbError> {
    let account_id_raw = account_id.to_string();
    let sql = match source_table {
        "inputs" => {
            "SELECT DISTINCT da.address
             FROM transaction_inputs ti
             JOIN digital_asset_addresses da ON da.id = ti.address_id
             WHERE ti.tx_id = ?1 AND da.account_id = ?2
             ORDER BY da.address ASC"
        }
        "outputs" => {
            "SELECT DISTINCT da.address
             FROM transaction_outputs to2
             JOIN digital_asset_addresses da ON da.id = to2.address_id
             WHERE to2.tx_id = ?1 AND da.account_id = ?2
             ORDER BY da.address ASC"
        }
        _ => return Err(DbError::new("Invalid UTXO source table for address lookup")),
    };

    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| DbError::new(format!("Failed to prepare UTXO address lookup: {err}")))?;
    let rows = stmt
        .query_map(params![chain_transaction_id, account_id_raw], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|err| DbError::new(format!("Failed to query UTXO addresses: {err}")))?;

    let mut addresses = Vec::new();
    for row in rows {
        let address =
            row.map_err(|err| DbError::new(format!("Failed to map UTXO address row: {err}")))?;
        if !address.trim().is_empty() {
            addresses.push(address);
        }
    }
    Ok(addresses)
}

pub(super) fn utxo_model_entries(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<Vec<LedgerBuildEntry>, DbError> {
    utxo_model_entries_with_row_visitor(conn, account_id, asset_id, network, || {})
}

fn utxo_model_entries_with_row_visitor(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
    mut visit_canonical_row: impl FnMut(),
) -> Result<Vec<LedgerBuildEntry>, DbError> {
    let account_id_raw = account_id.to_string();
    let mut same_block_parent_hashes =
        load_same_block_parent_hashes_for_account(conn, account_id, asset_id, network)?;
    let mut stmt = conn
        .prepare(
            "SELECT
                ct.id,
                ct.tx_hash,
                ct.status,
                ct.block_height,
                ct.block_time,
                ct.nonce,
                ct.created_at,
                ct.updated_at,
                ct.fee_amount_hi,
                ct.fee_amount_lo,
                COALESCE(owned_outputs.total_hi, 0) AS owned_output_total_hi,
                COALESCE(owned_outputs.total_lo, 0) AS owned_output_total_lo,
                COALESCE(owned_inputs.total_hi, 0) AS owned_input_total_hi,
                COALESCE(owned_inputs.total_lo, 0) AS owned_input_total_lo
             FROM chain_transactions ct
             LEFT JOIN (
                SELECT
                    to2.tx_id,
                    SUM(to2.value_amount_hi) AS total_hi,
                    SUM(to2.value_amount_lo) AS total_lo
                FROM transaction_outputs to2
                JOIN digital_asset_addresses da2 ON da2.id = to2.address_id
                WHERE da2.account_id = ?1
                GROUP BY to2.tx_id
             ) owned_outputs ON owned_outputs.tx_id = ct.id
             LEFT JOIN (
                SELECT
                    ti2.tx_id,
                    SUM(ti2.value_amount_hi) AS total_hi,
                    SUM(ti2.value_amount_lo) AS total_lo
                FROM transaction_inputs ti2
                JOIN digital_asset_addresses da3 ON da3.id = ti2.address_id
                WHERE da3.account_id = ?1
                GROUP BY ti2.tx_id
             ) owned_inputs ON owned_inputs.tx_id = ct.id
             WHERE ct.asset_id = ?2
               AND ct.network = ?3
               AND (owned_outputs.tx_id IS NOT NULL OR owned_inputs.tx_id IS NOT NULL)
             ORDER BY ct.created_at ASC, ct.tx_hash ASC",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare UTXO ledger query: {err}")))?;

    let rows = stmt
        .query_map(
            params![account_id_raw, asset_id.as_str(), network.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            },
        )
        .map_err(|err| DbError::new(format!("Failed to execute UTXO ledger query: {err}")))?;

    let mut entries = Vec::new();
    for row in rows {
        visit_canonical_row();
        let (
            chain_transaction_id,
            tx_hash,
            status_raw,
            block_height,
            block_time_raw,
            nonce,
            first_seen_raw,
            updated_at_raw,
            fee_hi,
            fee_lo,
            owned_output_total_hi,
            owned_output_total_lo,
            owned_input_total_hi,
            owned_input_total_lo,
        ) = row.map_err(|err| DbError::new(format!("Failed to map UTXO ledger row: {err}")))?;

        if tx_hash.trim().is_empty() {
            return Err(DbError::new("Invalid UTXO ledger row: empty tx_hash"));
        }

        let status = parse_chain_status(&status_raw)?;
        let block_time = block_time_raw
            .as_deref()
            .map(parse_datetime)
            .transpose()
            .map_err(|err| DbError::new(format!("Invalid block_time in DB: {err}")))?;
        let first_seen_at = parse_datetime(&first_seen_raw)
            .map_err(|err| DbError::new(format!("Invalid created_at in DB: {err}")))?;
        let updated_at = parse_datetime(&updated_at_raw)
            .map_err(|err| DbError::new(format!("Invalid updated_at in DB: {err}")))?;
        let owned_output_total = parse_split_sum_amount(
            owned_output_total_hi,
            owned_output_total_lo,
            "owned_output_total",
        )?;
        let owned_input_total = parse_split_sum_amount(
            owned_input_total_hi,
            owned_input_total_lo,
            "owned_input_total",
        )?;
        let has_owned_input = owned_input_total.value() > 0;
        let fee = if has_owned_input {
            parse_optional_split_amount(fee_hi, fee_lo, "fee_amount")?
        } else {
            None
        };

        let classified = classify_utxo_ledger_flow(owned_input_total, owned_output_total, fee);

        let incoming_signed = to_signed_amount(owned_output_total, "owned_output_total")?;
        let outgoing_signed = to_signed_amount(owned_input_total, "owned_input_total")?;
        let balance_delta = incoming_signed
            .checked_sub(outgoing_signed)
            .ok_or_else(|| DbError::new("Signed underflow while building UTXO balance delta"))?;
        let occurred_at = if status == ChainTransactionStatus::Confirmed {
            block_time.unwrap_or(updated_at)
        } else {
            first_seen_at
        };

        let from_addresses =
            load_utxo_owned_addresses_for_tx(conn, account_id, &chain_transaction_id, "inputs")?;
        let to_addresses =
            load_utxo_owned_addresses_for_tx(conn, account_id, &chain_transaction_id, "outputs")?;
        let same_block_parent_hashes = same_block_parent_hashes
            .remove(&chain_transaction_id)
            .unwrap_or_default();

        entries.push(LedgerBuildEntry {
            chain_transaction_id,
            tx_hash,
            status,
            occurred_at,
            first_seen_at,
            block_height,
            nonce,
            min_transfer_index: None,
            direction: classified.direction,
            value: classified.value,
            fee,
            balance_delta,
            closing_balance: None,
            same_block_parent_hashes,
            from_addresses,
            to_addresses,
        });
    }

    Ok(entries)
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) fn utxo_model_entry_and_visit_counts_for_test(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<(usize, usize), DbError> {
    let mut canonical_rows_visited = 0_usize;
    let entries = utxo_model_entries_with_row_visitor(conn, account_id, asset_id, network, || {
        canonical_rows_visited = canonical_rows_visited.saturating_add(1)
    })?;
    Ok((entries.len(), canonical_rows_visited))
}

fn load_same_block_parent_hashes_for_account(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<HashMap<String, Vec<String>>, DbError> {
    let mut statement = conn
        .prepare(
            "SELECT DISTINCT child.id, ti.prev_tx_hash
             FROM chain_transactions child
             JOIN transaction_inputs ti ON ti.tx_id = child.id
             JOIN chain_transactions parent
               ON parent.asset_id = child.asset_id
              AND parent.network = child.network
              AND parent.tx_hash = ti.prev_tx_hash
             WHERE child.asset_id = ?2
               AND child.network = ?3
               AND child.status = 'confirmed'
               AND parent.status = 'confirmed'
               AND parent.block_height = child.block_height
               AND (
                    EXISTS(
                        SELECT 1
                        FROM transaction_inputs parent_owned_input
                        JOIN digital_asset_addresses parent_input_address
                          ON parent_input_address.id = parent_owned_input.address_id
                        WHERE parent_owned_input.tx_id = parent.id
                          AND parent_input_address.account_id = ?1
                    )
                    OR EXISTS(
                        SELECT 1
                        FROM transaction_outputs parent_owned_output
                        JOIN digital_asset_addresses parent_output_address
                          ON parent_output_address.id = parent_owned_output.address_id
                        WHERE parent_owned_output.tx_id = parent.id
                          AND parent_output_address.account_id = ?1
                    )
               )
               AND (
                    EXISTS(
                        SELECT 1
                        FROM transaction_inputs owned_input
                        JOIN digital_asset_addresses input_address
                          ON input_address.id = owned_input.address_id
                        WHERE owned_input.tx_id = child.id
                          AND input_address.account_id = ?1
                    )
                    OR EXISTS(
                        SELECT 1
                        FROM transaction_outputs owned_output
                        JOIN digital_asset_addresses output_address
                          ON output_address.id = owned_output.address_id
                        WHERE owned_output.tx_id = child.id
                          AND output_address.account_id = ?1
                    )
               )
             ORDER BY child.id ASC, ti.prev_tx_hash ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare same-block Bitcoin dependency query: {err}"
            ))
        })?;
    let rows = statement
        .query_map(
            params![account_id.to_string(), asset_id.as_str(), network.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query same-block Bitcoin dependencies: {err}"
            ))
        })?;
    let mut by_child = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let (child_id, parent_hash) = row.map_err(|err| {
            DbError::new(format!(
                "Failed to read same-block Bitcoin dependency: {err}"
            ))
        })?;
        by_child.entry(child_id).or_default().push(parent_hash);
    }
    Ok(by_child)
}

fn order_confirmed_bitcoin_entries(entries: &[LedgerBuildEntry]) -> Result<Vec<usize>, DbError> {
    let mut by_height = BTreeMap::<i64, Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.status != ChainTransactionStatus::Confirmed {
            continue;
        }
        let height = entry.block_height.ok_or_else(|| {
            DbError::new(format!(
                "Confirmed Bitcoin transaction {} has no block height",
                entry.tx_hash
            ))
        })?;
        by_height.entry(height).or_default().push(index);
    }

    let mut ordered = Vec::new();
    for (height, indices) in by_height {
        let by_hash = indices
            .iter()
            .map(|index| (entries[*index].tx_hash.as_str(), *index))
            .collect::<HashMap<_, _>>();
        let mut indegree = indices
            .iter()
            .map(|index| (entries[*index].tx_hash.as_str(), 0_usize))
            .collect::<HashMap<_, _>>();
        let mut children = HashMap::<&str, Vec<&str>>::new();

        for index in &indices {
            let entry = &entries[*index];
            for parent_hash in entry
                .same_block_parent_hashes
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
            {
                if !by_hash.contains_key(parent_hash) {
                    return Err(DbError::new(format!(
                        "Unresolved same-block Bitcoin dependency {parent_hash} for {} at height {height}",
                        entry.tx_hash
                    )));
                }
                children
                    .entry(parent_hash)
                    .or_default()
                    .push(entry.tx_hash.as_str());
                *indegree
                    .get_mut(entry.tx_hash.as_str())
                    .ok_or_else(|| DbError::new("Missing Bitcoin dependency node"))? += 1;
            }
        }

        let mut ready = indegree
            .iter()
            .filter_map(|(hash, degree)| (*degree == 0).then_some(*hash))
            .collect::<BTreeSet<_>>();
        let mut height_ordered = 0_usize;
        while let Some(hash) = ready.pop_first() {
            ordered.push(
                *by_hash
                    .get(hash)
                    .ok_or_else(|| DbError::new("Missing ready Bitcoin transaction"))?,
            );
            height_ordered += 1;
            if let Some(dependents) = children.get(hash) {
                for dependent in dependents {
                    let degree = indegree
                        .get_mut(dependent)
                        .ok_or_else(|| DbError::new("Missing Bitcoin dependent transaction"))?;
                    *degree = degree
                        .checked_sub(1)
                        .ok_or_else(|| DbError::new("Invalid Bitcoin dependency count"))?;
                    if *degree == 0 {
                        ready.insert(dependent);
                    }
                }
            }
        }
        if height_ordered != indices.len() {
            return Err(DbError::new(format!(
                "Cycle in same-block Bitcoin dependencies at height {height}"
            )));
        }
    }
    Ok(ordered)
}

pub(super) fn assign_bitcoin_closing_balances(
    entries: &mut [LedgerBuildEntry],
    basis: NativeBalanceState,
) -> Result<(), DbError> {
    entries
        .iter_mut()
        .for_each(|entry| entry.closing_balance = None);
    let result = (|| {
        let mut running = match basis {
            NativeBalanceState::CanonicalZero => 0_i128,
            NativeBalanceState::KnownAmount(amount) => {
                i128::try_from(amount.value()).map_err(|_| {
                    DbError::new("Bitcoin balance basis exceeds the supported signed range")
                })?
            }
            NativeBalanceState::Unknown => return Ok(()),
        };
        for index in order_confirmed_bitcoin_entries(entries)? {
            running = running
                .checked_add(entries[index].balance_delta)
                .ok_or_else(|| {
                    DbError::new("Signed overflow while updating Bitcoin closing balance")
                })?;
            if running < 0 {
                return Err(DbError::new(format!(
                    "Bitcoin closing balance became negative at {}",
                    entries[index].tx_hash
                )));
            }
            let amount = u128::try_from(running)
                .map_err(|_| DbError::new("Failed to convert Bitcoin closing balance"))?;
            entries[index].closing_balance = Some(UnsignedAmount::from_u128(amount));
        }
        Ok(())
    })();
    if result.is_err() {
        entries
            .iter_mut()
            .for_each(|entry| entry.closing_balance = None);
    }
    result
}

pub(super) fn assign_closing_balances(
    entries: &mut [LedgerBuildEntry],
    opening_balance: Option<OpeningBalance>,
) -> Result<(), DbError> {
    let mut confirmed_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            (row.status == ChainTransactionStatus::Confirmed).then_some(index)
        })
        .collect();
    confirmed_indices.sort_by(|left, right| {
        let left_row = &entries[*left];
        let right_row = &entries[*right];
        left_row
            .occurred_at
            .cmp(&right_row.occurred_at)
            .then(
                left_row
                    .block_height
                    .unwrap_or(SQL_MAX_I64)
                    .cmp(&right_row.block_height.unwrap_or(SQL_MAX_I64)),
            )
            .then(
                left_row
                    .nonce
                    .unwrap_or(SQL_MAX_I64)
                    .cmp(&right_row.nonce.unwrap_or(SQL_MAX_I64)),
            )
            .then(
                left_row
                    .min_transfer_index
                    .unwrap_or(SQL_MAX_I64)
                    .cmp(&right_row.min_transfer_index.unwrap_or(SQL_MAX_I64)),
            )
            .then(left_row.tx_hash.cmp(&right_row.tx_hash))
    });

    let mut running = match opening_balance {
        Some(ob) => i128::try_from(ob.amount().value()).unwrap_or(0_i128),
        None => 0_i128,
    };
    for index in confirmed_indices {
        let row = &mut entries[index];
        apply_signed_delta(
            &mut running,
            row.balance_delta,
            "confirmed resulting balance",
        )?;
        row.closing_balance = Some(non_negative_signed_to_unsigned(running)?);
    }

    let mut pending_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            matches!(
                row.status,
                ChainTransactionStatus::Pending
                    | ChainTransactionStatus::Dropped
                    | ChainTransactionStatus::Failed
            )
            .then_some(index)
        })
        .collect();
    pending_indices.sort_by(|left, right| {
        let left_row = &entries[*left];
        let right_row = &entries[*right];
        left_row
            .first_seen_at
            .cmp(&right_row.first_seen_at)
            .then(
                left_row
                    .nonce
                    .unwrap_or(SQL_MAX_I64)
                    .cmp(&right_row.nonce.unwrap_or(SQL_MAX_I64)),
            )
            .then(left_row.tx_hash.cmp(&right_row.tx_hash))
    });

    let mut provisional_running = running;
    for index in pending_indices {
        let row = &mut entries[index];
        if row.status == ChainTransactionStatus::Pending {
            apply_signed_delta(
                &mut provisional_running,
                row.balance_delta,
                "pending closing balance",
            )?;
            row.closing_balance = Some(non_negative_signed_to_unsigned(provisional_running)?);
        } else {
            row.closing_balance = None;
        }
    }

    Ok(())
}

pub(super) fn compute_utxo_opening_balance_for_read_path(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    meta: &AccountMeta,
) -> Result<Option<OpeningBalance>, DbError> {
    if account_model_for(meta.asset_id) != AccountModel::Utxo {
        return Ok(None);
    }

    let entries = collect_entries_for_account(conn, account_id, meta.asset_id, meta.network)?;
    let api_balances = load_api_confirmed_balances_for_account_conn(conn, account_id)?;
    compute_utxo_opening_balance_from_inputs(account_id, &api_balances, &entries)
}

pub(super) fn compute_utxo_opening_balance_from_inputs(
    account_id: DigitalAssetAccountId,
    api_balances: &[crate::db::transaction_sync::AddressApiConfirmedBalanceRow],
    entries: &[LedgerBuildEntry],
) -> Result<Option<OpeningBalance>, DbError> {
    let basis = compute_incomplete_bitcoin_basis_from_inputs(api_balances, entries)?;
    let NativeBalanceState::KnownAmount(amount) = basis else {
        if let Some(row) = api_balances
            .iter()
            .find(|row| row.api_confirmed_balance.is_none())
        {
            tracing::debug!(
                account_id = %account_id,
                address_id = %row.address_id,
                "opening balance correction skipped: address with sync state has no api_confirmed_balance"
            );
        }
        return Ok(None);
    };
    Ok(Some(OpeningBalance(amount)))
}

pub(super) fn compute_incomplete_bitcoin_basis_from_inputs(
    api_balances: &[crate::db::transaction_sync::AddressApiConfirmedBalanceRow],
    entries: &[LedgerBuildEntry],
) -> Result<NativeBalanceState, DbError> {
    let Some(authoritative_total) = sum_complete_api_confirmed_balance(api_balances)? else {
        return Ok(NativeBalanceState::Unknown);
    };
    let confirmed_delta_total = sum_confirmed_balance_deltas(entries)?;
    let authoritative_signed = to_signed_amount(
        authoritative_total,
        "authoritative_api_confirmed_balance_total",
    )?;
    let opening = authoritative_signed
        .checked_sub(confirmed_delta_total)
        .ok_or_else(|| {
            DbError::new("Signed underflow while computing synthetic Bitcoin balance basis")
        })?;
    if opening < 0 {
        return Ok(NativeBalanceState::Unknown);
    }
    let amount = u128::try_from(opening)
        .map_err(|_| DbError::new("Synthetic Bitcoin balance basis exceeded u128 range"))?;
    Ok(NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
        amount,
    )))
}

/// Computes the opening balance for a UTXO-account ledger rebuild.
///
/// Returns `None` for non-UTXO accounts or when the completeness rule
/// (all sync-state-having addresses must have a non-NULL api_confirmed_balance)
/// is not satisfied.
pub(super) fn sum_complete_api_confirmed_balance(
    api_balances: &[crate::db::transaction_sync::AddressApiConfirmedBalanceRow],
) -> Result<Option<UnsignedAmount>, DbError> {
    if api_balances.is_empty() {
        return Ok(None);
    }

    let mut total = UnsignedAmount::zero();
    for row in api_balances {
        let Some(balance) = row.api_confirmed_balance else {
            return Ok(None);
        };
        total = add_amount(
            total,
            balance.amount(),
            "authoritative_api_confirmed_balance_total",
        )?;
    }

    Ok(Some(total))
}

pub(super) fn sum_confirmed_balance_deltas(entries: &[LedgerBuildEntry]) -> Result<i128, DbError> {
    entries
        .iter()
        .filter(|entry| entry.status == ChainTransactionStatus::Confirmed)
        .try_fold(0_i128, |acc, entry| {
            acc.checked_add(entry.balance_delta).ok_or_else(|| {
                DbError::new("Signed overflow while summing confirmed ledger balance deltas")
            })
        })
}

pub(super) fn collect_entries_for_account(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<Vec<LedgerBuildEntry>, DbError> {
    match account_model_for(asset_id) {
        AccountModel::Account => account_model_entries(conn, account_id, asset_id, network),
        AccountModel::Utxo => utxo_model_entries(conn, account_id, asset_id, network),
    }
}

#[derive(Debug, Clone, Copy)]
struct BitcoinCoverageCandidate {
    coverage: BitcoinAccountHistoryCoverage,
    repair_pending: bool,
}

pub(crate) fn load_bitcoin_history_repair_pending(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT EXISTS(
                SELECT 1
                FROM digital_asset_addresses da
                JOIN transaction_sync_state tss
                  ON tss.address_id = da.id
                 AND tss.scope = 'address'
                WHERE da.account_id = ?1
                  AND tss.mempool_history_scan_start_run_id IS NOT NULL
            )",
        [account_id.to_string()],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|err| {
        DbError::new(format!(
            "Failed to check pending Bitcoin history repair: {err}"
        ))
    })
}

fn load_candidate_bitcoin_history_coverage(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    network: Network,
    entries: &[LedgerBuildEntry],
    mask_pending_repair: bool,
) -> Result<BitcoinCoverageCandidate, DbError> {
    let repair_pending = load_bitcoin_history_repair_pending(conn, account_id)?;
    if entries.iter().any(|entry| {
        entry.status == ChainTransactionStatus::Confirmed && entry.block_height.is_none()
    }) {
        return Ok(BitcoinCoverageCandidate {
            coverage: BitcoinAccountHistoryCoverage::Syncing,
            repair_pending,
        });
    }

    let account_kind = conn
        .query_row(
            "SELECT account_kind
             FROM digital_asset_accounts
             WHERE id = ?1 AND asset_id = 'bitcoin'",
            [account_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map_err(|err| DbError::new(format!("Failed to load Bitcoin account kind: {err}")))?;
    let mut coverage_height = None::<i64>;
    if account_kind == "hd_pubkey" {
        let discovery_height = conn
            .query_row(
                "SELECT (
                    SELECT ass.last_scanned_height
                    FROM account_sync_state ass
                    WHERE ass.account_id = ?1
                      AND ass.last_scanned_height IS NOT NULL
                      AND ass.last_scanned_time IS NOT NULL
                      AND ass.mempool_history_next_address_id IS NULL
                      AND NOT EXISTS(
                          SELECT 1
                          FROM hd_account_chain_sync_state h
                          WHERE h.account_id = ass.account_id
                            AND h.derivation_change IN (0, 1)
                      )
                    LIMIT 1
                )",
                [account_id.to_string()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to load Bitcoin HD discovery completion: {err}"
                ))
            })?;
        let Some(discovery_height) = discovery_height else {
            return Ok(BitcoinCoverageCandidate {
                coverage: BitcoinAccountHistoryCoverage::Syncing,
                repair_pending,
            });
        };
        coverage_height = Some(discovery_height);
    }
    if repair_pending && mask_pending_repair {
        return Ok(BitcoinCoverageCandidate {
            coverage: BitcoinAccountHistoryCoverage::Syncing,
            repair_pending,
        });
    }

    let mut statement = conn
        .prepare(
            "SELECT da.id,
                    tss.mempool_history_complete_tx_count,
                    tss.mempool_history_complete_height,
                    tss.mempool_backfill_cursor_txid
             FROM digital_asset_addresses da
             LEFT JOIN transaction_sync_state tss
               ON tss.address_id = da.id
              AND tss.scope = 'address'
             WHERE da.account_id = ?1
             ORDER BY da.id ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare Bitcoin coverage address query: {err}"
            ))
        })?;
    let rows = statement
        .query_map([account_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query Bitcoin coverage addresses: {err}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::new(format!("Failed to read Bitcoin coverage address: {err}")))?;
    if rows.is_empty() {
        return Ok(BitcoinCoverageCandidate {
            coverage: BitcoinAccountHistoryCoverage::Unscanned,
            repair_pending,
        });
    }
    if rows.iter().any(|(_, _, _, cursor)| cursor.is_some()) {
        return Ok(BitcoinCoverageCandidate {
            coverage: BitcoinAccountHistoryCoverage::Syncing,
            repair_pending,
        });
    }

    let any_proof = rows
        .iter()
        .any(|(_, count, height, _)| count.is_some() && height.is_some());
    for (address_id, proof_count, proof_height, _) in rows {
        let (Some(proof_count), Some(proof_height)) = (proof_count, proof_height) else {
            return Ok(BitcoinCoverageCandidate {
                coverage: if any_proof {
                    BitcoinAccountHistoryCoverage::Syncing
                } else {
                    BitcoinAccountHistoryCoverage::Unscanned
                },
                repair_pending,
            });
        };
        let canonical_count = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM (
                    SELECT ct.tx_hash
                    FROM chain_transactions ct
                    JOIN transaction_inputs ti ON ti.tx_id = ct.id
                    WHERE ct.asset_id = 'bitcoin'
                      AND ct.network = ?2
                      AND ct.status = 'confirmed'
                      AND ti.address_id = ?1
                    UNION
                    SELECT ct.tx_hash
                    FROM chain_transactions ct
                    JOIN transaction_outputs txo ON txo.tx_id = ct.id
                    WHERE ct.asset_id = 'bitcoin'
                      AND ct.network = ?2
                      AND ct.status = 'confirmed'
                      AND txo.address_id = ?1
                 )",
                params![address_id, network.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to count canonical Bitcoin address history: {err}"
                ))
            })?;
        if proof_count != canonical_count {
            return Ok(BitcoinCoverageCandidate {
                coverage: BitcoinAccountHistoryCoverage::Syncing,
                repair_pending,
            });
        }
        coverage_height =
            Some(coverage_height.map_or(proof_height, |current| current.min(proof_height)));
    }

    let coverage_height = coverage_height
        .ok_or_else(|| DbError::new("Complete Bitcoin coverage has no proof height"))
        .and_then(|height| {
            crate::transactions::ChainTipHeight::try_new(height)
                .map_err(|err| DbError::new(format!("Invalid Bitcoin coverage height: {err}")))
        })?;
    Ok(BitcoinCoverageCandidate {
        coverage: BitcoinAccountHistoryCoverage::Complete { coverage_height },
        repair_pending,
    })
}

pub(crate) fn load_bitcoin_account_history_coverage(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<BitcoinAccountHistoryCoverage>, DbError> {
    let asset_id = conn
        .query_row(
            "SELECT asset_id FROM digital_asset_accounts WHERE id = ?1",
            [account_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load coverage account asset: {err}")))?;
    if asset_id.as_deref() != Some(SyncedAssetId::Bitcoin.as_str()) {
        return Ok(None);
    }
    let meta = load_account_meta(conn, account_id)?;
    let entries = collect_entries_for_account(conn, account_id, meta.asset_id, meta.network)?;
    let candidate =
        load_candidate_bitcoin_history_coverage(conn, account_id, meta.network, &entries, true)?;
    let coverage = match candidate.coverage {
        BitcoinAccountHistoryCoverage::Complete { .. }
            if !has_current_mempool_account_integration_success(conn, account_id)? =>
        {
            BitcoinAccountHistoryCoverage::Syncing
        }
        coverage => coverage,
    };
    Ok(Some(coverage))
}

pub(in crate::db) fn bitcoin_account_has_complete_history_proof_for_repair(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    let meta = load_account_meta(conn, account_id)?;
    let entries = collect_entries_for_account(conn, account_id, meta.asset_id, meta.network)?;
    Ok(matches!(
        load_candidate_bitcoin_history_coverage(conn, account_id, meta.network, &entries, false,)?
            .coverage,
        BitcoinAccountHistoryCoverage::Complete { .. }
    ) && has_current_mempool_account_integration_success(conn, account_id)?)
}

fn has_current_mempool_account_integration_success(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    let row = conn
        .query_row(
            "SELECT last_started_at, last_completed_at, last_result
             FROM account_integration_sync_state
             WHERE account_id = ?1 AND integration_id = ?2",
            params![
                account_id.to_string(),
                SyncIntegrationId::Mempool.as_db_value()
            ],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load Mempool account integration completion: {err}"
            ))
        })?;
    let Some((Some(started_at), Some(completed_at), Some(result))) = row else {
        return Ok(false);
    };
    if result != AggregateSyncResult::Success.as_db_value() {
        return Ok(false);
    }

    let started_at = parse_datetime(&started_at)
        .map_err(|err| DbError::new(format!("Invalid Mempool account start time: {err}")))?;
    let completed_at = parse_datetime(&completed_at)
        .map_err(|err| DbError::new(format!("Invalid Mempool account completion time: {err}")))?;
    Ok(completed_at >= started_at)
}

fn build_account_transaction_ledger(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    bitcoin_basis_override: Option<NativeBalanceState>,
) -> Result<(AccountMeta, Vec<LedgerBuildEntry>), DbError> {
    let meta = load_account_meta(conn, account_id)?;
    let mut entries = collect_entries_for_account(conn, account_id, meta.asset_id, meta.network)?;
    match account_model_for(meta.asset_id) {
        AccountModel::Account => assign_closing_balances(&mut entries, None)?,
        AccountModel::Utxo => {
            let basis = match bitcoin_basis_override {
                Some(basis) => basis,
                None => {
                    let candidate = load_candidate_bitcoin_history_coverage(
                        conn,
                        account_id,
                        meta.network,
                        &entries,
                        true,
                    )?;
                    match candidate.coverage {
                        BitcoinAccountHistoryCoverage::Complete { .. } => {
                            NativeBalanceState::CanonicalZero
                        }
                        _ if candidate.repair_pending => NativeBalanceState::Unknown,
                        BitcoinAccountHistoryCoverage::Unscanned
                        | BitcoinAccountHistoryCoverage::Syncing
                        | BitcoinAccountHistoryCoverage::Limited => {
                            let balances =
                                load_api_confirmed_balances_for_account_conn(conn, account_id)?;
                            compute_incomplete_bitcoin_basis_from_inputs(&balances, &entries)?
                        }
                    }
                }
            };
            assign_bitcoin_closing_balances(&mut entries, basis)?;
        }
    }
    Ok((meta, entries))
}

fn clear_account_closing_balances(
    conn: &mut rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    let transaction = conn.transaction().map_err(|err| {
        DbError::new(format!(
            "Failed to start unavailable Bitcoin ledger transaction: {err}"
        ))
    })?;
    transaction
        .execute(
            "UPDATE account_transaction_ledger
             SET closing_balance_hi = NULL,
                 closing_balance_lo = NULL,
                 updated_at = ?1
             WHERE account_id = ?2",
            params![observed_at.to_rfc3339(), account_id.to_string()],
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to clear unavailable Bitcoin closing balances: {err}"
            ))
        })?;
    transaction.commit().map_err(|err| {
        DbError::new(format!(
            "Failed to commit unavailable Bitcoin closing balances: {err}"
        ))
    })
}

pub(crate) fn rebuild_account_transaction_ledger(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        rebuild_account_transaction_ledger_conn_with_basis(conn, account_id, observed_at, None)
    })
}

pub(crate) fn rebuild_account_transaction_ledger_with_unknown_bitcoin_basis(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        rebuild_account_transaction_ledger_conn_with_basis(
            conn,
            account_id,
            observed_at,
            Some(NativeBalanceState::Unknown),
        )
    })
}

pub(crate) fn rebuild_account_transaction_ledger_conn(
    conn: &mut rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    observed_at: DateTime<Utc>,
) -> Result<(), DbError> {
    rebuild_account_transaction_ledger_conn_with_basis(conn, account_id, observed_at, None)
}

fn rebuild_account_transaction_ledger_conn_with_basis(
    conn: &mut rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    observed_at: DateTime<Utc>,
    bitcoin_basis_override: Option<NativeBalanceState>,
) -> Result<(), DbError> {
    let build = build_account_transaction_ledger(conn, account_id, bitcoin_basis_override);
    match build {
        Ok((meta, entries)) => {
            let replacement = replace_account_transaction_ledger_entries(
                conn,
                account_id,
                &meta,
                observed_at,
                entries,
            );
            if let Err(error) = replacement {
                if meta.asset_id == SyncedAssetId::Bitcoin
                    && account_model_for(meta.asset_id) == AccountModel::Utxo
                {
                    clear_account_closing_balances(conn, account_id, observed_at)?;
                }
                return Err(error);
            }
            Ok(())
        }
        Err(error) => {
            if load_account_meta(conn, account_id)
                .is_ok_and(|meta| account_model_for(meta.asset_id) == AccountModel::Utxo)
            {
                clear_account_closing_balances(conn, account_id, observed_at)?;
            }
            Err(error)
        }
    }
}

pub(crate) fn publish_bitcoin_account_completion(
    user_id: UserId,
    publication: BitcoinAccountCompletionPublication,
) -> Result<bool, DbError> {
    with_user_db_mut(user_id, |conn| {
        let clear_on_failure = load_account_meta(conn, publication.account_id).is_ok_and(|meta| {
            meta.asset_id == SyncedAssetId::Bitcoin
                && account_model_for(meta.asset_id) == AccountModel::Utxo
        });
        let transaction = conn.transaction().map_err(|err| {
            DbError::new(format!(
                "Failed to start Bitcoin completion publication: {err}"
            ))
        })?;
        let publication_result = publish_bitcoin_account_completion_tx(&transaction, publication);
        let complete = match publication_result {
            Ok(complete) => complete,
            Err(error) => {
                drop(transaction);
                if clear_on_failure {
                    clear_account_closing_balances(
                        conn,
                        publication.account_id,
                        publication.observed_at,
                    )?;
                }
                return Err(error);
            }
        };
        if let Err(error) = transaction.commit() {
            if clear_on_failure {
                clear_account_closing_balances(
                    conn,
                    publication.account_id,
                    publication.observed_at,
                )?;
            }
            return Err(DbError::new(format!(
                "Failed to commit Bitcoin completion publication: {error}"
            )));
        }
        Ok(complete)
    })
}

fn publish_bitcoin_account_completion_tx(
    transaction: &rusqlite::Transaction<'_>,
    publication: BitcoinAccountCompletionPublication,
) -> Result<bool, DbError> {
    if publication.final_address_proof.is_none() && publication.completed_hd_discovery.is_none() {
        return Err(DbError::new(
            "Bitcoin completion publication requires proof or discovery",
        ));
    }
    let meta = load_account_meta(transaction, publication.account_id)?;
    if meta.asset_id != SyncedAssetId::Bitcoin {
        return Err(DbError::new(
            "Bitcoin completion publication requires a Bitcoin account",
        ));
    }
    let mut entries = collect_entries_for_account(
        transaction,
        publication.account_id,
        meta.asset_id,
        meta.network,
    )?;

    if let Some(address_proof) = publication.final_address_proof {
        let belongs_to_account = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM digital_asset_addresses
                    WHERE id = ?1 AND account_id = ?2
                )",
                params![
                    address_proof.address_id.to_string(),
                    publication.account_id.to_string(),
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to validate Bitcoin proof account ownership: {err}"
                ))
            })?;
        if !belongs_to_account {
            return Err(DbError::new(
                "Bitcoin proof address does not belong to completion account",
            ));
        }
        let updated_at = publication.observed_at.to_rfc3339();
        match address_proof.scan_start_run_id {
            Some(scan_start_run_id) => {
                crate::db::transaction_sync::publish_strict_mempool_history_proof_conn(
                    transaction,
                    address_proof.address_id,
                    scan_start_run_id,
                    address_proof.proof,
                    &updated_at,
                )?
            }
            None => crate::db::transaction_sync::publish_mempool_history_proof_conn(
                transaction,
                address_proof.address_id,
                address_proof.proof,
                &updated_at,
            )?,
        }
    }

    if let Some(discovery) = publication.completed_hd_discovery {
        crate::db::transaction_sync::complete_hd_account_discovery_conn(
            transaction,
            publication.account_id,
            discovery.external_last_index,
            discovery.internal_last_index,
            discovery.completed_tip,
            discovery.completed_at,
        )?;
    }

    if !matches!(
        load_candidate_bitcoin_history_coverage(
            transaction,
            publication.account_id,
            meta.network,
            &entries,
            true,
        )?
        .coverage,
        BitcoinAccountHistoryCoverage::Complete { .. }
    ) {
        return Ok(false);
    }

    assign_bitcoin_closing_balances(&mut entries, NativeBalanceState::CanonicalZero)?;
    replace_account_transaction_ledger_entries_conn(
        transaction,
        publication.account_id,
        &meta,
        publication.observed_at,
        entries,
    )?;
    Ok(true)
}

fn replace_account_transaction_ledger_entries(
    conn: &mut rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    meta: &AccountMeta,
    observed_at: DateTime<Utc>,
    entries: Vec<LedgerBuildEntry>,
) -> Result<(), DbError> {
    let sql_tx = conn
        .transaction()
        .map_err(|err| DbError::new(format!("Failed to start ledger transaction: {err}")))?;
    replace_account_transaction_ledger_entries_conn(
        &sql_tx,
        account_id,
        meta,
        observed_at,
        entries,
    )?;
    sql_tx.commit().map_err(|err| {
        DbError::new(format!(
            "Failed to commit account transaction ledger rebuild: {err}"
        ))
    })
}

fn replace_account_transaction_ledger_entries_conn(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    meta: &AccountMeta,
    observed_at: DateTime<Utc>,
    entries: Vec<LedgerBuildEntry>,
) -> Result<(), DbError> {
    let now_raw = observed_at.to_rfc3339();
    let account_id_raw = account_id.to_string();
    conn.execute(
        "DELETE FROM account_transaction_ledger WHERE account_id = ?1",
        params![account_id_raw],
    )
    .map_err(|err| DbError::new(format!("Failed to clear account transaction ledger: {err}")))?;

    let mut insert_stmt = conn
        .prepare(
            "INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status, occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type, from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo, fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo, balance_delta_hi, balance_delta_lo, balance_delta_negative, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare account transaction ledger insert statement: {err}"
            ))
        })?;

    for row in entries {
        let value_parts = split_unsigned_amount(row.value, "value_amount")?;
        let (fee_hi, fee_lo) = match row.fee {
            Some(value) => {
                let parts = split_unsigned_amount(value, "fee_amount")?;
                (Some(parts.hi), Some(parts.lo))
            }
            None => (None, None),
        };
        let (closing_hi, closing_lo) = match row.closing_balance {
            Some(value) => {
                let parts = split_unsigned_amount(value, "closing_balance")?;
                (Some(parts.hi), Some(parts.lo))
            }
            None => (None, None),
        };
        let from_addresses_json = serde_json::to_string(&row.from_addresses)
            .map_err(|err| DbError::new(format!("Failed to encode from addresses JSON: {err}")))?;
        let to_addresses_json = serde_json::to_string(&row.to_addresses)
            .map_err(|err| DbError::new(format!("Failed to encode to addresses JSON: {err}")))?;
        let (delta_parts, delta_negative) = split_signed_balance_delta(row.balance_delta)?;

        insert_stmt
            .execute(params![
                Ulid::new().to_string(),
                account_id.to_string(),
                row.chain_transaction_id,
                meta.asset_id.as_str(),
                meta.network.as_str(),
                row.tx_hash,
                row.status.as_db_value(),
                row.occurred_at.to_rfc3339(),
                row.first_seen_at.to_rfc3339(),
                row.block_height,
                row.nonce,
                row.min_transfer_index,
                direction_to_db_value(row.direction),
                from_addresses_json,
                to_addresses_json,
                value_parts.hi,
                value_parts.lo,
                fee_hi,
                fee_lo,
                closing_hi,
                closing_lo,
                delta_parts.hi,
                delta_parts.lo,
                i64::from(delta_negative),
                now_raw,
                now_raw,
            ])
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to insert account transaction ledger row: {err}"
                ))
            })?;
    }

    drop(insert_stmt);
    Ok(())
}

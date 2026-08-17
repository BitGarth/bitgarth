use super::super::amount_storage::{
    parse_optional_split_amount as parse_optional_split_amount_parts,
    split_unsigned_amount as split_unsigned_amount_parts,
};
use super::super::error::DbError;
use super::super::raw_ingestion::SyncRunId;
use super::types::*;
use crate::models::parse_datetime;
use crate::transactions::{
    AddressCount, AggregateSyncResult, ApiConfirmedBalance, ChainTipHeight, EthereumBlockNumber,
    MempoolCursorTxid, TrackedAddress, TransactionCount, TransactionSyncResult,
};
use crate::wallets::{
    AddressScheme, DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId,
};
use chrono::{DateTime, Utc};
use std::str::FromStr;

pub(super) fn parse_transaction_count(
    value: i64,
    field: &'static str,
) -> Result<TransactionCount, DbError> {
    TransactionCount::try_new(value)
        .map_err(|err| DbError::new(format!("Invalid {field} in DB: {err}")))
}

pub(super) fn split_api_confirmed_balance(
    balance: ApiConfirmedBalance,
) -> Result<(i64, i64), DbError> {
    let parts = split_unsigned_amount_parts(balance.amount())
        .map_err(|err| DbError::new(format!("Failed to split api_confirmed_balance: {err}")))?;
    Ok((parts.hi, parts.lo))
}

pub(super) fn parse_optional_api_confirmed_balance(
    hi: Option<i64>,
    lo: Option<i64>,
) -> Result<Option<ApiConfirmedBalance>, DbError> {
    parse_optional_split_amount_parts(hi, lo)
        .map(|value| value.map(ApiConfirmedBalance::from_amount))
        .map_err(|err| DbError::new(format!("Invalid api_confirmed_balance in DB: {err}")))
}

pub(super) fn parse_address_count(
    value: i64,
    field: &'static str,
) -> Result<AddressCount, DbError> {
    let as_u32 =
        u32::try_from(value).map_err(|_| DbError::new(format!("Invalid {field} in DB")))?;
    Ok(AddressCount::from_u32(as_u32))
}

pub(super) fn parse_address_id(id: &str) -> Result<DigitalAssetAddressId, DbError> {
    DigitalAssetAddressId::from_str(id)
        .map_err(|err| DbError::new(format!("Invalid address id in DB: {err}")))
}

pub(super) fn parse_account_id(id: &str) -> Result<DigitalAssetAccountId, DbError> {
    DigitalAssetAccountId::from_str(id)
        .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))
}

pub(super) fn parse_tracked_address(address: &str) -> Result<TrackedAddress, DbError> {
    TrackedAddress::parse(address)
        .map_err(|err| DbError::new(format!("Invalid address in DB: {err}")))
}

pub(super) fn parse_asset_id(asset_id: &str) -> Result<SyncedAssetId, DbError> {
    SyncedAssetId::from_str(asset_id)
        .ok_or_else(|| DbError::new(format!("Invalid asset_id in DB: {asset_id}")))
}

pub(super) fn parse_network(network: &str) -> Result<Network, DbError> {
    Network::from_str(network)
        .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network}")))
}

pub(super) fn parse_address_scheme(value: &str) -> Result<AddressScheme, DbError> {
    AddressScheme::from_str(value)
        .ok_or_else(|| DbError::new(format!("Invalid address scheme in DB: {value}")))
}

pub(super) fn parse_optional_u32(
    value: Option<i64>,
    field: &'static str,
) -> Result<Option<u32>, DbError> {
    match value {
        Some(value) if value < 0 => Err(DbError::new(format!("{field} cannot be negative"))),
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| DbError::new(format!("{field} out of u32 range"))),
        None => Ok(None),
    }
}

pub(super) fn parse_required_u32(value: i64, field: &'static str) -> Result<u32, DbError> {
    if value < 0 {
        return Err(DbError::new(format!("{field} cannot be negative")));
    }
    u32::try_from(value).map_err(|_| DbError::new(format!("{field} out of u32 range")))
}

pub(super) fn parse_hd_account_chain_frontier_phase(
    value: &str,
) -> Result<HdAccountChainFrontierPhase, DbError> {
    match value {
        "existing_addresses" => Ok(HdAccountChainFrontierPhase::ExistingAddresses),
        "derived_addresses" => Ok(HdAccountChainFrontierPhase::DerivedAddresses),
        "active_rescan" => Ok(HdAccountChainFrontierPhase::ActiveRescan),
        _ => Err(DbError::new(format!(
            "Invalid frontier_phase in hd_account_chain_sync_state: {value}"
        ))),
    }
}

pub(super) fn parse_optional_tip_height(
    value: Option<i64>,
) -> Result<Option<ChainTipHeight>, DbError> {
    value
        .map(ChainTipHeight::try_new)
        .transpose()
        .map_err(|err| DbError::new(format!("Invalid last_tip_height in DB: {err}")))
}

pub(super) fn parse_optional_transaction_count(
    value: Option<i64>,
    field: &'static str,
) -> Result<Option<TransactionCount>, DbError> {
    value
        .map(|raw| parse_transaction_count(raw, field))
        .transpose()
}

pub(super) fn parse_optional_mempool_history_proof(
    confirmed_tx_count: Option<i64>,
    complete_height: Option<i64>,
) -> Result<Option<MempoolHistoryProof>, DbError> {
    match (confirmed_tx_count, complete_height) {
        (None, None) => Ok(None),
        (Some(confirmed_tx_count), Some(complete_height)) => Ok(Some(MempoolHistoryProof {
            confirmed_tx_count: parse_transaction_count(
                confirmed_tx_count,
                "mempool_history_complete_tx_count",
            )?,
            complete_height: ChainTipHeight::try_new(complete_height).map_err(|err| {
                DbError::new(format!(
                    "Invalid mempool_history_complete_height in DB: {err}"
                ))
            })?,
        })),
        _ => Err(DbError::new("Invalid unpaired mempool history proof in DB")),
    }
}

pub(super) fn parse_optional_sync_run_id(
    raw: Option<String>,
    field: &'static str,
) -> Result<Option<SyncRunId>, DbError> {
    raw.as_deref()
        .map(SyncRunId::from_str)
        .transpose()
        .map_err(|err| DbError::new(format!("Invalid {field} in DB: {err}")))
}

pub(super) fn parse_optional_sync_result(
    value: Option<String>,
) -> Result<Option<TransactionSyncResult>, DbError> {
    value
        .map(|raw| {
            TransactionSyncResult::from_db_value(&raw)
                .ok_or_else(|| DbError::new(format!("Invalid sync result in DB: {raw}")))
        })
        .transpose()
}

pub(super) fn parse_optional_aggregate_sync_result(
    value: Option<String>,
) -> Result<Option<AggregateSyncResult>, DbError> {
    value
        .map(|raw| {
            AggregateSyncResult::from_db_value(&raw)
                .ok_or_else(|| DbError::new(format!("Invalid aggregate sync result in DB: {raw}")))
        })
        .transpose()
}

pub(super) fn parse_etherscan_history_checkpoint_verified(
    value: Option<i64>,
) -> Result<bool, DbError> {
    match value {
        None => Ok(false),
        Some(1) => Ok(true),
        Some(raw) => Err(DbError::new(format!(
            "Invalid etherscan history checkpoint version in DB: {raw}"
        ))),
    }
}

pub(super) fn parse_optional_time(
    raw: Option<String>,
    field_name: &'static str,
) -> Result<Option<DateTime<Utc>>, DbError> {
    raw.as_deref()
        .map(parse_datetime)
        .transpose()
        .map_err(|err| DbError::new(format!("Invalid {field_name} in DB: {err}")))
}

pub(super) fn parse_optional_mempool_cursor_txid(
    raw: Option<String>,
) -> Result<Option<MempoolCursorTxid>, DbError> {
    raw.as_deref()
        .map(MempoolCursorTxid::parse)
        .transpose()
        .map_err(|err| DbError::new(format!("Invalid mempool_backfill_cursor_txid in DB: {err}")))
}

pub(super) fn parse_optional_ethereum_block_number(
    raw: Option<i64>,
) -> Result<Option<EthereumBlockNumber>, DbError> {
    raw.map(EthereumBlockNumber::try_new)
        .transpose()
        .map_err(|err| DbError::new(format!("Invalid etherscan_backfill_end_block in DB: {err}")))
}

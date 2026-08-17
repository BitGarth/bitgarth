use crate::amounts::AmountSplitParts;
use crate::amounts::UnsignedAmount;
use crate::balance_reliability::BalanceReliability;
use crate::db::amount_storage::{
    parse_optional_split_amount as parse_optional_split_amount_parts,
    parse_split_amount as parse_split_amount_parts, parse_split_sum,
    split_unsigned_amount as split_unsigned_amount_parts,
};
use crate::db::error::DbError;
use crate::transactions::{AccountTransactionDirection, ChainTransactionStatus};
use crate::wallets::{DigitalAssetAccountId, Network, SyncedAssetId, WalletId};
use chrono::DateTime;
use chrono::Utc;
use std::str::FromStr;

pub(super) use crate::transactions::NativeBalanceState;

pub(super) const TX_TYPE_RECEIVE: &str = "receive";
pub(super) const TX_TYPE_SEND: &str = "send";
pub(super) const TX_TYPE_SELF_TRANSFER: &str = "self_transfer";
pub(super) const SQL_MAX_I64: i64 = 9_223_372_036_854_775_807;

/// The opening balance for a ledger rebuild, derived from the authoritative
/// API-reported confirmed balance minus the sum of fetched confirmed deltas.
/// Wraps `UnsignedAmount` in the asset's smallest unit (satoshis for Bitcoin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpeningBalance(pub(super) UnsignedAmount);

impl OpeningBalance {
    pub(super) fn amount(self) -> UnsignedAmount {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountTransactionTableKind {
    Pending,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountTransactionLedgerRow {
    pub(crate) tx_hash: String,
    pub(crate) status: ChainTransactionStatus,
    pub(crate) direction: AccountTransactionDirection,
    pub(crate) occurred_at: DateTime<Utc>,
    pub(crate) from_addresses: Vec<String>,
    pub(crate) to_addresses: Vec<String>,
    pub(crate) value: UnsignedAmount,
    pub(crate) fee: Option<UnsignedAmount>,
    pub(crate) closing_balance: Option<UnsignedAmount>,
    pub(crate) balance_reliability: BalanceReliability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountTransactionLedgerPage {
    pub(crate) page: u32,
    pub(crate) page_size: u32,
    pub(crate) total: u32,
    pub(crate) rows: Vec<AccountTransactionLedgerRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountReferenceInfo {
    pub(crate) is_hd: bool,
    pub(crate) address_scheme: crate::wallets::AddressScheme,
    pub(crate) reference_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountTransactionsPages {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) wallet_id: WalletId,
    pub(crate) wallet_label: String,
    pub(crate) account_label: Option<String>,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) bitcoin_history_coverage:
        Option<crate::db::transaction_sync::BitcoinAccountHistoryCoverage>,
    pub(crate) account_reference: AccountReferenceInfo,
    pub(crate) has_ingested_history: bool,
    pub(crate) current_balance_state: NativeBalanceState,
    pub(crate) current_balance_checked_at: Option<DateTime<Utc>>,
    pub(crate) opening_balance_state: NativeBalanceState,
    pub(crate) opening_balance_reliability: BalanceReliability,
    pub(crate) opening_balance_date: Option<DateTime<Utc>>,
    pub(crate) closing_balance_state: NativeBalanceState,
    pub(crate) closing_balance_reliability: BalanceReliability,
    pub(crate) closing_balance_date: Option<DateTime<Utc>>,
    pub(crate) pending: AccountTransactionLedgerPage,
    pub(crate) confirmed: AccountTransactionLedgerPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeBalanceBoundaryKind {
    Opening,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeBalanceBoundaryResolution {
    pub(super) state: NativeBalanceState,
    pub(super) amount: Option<UnsignedAmount>,
    pub(super) balance_reliability: BalanceReliability,
    pub(super) balance_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountMeta {
    pub(super) wallet_id: WalletId,
    pub(super) asset_id: SyncedAssetId,
    pub(super) network: Network,
    pub(super) label: Option<String>,
    pub(super) wallet_label: String,
}

pub(in crate::db) fn split_unsigned_amount(
    amount: UnsignedAmount,
    field_name: &'static str,
) -> Result<AmountSplitParts, DbError> {
    split_unsigned_amount_parts(amount)
        .map_err(|err| DbError::new(format!("Failed to split {field_name}: {err}")))
}

pub(in crate::db) fn parse_split_amount(
    hi: i64,
    lo: i64,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    parse_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

pub(super) fn parse_optional_split_amount(
    hi: Option<i64>,
    lo: Option<i64>,
    field_name: &'static str,
) -> Result<Option<UnsignedAmount>, DbError> {
    parse_optional_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

pub(super) fn parse_split_sum_amount(
    hi_sum: i64,
    lo_sum: i64,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    parse_split_sum(hi_sum, lo_sum)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split sum from DB: {err}")))
}

pub(super) fn parse_chain_status(raw: &str) -> Result<ChainTransactionStatus, DbError> {
    ChainTransactionStatus::from_db_value(raw)
        .ok_or_else(|| DbError::new(format!("Invalid transaction status in DB: {raw}")))
}

pub(super) fn parse_asset_id(raw: &str) -> Result<SyncedAssetId, DbError> {
    SyncedAssetId::from_str(raw)
        .ok_or_else(|| DbError::new(format!("Invalid asset_id in DB: {raw}")))
}

pub(super) fn parse_network(raw: &str) -> Result<Network, DbError> {
    Network::from_str(raw).ok_or_else(|| DbError::new(format!("Invalid network in DB: {raw}")))
}

pub(super) fn parse_wallet_id(raw: &str) -> Result<WalletId, DbError> {
    WalletId::from_str(raw).map_err(|err| DbError::new(format!("Invalid wallet_id in DB: {err}")))
}

pub(super) fn parse_direction(raw: &str) -> Result<AccountTransactionDirection, DbError> {
    match raw {
        TX_TYPE_RECEIVE => Ok(AccountTransactionDirection::Incoming),
        TX_TYPE_SEND => Ok(AccountTransactionDirection::Outgoing),
        TX_TYPE_SELF_TRANSFER => Ok(AccountTransactionDirection::SelfTransfer),
        _ => Err(DbError::new(format!("Invalid tx_type in DB: {raw}"))),
    }
}

pub(super) fn direction_to_db_value(direction: AccountTransactionDirection) -> &'static str {
    match direction {
        AccountTransactionDirection::Incoming => TX_TYPE_RECEIVE,
        AccountTransactionDirection::Outgoing => TX_TYPE_SEND,
        AccountTransactionDirection::SelfTransfer => TX_TYPE_SELF_TRANSFER,
    }
}

pub(super) fn i64_to_u32(value: i64, field: &'static str) -> Result<u32, DbError> {
    u32::try_from(value).map_err(|_| DbError::new(format!("{field} out of range")))
}

pub(super) fn to_signed_amount(
    value: UnsignedAmount,
    field_name: &'static str,
) -> Result<i128, DbError> {
    i128::try_from(value.value())
        .map_err(|_| DbError::new(format!("Amount {field_name} exceeds i128 range")))
}

/// Splits a signed per-transaction balance delta into an unsigned magnitude
/// (hi/lo split parts) plus a sign flag. A zero delta is canonically
/// non-negative, so the magnitude is never paired with a `true` sign.
pub(super) fn split_signed_balance_delta(delta: i128) -> Result<(AmountSplitParts, bool), DbError> {
    let negative = delta < 0;
    let magnitude = UnsignedAmount::from_u128(delta.unsigned_abs());
    let parts = split_unsigned_amount(magnitude, "balance_delta")?;
    Ok((parts, negative))
}

/// Rebuilds a signed balance delta from stored magnitude parts and the sign flag.
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(in crate::db) fn signed_balance_delta_from_split(
    hi: i64,
    lo: i64,
    negative: bool,
) -> Result<i128, DbError> {
    let magnitude = parse_split_amount(hi, lo, "balance_delta")?;
    let raw = magnitude.value();

    if negative && raw == i128::MAX as u128 + 1 {
        return Ok(i128::MIN);
    }

    let as_i128 = i128::try_from(raw)
        .map_err(|_| DbError::new("balance_delta magnitude exceeds i128 range"))?;
    Ok(if negative { -as_i128 } else { as_i128 })
}

pub(super) fn apply_signed_delta(
    running_total: &mut i128,
    delta: i128,
    field_name: &str,
) -> Result<(), DbError> {
    *running_total = running_total
        .checked_add(delta)
        .ok_or_else(|| DbError::new(format!("Signed overflow while updating {field_name}")))?;
    Ok(())
}

pub(super) fn non_negative_signed_to_unsigned(value: i128) -> Result<UnsignedAmount, DbError> {
    if value <= 0 {
        return Ok(UnsignedAmount::zero());
    }

    let as_u128 =
        u128::try_from(value).map_err(|_| DbError::new("Failed to convert signed amount"))?;
    Ok(UnsignedAmount::from_u128(as_u128))
}

pub(super) fn add_amount(
    accumulator: UnsignedAmount,
    value: UnsignedAmount,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    accumulator
        .checked_add(value)
        .map_err(|err| DbError::new(format!("Overflow while summing {field_name}: {err}")))
}

#[cfg(test)]
mod balance_delta_tests {
    use super::*;

    #[test]
    fn signed_delta_round_trips_negative() {
        let (parts, negative) = split_signed_balance_delta(-888).expect("split");
        assert!(negative);
        let restored =
            signed_balance_delta_from_split(parts.hi, parts.lo, negative).expect("parse");
        assert_eq!(restored, -888);
    }

    #[test]
    fn i128_min_exceeds_current_split_storage_range() {
        let result = split_signed_balance_delta(i128::MIN);

        assert!(
            result.is_err(),
            "current 10^18 hi/lo storage cannot encode 2^127"
        );
    }

    #[test]
    fn signed_delta_round_trips_positive() {
        let (parts, negative) = split_signed_balance_delta(14_352_846_507_848).expect("split");
        assert!(!negative);
        let restored =
            signed_balance_delta_from_split(parts.hi, parts.lo, negative).expect("parse");
        assert_eq!(restored, 14_352_846_507_848);
    }

    #[test]
    fn zero_delta_is_canonically_non_negative() {
        let (parts, negative) = split_signed_balance_delta(0).expect("split");
        assert!(!negative, "zero delta must never be flagged negative");
        assert_eq!(parts.hi, 0);
        assert_eq!(parts.lo, 0);
    }
}

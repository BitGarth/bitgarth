#![cfg_attr(
    not(feature = "server"),
    expect(
        dead_code,
        reason = "Transaction sync domain types are primarily exercised on server paths"
    )
)]

#[cfg(any(feature = "server", test))]
use crate::amounts::AmountError;
use crate::amounts::UnsignedAmount;
use crate::wallets::SyncedAssetId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TrackedAddress(String);

impl TrackedAddress {
    pub(crate) fn parse(raw: &str) -> Result<Self, AddressError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(AddressError::Empty);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddressError {
    Empty,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::Empty => write!(f, "address cannot be empty"),
        }
    }
}

impl std::error::Error for AddressError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TxHash(String);

impl TxHash {
    pub(crate) fn parse(raw: &str) -> Result<Self, TxHashError> {
        let trimmed = raw.trim();
        if trimmed.len() != 64 {
            return Err(TxHashError::WrongLength(trimmed.len()));
        }
        if !trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(TxHashError::InvalidCharacters);
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TxHashError {
    WrongLength(usize),
    InvalidCharacters,
}

impl fmt::Display for TxHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxHashError::WrongLength(length) => {
                write!(f, "tx hash must be 64 hex chars, got length {length}")
            }
            TxHashError::InvalidCharacters => write!(f, "tx hash contains non-hex characters"),
        }
    }
}

impl std::error::Error for TxHashError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct MempoolCursorTxid(TxHash);

impl MempoolCursorTxid {
    pub(crate) fn parse(raw: &str) -> Result<Self, TxHashError> {
        TxHash::parse(raw).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChainTransactionStatus {
    Pending,
    Confirmed,
    Dropped,
    /// Transaction was included in a block but reverted (e.g. EVM revert).
    Failed,
}

impl ChainTransactionStatus {
    pub(crate) fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "confirmed" => Some(Self::Confirmed),
            "dropped" => Some(Self::Dropped),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub(crate) const fn as_db_value(self) -> &'static str {
        match self {
            ChainTransactionStatus::Pending => "pending",
            ChainTransactionStatus::Confirmed => "confirmed",
            ChainTransactionStatus::Dropped => "dropped",
            ChainTransactionStatus::Failed => "failed",
        }
    }
}

/// Provider-authoritative confirmed balance observed during sync.
/// Stored in the asset's smallest unit for the tracked address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ApiConfirmedBalance(UnsignedAmount);

impl ApiConfirmedBalance {
    #[cfg(any(feature = "server", test))]
    pub(crate) fn from_smallest_unit_i64(value: i64) -> Result<Self, AmountError> {
        Ok(Self(UnsignedAmount::try_from_i64(value)?))
    }

    pub(crate) const fn from_amount(amount: UnsignedAmount) -> Self {
        Self(amount)
    }

    pub(crate) fn amount(self) -> UnsignedAmount {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TransactionSyncRunId(Ulid);

impl TransactionSyncRunId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for TransactionSyncRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TransactionSyncRunId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for TransactionSyncRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TransactionSyncRunId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChainTipHeight(i64);

impl ChainTipHeight {
    pub(crate) fn try_new(value: i64) -> Result<Self, ChainTipHeightError> {
        if value < 0 {
            return Err(ChainTipHeightError::Negative(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn value(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChainTipHeightError {
    Negative(i64),
}

impl fmt::Display for ChainTipHeightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainTipHeightError::Negative(value) => {
                write!(f, "chain tip height must be non-negative, got {value}")
            }
        }
    }
}

impl std::error::Error for ChainTipHeightError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct EthereumBlockNumber(i64);

impl EthereumBlockNumber {
    pub(crate) fn try_new(value: i64) -> Result<Self, EthereumBlockNumberError> {
        if value < 0 {
            return Err(EthereumBlockNumberError::Negative(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn from_u64(value: u64) -> Result<Self, EthereumBlockNumberError> {
        let as_i64 =
            i64::try_from(value).map_err(|_| EthereumBlockNumberError::OutOfRange(value))?;
        Self::try_new(as_i64)
    }

    pub(crate) fn value(self) -> i64 {
        self.0
    }

    pub(crate) fn as_u64(self) -> Result<u64, EthereumBlockNumberError> {
        u64::try_from(self.0).map_err(|_| EthereumBlockNumberError::Negative(self.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EthereumBlockNumberError {
    Negative(i64),
    OutOfRange(u64),
}

impl fmt::Display for EthereumBlockNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EthereumBlockNumberError::Negative(value) => {
                write!(f, "ethereum block number must be non-negative, got {value}")
            }
            EthereumBlockNumberError::OutOfRange(value) => {
                write!(f, "ethereum block number exceeds supported range: {value}")
            }
        }
    }
}

impl std::error::Error for EthereumBlockNumberError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TransactionCount(u32);

impl TransactionCount {
    pub(crate) const fn zero() -> Self {
        Self(0)
    }

    pub(crate) fn try_new(value: i64) -> Result<Self, TransactionCountError> {
        let as_u32 = u32::try_from(value).map_err(|_| TransactionCountError::OutOfRange(value))?;
        Ok(Self(as_u32))
    }

    pub(crate) fn from_u32(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn value(self) -> u32 {
        self.0
    }

    pub(crate) fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ConsecutiveFailureCount(u32);

impl ConsecutiveFailureCount {
    pub(crate) const fn zero() -> Self {
        Self(0)
    }

    pub(crate) fn try_new(value: i64) -> Result<Self, ConsecutiveFailureCountError> {
        let as_u32 =
            u32::try_from(value).map_err(|_| ConsecutiveFailureCountError::OutOfRange(value))?;
        Ok(Self(as_u32))
    }

    pub(crate) fn value(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsecutiveFailureCountError {
    OutOfRange(i64),
}

impl fmt::Display for ConsecutiveFailureCountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsecutiveFailureCountError::OutOfRange(value) => {
                write!(f, "consecutive failure count out of range: {value}")
            }
        }
    }
}

impl std::error::Error for ConsecutiveFailureCountError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct AddressCount(u32);

impl AddressCount {
    pub(crate) const fn zero() -> Self {
        Self(0)
    }

    pub(crate) fn from_u32(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn value(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TxCountEstimate {
    Exact(TransactionCount),
    AtLeast(TransactionCount),
}

impl TxCountEstimate {
    pub(crate) fn transaction_count(self) -> TransactionCount {
        match self {
            TxCountEstimate::Exact(value) | TxCountEstimate::AtLeast(value) => value,
        }
    }

    pub(crate) fn is_lower_bound(self) -> bool {
        matches!(self, TxCountEstimate::AtLeast(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransactionCountError {
    OutOfRange(i64),
}

impl fmt::Display for TransactionCountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionCountError::OutOfRange(value) => {
                write!(f, "transaction count out of range: {value}")
            }
        }
    }
}

impl std::error::Error for TransactionCountError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SyncErrorMessage(String);

impl SyncErrorMessage {
    pub(crate) fn sanitize(raw: impl AsRef<str>) -> Self {
        const MAX_LEN: usize = 512;
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Self("Unknown sync error".to_string());
        }
        let mut value = trimmed.to_string();
        if value.len() > MAX_LEN {
            value.truncate(MAX_LEN);
        }
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// True when the error is a configuration problem retries cannot fix
    /// (currently: missing Etherscan API key).
    pub(crate) fn is_configuration_error(&self) -> bool {
        self.0
            .starts_with(crate::transactions::MISSING_ETHERSCAN_API_KEY_ERROR)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncResult {
    Success,
    Failure,
}

impl TransactionSyncResult {
    pub(crate) fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "failure" => Some(Self::Failure),
            _ => None,
        }
    }

    pub(crate) const fn as_db_value(self) -> &'static str {
        match self {
            TransactionSyncResult::Success => "success",
            TransactionSyncResult::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SyncIntegrationId {
    Mempool,
    Etherscan,
}

impl SyncIntegrationId {
    pub(crate) const fn as_db_value(self) -> &'static str {
        match self {
            SyncIntegrationId::Mempool => "mempool",
            SyncIntegrationId::Etherscan => "etherscan",
        }
    }

    pub(crate) fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "mempool" => Some(Self::Mempool),
            "etherscan" => Some(Self::Etherscan),
            _ => None,
        }
    }

    pub(crate) const fn for_asset(asset_id: SyncedAssetId) -> Self {
        match asset_id {
            SyncedAssetId::Bitcoin => Self::Mempool,
            SyncedAssetId::Ethereum => Self::Etherscan,
        }
    }
}

impl fmt::Display for SyncIntegrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_value())
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn sync_error_message_classifies_missing_api_key_as_configuration_error() {
        let config_error =
            SyncErrorMessage::sanitize(crate::transactions::MISSING_ETHERSCAN_API_KEY_ERROR);
        assert!(config_error.is_configuration_error());

        let transient_error = SyncErrorMessage::sanitize("Rate limit reached for mempool");
        assert!(!transient_error.is_configuration_error());
    }

    #[test]
    fn tx_hash_requires_64_hex_characters() {
        assert!(TxHash::parse("abcd").is_err());
        assert!(
            TxHash::parse("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")
                .is_err()
        );
        assert!(
            TxHash::parse("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .is_ok()
        );
    }

    #[test]
    fn chain_tip_height_rejects_negative() {
        assert!(matches!(
            ChainTipHeight::try_new(-1),
            Err(ChainTipHeightError::Negative(-1))
        ));
    }

    #[test]
    fn ethereum_block_number_rejects_negative_values() {
        assert_eq!(
            EthereumBlockNumber::try_new(-1),
            Err(EthereumBlockNumberError::Negative(-1))
        );
    }

    #[test]
    fn ethereum_block_number_roundtrips_u64_when_in_range() {
        let block = EthereumBlockNumber::from_u64(12_345).expect("valid block number");
        assert_eq!(block.value(), 12_345);
        assert_eq!(block.as_u64(), Ok(12_345));
    }

    #[test]
    fn ethereum_block_number_rejects_out_of_range_u64() {
        assert_eq!(
            EthereumBlockNumber::from_u64(u64::MAX),
            Err(EthereumBlockNumberError::OutOfRange(u64::MAX))
        );
    }

    #[test]
    fn tx_count_estimate_helpers_report_count_and_bound_kind() {
        let exact = TxCountEstimate::Exact(TransactionCount::from_u32(12));
        let lower_bound = TxCountEstimate::AtLeast(TransactionCount::from_u32(10_000));

        assert_eq!(exact.transaction_count(), TransactionCount::from_u32(12));
        assert!(!exact.is_lower_bound());
        assert_eq!(
            lower_bound.transaction_count(),
            TransactionCount::from_u32(10_000)
        );
        assert!(lower_bound.is_lower_bound());
    }

    #[test]
    fn transaction_count_saturating_add() {
        let a = TransactionCount::from_u32(5);
        let b = TransactionCount::from_u32(3);
        assert_eq!(a.saturating_add(b).value(), 8);

        let max = TransactionCount::from_u32(u32::MAX);
        assert_eq!(max.saturating_add(a).value(), u32::MAX);
    }
}

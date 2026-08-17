use chrono::NaiveDate;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

pub(crate) const WALLET_LABEL_MAX_LENGTH: usize = 255;
pub(crate) const ACCOUNT_LABEL_MAX_LENGTH: usize = 255;
pub(crate) const BIP44_GAP_LIMIT: u32 = 20;
pub(crate) const DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE: u32 = 50;
pub(crate) const MAX_ACCOUNT_ADDRESSES_PAGE_SIZE: u32 = 100;
pub(crate) const ACCOUNT_TRANSACTIONS_PAGE_SIZE: u32 = 50;
pub(super) const ACCOUNT_INDEX_HARDENED_OFFSET: u32 = 1 << 31;

pub(crate) type WalletReportDateRange = crate::report_dates::LocalReportDateRange;

// ============ Sort Direction ============

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TransactionSortDirection {
    Ascending,
    Descending,
}

impl TransactionSortDirection {
    pub(crate) fn from_query_value(value: &str) -> Self {
        match value {
            "desc" => Self::Descending,
            _ => Self::Ascending,
        }
    }

    pub(crate) fn as_query_value(&self) -> &'static str {
        match self {
            Self::Ascending => "asc",
            Self::Descending => "desc",
        }
    }

    pub(crate) fn toggled(&self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ReportDateParam(NaiveDate);

impl ReportDateParam {
    pub(crate) fn from_naive_date(value: NaiveDate) -> Self {
        Self(value)
    }

    pub(crate) fn into_naive_date(self) -> NaiveDate {
        self.0
    }
}

impl fmt::Display for ReportDateParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.format("%Y-%m-%d"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportDateParamParseError;

impl fmt::Display for ReportDateParamParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected date in YYYY-MM-DD format")
    }
}

impl std::error::Error for ReportDateParamParseError {}

impl FromStr for ReportDateParam {
    type Err = ReportDateParamParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
            .map(Self)
            .map_err(|_| ReportDateParamParseError)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ReportTimezoneParam(pub(crate) crate::models::UserTimezone);

impl ReportTimezoneParam {
    pub(crate) fn into_user_timezone(self) -> crate::models::UserTimezone {
        self.0
    }
}

impl fmt::Display for ReportTimezoneParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportTimezoneParamParseError;

impl fmt::Display for ReportTimezoneParamParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected a valid IANA timezone")
    }
}

impl std::error::Error for ReportTimezoneParamParseError {}

impl FromStr for ReportTimezoneParam {
    type Err = ReportTimezoneParamParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .trim()
            .parse::<Tz>()
            .map(crate::models::UserTimezone)
            .map(Self)
            .map_err(|_| ReportTimezoneParamParseError)
    }
}

// ============ Identifiers ============

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct WalletId(Ulid);

impl WalletId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WalletId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for WalletId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(WalletId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct WalletAccessorId(Ulid);

impl WalletAccessorId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for WalletAccessorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WalletAccessorId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for WalletAccessorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for WalletAccessorId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(WalletAccessorId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DigitalAssetAccountId(Ulid);

impl DigitalAssetAccountId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for DigitalAssetAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DigitalAssetAccountId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for DigitalAssetAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DigitalAssetAccountId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(DigitalAssetAccountId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct WalletAccountId(Ulid);

impl WalletAccountId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for WalletAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WalletAccountId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for WalletAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for WalletAccountId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(WalletAccountId)
    }
}

impl From<DigitalAssetAccountId> for WalletAccountId {
    fn from(value: DigitalAssetAccountId) -> Self {
        Self(value.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct HdKeyId(Ulid);

impl HdKeyId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for HdKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HdKeyId").field(&self.0.to_string()).finish()
    }
}

impl fmt::Display for HdKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for HdKeyId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(HdKeyId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DigitalAssetAddressId(Ulid);

impl DigitalAssetAddressId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for DigitalAssetAddressId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DigitalAssetAddressId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for DigitalAssetAddressId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DigitalAssetAddressId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(DigitalAssetAddressId)
    }
}

// ============ Enums ============

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccessorKind {
    Trezor,
    Ledger,
    Software,
    Unknown,
}

impl AccessorKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AccessorKind::Trezor => "trezor",
            AccessorKind::Ledger => "ledger",
            AccessorKind::Software => "software",
            AccessorKind::Unknown => "unknown",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "trezor" => Some(AccessorKind::Trezor),
            "ledger" => Some(AccessorKind::Ledger),
            "software" => Some(AccessorKind::Software),
            "unknown" => Some(AccessorKind::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SyncedAssetId {
    Bitcoin,
    Ethereum,
}

impl SyncedAssetId {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SyncedAssetId::Bitcoin => "bitcoin",
            SyncedAssetId::Ethereum => "ethereum",
        }
    }

    pub(crate) fn display_name(&self) -> &'static str {
        match self {
            SyncedAssetId::Bitcoin => "Bitcoin",
            SyncedAssetId::Ethereum => "Ethereum",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "bitcoin" => Some(SyncedAssetId::Bitcoin),
            "ethereum" => Some(SyncedAssetId::Ethereum),
            _ => None,
        }
    }

    pub(crate) fn bip44_coin_type(&self) -> DerivationCoinType {
        match self {
            SyncedAssetId::Bitcoin => DerivationCoinType::new(0),
            SyncedAssetId::Ethereum => DerivationCoinType::new(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentitySource {
    DeviceVerified,
    UserProvided,
    Inferred,
}

impl IdentitySource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            IdentitySource::DeviceVerified => "device_verified",
            IdentitySource::UserProvided => "user_provided",
            IdentitySource::Inferred => "inferred",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "device_verified" => Some(IdentitySource::DeviceVerified),
            "user_provided" => Some(IdentitySource::UserProvided),
            "inferred" => Some(IdentitySource::Inferred),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Network {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
}

impl Network {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "mainnet" => Some(Network::Mainnet),
            "testnet" => Some(Network::Testnet),
            "signet" => Some(Network::Signet),
            "regtest" => Some(Network::Regtest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountKind {
    HdPubkey,
    SingleAddress,
}

impl AccountKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AccountKind::HdPubkey => "hd_pubkey",
            AccountKind::SingleAddress => "single_address",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "hd_pubkey" => Some(AccountKind::HdPubkey),
            "single_address" => Some(AccountKind::SingleAddress),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AddressScheme {
    Legacy,
    NestedSegwit,
    NativeSegwit,
    Taproot,
    Standard,
}

impl AddressScheme {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AddressScheme::Legacy => "legacy",
            AddressScheme::NestedSegwit => "nested_segwit",
            AddressScheme::NativeSegwit => "native_segwit",
            AddressScheme::Taproot => "taproot",
            AddressScheme::Standard => "standard",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "legacy" => Some(AddressScheme::Legacy),
            "nested_segwit" => Some(AddressScheme::NestedSegwit),
            "native_segwit" => Some(AddressScheme::NativeSegwit),
            "taproot" => Some(AddressScheme::Taproot),
            "standard" => Some(AddressScheme::Standard),
            _ => None,
        }
    }

    pub(crate) fn purpose(&self) -> DerivationPurpose {
        match self {
            AddressScheme::Legacy => DerivationPurpose::Bip44,
            AddressScheme::NestedSegwit => DerivationPurpose::Bip49,
            AddressScheme::NativeSegwit => DerivationPurpose::Bip84,
            AddressScheme::Taproot => DerivationPurpose::Bip86,
            AddressScheme::Standard => DerivationPurpose::Bip44,
        }
    }

    pub(crate) fn scheme_note(&self) -> Option<&'static str> {
        match self {
            AddressScheme::Legacy => Some("Addresses start with 1"),
            AddressScheme::NestedSegwit => Some("Addresses start with 3"),
            AddressScheme::NativeSegwit => Some("Addresses start with bc1q"),
            AddressScheme::Taproot => None,
            AddressScheme::Standard => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AddressSourceType {
    Derived,
    UserProvided,
    Imported,
    Observed,
}

impl AddressSourceType {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AddressSourceType::Derived => "derived",
            AddressSourceType::UserProvided => "user_provided",
            AddressSourceType::Imported => "imported",
            AddressSourceType::Observed => "observed",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "derived" => Some(AddressSourceType::Derived),
            "user_provided" => Some(AddressSourceType::UserProvided),
            "imported" => Some(AddressSourceType::Imported),
            "observed" => Some(AddressSourceType::Observed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyRole {
    Primary,
    Cosigner,
    Backup,
}

impl KeyRole {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            KeyRole::Primary => "primary",
            KeyRole::Cosigner => "cosigner",
            KeyRole::Backup => "backup",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "primary" => Some(KeyRole::Primary),
            "cosigner" => Some(KeyRole::Cosigner),
            "backup" => Some(KeyRole::Backup),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeySource {
    DeviceVerified,
    UserProvided,
    Inferred,
}

impl KeySource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            KeySource::DeviceVerified => "device_verified",
            KeySource::UserProvided => "user_provided",
            KeySource::Inferred => "inferred",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "device_verified" => Some(KeySource::DeviceVerified),
            "user_provided" => Some(KeySource::UserProvided),
            "inferred" => Some(KeySource::Inferred),
            _ => None,
        }
    }
}

// ============ Validated Types ============

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawAccountIndex(u32);

impl RawAccountIndex {
    #[cfg(any(target_arch = "wasm32", feature = "desktop", test))]
    pub(crate) fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn validate(self) -> Result<AccountIndex, AccountIndexError> {
        AccountIndex::new(self.0)
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    pub(crate) fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct AccountIndex(u32);

impl AccountIndex {
    pub(crate) fn new(value: u32) -> Result<Self, AccountIndexError> {
        if value >= ACCOUNT_INDEX_HARDENED_OFFSET {
            return Err(AccountIndexError::TooLarge(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_u32(&self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for AccountIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        AccountIndex::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AccountIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountIndexError {
    TooLarge(u32),
}

impl fmt::Display for AccountIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccountIndexError::TooLarge(value) => {
                write!(f, "Account index {value} exceeds hardened offset")
            }
        }
    }
}

impl std::error::Error for AccountIndexError {}

// ============ Derivation ============

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DerivationPurpose {
    Bip44,
    Bip49,
    Bip84,
    Bip86,
}

impl DerivationPurpose {
    pub(crate) fn value(&self) -> u32 {
        match self {
            DerivationPurpose::Bip44 => 44,
            DerivationPurpose::Bip49 => 49,
            DerivationPurpose::Bip84 => 84,
            DerivationPurpose::Bip86 => 86,
        }
    }

    pub(crate) fn from_value(value: u32) -> Option<Self> {
        match value {
            44 => Some(DerivationPurpose::Bip44),
            49 => Some(DerivationPurpose::Bip49),
            84 => Some(DerivationPurpose::Bip84),
            86 => Some(DerivationPurpose::Bip86),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DerivationCoinType(u32);

impl DerivationCoinType {
    pub(crate) fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn value(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DerivationPath {
    pub purpose: DerivationPurpose,
    pub coin_type: DerivationCoinType,
    pub account: AccountIndex,
}

impl DerivationPath {
    pub(crate) fn bitcoin_for_address_scheme(
        account: AccountIndex,
        address_scheme: AddressScheme,
    ) -> Self {
        Self {
            purpose: address_scheme.purpose(),
            coin_type: SyncedAssetId::Bitcoin.bip44_coin_type(),
            account,
        }
    }
}

impl fmt::Display for DerivationPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "m/{}'/{}'/{}'",
            self.purpose.value(),
            self.coin_type.value(),
            self.account.as_u32()
        )
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_account_index_validation() {
        assert!(AccountIndex::new(0).is_ok());
        assert!(AccountIndex::new(1).is_ok());
        assert!(AccountIndex::new(ACCOUNT_INDEX_HARDENED_OFFSET).is_err());
    }

    #[test]
    fn test_scheme_note_returns_expected_strings() {
        assert_eq!(
            AddressScheme::Legacy.scheme_note(),
            Some("Addresses start with 1")
        );
        assert_eq!(
            AddressScheme::NestedSegwit.scheme_note(),
            Some("Addresses start with 3")
        );
        assert_eq!(
            AddressScheme::NativeSegwit.scheme_note(),
            Some("Addresses start with bc1q")
        );
        assert_eq!(AddressScheme::Taproot.scheme_note(), None);
        assert_eq!(AddressScheme::Standard.scheme_note(), None);
    }

    #[test]
    fn report_date_param_parses_iso_date() {
        let parsed = "2026-03-30"
            .parse::<ReportDateParam>()
            .expect("report date should parse");

        assert_eq!(
            parsed.into_naive_date(),
            NaiveDate::from_ymd_opt(2026, 3, 30).expect("valid date")
        );
        assert_eq!(parsed.to_string(), "2026-03-30");
    }

    #[test]
    fn report_date_param_rejects_invalid_input() {
        let result = "03/30/2026".parse::<ReportDateParam>();
        assert!(matches!(result, Err(ReportDateParamParseError)));
    }

    #[test]
    fn report_timezone_param_parses_iana_timezone() {
        let parsed = "Europe/Amsterdam"
            .parse::<ReportTimezoneParam>()
            .expect("report timezone should parse");

        assert_eq!(parsed.to_string(), "Europe/Amsterdam");
        assert_eq!(
            parsed.into_user_timezone(),
            crate::models::UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"))
        );
    }
}

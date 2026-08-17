#[cfg(feature = "server")]
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
#[cfg(feature = "server")]
use rand::{RngCore, rngs::OsRng};
use serde::de::Error as _;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct EntitlementHolderId(Ulid);

impl EntitlementHolderId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }

    pub(crate) fn to_storage_value(self) -> String {
        self.0.to_string()
    }
}

impl fmt::Display for EntitlementHolderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for EntitlementHolderId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ulid(value, "entitlement_holder_id").map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct PaymentOrderId(Ulid);

impl PaymentOrderId {
    pub(crate) fn to_storage_value(self) -> String {
        self.0.to_string()
    }
}

impl fmt::Display for PaymentOrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PaymentOrderId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ulid(value, "order_id").map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct PaymentAttemptId(Ulid);

impl fmt::Display for PaymentAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PaymentAttemptId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ulid(value, "payment_attempt_id").map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TokenId(Ulid);

impl TokenId {
    pub(crate) fn to_storage_value(self) -> String {
        self.0.to_string()
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TokenId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ulid(value, "token_id").map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SubscriptionSubjectId(Ulid);

impl SubscriptionSubjectId {
    pub(crate) fn to_storage_value(self) -> String {
        self.0.to_string()
    }
}

impl fmt::Display for SubscriptionSubjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SubscriptionSubjectId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ulid(value, "subscription_subject_id").map(Self)
    }
}

fn parse_ulid(value: &str, field: &'static str) -> Result<Ulid, PaymentTypeError> {
    Ulid::from_string(value).map_err(|_| PaymentTypeError::Ulid { field })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaymentSecret(String);

impl PaymentSecret {
    #[cfg(feature = "server")]
    pub(crate) fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub(crate) fn from_raw(value: impl Into<String>) -> Result<Self, PaymentTypeError> {
        let value = value.into();
        if value.len() != 43
            || value.contains('=')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PaymentTypeError::Secret);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProductTier {
    Basic,
    Premium,
}

impl ProductTier {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Premium => "premium",
        }
    }
}

impl fmt::Display for ProductTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProductTier {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "basic" => Ok(Self::Basic),
            "premium" => Ok(Self::Premium),
            _ => Err(PaymentTypeError::ProductTier),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EntitlementTier {
    Free,
    Basic,
    Premium,
    Unknown(String),
}

impl EntitlementTier {
    pub(crate) fn from_raw(value: impl Into<String>) -> Result<Self, PaymentTypeError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
            return Err(PaymentTypeError::EntitlementTier);
        }
        Ok(match trimmed {
            "free" => Self::Free,
            "basic" => Self::Basic,
            "premium" => Self::Premium,
            other => Self::Unknown(other.to_string()),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Free => "free",
            Self::Basic => "basic",
            Self::Premium => "premium",
            Self::Unknown(value) => value.as_str(),
        }
    }

    pub(crate) fn display_name(&self) -> String {
        match self {
            Self::Free => "Free".to_string(),
            Self::Basic => "Basic".to_string(),
            Self::Premium => "Premium".to_string(),
            Self::Unknown(_) => "Paid plan".to_string(),
        }
    }
}

impl fmt::Display for EntitlementTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EntitlementTier {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_raw(value.to_string())
    }
}

impl Serialize for EntitlementTier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct EntitlementTierVisitor;

impl<'de> Visitor<'de> for EntitlementTierVisitor {
    type Value = EntitlementTier;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a non-empty entitlement tier string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        EntitlementTier::from_raw(value.to_string()).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for EntitlementTier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EntitlementTierVisitor)
    }
}

pub(crate) const CAPABILITY_SCHEMA_VERSION_LEGACY: u16 = 2;
pub(crate) const CAPABILITY_SCHEMA_VERSION_V3: u16 = 3;
const FREE_SYNCED_ACCOUNTS: u16 = 5;
const FREE_MAX_TRANSACTIONS_PER_ACCOUNT: u32 = 0;

pub(crate) fn default_capability_schema_version() -> u16 {
    CAPABILITY_SCHEMA_VERSION_LEGACY
}

fn default_synced_accounts() -> u16 {
    FREE_SYNCED_ACCOUNTS
}

fn default_max_transactions_per_account() -> u32 {
    FREE_MAX_TRANSACTIONS_PER_ACCOUNT
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HistoryLimits {
    #[serde(default = "default_max_transactions_per_account")]
    pub(crate) max_transactions_per_account: u32,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_transactions_per_account: FREE_MAX_TRANSACTIONS_PER_ACCOUNT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountLimits {
    pub(crate) total: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EntitlementFeatureFlags {
    #[serde(default)]
    pub(crate) historical_sync: bool,
    #[serde(default)]
    pub(crate) transaction_history_sync: bool,
    #[serde(default)]
    pub(crate) balance_sync: bool,
    #[serde(default)]
    pub(crate) exchange_rates_current: bool,
    #[serde(default)]
    pub(crate) exchange_rates_history: bool,
    #[serde(default)]
    pub(crate) price_overrides: bool,
    #[serde(default)]
    pub(crate) balance_assertions: bool,
    #[serde(default)]
    pub(crate) hledger_export: bool,
    #[serde(default)]
    pub(crate) tax_reports: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EntitlementCapabilityLimits {
    #[serde(default)]
    pub(crate) accounts: Option<AccountLimits>,
    #[serde(default = "default_synced_accounts")]
    pub(crate) synced_accounts: u16,
    #[serde(default)]
    pub(crate) history: HistoryLimits,
}

impl Default for EntitlementCapabilityLimits {
    fn default() -> Self {
        Self {
            accounts: None,
            synced_accounts: FREE_SYNCED_ACCOUNTS,
            history: HistoryLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EntitlementCapabilities {
    #[serde(default)]
    pub(crate) limits: EntitlementCapabilityLimits,
    #[serde(default)]
    pub(crate) features: EntitlementFeatureFlags,
}

impl EntitlementCapabilities {
    pub(crate) const fn legacy_from_parts(
        synced_accounts: u16,
        max_transactions_per_account: u32,
        historical_sync: bool,
    ) -> Self {
        Self {
            limits: EntitlementCapabilityLimits {
                accounts: None,
                synced_accounts,
                history: HistoryLimits {
                    max_transactions_per_account,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync,
                transaction_history_sync: false,
                balance_sync: false,
                exchange_rates_current: false,
                exchange_rates_history: false,
                price_overrides: false,
                balance_assertions: false,
                hledger_export: false,
                tax_reports: false,
            },
        }
    }

    #[cfg(test)]
    pub(crate) const fn v3_from_parts(
        total_accounts: u16,
        max_transactions_per_account: u32,
        transaction_history_sync: bool,
    ) -> Self {
        Self {
            limits: EntitlementCapabilityLimits {
                accounts: Some(AccountLimits {
                    total: total_accounts,
                }),
                synced_accounts: FREE_SYNCED_ACCOUNTS,
                history: HistoryLimits {
                    max_transactions_per_account,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: false,
                transaction_history_sync,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        }
    }

    pub(crate) const fn free() -> Self {
        Self::legacy_from_parts(
            FREE_SYNCED_ACCOUNTS,
            FREE_MAX_TRANSACTIONS_PER_ACCOUNT,
            false,
        )
    }

    pub(crate) const fn account_limit_for_schema(&self, capability_schema_version: u16) -> u16 {
        match capability_schema_version {
            CAPABILITY_SCHEMA_VERSION_LEGACY => self.limits.synced_accounts,
            CAPABILITY_SCHEMA_VERSION_V3 => match self.limits.accounts {
                Some(accounts) => accounts.total,
                None => FREE_SYNCED_ACCOUNTS,
            },
            _ => FREE_SYNCED_ACCOUNTS,
        }
    }

    pub(crate) const fn transaction_history_enabled_for_schema(
        &self,
        capability_schema_version: u16,
    ) -> bool {
        match capability_schema_version {
            CAPABILITY_SCHEMA_VERSION_LEGACY => self.features.historical_sync,
            CAPABILITY_SCHEMA_VERSION_V3 => self.features.transaction_history_sync,
            _ => false,
        }
    }

    pub(crate) const fn transaction_limit_for_schema(&self, capability_schema_version: u16) -> u32 {
        match capability_schema_version {
            CAPABILITY_SCHEMA_VERSION_LEGACY | CAPABILITY_SCHEMA_VERSION_V3 => {
                self.limits.history.max_transactions_per_account
            }
            _ => FREE_MAX_TRANSACTIONS_PER_ACCOUNT,
        }
    }

    #[cfg(test)]
    pub(crate) fn to_storage_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize)]
struct EntitlementCapabilitiesStorageJson<'a> {
    capability_schema_version: u16,
    capabilities: &'a EntitlementCapabilities,
}

#[derive(Deserialize)]
struct EntitlementCapabilitiesStorageSchema {
    #[serde(default = "default_capability_schema_version")]
    capability_schema_version: u16,
}

pub(crate) fn entitlement_capabilities_storage_json(
    capability_schema_version: u16,
    capabilities: &EntitlementCapabilities,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&EntitlementCapabilitiesStorageJson {
        capability_schema_version,
        capabilities,
    })
}

pub(crate) fn capability_schema_version_from_storage_json(capabilities_json: Option<&str>) -> u16 {
    capabilities_json
        .and_then(|raw| serde_json::from_str::<EntitlementCapabilitiesStorageSchema>(raw).ok())
        .map_or_else(default_capability_schema_version, |storage| {
            storage.capability_schema_version
        })
}

impl Default for EntitlementCapabilities {
    fn default() -> Self {
        Self::free()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntitlementSource {
    LocalFree,
    SignedCentralToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeatureEntitlements {
    pub(crate) tier: EntitlementTier,
    pub(crate) sync_account_slots_limit: u16,
    pub(crate) historical_backfill_enabled: bool,
    pub(crate) historical_backfill_transactions_per_account: u32,
    pub(crate) tax_reports: bool,
    pub(crate) exchange_rates_history: bool,
    pub(crate) price_overrides: bool,
    pub(crate) subscription_valid_until: Option<DateTime<Utc>>,
    pub(crate) token_expires_at: Option<DateTime<Utc>>,
    pub(crate) source: EntitlementSource,
}

impl FeatureEntitlements {
    #[cfg(test)]
    pub(crate) fn free() -> Self {
        Self::from_capabilities(
            EntitlementTier::Free,
            CAPABILITY_SCHEMA_VERSION_LEGACY,
            EntitlementCapabilities::free(),
            None,
            None,
            EntitlementSource::LocalFree,
        )
    }

    pub(crate) fn from_capabilities(
        tier: EntitlementTier,
        capability_schema_version: u16,
        capabilities: EntitlementCapabilities,
        subscription_valid_until: Option<DateTime<Utc>>,
        token_expires_at: Option<DateTime<Utc>>,
        source: EntitlementSource,
    ) -> Self {
        let schema_supported = matches!(
            capability_schema_version,
            CAPABILITY_SCHEMA_VERSION_LEGACY | CAPABILITY_SCHEMA_VERSION_V3
        );
        let (tier, capabilities, subscription_valid_until, token_expires_at, source) =
            if schema_supported {
                (
                    tier,
                    capabilities,
                    subscription_valid_until,
                    token_expires_at,
                    source,
                )
            } else {
                (
                    EntitlementTier::Free,
                    EntitlementCapabilities::free(),
                    None,
                    None,
                    EntitlementSource::LocalFree,
                )
            };

        let (tax_reports, exchange_rates_history, price_overrides) =
            if capability_schema_version == CAPABILITY_SCHEMA_VERSION_V3 {
                (
                    capabilities.features.tax_reports,
                    capabilities.features.exchange_rates_history,
                    capabilities.features.price_overrides,
                )
            } else {
                (false, false, false)
            };

        Self {
            tier,
            sync_account_slots_limit: capabilities
                .account_limit_for_schema(capability_schema_version),
            historical_backfill_enabled: capabilities
                .transaction_history_enabled_for_schema(capability_schema_version),
            historical_backfill_transactions_per_account: capabilities
                .transaction_limit_for_schema(capability_schema_version),
            tax_reports,
            exchange_rates_history,
            price_overrides,
            subscription_valid_until,
            token_expires_at,
            source,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProductOptionId(String);

impl ProductOptionId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_raw(value: impl Into<String>) -> Result<Self, PaymentTypeError> {
        let value = value.into();
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(PaymentTypeError::ProductOptionId);
        }
        Ok(Self(value))
    }
}

impl fmt::Display for ProductOptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProductOptionId {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_raw(value.to_string())
    }
}

impl Serialize for ProductOptionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProductOptionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_raw(value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentAmount {
    pub(crate) minor_units: u64,
    pub(crate) currency: String,
    #[serde(default)]
    pub(crate) currency_symbol: Option<String>,
    #[serde(alias = "display_scale")]
    pub(crate) decimal_precision: u8,
}

impl PaymentAmount {
    pub(crate) fn atlos_decimal_amount(&self) -> String {
        format_minor_units(self.minor_units, self.decimal_precision)
    }
}

pub(crate) fn format_minor_units(minor_units: u64, decimal_precision: u8) -> String {
    if decimal_precision == 0 {
        return minor_units.to_string();
    }

    let scale = usize::from(decimal_precision);
    let divisor = 10_u64.pow(u32::from(decimal_precision));
    let whole = minor_units / divisor;
    let fractional = minor_units % divisor;
    format!("{whole}.{fractional:0scale$}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaymentOrderStatus {
    Pending,
    Paid,
    Expired,
    Failed,
    Canceled,
}

impl PaymentOrderStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Expired => "expired",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

impl FromStr for PaymentOrderStatus {
    type Err = PaymentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "paid" => Ok(Self::Paid),
            "expired" => Ok(Self::Expired),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            _ => Err(PaymentTypeError::OrderStatus),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CentralOrderStatus {
    Pending,
    Paid,
    Expired,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CentralOrderVerificationState {
    AwaitingPayment,
    PaymentConfirmedUnverified,
    AdditionalPaymentRequired,
    UnderManualReview,
    PremiumGranted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CentralOrderNextAction {
    KeepPolling,
    RequestAdditionalPayment,
    UnlockPremium,
    ShowManualReview,
    OfferRetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CentralRefreshStatus {
    Active,
    Revoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefreshRevokedReason {
    Expired,
    TokenSuperseded,
    Inactive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenClaims {
    pub(crate) token_id: TokenId,
    pub(crate) subscription_subject_id: SubscriptionSubjectId,
    pub(crate) entitlement_holder_id: EntitlementHolderId,
    pub(crate) tier: EntitlementTier,
    #[serde(default)]
    pub(crate) capability_set_id: Option<String>,
    #[serde(default = "default_capability_schema_version")]
    pub(crate) capability_schema_version: u16,
    #[serde(default)]
    pub(crate) capabilities: EntitlementCapabilities,
    pub(crate) subscription_valid_until: DateTime<Utc>,
    pub(crate) token_expires_at: DateTime<Utc>,
    pub(crate) issued_at: DateTime<Utc>,
}

pub(crate) fn payment_state_status_from_order(
    status: PaymentOrderStatus,
) -> crate::payments::views::PaymentStateStatus {
    use crate::payments::views::PaymentStateStatus;
    match status {
        PaymentOrderStatus::Pending => PaymentStateStatus::Pending,
        PaymentOrderStatus::Paid => PaymentStateStatus::Active,
        PaymentOrderStatus::Expired => PaymentStateStatus::Expired,
        PaymentOrderStatus::Failed => PaymentStateStatus::Failed,
        PaymentOrderStatus::Canceled => PaymentStateStatus::Canceled,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaymentTypeError {
    Ulid { field: &'static str },
    Secret,
    ProductTier,
    EntitlementTier,
    ProductOptionId,
    OrderStatus,
}

impl fmt::Display for PaymentTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ulid { field } => write!(f, "{field} must be a valid ULID"),
            Self::Secret => write!(f, "secret must be a 32-byte base64url-no-padding value"),
            Self::ProductTier => write!(f, "product_tier must be basic or premium"),
            Self::EntitlementTier => write!(f, "entitlement tier must be a non-empty string"),
            Self::ProductOptionId => {
                write!(f, "product_option_id must be a non-empty string")
            }
            Self::OrderStatus => write!(f, "payment order status is invalid"),
        }
    }
}

impl std::error::Error for PaymentTypeError {}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn amount_formatting_uses_decimal_string_without_float() {
        let amount = PaymentAmount {
            minor_units: 999,
            currency: "USD".to_string(),
            currency_symbol: Some("$".to_string()),
            decimal_precision: 2,
        };

        assert_eq!(amount.atlos_decimal_amount(), "9.99");
        assert_eq!(format_minor_units(42, 0), "42");
        assert_eq!(format_minor_units(5, 2), "0.05");
        assert_eq!(format_minor_units(100, 2), "1.00");
    }

    #[test]
    fn payment_secret_accepts_central_secret_shape() {
        let secret = PaymentSecret::from_raw("frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI")
            .expect("central-shaped secret should parse");
        assert_eq!(
            secret.as_str(),
            "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI"
        );
    }

    #[test]
    fn payment_secret_rejects_padding_and_wrong_length() {
        assert!(PaymentSecret::from_raw("abc=").is_err());
        assert!(PaymentSecret::from_raw("short").is_err());
    }

    #[test]
    fn product_option_id_accepts_unknown_central_values() {
        let parsed = ProductOptionId::from_str("premium_test_1_day_usd")
            .expect("unknown but valid central option id should parse");

        assert_eq!(parsed.as_str(), "premium_test_1_day_usd");
    }

    #[test]
    fn product_option_id_rejects_empty_values() {
        assert!(ProductOptionId::from_str("").is_err());
        assert!(ProductOptionId::from_str("   ").is_err());
    }

    #[test]
    fn payment_order_status_round_trips_storage_values() {
        for status in [
            PaymentOrderStatus::Pending,
            PaymentOrderStatus::Paid,
            PaymentOrderStatus::Expired,
            PaymentOrderStatus::Failed,
            PaymentOrderStatus::Canceled,
        ] {
            assert_eq!(status.as_str().parse(), Ok(status));
        }
    }

    #[test]
    fn payment_state_reducer_maps_order_statuses() {
        use crate::payments::views::PaymentStateStatus;
        assert_eq!(
            payment_state_status_from_order(PaymentOrderStatus::Pending),
            PaymentStateStatus::Pending
        );
        assert_eq!(
            payment_state_status_from_order(PaymentOrderStatus::Canceled),
            PaymentStateStatus::Canceled
        );
        assert_eq!(RefreshRevokedReason::Expired, RefreshRevokedReason::Expired);
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn v3_capabilities_use_accounts_total_and_transaction_history_sync() {
        let capabilities = EntitlementCapabilities {
            limits: EntitlementCapabilityLimits {
                accounts: Some(AccountLimits { total: 10 }),
                synced_accounts: 2,
                history: HistoryLimits {
                    max_transactions_per_account: 5000,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: false,
                transaction_history_sync: true,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        };

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Basic,
            CAPABILITY_SCHEMA_VERSION_V3,
            capabilities,
            None,
            None,
            EntitlementSource::SignedCentralToken,
        );

        assert_eq!(entitlements.sync_account_slots_limit, 10);
        assert!(entitlements.historical_backfill_enabled);
        assert_eq!(
            entitlements.historical_backfill_transactions_per_account,
            5000
        );
    }

    #[test]
    fn v3_capabilities_project_report_and_price_features() {
        let capabilities = EntitlementCapabilities {
            limits: EntitlementCapabilityLimits {
                accounts: Some(AccountLimits { total: 10 }),
                synced_accounts: 2,
                history: HistoryLimits {
                    max_transactions_per_account: 5000,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: false,
                transaction_history_sync: true,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        };

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Basic,
            CAPABILITY_SCHEMA_VERSION_V3,
            capabilities,
            None,
            None,
            EntitlementSource::SignedCentralToken,
        );

        assert!(entitlements.tax_reports);
        assert!(entitlements.exchange_rates_history);
        assert!(entitlements.price_overrides);
    }

    #[test]
    fn legacy_capabilities_use_synced_accounts_and_historical_sync() {
        let capabilities = EntitlementCapabilities::legacy_from_parts(10, 10000, true);

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Basic,
            CAPABILITY_SCHEMA_VERSION_LEGACY,
            capabilities,
            None,
            None,
            EntitlementSource::SignedCentralToken,
        );

        assert_eq!(entitlements.sync_account_slots_limit, 10);
        assert!(entitlements.historical_backfill_enabled);
        assert_eq!(
            entitlements.historical_backfill_transactions_per_account,
            10000
        );
    }

    #[test]
    fn v3_missing_accounts_total_fails_closed_to_free_limit() {
        let capabilities = EntitlementCapabilities {
            limits: EntitlementCapabilityLimits {
                accounts: None,
                synced_accounts: 50,
                history: HistoryLimits {
                    max_transactions_per_account: 100000,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: true,
                transaction_history_sync: true,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        };

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Premium,
            CAPABILITY_SCHEMA_VERSION_V3,
            capabilities,
            None,
            None,
            EntitlementSource::SignedCentralToken,
        );

        assert_eq!(entitlements.sync_account_slots_limit, 5);
    }

    #[test]
    fn unknown_capability_schema_fails_closed_to_free_capabilities() {
        let capabilities = EntitlementCapabilities {
            limits: EntitlementCapabilityLimits {
                accounts: Some(AccountLimits { total: 50 }),
                synced_accounts: 50,
                history: HistoryLimits {
                    max_transactions_per_account: 100000,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: true,
                transaction_history_sync: true,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        };

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Premium,
            CAPABILITY_SCHEMA_VERSION_V3 + 1,
            capabilities,
            Some("2027-05-08T12:00:00Z".parse().unwrap()),
            Some("2026-05-15T12:00:00Z".parse().unwrap()),
            EntitlementSource::SignedCentralToken,
        );

        assert_eq!(entitlements.tier, EntitlementTier::Free);
        assert_eq!(entitlements.sync_account_slots_limit, 5);
        assert!(!entitlements.historical_backfill_enabled);
        assert_eq!(entitlements.historical_backfill_transactions_per_account, 0);
        assert!(entitlements.subscription_valid_until.is_none());
        assert!(entitlements.token_expires_at.is_none());
        assert_eq!(entitlements.source, EntitlementSource::LocalFree);
    }

    #[test]
    fn unknown_capability_schema_drops_report_and_price_features() {
        let capabilities = EntitlementCapabilities {
            limits: EntitlementCapabilityLimits {
                accounts: Some(AccountLimits { total: 50 }),
                synced_accounts: 50,
                history: HistoryLimits {
                    max_transactions_per_account: 100000,
                },
            },
            features: EntitlementFeatureFlags {
                historical_sync: true,
                transaction_history_sync: true,
                balance_sync: true,
                exchange_rates_current: true,
                exchange_rates_history: true,
                price_overrides: true,
                balance_assertions: true,
                hledger_export: true,
                tax_reports: true,
            },
        };

        let entitlements = FeatureEntitlements::from_capabilities(
            EntitlementTier::Premium,
            CAPABILITY_SCHEMA_VERSION_V3 + 1,
            capabilities,
            None,
            None,
            EntitlementSource::SignedCentralToken,
        );

        assert!(!entitlements.tax_reports);
        assert!(!entitlements.exchange_rates_history);
        assert!(!entitlements.price_overrides);
    }

    #[test]
    fn storage_json_omits_background_sync_but_legacy_json_still_reads() {
        let capabilities = EntitlementCapabilities::legacy_from_parts(10, 10000, true);
        let json = capabilities
            .to_storage_json()
            .expect("capabilities serialize");
        assert!(!json.contains("background_sync"));

        let legacy: EntitlementCapabilities = serde_json::from_str(
            r#"{"limits":{"synced_accounts":10,"history":{"max_transactions_per_account":10000}},"features":{"historical_sync":true,"background_sync":true}}"#,
        )
        .expect("legacy capability JSON should still parse");
        assert!(legacy.features.historical_sync);
    }
}

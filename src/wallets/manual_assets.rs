#[cfg(any(feature = "server", test))]
use super::labels::ManualAssetDisplayScale;
use super::primitives::{ReportDateParam, WalletAccountId, WalletId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

#[cfg(any(feature = "server", test))]
use crate::amounts::UnsignedAmount;

const MANUAL_ASSET_BALANCE_LITERAL_MAX_LENGTH: usize = 64;
const MANUAL_ASSET_BALANCE_MAX_FRACTIONAL_DIGITS: u8 = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ManualAssetBalanceAssertionId(Ulid);

impl ManualAssetBalanceAssertionId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Display for ManualAssetBalanceAssertionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ManualAssetBalanceAssertionId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawManualAssetBalance(String);

impl RawManualAssetBalance {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedManualAssetBalanceLiteral {
    trimmed: String,
    normalized_digits: String,
    fractional_digits: u8,
}

fn parse_manual_asset_balance_literal(
    value: &str,
) -> Result<ParsedManualAssetBalanceLiteral, ManualAssetBalanceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ManualAssetBalanceError::Empty);
    }
    if trimmed.len() > MANUAL_ASSET_BALANCE_LITERAL_MAX_LENGTH {
        return Err(ManualAssetBalanceError::TooLong {
            max: MANUAL_ASSET_BALANCE_LITERAL_MAX_LENGTH,
            actual: trimmed.len(),
        });
    }
    if trimmed.starts_with('-') {
        return Err(ManualAssetBalanceError::NegativeNotAllowed);
    }

    let mut parts = trimmed.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() {
        return Err(ManualAssetBalanceError::InvalidFormat);
    }

    if !whole.is_empty() && !whole.as_bytes().iter().all(u8::is_ascii_digit) {
        return Err(ManualAssetBalanceError::InvalidFormat);
    }

    let fraction = match fraction {
        Some(value) => {
            if !value.as_bytes().iter().all(u8::is_ascii_digit) {
                return Err(ManualAssetBalanceError::InvalidFormat);
            }
            if value.len() > usize::from(MANUAL_ASSET_BALANCE_MAX_FRACTIONAL_DIGITS) {
                return Err(ManualAssetBalanceError::TooManyFractionalDigits {
                    max: MANUAL_ASSET_BALANCE_MAX_FRACTIONAL_DIGITS,
                    actual: value.len(),
                });
            }
            value
        }
        None => "",
    };

    if whole.is_empty() && fraction.is_empty() {
        return Err(ManualAssetBalanceError::InvalidFormat);
    }

    let normalized_whole = if whole.is_empty() { "0" } else { whole };
    let mut normalized_digits = String::from(normalized_whole);
    normalized_digits.push_str(fraction);
    normalized_digits
        .parse::<u128>()
        .map_err(|_| ManualAssetBalanceError::Overflow)?;

    Ok(ParsedManualAssetBalanceLiteral {
        trimmed: trimmed.to_string(),
        normalized_digits,
        fractional_digits: fraction.len() as u8,
    })
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManualAssetAmountRescaleError {
    ScaleDecreaseNotAllowed { from: u8, to: u8 },
    Overflow,
}

#[cfg(any(feature = "server", test))]
impl fmt::Display for ManualAssetAmountRescaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScaleDecreaseNotAllowed { from, to } => {
                write!(
                    f,
                    "manual asset precision cannot be reduced during rescale: {from} -> {to}"
                )
            }
            Self::Overflow => write!(f, "manual asset amount rescale overflowed"),
        }
    }
}

#[cfg(any(feature = "server", test))]
impl std::error::Error for ManualAssetAmountRescaleError {}

#[cfg(any(feature = "server", test))]
pub(crate) fn rescale_manual_asset_amount(
    amount: UnsignedAmount,
    from_scale: ManualAssetDisplayScale,
    to_scale: ManualAssetDisplayScale,
) -> Result<UnsignedAmount, ManualAssetAmountRescaleError> {
    let from = from_scale.as_u8();
    let to = to_scale.as_u8();
    if to < from {
        return Err(ManualAssetAmountRescaleError::ScaleDecreaseNotAllowed { from, to });
    }

    let exponent = to - from;
    let multiplier = 10_u128
        .checked_pow(u32::from(exponent))
        .ok_or(ManualAssetAmountRescaleError::Overflow)?;
    let rescaled = amount
        .value()
        .checked_mul(multiplier)
        .ok_or(ManualAssetAmountRescaleError::Overflow)?;
    Ok(UnsignedAmount::from_u128(rescaled))
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedManualAssetBalanceLiteral {
    trimmed: String,
    normalized_digits: String,
    entered_fractional_digits: u8,
}

#[cfg(feature = "server")]
impl ValidatedManualAssetBalanceLiteral {
    pub(crate) fn parse(value: &str) -> Result<Self, ManualAssetBalanceError> {
        let parsed = parse_manual_asset_balance_literal(value)?;
        Ok(Self {
            trimmed: parsed.trimmed,
            normalized_digits: parsed.normalized_digits,
            entered_fractional_digits: parsed.fractional_digits,
        })
    }

    pub(crate) fn trimmed(&self) -> &str {
        &self.trimmed
    }

    pub(crate) const fn entered_fractional_digits(&self) -> u8 {
        self.entered_fractional_digits
    }

    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    pub(crate) fn normalized_digits(&self) -> &str {
        &self.normalized_digits
    }

    pub(crate) fn parse_at_scale(
        &self,
        scale: ManualAssetDisplayScale,
    ) -> Result<ValidatedManualAssetBalance, ManualAssetBalanceError> {
        let entered_scale = ManualAssetDisplayScale::from_u8(self.entered_fractional_digits);
        let base_amount = ValidatedManualAssetBalance::parse(self.trimmed(), entered_scale)?;
        let amount = match rescale_manual_asset_amount(base_amount.amount(), entered_scale, scale) {
            Ok(value) => value,
            Err(ManualAssetAmountRescaleError::ScaleDecreaseNotAllowed { .. }) => {
                return Err(ManualAssetBalanceError::TooManyFractionalDigits {
                    max: scale.as_u8(),
                    actual: usize::from(self.entered_fractional_digits),
                });
            }
            Err(ManualAssetAmountRescaleError::Overflow) => {
                return Err(ManualAssetBalanceError::Overflow);
            }
        };

        Ok(ValidatedManualAssetBalance(amount))
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValidatedManualAssetBalance(UnsignedAmount);

#[cfg(feature = "server")]
impl ValidatedManualAssetBalance {
    pub(crate) fn parse(
        value: &str,
        scale: ManualAssetDisplayScale,
    ) -> Result<Self, ManualAssetBalanceError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ManualAssetBalanceError::Empty);
        }
        if trimmed.starts_with('-') {
            return Err(ManualAssetBalanceError::NegativeNotAllowed);
        }

        let mut parts = trimmed.split('.');
        let whole = parts.next().unwrap_or_default();
        let fraction = parts.next();
        if parts.next().is_some() {
            return Err(ManualAssetBalanceError::InvalidFormat);
        }

        if !whole.is_empty() && !whole.as_bytes().iter().all(u8::is_ascii_digit) {
            return Err(ManualAssetBalanceError::InvalidFormat);
        }

        let scale_len = usize::from(scale.as_u8());
        let fraction_value = match fraction {
            Some(value) => {
                if !value.as_bytes().iter().all(u8::is_ascii_digit) {
                    return Err(ManualAssetBalanceError::InvalidFormat);
                }
                if value.len() > scale_len {
                    return Err(ManualAssetBalanceError::TooManyFractionalDigits {
                        max: scale.as_u8(),
                        actual: value.len(),
                    });
                }
                value
            }
            None => "",
        };

        if whole.is_empty() && fraction_value.is_empty() {
            return Err(ManualAssetBalanceError::InvalidFormat);
        }

        let normalized_whole = if whole.is_empty() { "0" } else { whole };
        let mut digits = String::from(normalized_whole);
        digits.push_str(fraction_value);
        if scale_len > fraction_value.len() {
            digits.push_str(&"0".repeat(scale_len - fraction_value.len()));
        }

        let raw = digits
            .parse::<u128>()
            .map_err(|_| ManualAssetBalanceError::Overflow)?;
        Ok(Self(UnsignedAmount::from_u128(raw)))
    }

    pub(crate) const fn amount(self) -> UnsignedAmount {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetBalanceError {
    Empty,
    TooLong { max: usize, actual: usize },
    InvalidFormat,
    TooManyFractionalDigits { max: u8, actual: usize },
    NegativeNotAllowed,
    Overflow,
}

impl fmt::Display for ManualAssetBalanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "balance cannot be empty"),
            Self::TooLong { max, actual } => {
                write!(f, "balance must be at most {max} characters, got {actual}")
            }
            Self::InvalidFormat => write!(f, "balance must be a non-negative decimal number"),
            Self::TooManyFractionalDigits { max, actual } => {
                write!(
                    f,
                    "balance supports at most {max} fractional digits, got {actual}"
                )
            }
            Self::NegativeNotAllowed => write!(f, "balance cannot be negative"),
            Self::Overflow => write!(f, "balance is too large"),
        }
    }
}

impl std::error::Error for ManualAssetBalanceError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawManualAssetAssertionNote(String);

impl RawManualAssetAssertionNote {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedManualAssetAssertionNote(String);

#[cfg(feature = "server")]
impl ValidatedManualAssetAssertionNote {
    pub(crate) const MAX_LEN: usize = 500;

    pub(crate) fn parse_optional(
        value: Option<RawManualAssetAssertionNote>,
    ) -> Result<Option<Self>, ManualAssetAssertionNoteError> {
        let Some(raw) = value else {
            return Ok(None);
        };
        let trimmed = raw.0.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        if trimmed.len() > Self::MAX_LEN {
            return Err(ManualAssetAssertionNoteError::TooLong {
                max: Self::MAX_LEN,
                actual: trimmed.len(),
            });
        }
        Ok(Some(Self(trimmed.to_string())))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetAssertionNoteError {
    TooLong { max: usize, actual: usize },
}

#[cfg(feature = "server")]
impl fmt::Display for ManualAssetAssertionNoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { max, actual } => {
                write!(f, "note exceeds max length {max}: got {actual}")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for ManualAssetAssertionNoteError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddManualAssetBalanceAssertionRequest {
    pub account_id: WalletAccountId,
    pub asserted_on: ReportDateParam,
    pub balance: RawManualAssetBalance,
    pub note: Option<RawManualAssetAssertionNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddManualAssetBalanceAssertionResponse {
    pub assertion_id: ManualAssetBalanceAssertionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UpdateManualAssetBalanceAssertionRequest {
    pub assertion_id: ManualAssetBalanceAssertionId,
    pub account_id: WalletAccountId,
    pub asserted_on: ReportDateParam,
    pub balance: RawManualAssetBalance,
    pub note: Option<RawManualAssetAssertionNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeleteManualAssetBalanceAssertionRequest {
    pub assertion_id: ManualAssetBalanceAssertionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManualAssetBalanceAssertionRowResponse {
    pub assertion_id: ManualAssetBalanceAssertionId,
    pub asserted_on: String,
    pub asserted_balance: crate::backend::BalanceAmountView,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManualAssetBalanceAssertionTableResponse {
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub start: u32,
    pub end: u32,
    pub rows: Vec<ManualAssetBalanceAssertionRowResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualAssetPrecisionStatus {
    NotInferredYet,
    Inferred,
    LegacyBaseline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetAccountTransactionsResponse {
    pub account_id: WalletAccountId,
    pub wallet_id: WalletId,
    pub wallet_label: String,
    pub account_label: String,
    pub account_state: crate::backend::AccountStateView,
    pub sync_control_enabled: bool,
    pub unit_code: String,
    pub decimal_precision: u8,
    pub precision_status: ManualAssetPrecisionStatus,
    pub precision_shared_with_other_accounts: bool,
    pub symbol: Option<String>,
    pub asset_name: Option<String>,
    pub network_name: Option<String>,
    pub opening_balance_state: crate::backend::AccountBalanceStateView,
    pub opening_balance_date: Option<String>,
    pub closing_balance_state: crate::backend::AccountBalanceStateView,
    pub closing_balance_date: Option<String>,
    pub sort: super::primitives::TransactionSortDirection,
    pub active_from_date: Option<String>,
    pub active_to_date: Option<String>,
    pub assertions: ManualAssetBalanceAssertionTableResponse,
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn validated_manual_asset_balance_literal_preserves_trimmed_text_and_precision() {
        let literal = ValidatedManualAssetBalanceLiteral::parse("  .5000  ")
            .expect("literal should validate");

        assert_eq!(literal.trimmed(), ".5000");
        assert_eq!(literal.entered_fractional_digits(), 4);
        assert_eq!(literal.normalized_digits(), "05000");
    }

    #[test]
    fn validated_manual_asset_balance_literal_rejects_too_many_fractional_digits() {
        assert!(matches!(
            ValidatedManualAssetBalanceLiteral::parse("1.1234567890123456789"),
            Err(ManualAssetBalanceError::TooManyFractionalDigits {
                max: 18,
                actual: 19,
            })
        ));
    }

    #[test]
    fn validated_manual_asset_balance_literal_rejects_too_long_input() {
        let input = format!("1.{}", "0".repeat(63));
        assert!(matches!(
            ValidatedManualAssetBalanceLiteral::parse(&input),
            Err(ManualAssetBalanceError::TooLong {
                max: 64,
                actual: 65
            })
        ));
    }

    #[test]
    fn rescale_manual_asset_amount_multiplies_exactly_for_precision_growth() {
        let rescaled = rescale_manual_asset_amount(
            UnsignedAmount::from_u128(1_234),
            ManualAssetDisplayScale::from_u8(3),
            ManualAssetDisplayScale::from_u8(9),
        )
        .expect("rescale should succeed");

        assert_eq!(rescaled, UnsignedAmount::from_u128(1_234_000_000));
    }

    #[test]
    fn rescale_manual_asset_amount_rejects_precision_decrease() {
        assert!(matches!(
            rescale_manual_asset_amount(
                UnsignedAmount::from_u128(1_234),
                ManualAssetDisplayScale::from_u8(9),
                ManualAssetDisplayScale::from_u8(3),
            ),
            Err(ManualAssetAmountRescaleError::ScaleDecreaseNotAllowed { from: 9, to: 3 })
        ));
    }

    #[test]
    fn validated_manual_asset_balance_literal_parses_at_higher_scale() {
        let literal =
            ValidatedManualAssetBalanceLiteral::parse("1.2300").expect("literal should validate");
        let amount = literal
            .parse_at_scale(ManualAssetDisplayScale::from_u8(6))
            .expect("higher scale parse should succeed");

        assert_eq!(amount.amount(), UnsignedAmount::from_u128(1_230_000));
    }
}

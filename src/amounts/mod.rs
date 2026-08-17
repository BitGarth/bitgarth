use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(any(feature = "server", test))]
pub(crate) const GLOBAL_SPLIT_DIVISOR: i64 = 1_000_000_000_000_000_000;

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssetClass {
    Crypto,
    Fiat,
    Equity,
    Commodity,
    Nft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UnsignedAmount(u128);

impl UnsignedAmount {
    pub(crate) const fn zero() -> Self {
        Self(0)
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) fn try_from_i64(value: i64) -> Result<Self, AmountError> {
        if value < 0 {
            return Err(AmountError::NegativeNotAllowed {
                value: i128::from(value),
            });
        }
        Ok(Self(value as u128))
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) const fn value(self) -> u128 {
        self.0
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) fn raw_string(self) -> String {
        self.0.to_string()
    }

    pub(crate) fn checked_add(self, other: Self) -> Result<Self, AmountError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(AmountError::Overflow)
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmountSplitConfig {
    divisor: i64,
}

#[cfg(any(feature = "server", test))]
impl AmountSplitConfig {
    #[cfg(any(feature = "server", test))]
    pub(crate) fn encode_unsigned(
        self,
        amount: UnsignedAmount,
    ) -> Result<AmountSplitParts, AmountError> {
        let divisor_u128 = self.divisor as u128;
        let hi_u128 = amount.value() / divisor_u128;
        let lo_u128 = amount.value() % divisor_u128;

        let hi = i64::try_from(hi_u128).map_err(|_| AmountError::Overflow)?;
        let lo = i64::try_from(lo_u128).map_err(|_| AmountError::Overflow)?;

        Ok(AmountSplitParts { hi, lo })
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) fn decode_unsigned(self, hi: i64, lo: i64) -> Result<UnsignedAmount, AmountError> {
        if hi < 0 {
            return Err(AmountError::InvalidSplitHi { hi });
        }

        if !(0..self.divisor).contains(&lo) {
            return Err(AmountError::InvalidSplitLo {
                lo,
                divisor: self.divisor,
            });
        }

        let value = (hi as u128)
            .checked_mul(self.divisor as u128)
            .and_then(|base| base.checked_add(lo as u128))
            .ok_or(AmountError::Overflow)?;

        Ok(UnsignedAmount::from_u128(value))
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) fn global_split_config() -> AmountSplitConfig {
    AmountSplitConfig {
        divisor: GLOBAL_SPLIT_DIVISOR,
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmountSplitParts {
    pub hi: i64,
    pub lo: i64,
}

#[cfg(any(feature = "server", test))]
pub(crate) fn format_unsigned_amount(value: UnsignedAmount, scale: u8) -> String {
    if value.value() == 0 {
        return "0".to_string();
    }

    if scale == 0 {
        return value.raw_string();
    }

    let raw = value.raw_string();
    let scale_len = usize::from(scale);

    if raw.len() <= scale_len {
        let mut fraction = String::with_capacity(scale_len);
        fraction.push_str(&"0".repeat(scale_len - raw.len()));
        fraction.push_str(&raw);

        let trimmed_fraction = fraction.trim_end_matches('0');
        if trimmed_fraction.is_empty() {
            "0".to_string()
        } else {
            format!("0.{trimmed_fraction}")
        }
    } else {
        let split_at = raw.len() - scale_len;
        let whole = &raw[..split_at];
        let fraction = &raw[split_at..];
        let trimmed_fraction = fraction.trim_end_matches('0');

        if trimmed_fraction.is_empty() {
            whole.to_string()
        } else {
            format!("{whole}.{trimmed_fraction}")
        }
    }
}

#[cfg(feature = "server")] // used in server-side export code, dead in WASM client build
pub(crate) fn format_unsigned_amount_fixed(value: UnsignedAmount, scale: u8) -> String {
    if scale == 0 {
        return value.raw_string();
    }

    let raw = value.raw_string();
    let scale_len = usize::from(scale);

    if raw.len() <= scale_len {
        let mut fraction = String::with_capacity(scale_len);
        fraction.push_str(&"0".repeat(scale_len - raw.len()));
        fraction.push_str(&raw);
        format!("0.{fraction}")
    } else {
        let split_at = raw.len() - scale_len;
        let whole = &raw[..split_at];
        let fraction = &raw[split_at..];
        format!("{whole}.{fraction}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AmountError {
    #[cfg(any(feature = "server", test))]
    NegativeNotAllowed {
        value: i128,
    },
    ParseError {
        input: String,
        reason: String,
    },
    #[cfg(any(feature = "server", test))]
    InvalidSplitHi {
        hi: i64,
    },
    #[cfg(any(feature = "server", test))]
    InvalidSplitLo {
        lo: i64,
        divisor: i64,
    },
    Overflow,
}

impl fmt::Display for AmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(any(feature = "server", test))]
            AmountError::NegativeNotAllowed { value } => {
                write!(f, "amount must be non-negative, got {value}")
            }
            AmountError::ParseError { input, reason } => {
                write!(f, "failed to parse amount '{input}': {reason}")
            }
            #[cfg(any(feature = "server", test))]
            AmountError::InvalidSplitHi { hi } => {
                write!(f, "split hi must be non-negative, got {hi}")
            }
            #[cfg(any(feature = "server", test))]
            AmountError::InvalidSplitLo { lo, divisor } => {
                write!(f, "split lo must be in range [0, {divisor}), got {lo}")
            }
            AmountError::Overflow => write!(f, "amount overflow"),
        }
    }
}

impl std::error::Error for AmountError {}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn unsigned_amount_rejects_negative_i64() {
        assert!(matches!(
            UnsignedAmount::try_from_i64(-1),
            Err(AmountError::NegativeNotAllowed { .. })
        ));
    }

    #[test]
    fn split_roundtrip_unsigned_uses_global_divisor() {
        let cfg = global_split_config();
        let amount = UnsignedAmount::from_u128(1_500_000_000);
        let parts = cfg.encode_unsigned(amount).expect("encode");
        assert_eq!(parts.hi, 0);
        assert_eq!(parts.lo, 1_500_000_000);

        let decoded = cfg.decode_unsigned(parts.hi, parts.lo).expect("decode");
        assert_eq!(decoded, amount);
    }

    #[test]
    fn split_roundtrip_unsigned_preserves_high_and_low_parts() {
        let cfg = global_split_config();
        let amount =
            UnsignedAmount::from_u128((GLOBAL_SPLIT_DIVISOR as u128 * 42) + 123_456_789_u128);
        let parts = cfg.encode_unsigned(amount).expect("encode");
        assert_eq!(parts.hi, 42);
        assert_eq!(parts.lo, 123_456_789);

        let decoded = cfg.decode_unsigned(parts.hi, parts.lo).expect("decode");
        assert_eq!(decoded, amount);
    }

    #[test]
    fn format_unsigned_amount_trims_trailing_zeros() {
        assert_eq!(
            format_unsigned_amount(UnsignedAmount::from_u128(123_400_000), 8),
            "1.234"
        );
        assert_eq!(
            format_unsigned_amount(UnsignedAmount::from_u128(100_000_000), 8),
            "1"
        );
        assert_eq!(format_unsigned_amount(UnsignedAmount::zero(), 8), "0");
    }

    #[cfg(feature = "server")]
    #[test]
    fn format_unsigned_amount_fixed_keeps_scale() {
        assert_eq!(
            format_unsigned_amount_fixed(UnsignedAmount::from_u128(123_400_000), 8),
            "1.23400000"
        );
        assert_eq!(
            format_unsigned_amount_fixed(UnsignedAmount::from_u128(100_000_000), 8),
            "1.00000000"
        );
        assert_eq!(
            format_unsigned_amount_fixed(UnsignedAmount::from_u128(10), 8),
            "0.00000010"
        );
        assert_eq!(
            format_unsigned_amount_fixed(UnsignedAmount::zero(), 8),
            "0.00000000"
        );
        assert_eq!(
            format_unsigned_amount_fixed(UnsignedAmount::from_u128(42), 0),
            "42"
        );
    }
}

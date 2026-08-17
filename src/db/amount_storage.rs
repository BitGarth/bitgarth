use crate::amounts::{
    AmountError, AmountSplitParts, GLOBAL_SPLIT_DIVISOR, UnsignedAmount, global_split_config,
};

pub(super) fn split_unsigned_amount(
    amount: UnsignedAmount,
) -> Result<AmountSplitParts, AmountError> {
    global_split_config().encode_unsigned(amount)
}

pub(super) fn parse_split_amount(hi: i64, lo: i64) -> Result<UnsignedAmount, AmountError> {
    global_split_config().decode_unsigned(hi, lo)
}

pub(super) fn parse_optional_split_amount(
    hi: Option<i64>,
    lo: Option<i64>,
) -> Result<Option<UnsignedAmount>, AmountError> {
    match (hi, lo) {
        (None, None) => Ok(None),
        (Some(hi), Some(lo)) => parse_split_amount(hi, lo).map(Some),
        (Some(_), None) | (None, Some(_)) => Err(AmountError::ParseError {
            input: format!("hi={hi:?},lo={lo:?}"),
            reason: "split amount parts must be both present or both absent".to_string(),
        }),
    }
}

pub(super) fn normalize_split_sums(
    hi_sum: i64,
    lo_sum: i64,
) -> Result<AmountSplitParts, AmountError> {
    if hi_sum < 0 {
        return Err(AmountError::InvalidSplitHi { hi: hi_sum });
    }

    if lo_sum < 0 {
        return Err(AmountError::InvalidSplitLo {
            lo: lo_sum,
            divisor: GLOBAL_SPLIT_DIVISOR,
        });
    }

    let divisor = i128::from(GLOBAL_SPLIT_DIVISOR);
    let carry = i128::from(lo_sum) / divisor;
    let normalized_hi = i128::from(hi_sum)
        .checked_add(carry)
        .ok_or(AmountError::Overflow)?;
    let normalized_lo = i128::from(lo_sum) % divisor;

    Ok(AmountSplitParts {
        hi: i64::try_from(normalized_hi).map_err(|_| AmountError::Overflow)?,
        lo: i64::try_from(normalized_lo).map_err(|_| AmountError::Overflow)?,
    })
}

pub(super) fn parse_split_sum(hi_sum: i64, lo_sum: i64) -> Result<UnsignedAmount, AmountError> {
    let normalized = normalize_split_sums(hi_sum, lo_sum)?;
    parse_split_amount(normalized.hi, normalized.lo)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn split_round_trip_preserves_amount() {
        let amount = UnsignedAmount::from_u128(1_234_567_890_123_456_789);
        let parts = split_unsigned_amount(amount).expect("amount should split");
        let decoded = parse_split_amount(parts.hi, parts.lo).expect("split amount should decode");

        assert_eq!(decoded, amount);
    }

    #[test]
    fn optional_parse_rejects_partial_pairs() {
        let result = parse_optional_split_amount(Some(1), None);

        assert!(result.is_err(), "partial split pairs should be rejected");
    }

    #[test]
    fn split_sum_normalizes_lo_carry() {
        let amount = parse_split_sum(1, GLOBAL_SPLIT_DIVISOR + 42).expect("sum should normalize");

        assert_eq!(amount.value(), 2_000_000_000_000_000_042_u128);
    }

    #[test]
    fn split_sum_rejects_negative_lo() {
        let result = parse_split_sum(0, -1);

        assert!(result.is_err(), "negative summed lo should be rejected");
    }
}

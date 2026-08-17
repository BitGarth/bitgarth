use crate::amounts::UnsignedAmount;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountBalanceDisplayState {
    KnownLedger {
        amount: UnsignedAmount,
        as_of: Option<DateTime<Utc>>,
    },
    KnownApiConfirmed {
        amount: UnsignedAmount,
        as_of: DateTime<Utc>,
    },
    CanonicalZero,
    Unknown,
    UnavailableOnFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountBalanceBoundaryKind {
    Opening,
    Closing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentAccountBalanceInputs {
    pub(crate) ledger_amount: Option<UnsignedAmount>,
    pub(crate) ledger_as_of: Option<DateTime<Utc>>,
    pub(crate) api_confirmed_amount: Option<UnsignedAmount>,
    pub(crate) api_confirmed_as_of: Option<DateTime<Utc>>,
    pub(crate) free_balance_unavailable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundaryAccountBalanceInputs {
    pub(crate) boundary_kind: AccountBalanceBoundaryKind,
    pub(crate) requested_boundary_date: Option<DateTime<Utc>>,
    pub(crate) first_transaction_date: Option<DateTime<Utc>>,
    pub(crate) last_successful_sync_date: Option<DateTime<Utc>>,
    pub(crate) ledger_amount: Option<UnsignedAmount>,
    pub(crate) api_confirmed_amount: Option<UnsignedAmount>,
    pub(crate) free_balance_unavailable: bool,
    pub(crate) transaction_history_pending: bool,
}

pub(crate) fn resolve_current_account_balance_state(
    inputs: CurrentAccountBalanceInputs,
) -> AccountBalanceDisplayState {
    if let Some(amount) = inputs.ledger_amount {
        return AccountBalanceDisplayState::KnownLedger {
            amount,
            as_of: inputs.ledger_as_of,
        };
    }

    if inputs.free_balance_unavailable {
        return AccountBalanceDisplayState::UnavailableOnFree;
    }

    if let (Some(amount), Some(as_of)) = (inputs.api_confirmed_amount, inputs.api_confirmed_as_of) {
        return AccountBalanceDisplayState::KnownApiConfirmed { amount, as_of };
    }

    AccountBalanceDisplayState::Unknown
}

pub(crate) fn resolve_boundary_account_balance_state(
    inputs: BoundaryAccountBalanceInputs,
) -> AccountBalanceDisplayState {
    if let Some(amount) = inputs.ledger_amount {
        return AccountBalanceDisplayState::KnownLedger {
            amount,
            as_of: inputs.requested_boundary_date,
        };
    }

    if inputs.free_balance_unavailable {
        return AccountBalanceDisplayState::UnavailableOnFree;
    }

    if boundary_can_use_api_confirmed_balance(&inputs)
        && let (Some(amount), Some(as_of)) = (
            inputs.api_confirmed_amount,
            inputs.last_successful_sync_date,
        )
    {
        return AccountBalanceDisplayState::KnownApiConfirmed { amount, as_of };
    }

    if inputs.first_transaction_date.is_none()
        && !inputs.transaction_history_pending
        && let (Some(boundary_date), Some(last_sync)) = (
            inputs.requested_boundary_date,
            inputs.last_successful_sync_date,
        )
        && boundary_date <= last_sync
    {
        return AccountBalanceDisplayState::CanonicalZero;
    }

    AccountBalanceDisplayState::Unknown
}

fn boundary_can_use_api_confirmed_balance(inputs: &BoundaryAccountBalanceInputs) -> bool {
    match (
        inputs.boundary_kind,
        inputs.requested_boundary_date,
        inputs.last_successful_sync_date,
    ) {
        (AccountBalanceBoundaryKind::Closing, Some(requested), Some(last_sync)) => {
            requested >= last_sync
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn amount(value: u128) -> UnsignedAmount {
        UnsignedAmount::from_u128(value)
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("test timestamp should be valid")
    }

    #[test]
    fn current_balance_preserves_ledger_zero_over_api_confirmed_amount() {
        let as_of = at(100);
        let state = resolve_current_account_balance_state(CurrentAccountBalanceInputs {
            ledger_amount: Some(UnsignedAmount::zero()),
            ledger_as_of: Some(as_of),
            api_confirmed_amount: Some(amount(50)),
            api_confirmed_as_of: Some(at(200)),
            free_balance_unavailable: false,
        });

        assert_eq!(
            state,
            AccountBalanceDisplayState::KnownLedger {
                amount: UnsignedAmount::zero(),
                as_of: Some(as_of),
            }
        );
    }

    #[test]
    fn current_balance_uses_api_confirmed_when_no_ledger_basis_exists() {
        let api_as_of = at(200);
        let state = resolve_current_account_balance_state(CurrentAccountBalanceInputs {
            ledger_amount: None,
            ledger_as_of: None,
            api_confirmed_amount: Some(amount(50)),
            api_confirmed_as_of: Some(api_as_of),
            free_balance_unavailable: false,
        });

        assert_eq!(
            state,
            AccountBalanceDisplayState::KnownApiConfirmed {
                amount: amount(50),
                as_of: api_as_of,
            }
        );
    }

    #[test]
    fn current_balance_reports_free_unavailable_before_api_fallback() {
        let state = resolve_current_account_balance_state(CurrentAccountBalanceInputs {
            ledger_amount: None,
            ledger_as_of: None,
            api_confirmed_amount: Some(amount(50)),
            api_confirmed_as_of: Some(at(200)),
            free_balance_unavailable: true,
        });

        assert_eq!(state, AccountBalanceDisplayState::UnavailableOnFree);
    }

    #[test]
    fn closing_boundary_after_sync_uses_api_confirmed_without_ledger_basis() {
        let state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
            boundary_kind: AccountBalanceBoundaryKind::Closing,
            requested_boundary_date: Some(at(300)),
            first_transaction_date: None,
            last_successful_sync_date: Some(at(200)),
            ledger_amount: None,
            api_confirmed_amount: Some(amount(50)),
            free_balance_unavailable: false,
            transaction_history_pending: false,
        });

        assert_eq!(
            state,
            AccountBalanceDisplayState::KnownApiConfirmed {
                amount: amount(50),
                as_of: at(200),
            }
        );
    }

    #[test]
    fn opening_boundary_before_sync_does_not_use_later_api_balance() {
        let state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
            boundary_kind: AccountBalanceBoundaryKind::Opening,
            requested_boundary_date: Some(at(100)),
            first_transaction_date: None,
            last_successful_sync_date: Some(at(200)),
            ledger_amount: None,
            api_confirmed_amount: Some(amount(50)),
            free_balance_unavailable: false,
            transaction_history_pending: false,
        });

        assert_eq!(state, AccountBalanceDisplayState::CanonicalZero);
    }

    #[test]
    fn opening_boundary_is_unknown_when_history_is_pending() {
        let state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
            boundary_kind: AccountBalanceBoundaryKind::Opening,
            requested_boundary_date: Some(at(100)),
            first_transaction_date: None,
            last_successful_sync_date: Some(at(200)),
            ledger_amount: None,
            api_confirmed_amount: Some(amount(50)),
            free_balance_unavailable: false,
            transaction_history_pending: true,
        });
        assert_eq!(state, AccountBalanceDisplayState::Unknown);
    }

    #[test]
    fn closing_boundary_before_sync_is_unknown_when_history_is_pending() {
        let state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
            boundary_kind: AccountBalanceBoundaryKind::Closing,
            requested_boundary_date: Some(at(100)),
            first_transaction_date: None,
            last_successful_sync_date: Some(at(200)),
            ledger_amount: None,
            api_confirmed_amount: Some(amount(50)),
            free_balance_unavailable: false,
            transaction_history_pending: true,
        });
        assert_eq!(state, AccountBalanceDisplayState::Unknown);
    }

    #[test]
    fn boundary_ledger_basis_wins_over_api_confirmed_amount() {
        let state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
            boundary_kind: AccountBalanceBoundaryKind::Closing,
            requested_boundary_date: Some(at(300)),
            first_transaction_date: None,
            last_successful_sync_date: Some(at(200)),
            ledger_amount: Some(UnsignedAmount::zero()),
            api_confirmed_amount: Some(amount(50)),
            free_balance_unavailable: false,
            transaction_history_pending: false,
        });

        assert_eq!(
            state,
            AccountBalanceDisplayState::KnownLedger {
                amount: UnsignedAmount::zero(),
                as_of: Some(at(300)),
            }
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAccountMode {
    Transactions,
    BalanceOnly,
    Inactive,
}

impl NativeAccountMode {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Transactions => "Transaction syncing",
            Self::BalanceOnly => "Balance only",
            Self::Inactive => "Inactive",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncPauseReason {
    AccountAllowance,
    TransactionThreshold,
    Inactive,
}

impl TransactionSyncPauseReason {
    pub(crate) const fn notice(self) -> &'static str {
        match self {
            Self::AccountAllowance => {
                "Transaction syncing is not included for this account under your current allowance. Balances continue to update independently."
            }
            Self::TransactionThreshold => {
                "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. Balances continue to update independently."
            }
            Self::Inactive => {
                "This account is inactive under your current allowance. Neither balance nor transaction syncing is active."
            }
        }
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) fn transaction_sync_pause_reason(
    mode: NativeAccountMode,
    canonical_transaction_count: u32,
    threshold: u32,
) -> Option<TransactionSyncPauseReason> {
    match mode {
        NativeAccountMode::Transactions if canonical_transaction_count >= threshold => {
            Some(TransactionSyncPauseReason::TransactionThreshold)
        }
        NativeAccountMode::Transactions => None,
        NativeAccountMode::BalanceOnly => Some(TransactionSyncPauseReason::AccountAllowance),
        NativeAccountMode::Inactive => Some(TransactionSyncPauseReason::Inactive),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_reason_follows_mode_then_canonical_threshold() {
        assert_eq!(
            transaction_sync_pause_reason(NativeAccountMode::Transactions, 999, 1000),
            None
        );
        assert_eq!(
            transaction_sync_pause_reason(NativeAccountMode::Transactions, 1000, 1000),
            Some(TransactionSyncPauseReason::TransactionThreshold)
        );
        assert_eq!(
            transaction_sync_pause_reason(NativeAccountMode::BalanceOnly, 1000, 1000),
            Some(TransactionSyncPauseReason::AccountAllowance)
        );
        assert_eq!(
            transaction_sync_pause_reason(NativeAccountMode::Inactive, 0, 1000),
            Some(TransactionSyncPauseReason::Inactive)
        );
    }

    #[test]
    fn threshold_notice_describes_both_missing_history_directions() {
        assert_eq!(
            TransactionSyncPauseReason::TransactionThreshold.notice(),
            "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. Balances continue to update independently."
        );
    }
}

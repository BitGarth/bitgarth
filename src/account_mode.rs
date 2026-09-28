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

    /// The label to show users: a transaction-syncing account that hit its plan
    /// limit is not syncing history, so it must not say it is.
    pub(crate) const fn status_label(self, history_paused_at_limit: bool) -> &'static str {
        match self {
            Self::Transactions if history_paused_at_limit => "History paused (plan limit)",
            other => other.label(),
        }
    }
}

/// Product policy: how often an active account is refreshed when nothing is
/// urgent. It is the scheduler's staleness threshold, the balance-refresh TTL
/// for accounts not fetching history, and the Etherscan success cooldown, and
/// the pause notices promise it to users. Change it here only.
pub(crate) const ACCOUNT_REFRESH_INTERVAL_MINUTES: u64 = 15;

#[cfg(feature = "server")]
pub(crate) const ACCOUNT_REFRESH_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(ACCOUNT_REFRESH_INTERVAL_MINUTES * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncPauseReason {
    AccountAllowance,
    TransactionThreshold,
    Inactive,
}

impl TransactionSyncPauseReason {
    pub(crate) fn notice(self) -> String {
        let balances = format!(
            "Balances still refresh about every {ACCOUNT_REFRESH_INTERVAL_MINUTES} minutes."
        );
        match self {
            Self::AccountAllowance => format!(
                "Transaction syncing is not included for this account under your current allowance. {balances}"
            ),
            Self::TransactionThreshold => format!(
                "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. {balances}"
            ),
            Self::Inactive => "This account is inactive under your current allowance. Neither balance nor transaction syncing is active.".to_string(),
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
    fn status_label_reports_paused_history_only_for_transaction_mode() {
        assert_eq!(
            NativeAccountMode::Transactions.status_label(false),
            "Transaction syncing"
        );
        assert_eq!(
            NativeAccountMode::Transactions.status_label(true),
            "History paused (plan limit)"
        );
        assert_eq!(
            NativeAccountMode::BalanceOnly.status_label(true),
            "Balance only"
        );
    }

    #[test]
    fn threshold_notice_describes_both_missing_history_directions() {
        assert_eq!(
            TransactionSyncPauseReason::TransactionThreshold.notice(),
            "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. Balances still refresh about every 15 minutes."
        );
    }
}

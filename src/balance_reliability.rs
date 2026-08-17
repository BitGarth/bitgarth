use serde::{Deserialize, Serialize};
#[cfg(any(feature = "server", test))]
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BalanceProvisionalReason {
    FirstSuccessfulSyncPending,
    InactiveAccountNotSyncing,
    HistoricalBackfillInProgress,
    HistoricalCoverageLimited,
    PartialSyncRecoveryPending,
    PendingLedgerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BitcoinHistoryCoverageView {
    Unscanned,
    Syncing,
    Limited,
    Complete,
}

impl BitcoinHistoryCoverageView {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Unscanned => "Unscanned",
            Self::Syncing => "Syncing",
            Self::Limited => "Coverage limited",
            Self::Complete => "Complete",
        }
    }
}

#[cfg(feature = "server")]
impl From<crate::db::BitcoinAccountHistoryCoverage> for BitcoinHistoryCoverageView {
    fn from(value: crate::db::BitcoinAccountHistoryCoverage) -> Self {
        match value {
            crate::db::BitcoinAccountHistoryCoverage::Unscanned => Self::Unscanned,
            crate::db::BitcoinAccountHistoryCoverage::Syncing => Self::Syncing,
            crate::db::BitcoinAccountHistoryCoverage::Limited => Self::Limited,
            crate::db::BitcoinAccountHistoryCoverage::Complete { .. } => Self::Complete,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BalanceReliability {
    #[default]
    Final,
    Provisional {
        reasons: Vec<BalanceProvisionalReason>,
    },
}

impl BalanceReliability {
    pub(crate) fn is_provisional(&self) -> bool {
        matches!(self, Self::Provisional { .. })
    }
}

#[cfg(any(feature = "server", test))]
impl BalanceReliability {
    #[cfg(feature = "server")]
    pub(crate) fn finalized() -> Self {
        Self::Final
    }

    pub(crate) fn from_reasons<I>(reasons: I) -> Self
    where
        I: IntoIterator<Item = BalanceProvisionalReason>,
    {
        let deduped = reasons.into_iter().collect::<BTreeSet<_>>();
        if deduped.is_empty() {
            return Self::Final;
        }

        Self::Provisional {
            reasons: deduped.into_iter().collect(),
        }
    }

    pub(crate) fn reasons(&self) -> &[BalanceProvisionalReason] {
        match self {
            Self::Final => &[],
            Self::Provisional { reasons } => reasons.as_slice(),
        }
    }

    pub(crate) fn combine(&self, other: &Self) -> Self {
        let reasons = self
            .reasons()
            .iter()
            .copied()
            .chain(other.reasons().iter().copied());
        Self::from_reasons(reasons)
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn from_reasons_deduplicates_and_sorts() {
        let reliability = BalanceReliability::from_reasons([
            BalanceProvisionalReason::PendingLedgerState,
            BalanceProvisionalReason::FirstSuccessfulSyncPending,
            BalanceProvisionalReason::PendingLedgerState,
        ]);

        assert_eq!(
            reliability,
            BalanceReliability::Provisional {
                reasons: vec![
                    BalanceProvisionalReason::FirstSuccessfulSyncPending,
                    BalanceProvisionalReason::PendingLedgerState,
                ],
            }
        );
    }

    #[test]
    fn combine_unions_reasons() {
        let left = BalanceReliability::from_reasons([
            BalanceProvisionalReason::HistoricalBackfillInProgress,
        ]);
        let right = BalanceReliability::from_reasons([
            BalanceProvisionalReason::FirstSuccessfulSyncPending,
        ]);

        assert_eq!(
            left.combine(&right),
            BalanceReliability::Provisional {
                reasons: vec![
                    BalanceProvisionalReason::FirstSuccessfulSyncPending,
                    BalanceProvisionalReason::HistoricalBackfillInProgress,
                ],
            }
        );
    }

    #[test]
    fn is_provisional_detects_provisional_state() {
        let reliability =
            BalanceReliability::from_reasons([BalanceProvisionalReason::PendingLedgerState]);

        assert!(reliability.is_provisional());
        assert_eq!(
            reliability.reasons(),
            &[BalanceProvisionalReason::PendingLedgerState]
        );
    }
}

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
    CompleteThrough { block_height: i64 },
}

impl BitcoinHistoryCoverageView {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Unscanned => "Unscanned".to_string(),
            Self::Syncing => "Syncing".to_string(),
            Self::Limited => "Coverage limited".to_string(),
            Self::Complete => "Coverage boundary unknown".to_string(),
            Self::CompleteThrough { block_height } => {
                format!("Verified through block {block_height}")
            }
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
            crate::db::BitcoinAccountHistoryCoverage::Complete { coverage_height } => {
                Self::CompleteThrough {
                    block_height: coverage_height.value(),
                }
            }
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

    #[test]
    fn completed_bitcoin_proof_keeps_its_height_when_tip_moves() {
        let newer_observed_tip = 900_001;
        let view = BitcoinHistoryCoverageView::CompleteThrough {
            block_height: 900_000,
        };
        assert!(
            matches!(view, BitcoinHistoryCoverageView::CompleteThrough { block_height } if block_height < newer_observed_tip)
        );
        assert_eq!(view.label(), "Verified through block 900000");
        let legacy: BitcoinHistoryCoverageView =
            serde_json::from_str("\"complete\"").expect("legacy coverage should parse");
        assert_eq!(legacy.label(), "Coverage boundary unknown");
    }
}

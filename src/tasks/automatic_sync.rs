use crate::sync_control::SyncControlMode;
use crate::transactions::{AccountSyncSnapshot, SyncIntegrationId, TransactionSyncScope};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::time::Duration;

pub(crate) const AUTOMATIC_SYNC_FALLBACK_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const AUTOMATIC_SYNC_WARM_STALE_AFTER: Duration = Duration::from_secs(15 * 60);
pub(crate) const AUTOMATIC_SYNC_COLD_STALE_AFTER: Duration = Duration::from_secs(60 * 60);
pub(crate) const AUTOMATIC_SYNC_IDLE_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncIneligibilityReason {
    MissingActiveSession,
    UserDbUnavailable,
    MissingActiveSessionAndUserDbUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncEligibility {
    Eligible,
    Ineligible {
        reason: AutomaticSyncIneligibilityReason,
    },
}

/// Determine if automatic sync is eligible for a user.
///
/// An "open user DB" means the user database has been initialized with a valid
/// `UserDbOpenMode` (encrypted with DEK, unencrypted dev, or plaintext test).
/// For encrypted databases, this implies the DEK is available in memory and
/// the database is unlocked.
pub(crate) fn automatic_sync_eligibility(
    has_open_user_db: bool,
    has_active_session: bool,
) -> AutomaticSyncEligibility {
    match (has_open_user_db, has_active_session) {
        (true, true) => AutomaticSyncEligibility::Eligible,
        (true, false) => AutomaticSyncEligibility::Ineligible {
            reason: AutomaticSyncIneligibilityReason::MissingActiveSession,
        },
        (false, true) => AutomaticSyncEligibility::Ineligible {
            reason: AutomaticSyncIneligibilityReason::UserDbUnavailable,
        },
        (false, false) => AutomaticSyncEligibility::Ineligible {
            reason: AutomaticSyncIneligibilityReason::MissingActiveSessionAndUserDbUnavailable,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncFreshnessClass {
    Urgent,
    Warm,
    Cold,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AutomaticSyncFreshnessSummary {
    pub(crate) urgent_accounts: usize,
    pub(crate) stale_warm_accounts: usize,
    pub(crate) stale_cold_accounts: usize,
    pub(crate) fresh_accounts: usize,
    pub(crate) next_due_in: Option<Duration>,
    stale_integrations: HashSet<SyncIntegrationId>,
    has_unknown_stale_integration: bool,
}

impl AutomaticSyncFreshnessSummary {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn has_stale_work(&self) -> bool {
        self.urgent_accounts > 0 || self.stale_warm_accounts > 0 || self.stale_cold_accounts > 0
    }

    pub(crate) fn highest_stale_class(&self) -> Option<SyncFreshnessClass> {
        if self.urgent_accounts > 0 {
            Some(SyncFreshnessClass::Urgent)
        } else if self.stale_warm_accounts > 0 {
            Some(SyncFreshnessClass::Warm)
        } else if self.stale_cold_accounts > 0 {
            Some(SyncFreshnessClass::Cold)
        } else {
            None
        }
    }

    pub(crate) fn stale_integrations(&self) -> &HashSet<SyncIntegrationId> {
        &self.stale_integrations
    }

    pub(crate) fn has_unknown_stale_integration(&self) -> bool {
        self.has_unknown_stale_integration
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncBlockState {
    Unblocked,
    PartiallyBlocked,
    FullyBlocked { next_retry_in: Duration },
}

pub(crate) fn automatic_sync_block_state(
    summary: &AutomaticSyncFreshnessSummary,
    blocked_integrations: &HashSet<SyncIntegrationId>,
    blocked_retry_in: Option<Duration>,
) -> AutomaticSyncBlockState {
    if !summary.has_stale_work() {
        return AutomaticSyncBlockState::Unblocked;
    }

    if summary.has_unknown_stale_integration() {
        return AutomaticSyncBlockState::PartiallyBlocked;
    }

    let stale_integrations = summary.stale_integrations();
    if stale_integrations.is_empty() {
        return AutomaticSyncBlockState::Unblocked;
    }

    if stale_integrations
        .iter()
        .all(|integration_id| blocked_integrations.contains(integration_id))
    {
        return AutomaticSyncBlockState::FullyBlocked {
            next_retry_in: blocked_retry_in.unwrap_or(AUTOMATIC_SYNC_FALLBACK_INTERVAL),
        };
    }

    AutomaticSyncBlockState::PartiallyBlocked
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncEnqueueReason {
    UrgentWork,
    StaleWork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncKeepScheduledReason {
    FreshUntilNextWindow,
    BlockedUntilRetry,
    InspectionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncSkipReason {
    Ineligible {
        reason: AutomaticSyncIneligibilityReason,
    },
    DisabledBySyncControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncDecision {
    EnqueueNow {
        reason: AutomaticSyncEnqueueReason,
    },
    KeepScheduled {
        reason: AutomaticSyncKeepScheduledReason,
        next_due_in: Duration,
    },
    Skip {
        reason: AutomaticSyncSkipReason,
    },
}

pub(crate) fn automatic_sync_decision(
    eligibility: AutomaticSyncEligibility,
    freshness: &AutomaticSyncFreshnessSummary,
    block_state: AutomaticSyncBlockState,
) -> AutomaticSyncDecision {
    match eligibility {
        AutomaticSyncEligibility::Ineligible { reason } => AutomaticSyncDecision::Skip {
            reason: AutomaticSyncSkipReason::Ineligible { reason },
        },
        AutomaticSyncEligibility::Eligible if !freshness.has_stale_work() => {
            AutomaticSyncDecision::KeepScheduled {
                reason: AutomaticSyncKeepScheduledReason::FreshUntilNextWindow,
                next_due_in: freshness
                    .next_due_in
                    .unwrap_or(AUTOMATIC_SYNC_IDLE_INTERVAL),
            }
        }
        AutomaticSyncEligibility::Eligible => match block_state {
            AutomaticSyncBlockState::FullyBlocked { next_retry_in } => {
                AutomaticSyncDecision::KeepScheduled {
                    reason: AutomaticSyncKeepScheduledReason::BlockedUntilRetry,
                    next_due_in: next_retry_in,
                }
            }
            AutomaticSyncBlockState::Unblocked | AutomaticSyncBlockState::PartiallyBlocked => {
                AutomaticSyncDecision::EnqueueNow {
                    reason: match freshness.highest_stale_class() {
                        Some(SyncFreshnessClass::Urgent) => AutomaticSyncEnqueueReason::UrgentWork,
                        Some(SyncFreshnessClass::Warm | SyncFreshnessClass::Cold) => {
                            AutomaticSyncEnqueueReason::StaleWork
                        }
                        None => AutomaticSyncEnqueueReason::StaleWork,
                    },
                }
            }
        },
    }
}

pub(crate) fn is_automatic_sync_suppressed(sync_control_mode: SyncControlMode) -> bool {
    matches!(sync_control_mode, SyncControlMode::Enabled)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticSyncAddTarget {
    BitcoinAddress { address_id: DigitalAssetAddressId },
    EthereumAddress { address_id: DigitalAssetAddressId },
    Account { account_id: DigitalAssetAccountId },
    MultiAccountImport,
}

pub(crate) fn automatic_add_sync_scope(target: AutomaticSyncAddTarget) -> TransactionSyncScope {
    match target {
        AutomaticSyncAddTarget::BitcoinAddress { address_id }
        | AutomaticSyncAddTarget::EthereumAddress { address_id } => {
            TransactionSyncScope::Address { address_id }
        }
        AutomaticSyncAddTarget::Account { account_id } => {
            TransactionSyncScope::Account { account_id }
        }
        AutomaticSyncAddTarget::MultiAccountImport => TransactionSyncScope::User,
    }
}

pub(crate) fn should_enqueue_automatic_add_sync(sync_control_mode: SyncControlMode) -> bool {
    !is_automatic_sync_suppressed(sync_control_mode)
}

fn elapsed_since(now_utc: DateTime<Utc>, then_utc: DateTime<Utc>) -> Duration {
    now_utc
        .signed_duration_since(then_utc)
        .to_std()
        .unwrap_or(Duration::ZERO)
}

fn min_due_in(current: Option<Duration>, candidate: Duration) -> Option<Duration> {
    match current {
        Some(existing) => Some(std::cmp::min(existing, candidate)),
        None => Some(candidate),
    }
}

pub(crate) fn summarize_automatic_sync_freshness(
    now_utc: DateTime<Utc>,
    snapshots: &[AccountSyncSnapshot],
    pending_account_ids: &HashSet<DigitalAssetAccountId>,
) -> AutomaticSyncFreshnessSummary {
    let mut summary = AutomaticSyncFreshnessSummary {
        urgent_accounts: 0,
        stale_warm_accounts: 0,
        stale_cold_accounts: 0,
        fresh_accounts: 0,
        next_due_in: None,
        stale_integrations: HashSet::new(),
        has_unknown_stale_integration: false,
    };

    for snapshot in snapshots {
        let has_pending_transactions = pending_account_ids.contains(&snapshot.account_id);
        let is_urgent = snapshot.addresses_never_synced.value() > 0
            || snapshot.addresses_in_progress.value() > 0
            || snapshot.addresses_failed.value() > 0
            || has_pending_transactions;

        if is_urgent {
            summary.urgent_accounts += 1;
            if let Some(integration_id) = snapshot.sync_integration_id {
                summary.stale_integrations.insert(integration_id);
            } else {
                summary.has_unknown_stale_integration = true;
            }
            continue;
        }

        let Some(last_success_at) = snapshot.last_success_at else {
            summary.stale_warm_accounts += 1;
            if let Some(integration_id) = snapshot.sync_integration_id {
                summary.stale_integrations.insert(integration_id);
            } else {
                summary.has_unknown_stale_integration = true;
            }
            continue;
        };

        let last_success_age = elapsed_since(now_utc, last_success_at);
        if last_success_age >= AUTOMATIC_SYNC_COLD_STALE_AFTER {
            summary.stale_cold_accounts += 1;
            if let Some(integration_id) = snapshot.sync_integration_id {
                summary.stale_integrations.insert(integration_id);
            } else {
                summary.has_unknown_stale_integration = true;
            }
            continue;
        }

        if last_success_age >= AUTOMATIC_SYNC_WARM_STALE_AFTER {
            summary.stale_warm_accounts += 1;
            if let Some(integration_id) = snapshot.sync_integration_id {
                summary.stale_integrations.insert(integration_id);
            } else {
                summary.has_unknown_stale_integration = true;
            }
            continue;
        }

        summary.fresh_accounts += 1;
        summary.next_due_in = min_due_in(
            summary.next_due_in,
            AUTOMATIC_SYNC_WARM_STALE_AFTER.saturating_sub(last_success_age),
        );
    }

    summary
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::{AccountSyncResult, AddressCount, ConsecutiveFailureCount};
    use chrono::TimeZone;

    fn make_snapshot(
        integration_id: Option<SyncIntegrationId>,
        last_success_at: Option<DateTime<Utc>>,
        addresses_never_synced: u32,
        addresses_in_progress: u32,
    ) -> AccountSyncSnapshot {
        AccountSyncSnapshot {
            account_id: DigitalAssetAccountId::new(),
            sync_integration_id: integration_id,
            addresses_total: AddressCount::from_u32(1),
            addresses_never_synced: AddressCount::from_u32(addresses_never_synced),
            addresses_synced: AddressCount::from_u32(1),
            addresses_failed: AddressCount::zero(),
            addresses_in_progress: AddressCount::from_u32(addresses_in_progress),
            max_consecutive_failures: ConsecutiveFailureCount::zero(),
            last_success_at,
            last_completed_at: last_success_at,
            last_result: Some(AccountSyncResult::Success),
            last_error: None,
            backfill_progress: None,
            etherscan_history_status: None,
            integration_states: Vec::new(),
        }
    }

    #[test]
    fn automatic_sync_eligibility_requires_open_user_db_and_active_session() {
        assert_eq!(
            automatic_sync_eligibility(true, true),
            AutomaticSyncEligibility::Eligible
        );
        assert_eq!(
            automatic_sync_eligibility(false, true),
            AutomaticSyncEligibility::Ineligible {
                reason: AutomaticSyncIneligibilityReason::UserDbUnavailable,
            }
        );
        assert_eq!(
            automatic_sync_eligibility(true, false),
            AutomaticSyncEligibility::Ineligible {
                reason: AutomaticSyncIneligibilityReason::MissingActiveSession,
            }
        );
        assert_eq!(
            automatic_sync_eligibility(false, false),
            AutomaticSyncEligibility::Ineligible {
                reason: AutomaticSyncIneligibilityReason::MissingActiveSessionAndUserDbUnavailable,
            }
        );
    }

    #[test]
    fn summarize_automatic_sync_freshness_marks_never_synced_work_as_urgent() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 14, 12, 0, 0)
            .single()
            .expect("test time should be valid");
        let snapshots = vec![make_snapshot(
            Some(SyncIntegrationId::Etherscan),
            None,
            1,
            0,
        )];

        let summary = summarize_automatic_sync_freshness(now, &snapshots, &HashSet::new());

        assert_eq!(summary.urgent_accounts, 1);
        assert!(summary.has_stale_work());
        assert_eq!(
            summary.highest_stale_class(),
            Some(SyncFreshnessClass::Urgent)
        );
        assert_eq!(
            summary.stale_integrations(),
            &HashSet::from([SyncIntegrationId::Etherscan])
        );
    }

    #[test]
    fn summarize_automatic_sync_freshness_uses_warm_and_cold_thresholds() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 14, 12, 0, 0)
            .single()
            .expect("test time should be valid");
        let fresh_at = Utc
            .with_ymd_and_hms(2026, 3, 14, 11, 50, 0)
            .single()
            .expect("test time should be valid");
        let warm_stale_at = Utc
            .with_ymd_and_hms(2026, 3, 14, 11, 40, 0)
            .single()
            .expect("test time should be valid");
        let cold_stale_at = Utc
            .with_ymd_and_hms(2026, 3, 14, 10, 30, 0)
            .single()
            .expect("test time should be valid");

        let snapshots = vec![
            make_snapshot(Some(SyncIntegrationId::Mempool), Some(fresh_at), 0, 0),
            make_snapshot(
                Some(SyncIntegrationId::Etherscan),
                Some(warm_stale_at),
                0,
                0,
            ),
            make_snapshot(Some(SyncIntegrationId::Mempool), Some(cold_stale_at), 0, 0),
        ];

        let summary = summarize_automatic_sync_freshness(now, &snapshots, &HashSet::new());

        assert_eq!(summary.fresh_accounts, 1);
        assert_eq!(summary.stale_warm_accounts, 1);
        assert_eq!(summary.stale_cold_accounts, 1);
        assert_eq!(summary.next_due_in, Some(Duration::from_secs(5 * 60)));
    }

    #[test]
    fn summarize_automatic_sync_freshness_treats_pending_accounts_as_urgent() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 14, 12, 0, 0)
            .single()
            .expect("test time should be valid");
        let snapshot = make_snapshot(Some(SyncIntegrationId::Mempool), Some(now), 0, 0);
        let pending_accounts = HashSet::from([snapshot.account_id]);

        let summary = summarize_automatic_sync_freshness(now, &[snapshot], &pending_accounts);

        assert_eq!(summary.urgent_accounts, 1);
        assert!(summary.has_stale_work());
    }

    #[test]
    fn summarize_automatic_sync_freshness_treats_failed_addresses_as_urgent() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 14, 12, 0, 0)
            .single()
            .expect("test time should be valid");
        let mut snapshot = make_snapshot(Some(SyncIntegrationId::Mempool), Some(now), 0, 0);
        snapshot.addresses_failed = AddressCount::from_u32(1);

        let summary = summarize_automatic_sync_freshness(now, &[snapshot], &HashSet::new());

        assert_eq!(summary.urgent_accounts, 1);
        assert!(summary.has_stale_work());
        assert_eq!(
            summary.stale_integrations(),
            &HashSet::from([SyncIntegrationId::Mempool])
        );
    }

    #[test]
    fn automatic_sync_block_state_only_fully_blocks_when_every_stale_integration_is_blocked() {
        let summary = AutomaticSyncFreshnessSummary {
            urgent_accounts: 0,
            stale_warm_accounts: 1,
            stale_cold_accounts: 0,
            fresh_accounts: 0,
            next_due_in: None,
            stale_integrations: HashSet::from([SyncIntegrationId::Etherscan]),
            has_unknown_stale_integration: false,
        };

        assert_eq!(
            automatic_sync_block_state(
                &summary,
                &HashSet::from([SyncIntegrationId::Etherscan]),
                Some(Duration::from_secs(120)),
            ),
            AutomaticSyncBlockState::FullyBlocked {
                next_retry_in: Duration::from_secs(120),
            }
        );
        assert_eq!(
            automatic_sync_block_state(
                &summary,
                &HashSet::from([SyncIntegrationId::Mempool]),
                Some(Duration::from_secs(120)),
            ),
            AutomaticSyncBlockState::PartiallyBlocked
        );
    }

    #[test]
    fn automatic_sync_decision_enqueues_urgent_or_stale_work_and_reschedules_blocked_or_fresh_work()
    {
        let stale_summary = AutomaticSyncFreshnessSummary {
            urgent_accounts: 0,
            stale_warm_accounts: 1,
            stale_cold_accounts: 0,
            fresh_accounts: 0,
            next_due_in: None,
            stale_integrations: HashSet::from([SyncIntegrationId::Etherscan]),
            has_unknown_stale_integration: false,
        };

        assert_eq!(
            automatic_sync_decision(
                AutomaticSyncEligibility::Eligible,
                &stale_summary,
                AutomaticSyncBlockState::Unblocked,
            ),
            AutomaticSyncDecision::EnqueueNow {
                reason: AutomaticSyncEnqueueReason::StaleWork,
            }
        );
        assert_eq!(
            automatic_sync_decision(
                AutomaticSyncEligibility::Eligible,
                &stale_summary,
                AutomaticSyncBlockState::FullyBlocked {
                    next_retry_in: Duration::from_secs(90),
                },
            ),
            AutomaticSyncDecision::KeepScheduled {
                reason: AutomaticSyncKeepScheduledReason::BlockedUntilRetry,
                next_due_in: Duration::from_secs(90),
            }
        );

        let fresh_summary = AutomaticSyncFreshnessSummary {
            urgent_accounts: 0,
            stale_warm_accounts: 0,
            stale_cold_accounts: 0,
            fresh_accounts: 1,
            next_due_in: Some(Duration::from_secs(45)),
            stale_integrations: HashSet::new(),
            has_unknown_stale_integration: false,
        };

        assert_eq!(
            automatic_sync_decision(
                AutomaticSyncEligibility::Eligible,
                &fresh_summary,
                AutomaticSyncBlockState::Unblocked,
            ),
            AutomaticSyncDecision::KeepScheduled {
                reason: AutomaticSyncKeepScheduledReason::FreshUntilNextWindow,
                next_due_in: Duration::from_secs(45),
            }
        );
    }

    #[test]
    fn automatic_sync_decision_skips_when_ineligible() {
        let summary = AutomaticSyncFreshnessSummary {
            urgent_accounts: 1,
            stale_warm_accounts: 0,
            stale_cold_accounts: 0,
            fresh_accounts: 0,
            next_due_in: None,
            stale_integrations: HashSet::from([SyncIntegrationId::Mempool]),
            has_unknown_stale_integration: false,
        };

        assert_eq!(
            automatic_sync_decision(
                AutomaticSyncEligibility::Ineligible {
                    reason: AutomaticSyncIneligibilityReason::MissingActiveSession,
                },
                &summary,
                AutomaticSyncBlockState::Unblocked,
            ),
            AutomaticSyncDecision::Skip {
                reason: AutomaticSyncSkipReason::Ineligible {
                    reason: AutomaticSyncIneligibilityReason::MissingActiveSession,
                },
            }
        );
    }

    #[test]
    fn automatic_add_sync_scope_uses_narrow_scope_for_single_target_and_user_for_multi_account() {
        let address_id = DigitalAssetAddressId::new();
        let account_id = DigitalAssetAccountId::new();

        assert_eq!(
            automatic_add_sync_scope(AutomaticSyncAddTarget::BitcoinAddress { address_id }),
            TransactionSyncScope::Address { address_id }
        );
        assert_eq!(
            automatic_add_sync_scope(AutomaticSyncAddTarget::EthereumAddress { address_id }),
            TransactionSyncScope::Address { address_id }
        );
        assert_eq!(
            automatic_add_sync_scope(AutomaticSyncAddTarget::Account { account_id }),
            TransactionSyncScope::Account { account_id }
        );
        assert_eq!(
            automatic_add_sync_scope(AutomaticSyncAddTarget::MultiAccountImport),
            TransactionSyncScope::User
        );
    }

    #[test]
    fn automatic_add_sync_is_suppressed_when_sync_control_is_enabled() {
        assert!(!should_enqueue_automatic_add_sync(SyncControlMode::Enabled));
        assert!(should_enqueue_automatic_add_sync(SyncControlMode::Disabled));
    }
}

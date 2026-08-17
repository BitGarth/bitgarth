use super::{
    RunContext, SyncIterationResult, UserTransactionMonitorError, UserTransactionMonitorSummary,
    default_user_transaction_monitor_schedule_hint,
};
use crate::db::{
    DbError, mark_address_sync_completed_failure, rebuild_account_transaction_ledger,
    rebuild_account_transaction_ledger_with_unknown_bitcoin_basis,
};
use crate::models::UserId;
use crate::transactions::{
    AddressCount, RateLimitedIntegration, SyncErrorMessage, TransactionCount, TransactionSyncRunId,
};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub(super) struct CycleAccumulator {
    pub(super) new_tx_count: TransactionCount,
    pub(super) updated_tx_count: TransactionCount,
    pub(super) addresses_total: u32,
    pub(super) addresses_synced: u32,
    pub(super) addresses_failed: u32,
    pub(super) addresses_skipped: u32,
    pub(super) addresses_skipped_tip_unchanged: u32,
    pub(super) addresses_early_exited: u32,
    pub(super) rate_limited: HashSet<String>,
    pub(super) touched_accounts: HashSet<DigitalAssetAccountId>,
    pub(super) coverage_invalidated_accounts: HashSet<DigitalAssetAccountId>,
    pub(super) failure_error: Option<SyncErrorMessage>,
    pub(super) bitcoin_history_repair_failure_error: Option<SyncErrorMessage>,
    pub(super) is_first_attempt: bool,
}

impl CycleAccumulator {
    pub(super) fn new(addresses_total: u32) -> Self {
        Self {
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            addresses_total,
            addresses_synced: 0_u32,
            addresses_failed: 0_u32,
            addresses_skipped: 0_u32,
            addresses_skipped_tip_unchanged: 0_u32,
            addresses_early_exited: 0_u32,
            rate_limited: HashSet::new(),
            touched_accounts: HashSet::new(),
            coverage_invalidated_accounts: HashSet::new(),
            failure_error: None,
            bitcoin_history_repair_failure_error: None,
            is_first_attempt: true,
        }
    }

    pub(super) fn add_synced(&mut self, summary: &SyncIterationResult) {
        self.new_tx_count = self.new_tx_count.saturating_add(summary.new_tx_count);
        self.updated_tx_count = self
            .updated_tx_count
            .saturating_add(summary.updated_tx_count);
        self.touched_accounts
            .extend(&summary.coverage_invalidation.account_ids);
        self.coverage_invalidated_accounts
            .extend(&summary.coverage_invalidation.account_ids);
        self.addresses_synced = self.addresses_synced.saturating_add(1);
    }

    pub(super) fn add_failed(
        &mut self,
        error: &UserTransactionMonitorError,
        bitcoin_history_repair_owned: bool,
    ) {
        self.addresses_failed = self.addresses_failed.saturating_add(1);
        let error = SyncErrorMessage::sanitize(error.to_string());
        if self.failure_error.is_none() {
            self.failure_error = Some(error.clone());
        }
        if bitcoin_history_repair_owned && self.bitcoin_history_repair_failure_error.is_none() {
            self.bitcoin_history_repair_failure_error = Some(error);
        }
    }

    pub(super) fn add_skipped(&mut self) {
        self.addresses_skipped = self.addresses_skipped.saturating_add(1);
    }

    pub(super) fn add_skipped_tip_unchanged(&mut self) {
        self.addresses_skipped_tip_unchanged =
            self.addresses_skipped_tip_unchanged.saturating_add(1);
    }

    pub(super) fn add_early_exited(&mut self) {
        self.addresses_early_exited = self.addresses_early_exited.saturating_add(1);
    }

    pub(super) fn add_total(&mut self, count: usize) {
        let to_add = u32::try_from(count).unwrap_or(u32::MAX);
        self.addresses_total = self.addresses_total.saturating_add(to_add);
    }

    pub(super) fn add_rate_limited(&mut self, integration: &str) {
        self.rate_limited.insert(integration.to_string());
    }

    pub(super) fn mark_account_dirty(&mut self, account_id: DigitalAssetAccountId) {
        self.touched_accounts.insert(account_id);
    }

    pub(super) fn mark_accounts_history_unavailable(
        &mut self,
        account_ids: &HashSet<DigitalAssetAccountId>,
    ) {
        self.touched_accounts.extend(account_ids);
        self.coverage_invalidated_accounts.extend(account_ids);
    }

    pub(super) fn rebuild_account_if_touched(
        &mut self,
        user_id: UserId,
        account_id: DigitalAssetAccountId,
        observed_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        if !self.touched_accounts.remove(&account_id) {
            return Ok(());
        }
        if self.coverage_invalidated_accounts.remove(&account_id) {
            rebuild_account_transaction_ledger_with_unknown_bitcoin_basis(
                user_id,
                account_id,
                observed_at,
            )
        } else {
            rebuild_account_transaction_ledger(user_id, account_id, observed_at)
        }
    }

    pub(super) fn into_summary(
        self,
        run_id: TransactionSyncRunId,
    ) -> UserTransactionMonitorSummary {
        let mut rate_limited = self
            .rate_limited
            .into_iter()
            .map(|integration| RateLimitedIntegration { integration })
            .collect::<Vec<RateLimitedIntegration>>();
        rate_limited.sort_by(|left, right| left.integration.cmp(&right.integration));

        UserTransactionMonitorSummary {
            run_id,
            new_tx_count: self.new_tx_count,
            updated_tx_count: self.updated_tx_count,
            addresses_total: AddressCount::from_u32(self.addresses_total),
            addresses_synced: AddressCount::from_u32(self.addresses_synced),
            addresses_failed: AddressCount::from_u32(self.addresses_failed),
            addresses_skipped: AddressCount::from_u32(self.addresses_skipped),
            addresses_skipped_tip_unchanged: AddressCount::from_u32(
                self.addresses_skipped_tip_unchanged,
            ),
            addresses_early_exited: AddressCount::from_u32(self.addresses_early_exited),
            pagination_cache_hits: 0,
            total_api_calls: 0,
            rate_limited,
            failure_error: self.failure_error,
            bitcoin_history_repair_failure_error: self.bitcoin_history_repair_failure_error,
            schedule_hint: default_user_transaction_monitor_schedule_hint(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CycleAccumulatorSnapshot {
    pub(super) new_tx_count: u32,
    pub(super) updated_tx_count: u32,
    pub(super) addresses_total: u32,
    pub(super) addresses_synced: u32,
    pub(super) addresses_failed: u32,
    pub(super) addresses_skipped: u32,
}

impl CycleAccumulatorSnapshot {
    pub(super) fn from_accumulator(accumulator: &CycleAccumulator) -> Self {
        Self {
            new_tx_count: accumulator.new_tx_count.value(),
            updated_tx_count: accumulator.updated_tx_count.value(),
            addresses_total: accumulator.addresses_total,
            addresses_synced: accumulator.addresses_synced,
            addresses_failed: accumulator.addresses_failed,
            addresses_skipped: accumulator.addresses_skipped,
        }
    }

    pub(super) fn delta_from(self, before: Self) -> Self {
        Self {
            new_tx_count: self.new_tx_count.saturating_sub(before.new_tx_count),
            updated_tx_count: self
                .updated_tx_count
                .saturating_sub(before.updated_tx_count),
            addresses_total: self.addresses_total.saturating_sub(before.addresses_total),
            addresses_synced: self
                .addresses_synced
                .saturating_sub(before.addresses_synced),
            addresses_failed: self
                .addresses_failed
                .saturating_sub(before.addresses_failed),
            addresses_skipped: self
                .addresses_skipped
                .saturating_sub(before.addresses_skipped),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AccountSyncLogSummary {
    pub(super) account_id: DigitalAssetAccountId,
    account_label: String,
    asset_id: SyncedAssetId,
    network: Network,
    started_at: Instant,
    completed_at: Instant,
    addresses_synced: u32,
    addresses_skipped: u32,
    addresses_failed: u32,
    new_tx_count: u32,
    updated_tx_count: u32,
    pub(super) addresses_derived: u32,
    pub(super) hd_keys_scanned: u32,
    pub(super) is_hd: bool,
}

impl AccountSyncLogSummary {
    pub(super) fn from_first_delta(
        account_id: DigitalAssetAccountId,
        account_label: String,
        asset_id: SyncedAssetId,
        network: Network,
        started_at: Instant,
        completed_at: Instant,
        delta: CycleAccumulatorSnapshot,
    ) -> Self {
        Self {
            account_id,
            account_label,
            asset_id,
            network,
            started_at,
            completed_at,
            addresses_synced: delta.addresses_synced,
            addresses_skipped: delta.addresses_skipped,
            addresses_failed: delta.addresses_failed,
            new_tx_count: delta.new_tx_count,
            updated_tx_count: delta.updated_tx_count,
            addresses_derived: 0,
            hd_keys_scanned: 0,
            is_hd: false,
        }
    }

    pub(super) fn apply_delta(
        &mut self,
        started_at: Instant,
        completed_at: Instant,
        delta: CycleAccumulatorSnapshot,
    ) {
        if started_at < self.started_at {
            self.started_at = started_at;
        }
        if completed_at > self.completed_at {
            self.completed_at = completed_at;
        }
        self.addresses_synced = self.addresses_synced.saturating_add(delta.addresses_synced);
        self.addresses_skipped = self
            .addresses_skipped
            .saturating_add(delta.addresses_skipped);
        self.addresses_failed = self.addresses_failed.saturating_add(delta.addresses_failed);
        self.new_tx_count = self.new_tx_count.saturating_add(delta.new_tx_count);
        self.updated_tx_count = self.updated_tx_count.saturating_add(delta.updated_tx_count);
    }
}

pub(super) fn account_label_for_log(
    account_labels: &HashMap<DigitalAssetAccountId, String>,
    account_id: DigitalAssetAccountId,
) -> String {
    account_labels
        .get(&account_id)
        .cloned()
        .unwrap_or_else(|| "Unknown account".to_string())
}

pub(super) fn log_account_sync_completed(run: RunContext<'_>, summary: &AccountSyncLogSummary) {
    let duration_ms_u128 = summary
        .completed_at
        .duration_since(summary.started_at)
        .as_millis();
    let duration_ms = u64::try_from(duration_ms_u128).unwrap_or(u64::MAX);
    let asset = format!("{}/{}", summary.asset_id.as_str(), summary.network.as_str());

    if summary.is_hd {
        tracing::info!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            account_id = %summary.account_id,
            asset = %asset,
            addresses_synced = summary.addresses_synced,
            addresses_skipped = summary.addresses_skipped,
            addresses_failed = summary.addresses_failed,
            new_tx_count = summary.new_tx_count,
            updated_tx_count = summary.updated_tx_count,
            duration_ms,
            addresses_derived = summary.addresses_derived,
            hd_keys_scanned = summary.hd_keys_scanned,
            "account_sync_completed"
        );
    } else {
        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            account_id = %summary.account_id,
            account_label = %summary.account_label,
            asset = %asset,
            addresses_synced = summary.addresses_synced,
            addresses_skipped = summary.addresses_skipped,
            addresses_failed = summary.addresses_failed,
            new_tx_count = summary.new_tx_count,
            updated_tx_count = summary.updated_tx_count,
            duration_ms,
            "account_sync_completed"
        );
    }
}

pub(super) fn mark_sync_failure(
    run: RunContext<'_>,
    address_id: DigitalAssetAddressId,
    error: &UserTransactionMonitorError,
    completed_at: DateTime<Utc>,
) {
    let sync_error = SyncErrorMessage::sanitize(error.to_string());
    if let Err(mark_err) = mark_address_sync_completed_failure(
        run.user_id,
        address_id,
        run.run_id,
        run.started_at,
        completed_at,
        &sync_error,
        error.counts_as_address_failure(),
    ) {
        tracing::error!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            address_id = %address_id,
            error = %mark_err,
            "transactions sync: failed to persist per-address failure state"
        );
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::super::context::SyncIterationResult;
    use super::*;
    use crate::transactions::ChainTipHeight;
    use chrono::{DateTime, TimeZone, Utc};

    fn test_utc_now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn sync_observability_counter_semantics_are_stable() {
        let mut accumulator = CycleAccumulator::new(2);

        // Generic skip should not count as tip-unchanged skip.
        accumulator.add_skipped();
        assert_eq!(accumulator.addresses_skipped, 1);
        assert_eq!(accumulator.addresses_skipped_tip_unchanged, 0);

        // Tip-unchanged skip must increment both the generic and specialized counter.
        accumulator.add_skipped();
        accumulator.add_skipped_tip_unchanged();
        assert_eq!(accumulator.addresses_skipped, 2);
        assert_eq!(accumulator.addresses_skipped_tip_unchanged, 1);

        let early_exit_summary = SyncIterationResult {
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            coverage_invalidation: crate::db::CoverageInvalidationTargets {
                address_ids: HashSet::new(),
                account_ids: HashSet::from([DigitalAssetAccountId::new()]),
            },
            tip_height: ChainTipHeight::try_new(1).expect("valid tip"),
            completed_at: test_utc_now(),
            has_more_work: false,
            early_exited: true,
            observed_activity: false,
            ledger_rebuild_required: false,
            raw_run_summary_json: None,
            api_confirmed_balance: None,
        };
        accumulator.add_synced(&early_exit_summary);
        assert_eq!(accumulator.touched_accounts.len(), 1);
        assert_eq!(
            accumulator.coverage_invalidated_accounts,
            early_exit_summary.coverage_invalidation.account_ids
        );
        if early_exit_summary.early_exited {
            accumulator.add_early_exited();
        }

        let summary = accumulator.into_summary(TransactionSyncRunId::new());
        assert_eq!(summary.addresses_skipped.value(), 2);
        assert_eq!(summary.addresses_skipped_tip_unchanged.value(), 1);
        assert_eq!(summary.addresses_early_exited.value(), 1);
    }
}

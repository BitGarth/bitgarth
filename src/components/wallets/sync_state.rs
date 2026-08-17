use crate::transactions::{
    AccountBackfillProgress, AccountIntegrationSyncSnapshot, AccountSyncResult,
    AccountSyncSnapshot, AddressCount, AggregateSyncResult, ConsecutiveFailureCount,
    SyncErrorMessage, SyncIntegrationId, TransactionCount, TransactionSyncEvent,
    TransactionSyncEventType, derive_account_sync_result,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

pub(crate) type AccountSyncStateMap =
    HashMap<crate::wallets::DigitalAssetAccountId, AccountSyncLiveState>;
pub(crate) type AccountSyncStateSignal = dioxus::prelude::Signal<AccountSyncStateMap>;
pub(super) type AccountSyncNowSignal = dioxus::prelude::Signal<Option<DateTime<Utc>>>;
pub(super) type GlobalSyncInProgressSignal = dioxus::prelude::Signal<bool>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncRunCompletion {
    pub(crate) run_id: Option<crate::transactions::TransactionSyncRunId>,
    pub(crate) occurred_at: DateTime<Utc>,
    pub(crate) failed: bool,
    pub(crate) new_tx_count: u32,
    pub(crate) updated_tx_count: u32,
    pub(crate) addresses_synced: u32,
    pub(crate) error: Option<crate::transactions::SyncErrorMessage>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct AccountSyncLiveState {
    pub(super) snapshot: AccountSyncSnapshot,
    pub(super) live_progress: Option<AccountSyncLiveProgress>,
    pub(super) integration_progress: HashMap<SyncIntegrationId, IntegrationLiveProgress>,
}

impl AccountSyncLiveState {
    pub(crate) fn is_any_integration_active(&self) -> bool {
        self.integration_progress.values().any(|p| p.is_active)
    }

    pub(crate) fn has_active_retry(&self) -> bool {
        self.integration_progress
            .values()
            .any(|p| p.retry_after.is_some_and(|ra| ra > chrono::Utc::now()))
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct AccountSyncLiveProgress {
    pub(super) fetched_tx_count: Option<TransactionCount>,
    pub(super) expected_tx_count: Option<TransactionCount>,
    pub(super) expected_tx_count_is_lower_bound: bool,
    pub(super) addresses_synced: Option<AddressCount>,
    pub(super) addresses_total: Option<AddressCount>,
    pub(super) is_first_sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IntegrationSyncTerminalResult {
    Success,
    Partial,
    Failed,
}

#[derive(Clone, PartialEq)]
pub(super) struct IntegrationLiveProgress {
    pub(super) is_active: bool,
    pub(super) fetched_tx_count: Option<TransactionCount>,
    pub(super) expected_tx_count: Option<TransactionCount>,
    pub(super) expected_tx_count_is_lower_bound: bool,
    pub(super) addresses_synced: Option<AddressCount>,
    pub(super) addresses_total: Option<AddressCount>,
    pub(super) is_first_sync: bool,
    pub(super) last_result: Option<IntegrationSyncTerminalResult>,
    pub(super) last_completed_at: Option<DateTime<Utc>>,
    pub(super) last_error: Option<SyncErrorMessage>,
    pub(super) retry_after: Option<DateTime<Utc>>,
}

impl IntegrationLiveProgress {
    pub(super) fn default_for_integration() -> Self {
        Self {
            is_active: false,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: false,
            addresses_synced: None,
            addresses_total: None,
            is_first_sync: false,
            last_result: None,
            last_completed_at: None,
            last_error: None,
            retry_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SyncDisplayStatus {
    Syncing,
    Blocked,
    Failing { streak: u32 },
    Retrying,
    Synced { at: Option<DateTime<Utc>> },
    NotSynced,
}

/// Single source of truth for the status mark on account rows.
/// Precedence is strict: a state earns exactly one mark, and an account can
/// never render activity or success it did not itself report.
pub(super) fn derive_sync_display_status(state: &AccountSyncLiveState) -> SyncDisplayStatus {
    let snapshot = &state.snapshot;

    let has_partial_successful_initial_sync = snapshot.addresses_never_synced.value() > 0
        && snapshot.addresses_synced.value() > 0
        && snapshot.addresses_failed.value() == 0;
    if snapshot.is_running()
        || snapshot.last_result == Some(AccountSyncResult::InProgress)
        || has_partial_successful_initial_sync
        || state.is_any_integration_active()
    {
        return SyncDisplayStatus::Syncing;
    }

    let integration_failure_pending = state
        .integration_progress
        .values()
        .any(|progress| progress.last_result == Some(IntegrationSyncTerminalResult::Failed));
    let snapshot_trouble = matches!(
        snapshot.last_result,
        Some(AccountSyncResult::Failure) | Some(AccountSyncResult::Partial)
    );

    if snapshot_trouble || integration_failure_pending {
        let configuration_error = snapshot
            .last_error
            .as_ref()
            .is_some_and(SyncErrorMessage::is_configuration_error)
            || state.integration_progress.values().any(|progress| {
                progress
                    .last_error
                    .as_ref()
                    .is_some_and(SyncErrorMessage::is_configuration_error)
            });
        if configuration_error {
            return SyncDisplayStatus::Blocked;
        }
        let streak = snapshot.max_consecutive_failures.value();
        if streak >= crate::transactions::ADDRESS_FAILURE_THRESHOLD {
            return SyncDisplayStatus::Failing { streak };
        }
        return SyncDisplayStatus::Retrying;
    }

    if snapshot.last_result == Some(AccountSyncResult::Success) {
        return SyncDisplayStatus::Synced {
            at: snapshot.last_success_at.or(snapshot.last_completed_at),
        };
    }

    SyncDisplayStatus::NotSynced
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum SyncBridgeMessage {
    StreamOpen,
    StreamError,
    StreamUnavailable,
    PollTick,
    SyncEvent { event_name: String, data: String },
}

pub(super) fn parse_sync_bridge_message(
    value: serde_json::Value,
) -> Result<SyncBridgeMessage, String> {
    match value {
        serde_json::Value::String(raw) => serde_json::from_str::<SyncBridgeMessage>(&raw)
            .map_err(|err| format!("Failed to parse sync stream message: {err}")),
        other => serde_json::from_value::<SyncBridgeMessage>(other)
            .map_err(|err| format!("Failed to parse sync stream message: {err}")),
    }
}

pub(super) fn integration_progress_from_snapshot(
    previous_states: Option<&HashMap<SyncIntegrationId, IntegrationLiveProgress>>,
    states: &[AccountIntegrationSyncSnapshot],
) -> HashMap<SyncIntegrationId, IntegrationLiveProgress> {
    states
        .iter()
        .map(|state| {
            let previous_progress = previous_states.and_then(|progress_by_integration| {
                progress_by_integration.get(&state.integration_id)
            });
            (
                state.integration_id,
                merge_integration_progress_from_snapshot(previous_progress, state),
            )
        })
        .collect()
}

pub(crate) fn build_account_sync_state_map(
    previous_states: &AccountSyncStateMap,
    snapshots: Vec<AccountSyncSnapshot>,
) -> AccountSyncStateMap {
    let mut by_account = HashMap::new();
    for snapshot in snapshots {
        let existing_state = previous_states.get(&snapshot.account_id);
        let integration_progress = integration_progress_from_snapshot(
            existing_state.map(|state| &state.integration_progress),
            &snapshot.integration_states,
        );
        let live_progress = merge_account_live_progress_from_snapshot(
            existing_state.and_then(|state| state.live_progress.as_ref()),
            &snapshot,
        );
        by_account.insert(
            snapshot.account_id,
            AccountSyncLiveState {
                snapshot,
                live_progress,
                integration_progress,
            },
        );
    }
    by_account
}

pub(super) fn upsert_account_sync_state(
    sync_states: &mut AccountSyncStateMap,
    account_id: crate::wallets::DigitalAssetAccountId,
) -> &mut AccountSyncLiveState {
    sync_states
        .entry(account_id)
        .or_insert_with(|| AccountSyncLiveState {
            snapshot: empty_account_sync_snapshot(account_id),
            live_progress: None,
            integration_progress: HashMap::new(),
        })
}

fn integration_terminal_result_from_aggregate(
    result: AggregateSyncResult,
) -> IntegrationSyncTerminalResult {
    match result {
        AggregateSyncResult::Success => IntegrationSyncTerminalResult::Success,
        AggregateSyncResult::Partial => IntegrationSyncTerminalResult::Partial,
        AggregateSyncResult::Failure => IntegrationSyncTerminalResult::Failed,
    }
}

fn integration_terminal_result_from_account_result(
    result: Option<AccountSyncResult>,
) -> Option<IntegrationSyncTerminalResult> {
    match result {
        Some(AccountSyncResult::Success) => Some(IntegrationSyncTerminalResult::Success),
        Some(AccountSyncResult::Partial) => Some(IntegrationSyncTerminalResult::Partial),
        Some(AccountSyncResult::Failure) => Some(IntegrationSyncTerminalResult::Failed),
        Some(AccountSyncResult::InProgress) | None => None,
    }
}

fn preserve_retry_after_from_previous_snapshot_merge(
    previous_progress: Option<&IntegrationLiveProgress>,
    snapshot_state: &AccountIntegrationSyncSnapshot,
) -> Option<DateTime<Utc>> {
    let previous_progress = previous_progress?;
    let retry_after = previous_progress.retry_after?;
    if snapshot_state.is_active {
        return None;
    }
    let same_failure_state = snapshot_state.last_result == Some(AggregateSyncResult::Failure)
        && previous_progress.last_result == Some(IntegrationSyncTerminalResult::Failed)
        && snapshot_state.last_error == previous_progress.last_error;
    same_failure_state.then_some(retry_after)
}

fn merge_integration_progress_from_snapshot(
    previous_progress: Option<&IntegrationLiveProgress>,
    snapshot_state: &AccountIntegrationSyncSnapshot,
) -> IntegrationLiveProgress {
    let mut progress = previous_progress
        .cloned()
        .unwrap_or_else(IntegrationLiveProgress::default_for_integration);
    progress.is_active = snapshot_state.is_active;
    progress.last_result = snapshot_state
        .last_result
        .map(integration_terminal_result_from_aggregate);
    progress.last_completed_at = snapshot_state.last_completed_at;
    progress.last_error = snapshot_state.last_error.clone();
    progress.retry_after =
        preserve_retry_after_from_previous_snapshot_merge(previous_progress, snapshot_state);

    if let Some(backfill_progress) = snapshot_state.backfill_progress.as_ref() {
        progress.fetched_tx_count = backfill_progress.fetched_tx_count;
        progress.expected_tx_count = backfill_progress.expected_tx_count();
        progress.expected_tx_count_is_lower_bound =
            backfill_progress.expected_tx_count_is_lower_bound;
        progress.addresses_synced = None;
        progress.addresses_total = None;
        progress.is_first_sync = true;
    } else if !snapshot_state.is_active {
        progress.fetched_tx_count = None;
        progress.expected_tx_count = None;
        progress.expected_tx_count_is_lower_bound = false;
        progress.addresses_synced = None;
        progress.addresses_total = None;
        progress.is_first_sync = false;
    }

    progress
}

pub(super) fn empty_account_sync_live_progress() -> AccountSyncLiveProgress {
    AccountSyncLiveProgress {
        fetched_tx_count: None,
        expected_tx_count: None,
        expected_tx_count_is_lower_bound: false,
        addresses_synced: None,
        addresses_total: None,
        is_first_sync: false,
    }
}

fn merge_account_live_progress_from_snapshot(
    previous_progress: Option<&AccountSyncLiveProgress>,
    snapshot: &AccountSyncSnapshot,
) -> Option<AccountSyncLiveProgress> {
    if !(snapshot.is_running() || snapshot.last_result == Some(AccountSyncResult::InProgress)) {
        return None;
    }

    let mut progress = previous_progress
        .cloned()
        .unwrap_or_else(empty_account_sync_live_progress);

    if let Some(backfill_progress) = snapshot.backfill_progress.as_ref() {
        progress.fetched_tx_count = backfill_progress.fetched_tx_count;
        progress.expected_tx_count = backfill_progress.expected_tx_count();
        progress.expected_tx_count_is_lower_bound =
            backfill_progress.expected_tx_count_is_lower_bound;
        progress.is_first_sync = true;
    }

    Some(progress)
}

fn apply_event_addresses_total(snapshot: &mut AccountSyncSnapshot, addresses_total: AddressCount) {
    if snapshot.addresses_total.value() == 0
        || addresses_total.value() > snapshot.addresses_total.value()
    {
        snapshot.addresses_total = addresses_total;
    }
}

fn apply_successful_account_completion(
    snapshot: &mut AccountSyncSnapshot,
    live_progress: Option<&AccountSyncLiveProgress>,
    occurred_at: DateTime<Utc>,
) {
    snapshot.addresses_in_progress = AddressCount::zero();

    let completed_first_sync_address = live_progress.is_some_and(|progress| progress.is_first_sync);
    if completed_first_sync_address {
        if snapshot.addresses_never_synced.value() > 0 {
            snapshot.addresses_never_synced =
                AddressCount::from_u32(snapshot.addresses_never_synced.value().saturating_sub(1));
            snapshot.addresses_synced =
                AddressCount::from_u32(snapshot.addresses_synced.value().saturating_add(1));
        } else if snapshot.addresses_failed.value() > 0 {
            snapshot.addresses_failed =
                AddressCount::from_u32(snapshot.addresses_failed.value().saturating_sub(1));
            snapshot.addresses_synced =
                AddressCount::from_u32(snapshot.addresses_synced.value().saturating_add(1));
        } else if snapshot.addresses_synced.value() == 0 && snapshot.addresses_total.value() == 1 {
            snapshot.addresses_synced = AddressCount::from_u32(1);
        }
    }

    snapshot.last_result = derive_account_sync_result(snapshot);
    if matches!(
        snapshot.last_result,
        Some(AccountSyncResult::Success) | Some(AccountSyncResult::Partial)
    ) {
        snapshot.last_success_at = Some(occurred_at);
    }
    if snapshot.last_result == Some(AccountSyncResult::Success) {
        snapshot.last_error = None;
    }
}

pub(super) fn apply_account_sync_event(
    sync_states: &mut AccountSyncStateMap,
    sync_event: &TransactionSyncEvent,
) {
    let Some(account_id) = sync_event.account_id else {
        return;
    };

    let state = upsert_account_sync_state(sync_states, account_id);

    match sync_event.event_type {
        TransactionSyncEventType::AccountSyncStarted => {
            if let Some(addresses_total) = sync_event.addresses_total {
                apply_event_addresses_total(&mut state.snapshot, addresses_total);
            }
            state.snapshot.addresses_in_progress = AddressCount::from_u32(1);
            state.snapshot.last_result = Some(AccountSyncResult::InProgress);
            state.live_progress = Some(AccountSyncLiveProgress {
                fetched_tx_count: sync_event.fetched_tx_count,
                expected_tx_count: sync_event.expected_tx_count,
                expected_tx_count_is_lower_bound: sync_event
                    .expected_tx_count_is_lower_bound
                    .unwrap_or(false),
                addresses_synced: sync_event.addresses_synced,
                addresses_total: sync_event.addresses_total,
                is_first_sync: sync_event.is_first_sync.unwrap_or(false),
            });
            apply_snapshot_backfill_progress(
                &mut state.snapshot,
                sync_event.fetched_tx_count,
                sync_event.expected_tx_count,
                sync_event.expected_tx_count_is_lower_bound,
            );
        }
        TransactionSyncEventType::AccountSyncProgress => {
            if let Some(addresses_total) = sync_event.addresses_total {
                apply_event_addresses_total(&mut state.snapshot, addresses_total);
            }
            state.snapshot.addresses_in_progress = AddressCount::from_u32(1);
            state.snapshot.last_result = Some(AccountSyncResult::InProgress);

            let progress = state
                .live_progress
                .get_or_insert_with(empty_account_sync_live_progress);

            if let Some(fetched_tx_count) = sync_event.fetched_tx_count {
                progress.fetched_tx_count = Some(fetched_tx_count);
            }
            if let Some(expected_tx_count) = sync_event.expected_tx_count {
                progress.expected_tx_count = Some(expected_tx_count);
            }
            if let Some(expected_is_lower_bound) = sync_event.expected_tx_count_is_lower_bound {
                progress.expected_tx_count_is_lower_bound = expected_is_lower_bound;
            }
            if let Some(addresses_synced) = sync_event.addresses_synced {
                progress.addresses_synced = Some(addresses_synced);
            }
            if let Some(addresses_total) = sync_event.addresses_total {
                progress.addresses_total = Some(addresses_total);
            }
            if let Some(is_first_sync) = sync_event.is_first_sync {
                progress.is_first_sync = is_first_sync;
            }
            apply_snapshot_backfill_progress(
                &mut state.snapshot,
                sync_event.fetched_tx_count,
                sync_event.expected_tx_count,
                sync_event.expected_tx_count_is_lower_bound,
            );
        }
        TransactionSyncEventType::AccountSyncCompleted => {
            let updated_backfill_progress = merged_snapshot_backfill_progress(
                state.snapshot.backfill_progress.clone(),
                state.live_progress.as_ref(),
            );
            let backfill_still_pending = updated_backfill_progress.as_ref().is_some_and(|_| {
                !state
                    .live_progress
                    .as_ref()
                    .is_some_and(live_progress_finishes_backfill)
            }) || state
                .live_progress
                .as_ref()
                .is_some_and(live_progress_has_unfinished_backfill);

            if backfill_still_pending {
                state.snapshot.addresses_in_progress = AddressCount::from_u32(1);
                state.snapshot.last_result = Some(AccountSyncResult::InProgress);
                state.snapshot.backfill_progress = updated_backfill_progress;
            } else {
                apply_successful_account_completion(
                    &mut state.snapshot,
                    state.live_progress.as_ref(),
                    sync_event.occurred_at,
                );
                state.snapshot.backfill_progress = None;
                state.live_progress = None;
            }
            state.snapshot.last_completed_at = Some(sync_event.occurred_at);
        }
        TransactionSyncEventType::AccountSyncFailed => {
            state.snapshot.addresses_in_progress = AddressCount::zero();
            state.snapshot.last_result = Some(AccountSyncResult::Failure);
            state.snapshot.last_completed_at = Some(sync_event.occurred_at);
            state.snapshot.last_error = if sync_event
                .rate_limited
                .as_ref()
                .is_some_and(|items| !items.is_empty())
            {
                Some(SyncErrorMessage::sanitize("Sync paused (rate limited)"))
            } else {
                sync_event.error.clone()
            };
            state.live_progress = None;
        }
        _ => {}
    }
}

pub(super) fn apply_account_integration_sync_event(
    sync_states: &mut AccountSyncStateMap,
    sync_event: &TransactionSyncEvent,
) {
    let Some(account_id) = sync_event.account_id else {
        return;
    };
    let Some(integration_id) = sync_event.integration_id else {
        return;
    };

    let state = upsert_account_sync_state(sync_states, account_id);
    let progress = state
        .integration_progress
        .entry(integration_id)
        .or_insert_with(IntegrationLiveProgress::default_for_integration);

    match sync_event.event_type {
        TransactionSyncEventType::AccountIntegrationSyncStarted => {
            progress.is_active = true;
            progress.last_result = None;
            progress.last_error = None;
            progress.retry_after = None;
            progress.fetched_tx_count = None;
            progress.expected_tx_count = sync_event.expected_tx_count;
            progress.expected_tx_count_is_lower_bound =
                sync_event.expected_tx_count_is_lower_bound.unwrap_or(false);
            progress.addresses_synced = sync_event.addresses_synced;
            progress.addresses_total = sync_event.addresses_total;
            progress.is_first_sync = sync_event.is_first_sync.unwrap_or(false);
        }
        TransactionSyncEventType::AccountIntegrationSyncProgress => {
            progress.is_active = true;
            if let Some(fetched) = sync_event.fetched_tx_count {
                progress.fetched_tx_count = Some(fetched);
            }
            if let Some(expected) = sync_event.expected_tx_count {
                progress.expected_tx_count = Some(expected);
            }
            if let Some(is_lower) = sync_event.expected_tx_count_is_lower_bound {
                progress.expected_tx_count_is_lower_bound = is_lower;
            }
            if let Some(synced) = sync_event.addresses_synced {
                progress.addresses_synced = Some(synced);
            }
            if let Some(total) = sync_event.addresses_total {
                progress.addresses_total = Some(total);
            }
            if let Some(is_first) = sync_event.is_first_sync {
                progress.is_first_sync = is_first;
            }
        }
        TransactionSyncEventType::AccountIntegrationSyncCompleted => {
            progress.is_active = false;
            progress.last_result =
                integration_terminal_result_from_account_result(state.snapshot.last_result);
            progress.last_completed_at = Some(sync_event.occurred_at);
            progress.last_error =
                if progress.last_result == Some(IntegrationSyncTerminalResult::Success) {
                    None
                } else {
                    state.snapshot.last_error.clone()
                };
            progress.retry_after = None;
        }
        TransactionSyncEventType::AccountIntegrationSyncFailed => {
            progress.is_active = false;
            progress.last_result = Some(IntegrationSyncTerminalResult::Failed);
            progress.last_completed_at = Some(sync_event.occurred_at);
            progress.last_error = sync_event.error.clone();
            progress.retry_after = sync_event.retry_after;
        }
        _ => {}
    }
}

pub(super) fn empty_account_sync_snapshot(
    account_id: crate::wallets::DigitalAssetAccountId,
) -> AccountSyncSnapshot {
    AccountSyncSnapshot {
        account_id,
        sync_integration_id: None,
        addresses_total: AddressCount::zero(),
        addresses_never_synced: AddressCount::zero(),
        addresses_synced: AddressCount::zero(),
        addresses_failed: AddressCount::zero(),
        addresses_in_progress: AddressCount::zero(),
        max_consecutive_failures: ConsecutiveFailureCount::zero(),
        last_success_at: None,
        last_completed_at: None,
        last_result: None,
        last_error: None,
        backfill_progress: None,
        etherscan_history_status: None,
        integration_states: Vec::new(),
    }
}

fn apply_snapshot_backfill_progress(
    snapshot: &mut AccountSyncSnapshot,
    fetched_tx_count: Option<TransactionCount>,
    expected_tx_count: Option<TransactionCount>,
    expected_tx_count_is_lower_bound: Option<bool>,
) {
    let Some(backfill_progress) = snapshot.backfill_progress.as_mut() else {
        return;
    };

    if let Some(fetched_tx_count) = fetched_tx_count {
        backfill_progress.set_fetched_tx_count(Some(fetched_tx_count));
    }
    if expected_tx_count.is_some() || expected_tx_count_is_lower_bound.is_some() {
        backfill_progress.set_expected_tx_count(
            expected_tx_count.or_else(|| backfill_progress.expected_tx_count()),
            expected_tx_count_is_lower_bound
                .unwrap_or(backfill_progress.expected_tx_count_is_lower_bound),
        );
    }
}

fn merged_snapshot_backfill_progress(
    snapshot_backfill_progress: Option<AccountBackfillProgress>,
    live_progress: Option<&AccountSyncLiveProgress>,
) -> Option<AccountBackfillProgress> {
    let mut backfill_progress = snapshot_backfill_progress?;
    if let Some(live_progress) = live_progress {
        if let Some(fetched_tx_count) = live_progress.fetched_tx_count {
            backfill_progress.set_fetched_tx_count(Some(fetched_tx_count));
        }
        if live_progress.expected_tx_count.is_some()
            || live_progress.expected_tx_count_is_lower_bound
        {
            backfill_progress.set_expected_tx_count(
                live_progress
                    .expected_tx_count
                    .or_else(|| backfill_progress.expected_tx_count()),
                live_progress.expected_tx_count_is_lower_bound,
            );
        }
    }
    Some(backfill_progress)
}

fn live_progress_has_unfinished_backfill(live_progress: &AccountSyncLiveProgress) -> bool {
    match (
        live_progress.fetched_tx_count,
        live_progress.expected_tx_count,
        live_progress.expected_tx_count_is_lower_bound,
    ) {
        (Some(_), Some(_), true) => true,
        (Some(fetched_tx_count), Some(expected_tx_count), false) => {
            fetched_tx_count.value() < expected_tx_count.value()
        }
        (Some(_), None, _) => true,
        _ => false,
    }
}

fn live_progress_finishes_backfill(live_progress: &AccountSyncLiveProgress) -> bool {
    match (
        live_progress.fetched_tx_count,
        live_progress.expected_tx_count,
        live_progress.expected_tx_count_is_lower_bound,
    ) {
        (Some(fetched_tx_count), Some(expected_tx_count), false) => {
            fetched_tx_count.value() >= expected_tx_count.value()
        }
        _ => false,
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::AddressBackfillCursor;

    fn parse_utc(timestamp: &str) -> DateTime<Utc> {
        timestamp
            .parse::<DateTime<Utc>>()
            .unwrap_or_else(|err| panic!("invalid test timestamp {timestamp}: {err}"))
    }

    fn base_failed_state(streak: u32, error: &str) -> AccountSyncLiveState {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let mut snapshot = empty_account_sync_snapshot(account_id);
        snapshot.addresses_total = AddressCount::from_u32(1);
        snapshot.addresses_failed = AddressCount::from_u32(1);
        snapshot.last_result = Some(AccountSyncResult::Failure);
        snapshot.last_completed_at = Some(parse_utc("2026-07-04T10:00:00Z"));
        snapshot.last_error = Some(SyncErrorMessage::sanitize(error));
        snapshot.max_consecutive_failures =
            crate::transactions::ConsecutiveFailureCount::try_new(i64::from(streak))
                .expect("test streak in range");
        AccountSyncLiveState {
            snapshot,
            live_progress: None,
            integration_progress: HashMap::new(),
        }
    }

    #[test]
    fn derive_display_status_not_synced_when_never_attempted() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let state = AccountSyncLiveState {
            snapshot: empty_account_sync_snapshot(account_id),
            live_progress: None,
            integration_progress: HashMap::new(),
        };
        assert_eq!(
            derive_sync_display_status(&state),
            SyncDisplayStatus::NotSynced
        );
    }

    #[test]
    fn derive_display_status_synced_on_success() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let mut snapshot = empty_account_sync_snapshot(account_id);
        snapshot.addresses_total = AddressCount::from_u32(1);
        snapshot.addresses_synced = AddressCount::from_u32(1);
        snapshot.last_result = Some(AccountSyncResult::Success);
        snapshot.last_success_at = Some(parse_utc("2026-07-05T08:00:00Z"));
        let state = AccountSyncLiveState {
            snapshot,
            live_progress: None,
            integration_progress: HashMap::new(),
        };
        assert_eq!(
            derive_sync_display_status(&state),
            SyncDisplayStatus::Synced {
                at: Some(parse_utc("2026-07-05T08:00:00Z"))
            }
        );
    }

    #[test]
    fn derive_display_status_retrying_below_threshold_and_failing_at_threshold() {
        assert_eq!(
            derive_sync_display_status(&base_failed_state(1, "Sync HTTP request failed: timeout")),
            SyncDisplayStatus::Retrying
        );
        assert_eq!(
            derive_sync_display_status(&base_failed_state(2, "Sync HTTP request failed: timeout")),
            SyncDisplayStatus::Failing { streak: 2 }
        );
    }

    #[test]
    fn derive_display_status_blocked_beats_failing_for_configuration_errors() {
        let state = base_failed_state(5, crate::transactions::MISSING_ETHERSCAN_API_KEY_ERROR);
        assert_eq!(
            derive_sync_display_status(&state),
            SyncDisplayStatus::Blocked
        );
    }

    #[test]
    fn derive_display_status_syncing_only_for_own_activity() {
        let mut state = base_failed_state(2, "Sync HTTP request failed: timeout");
        state.integration_progress.insert(
            SyncIntegrationId::Etherscan,
            IntegrationLiveProgress {
                is_active: true,
                ..IntegrationLiveProgress::default_for_integration()
            },
        );
        assert_eq!(
            derive_sync_display_status(&state),
            SyncDisplayStatus::Syncing
        );
    }

    #[test]
    fn derive_display_status_ignores_neighbor_sync_events() {
        // The reported flicker: account B is red; account A starts syncing.
        let failed_account = crate::wallets::DigitalAssetAccountId::new();
        let other_account = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started_at = parse_utc("2026-07-05T09:00:00Z");

        let mut sync_states = AccountSyncStateMap::new();
        sync_states.insert(
            failed_account,
            base_failed_state(2, "Sync HTTP request failed: timeout"),
        );
        let before = derive_sync_display_status(&sync_states[&failed_account]);

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_started_single_address(
                run_id,
                started_at,
                other_account,
                true,
                Some(TransactionCount::zero()),
                Some(false),
            ),
        );

        assert_eq!(
            derive_sync_display_status(&sync_states[&failed_account]),
            before
        );
    }

    #[test]
    fn derive_display_status_never_synced_while_integration_failure_pending() {
        // Mid-run optimistic events may set snapshot.last_result to Success while
        // the integration still reports a terminal failure. Must not render green.
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let mut snapshot = empty_account_sync_snapshot(account_id);
        snapshot.addresses_total = AddressCount::from_u32(1);
        snapshot.addresses_synced = AddressCount::from_u32(1);
        snapshot.last_result = Some(AccountSyncResult::Success);
        let mut integration_progress = HashMap::new();
        integration_progress.insert(
            SyncIntegrationId::Etherscan,
            IntegrationLiveProgress {
                last_result: Some(IntegrationSyncTerminalResult::Failed),
                last_error: Some(SyncErrorMessage::sanitize(
                    "Sync HTTP request failed: timeout",
                )),
                ..IntegrationLiveProgress::default_for_integration()
            },
        );
        let state = AccountSyncLiveState {
            snapshot,
            live_progress: None,
            integration_progress,
        };
        assert_eq!(
            derive_sync_display_status(&state),
            SyncDisplayStatus::Retrying
        );
    }

    #[test]
    fn parse_sync_bridge_message_parses_event_payload() {
        let raw = serde_json::Value::String(
            "{\"kind\":\"sync_event\",\"event_name\":\"account_sync_progress\",\"data\":\"{}\"}"
                .to_string(),
        );
        let parsed = parse_sync_bridge_message(raw)
            .unwrap_or_else(|err| panic!("bridge message should parse: {err}"));
        match parsed {
            SyncBridgeMessage::SyncEvent { event_name, data } => {
                assert_eq!(event_name, "account_sync_progress");
                assert_eq!(data, "{}");
            }
            _ => panic!("expected sync_event variant"),
        }
    }

    #[test]
    fn integration_progress_from_snapshot_preserves_partial_and_backfill_progress() {
        let completed_at = parse_utc("2026-03-02T21:00:00Z");
        let integration_states = vec![AccountIntegrationSyncSnapshot {
            integration_id: SyncIntegrationId::Mempool,
            is_active: true,
            last_started_at: Some(parse_utc("2026-03-02T20:59:00Z")),
            last_completed_at: Some(completed_at),
            last_result: Some(AggregateSyncResult::Partial),
            last_error: Some(SyncErrorMessage::sanitize("Some addresses failed")),
            backfill_progress: Some(AccountBackfillProgress::new(
                crate::transactions::AddressBackfillState::new(
                    AddressBackfillCursor::Mempool {
                        cursor_txid: crate::transactions::MempoolCursorTxid::parse(
                            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                        )
                        .expect("cursor should parse"),
                    },
                    Some(TransactionCount::from_u32(670)),
                ),
                Some(TransactionCount::from_u32(200)),
                false,
            )),
            etherscan_history_status: None,
        }];

        let progress_by_integration = integration_progress_from_snapshot(None, &integration_states);
        let progress = progress_by_integration
            .get(&SyncIntegrationId::Mempool)
            .unwrap_or_else(|| panic!("expected mempool integration progress"));

        assert_eq!(
            progress.last_result,
            Some(IntegrationSyncTerminalResult::Partial)
        );
        assert_eq!(progress.last_completed_at, Some(completed_at));
        assert_eq!(
            progress.fetched_tx_count,
            Some(TransactionCount::from_u32(200))
        );
        assert_eq!(
            progress.expected_tx_count,
            Some(TransactionCount::from_u32(670))
        );
        assert!(!progress.expected_tx_count_is_lower_bound);
        assert!(progress.is_first_sync);
    }

    #[test]
    fn build_account_sync_state_map_preserves_retry_after_for_same_failed_snapshot() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let retry_after = parse_utc("2026-03-02T22:15:00Z");
        let failed_at = parse_utc("2026-03-02T22:00:00Z");
        let rate_limited_error = SyncErrorMessage::sanitize("Sync paused (rate limited)");
        let mut previous_states = AccountSyncStateMap::new();
        previous_states.insert(
            account_id,
            AccountSyncLiveState {
                snapshot: empty_account_sync_snapshot(account_id),
                live_progress: None,
                integration_progress: HashMap::from([(
                    SyncIntegrationId::Etherscan,
                    IntegrationLiveProgress {
                        is_active: false,
                        fetched_tx_count: None,
                        expected_tx_count: None,
                        expected_tx_count_is_lower_bound: false,
                        addresses_synced: None,
                        addresses_total: None,
                        is_first_sync: false,
                        last_result: Some(IntegrationSyncTerminalResult::Failed),
                        last_completed_at: Some(failed_at),
                        last_error: Some(rate_limited_error.clone()),
                        retry_after: Some(retry_after),
                    },
                )]),
            },
        );

        let snapshots = vec![AccountSyncSnapshot {
            account_id,
            sync_integration_id: None,
            addresses_total: AddressCount::from_u32(1),
            addresses_never_synced: AddressCount::zero(),
            addresses_synced: AddressCount::zero(),
            addresses_failed: AddressCount::from_u32(1),
            addresses_in_progress: AddressCount::zero(),
            max_consecutive_failures: ConsecutiveFailureCount::zero(),
            last_success_at: None,
            last_completed_at: Some(failed_at),
            last_result: Some(crate::transactions::AccountSyncResult::Failure),
            last_error: Some(rate_limited_error.clone()),
            backfill_progress: None,
            etherscan_history_status: None,
            integration_states: vec![AccountIntegrationSyncSnapshot {
                integration_id: SyncIntegrationId::Etherscan,
                is_active: false,
                last_started_at: Some(parse_utc("2026-03-02T21:59:00Z")),
                last_completed_at: Some(failed_at),
                last_result: Some(AggregateSyncResult::Failure),
                last_error: Some(rate_limited_error),
                backfill_progress: None,
                etherscan_history_status: None,
            }],
        }];

        let rebuilt = build_account_sync_state_map(&previous_states, snapshots);
        let progress = rebuilt
            .get(&account_id)
            .and_then(|state| {
                state
                    .integration_progress
                    .get(&SyncIntegrationId::Etherscan)
            })
            .unwrap_or_else(|| panic!("expected etherscan integration progress"));

        assert_eq!(progress.retry_after, Some(retry_after));
    }

    #[test]
    fn account_sync_completed_keeps_in_progress_when_backfill_is_incomplete() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started_at = parse_utc("2026-03-02T22:00:00Z");
        let progressed_at = parse_utc("2026-03-02T22:00:01Z");
        let completed_at = parse_utc("2026-03-02T22:00:02Z");
        let mut sync_states = AccountSyncStateMap::new();

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                true,
                Some(TransactionCount::from_u32(670)),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_progress_single_address(
                run_id,
                progressed_at,
                account_id,
                true,
                TransactionCount::from_u32(200),
                Some(TransactionCount::from_u32(670)),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_completed(
                run_id,
                completed_at,
                account_id,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
        );

        let state = sync_states
            .get(&account_id)
            .unwrap_or_else(|| panic!("expected account state for {account_id}"));
        assert_eq!(
            state.snapshot.last_result,
            Some(AccountSyncResult::InProgress)
        );
        assert_eq!(state.snapshot.addresses_in_progress.value(), 1);
        assert!(state.snapshot.backfill_progress.is_none());
        assert!(state.snapshot.last_success_at.is_none());
        assert!(state.live_progress.is_some());
    }

    #[test]
    fn account_sync_completed_sets_success_when_backfill_is_fully_fetched() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started_at = parse_utc("2026-03-02T23:00:00Z");
        let progressed_at = parse_utc("2026-03-02T23:00:01Z");
        let completed_at = parse_utc("2026-03-02T23:00:02Z");
        let mut sync_states = AccountSyncStateMap::new();

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                true,
                Some(TransactionCount::from_u32(670)),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_progress_single_address(
                run_id,
                progressed_at,
                account_id,
                true,
                TransactionCount::from_u32(670),
                Some(TransactionCount::from_u32(670)),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_completed(
                run_id,
                completed_at,
                account_id,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
        );

        let state = sync_states
            .get(&account_id)
            .unwrap_or_else(|| panic!("expected account state for {account_id}"));
        assert_eq!(state.snapshot.last_result, Some(AccountSyncResult::Success));
        assert_eq!(state.snapshot.addresses_in_progress.value(), 0);
        assert_eq!(state.snapshot.last_success_at, Some(completed_at));
        assert!(state.snapshot.backfill_progress.is_none());
        assert!(state.live_progress.is_none());
    }

    #[test]
    fn single_address_completion_preserves_partial_account_status_after_prior_failure() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let failed_at = parse_utc("2026-04-12T18:53:56Z");
        let started_at = parse_utc("2026-04-12T18:55:26Z");
        let completed_at = parse_utc("2026-04-12T18:55:27Z");
        let rate_limit_error = SyncErrorMessage::sanitize("Rate limit reached for mempool");
        let mut sync_states = AccountSyncStateMap::new();
        sync_states.insert(
            account_id,
            AccountSyncLiveState {
                snapshot: AccountSyncSnapshot {
                    account_id,
                    sync_integration_id: Some(SyncIntegrationId::Mempool),
                    addresses_total: AddressCount::from_u32(40),
                    addresses_never_synced: AddressCount::from_u32(1),
                    addresses_synced: AddressCount::from_u32(38),
                    addresses_failed: AddressCount::from_u32(1),
                    addresses_in_progress: AddressCount::zero(),
                    max_consecutive_failures: ConsecutiveFailureCount::zero(),
                    last_success_at: Some(failed_at),
                    last_completed_at: Some(failed_at),
                    last_result: Some(AccountSyncResult::Partial),
                    last_error: Some(rate_limit_error.clone()),
                    backfill_progress: None,
                    etherscan_history_status: None,
                    integration_states: vec![AccountIntegrationSyncSnapshot {
                        integration_id: SyncIntegrationId::Mempool,
                        is_active: false,
                        last_started_at: Some(parse_utc("2026-04-12T18:53:50Z")),
                        last_completed_at: Some(failed_at),
                        last_result: Some(AggregateSyncResult::Partial),
                        last_error: Some(rate_limit_error.clone()),
                        backfill_progress: None,
                        etherscan_history_status: None,
                    }],
                },
                live_progress: None,
                integration_progress: HashMap::from([(
                    SyncIntegrationId::Mempool,
                    IntegrationLiveProgress {
                        is_active: false,
                        fetched_tx_count: None,
                        expected_tx_count: None,
                        expected_tx_count_is_lower_bound: false,
                        addresses_synced: None,
                        addresses_total: None,
                        is_first_sync: false,
                        last_result: Some(IntegrationSyncTerminalResult::Partial),
                        last_completed_at: Some(failed_at),
                        last_error: Some(rate_limit_error.clone()),
                        retry_after: None,
                    },
                )]),
            },
        );

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                true,
                Some(TransactionCount::zero()),
                Some(false),
            ),
        );
        apply_account_integration_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_integration_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                SyncIntegrationId::Mempool,
                true,
                Some(TransactionCount::zero()),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_completed(
                run_id,
                completed_at,
                account_id,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
        );
        apply_account_integration_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_integration_sync_completed(
                run_id,
                completed_at,
                account_id,
                SyncIntegrationId::Mempool,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
        );

        let state = sync_states
            .get(&account_id)
            .unwrap_or_else(|| panic!("expected account state for {account_id}"));
        assert_eq!(state.snapshot.addresses_total.value(), 40);
        assert_eq!(state.snapshot.addresses_never_synced.value(), 0);
        assert_eq!(state.snapshot.addresses_synced.value(), 39);
        assert_eq!(state.snapshot.addresses_failed.value(), 1);
        assert_eq!(state.snapshot.last_result, Some(AccountSyncResult::Partial));
        assert_eq!(
            state
                .snapshot
                .last_error
                .as_ref()
                .map(SyncErrorMessage::as_str),
            Some("Rate limit reached for mempool")
        );
        assert_eq!(
            state
                .integration_progress
                .get(&SyncIntegrationId::Mempool)
                .and_then(|progress| progress.last_result.clone()),
            Some(IntegrationSyncTerminalResult::Partial)
        );
    }

    #[test]
    fn single_address_completion_clears_partial_after_failed_address_retry_succeeds() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let failed_at = parse_utc("2026-04-12T18:53:56Z");
        let started_at = parse_utc("2026-04-12T19:36:50Z");
        let completed_at = parse_utc("2026-04-12T19:36:56Z");
        let rate_limit_error = SyncErrorMessage::sanitize("Rate limit reached for mempool");
        let mut sync_states = AccountSyncStateMap::new();
        sync_states.insert(
            account_id,
            AccountSyncLiveState {
                snapshot: AccountSyncSnapshot {
                    account_id,
                    sync_integration_id: Some(SyncIntegrationId::Mempool),
                    addresses_total: AddressCount::from_u32(40),
                    addresses_never_synced: AddressCount::zero(),
                    addresses_synced: AddressCount::from_u32(39),
                    addresses_failed: AddressCount::from_u32(1),
                    addresses_in_progress: AddressCount::zero(),
                    max_consecutive_failures: ConsecutiveFailureCount::zero(),
                    last_success_at: Some(failed_at),
                    last_completed_at: Some(failed_at),
                    last_result: Some(AccountSyncResult::Partial),
                    last_error: Some(rate_limit_error.clone()),
                    backfill_progress: None,
                    etherscan_history_status: None,
                    integration_states: vec![AccountIntegrationSyncSnapshot {
                        integration_id: SyncIntegrationId::Mempool,
                        is_active: false,
                        last_started_at: Some(parse_utc("2026-04-12T18:53:50Z")),
                        last_completed_at: Some(failed_at),
                        last_result: Some(AggregateSyncResult::Partial),
                        last_error: Some(rate_limit_error),
                        backfill_progress: None,
                        etherscan_history_status: None,
                    }],
                },
                live_progress: None,
                integration_progress: HashMap::from([(
                    SyncIntegrationId::Mempool,
                    IntegrationLiveProgress {
                        is_active: false,
                        fetched_tx_count: None,
                        expected_tx_count: None,
                        expected_tx_count_is_lower_bound: false,
                        addresses_synced: None,
                        addresses_total: None,
                        is_first_sync: false,
                        last_result: Some(IntegrationSyncTerminalResult::Partial),
                        last_completed_at: Some(failed_at),
                        last_error: Some(SyncErrorMessage::sanitize(
                            "Rate limit reached for mempool",
                        )),
                        retry_after: None,
                    },
                )]),
            },
        );

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                true,
                Some(TransactionCount::zero()),
                Some(false),
            ),
        );
        apply_account_integration_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_integration_sync_started_single_address(
                run_id,
                started_at,
                account_id,
                SyncIntegrationId::Mempool,
                true,
                Some(TransactionCount::zero()),
                Some(false),
            ),
        );
        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_completed(
                run_id,
                completed_at,
                account_id,
                TransactionCount::from_u32(1),
                TransactionCount::zero(),
            ),
        );
        apply_account_integration_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_integration_sync_completed(
                run_id,
                completed_at,
                account_id,
                SyncIntegrationId::Mempool,
                TransactionCount::from_u32(1),
                TransactionCount::zero(),
            ),
        );

        let state = sync_states
            .get(&account_id)
            .unwrap_or_else(|| panic!("expected account state for {account_id}"));
        assert_eq!(state.snapshot.addresses_total.value(), 40);
        assert_eq!(state.snapshot.addresses_never_synced.value(), 0);
        assert_eq!(state.snapshot.addresses_synced.value(), 40);
        assert_eq!(state.snapshot.addresses_failed.value(), 0);
        assert_eq!(state.snapshot.last_result, Some(AccountSyncResult::Success));
        assert!(state.snapshot.last_error.is_none());
        assert_eq!(
            state
                .integration_progress
                .get(&SyncIntegrationId::Mempool)
                .and_then(|progress| progress.last_result.clone()),
            Some(IntegrationSyncTerminalResult::Success)
        );
    }

    #[test]
    fn account_sync_completed_uses_snapshot_backfill_progress_when_live_progress_missing() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let completed_at = parse_utc("2026-03-02T23:30:00Z");
        let mut sync_states = AccountSyncStateMap::new();
        let state = upsert_account_sync_state(&mut sync_states, account_id);
        state.snapshot.addresses_total = AddressCount::from_u32(1);
        state.snapshot.addresses_in_progress = AddressCount::from_u32(1);
        state.snapshot.last_result = Some(AccountSyncResult::InProgress);
        state.snapshot.backfill_progress = Some(AccountBackfillProgress::new(
            crate::transactions::AddressBackfillState::new(
                AddressBackfillCursor::Mempool {
                    cursor_txid: crate::transactions::MempoolCursorTxid::parse(
                        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    )
                    .expect("cursor should parse"),
                },
                Some(TransactionCount::from_u32(670)),
            ),
            Some(TransactionCount::from_u32(200)),
            false,
        ));
        state.live_progress = None;

        apply_account_sync_event(
            &mut sync_states,
            &crate::transactions::TransactionSyncEvent::account_sync_completed(
                run_id,
                completed_at,
                account_id,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
        );

        let state = sync_states
            .get(&account_id)
            .unwrap_or_else(|| panic!("expected account state for {account_id}"));
        assert_eq!(
            state.snapshot.last_result,
            Some(AccountSyncResult::InProgress)
        );
        assert_eq!(state.snapshot.addresses_in_progress.value(), 1);
        let backfill_progress = state
            .snapshot
            .backfill_progress
            .as_ref()
            .expect("snapshot backfill progress should remain");
        assert_eq!(
            backfill_progress.fetched_tx_count,
            Some(TransactionCount::from_u32(200))
        );
        assert_eq!(
            backfill_progress.expected_tx_count(),
            Some(TransactionCount::from_u32(670))
        );
        assert!(state.snapshot.last_success_at.is_none());
        assert!(state.live_progress.is_none());
    }
}

#![cfg_attr(
    not(feature = "server"),
    expect(
        dead_code,
        reason = "Transaction sync domain types are primarily exercised on server paths"
    )
)]

use super::types::*;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Consecutive per-address failures after which the sync planner stops rapid
/// retries and the UI escalates from "Retrying" to "Sync failing". One
/// constant so scheduler behavior and UI alarm timing cannot drift apart.
pub(crate) const ADDRESS_FAILURE_THRESHOLD: u32 = 2;

/// Exact user-facing message for a missing Etherscan API key. Shared between
/// the sync error `Display` impl and `SyncErrorMessage::is_configuration_error`
/// so client-side classification cannot drift from the backend wording.
pub(crate) const MISSING_ETHERSCAN_API_KEY_ERROR: &str =
    "Ethereum sync requires an Etherscan API key in user settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AggregateSyncResult {
    Success,
    Partial,
    Failure,
}

impl AggregateSyncResult {
    pub(crate) fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "partial" => Some(Self::Partial),
            "failure" => Some(Self::Failure),
            _ => None,
        }
    }

    pub(crate) const fn as_db_value(self) -> &'static str {
        match self {
            AggregateSyncResult::Success => "success",
            AggregateSyncResult::Partial => "partial",
            AggregateSyncResult::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AggregateSyncSnapshot {
    pub is_running: bool,
    pub addresses_total: AddressCount,
    pub addresses_synced: AddressCount,
    pub addresses_failed: AddressCount,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub last_result: Option<AggregateSyncResult>,
    pub last_error: Option<SyncErrorMessage>,
    pub new_tx_count: TransactionCount,
    pub updated_tx_count: TransactionCount,
}

impl Default for AggregateSyncSnapshot {
    fn default() -> Self {
        Self {
            is_running: false,
            addresses_total: AddressCount::zero(),
            addresses_synced: AddressCount::zero(),
            addresses_failed: AddressCount::zero(),
            last_completed_at: None,
            last_result: None,
            last_error: None,
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
        }
    }
}

pub(crate) fn compute_aggregate_sync_result(
    synced: AddressCount,
    failed: AddressCount,
) -> AggregateSyncResult {
    if failed.value() == 0 {
        AggregateSyncResult::Success
    } else if synced.value() == 0 {
        AggregateSyncResult::Failure
    } else {
        AggregateSyncResult::Partial
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountSyncResult {
    Success,
    Partial,
    Failure,
    InProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountAddressSyncStatus {
    NotSynced,
    Syncing,
    Synced,
    Failed,
}

pub(crate) fn derive_account_address_sync_status(
    last_started_at: Option<DateTime<Utc>>,
    last_completed_at: Option<DateTime<Utc>>,
    last_result: Option<TransactionSyncResult>,
    has_mempool_backfill_cursor: bool,
    has_etherscan_backfill_end_block: bool,
) -> AccountAddressSyncStatus {
    if let Some(last_started_at_value) = last_started_at
        && (has_mempool_backfill_cursor
            || has_etherscan_backfill_end_block
            || last_completed_at.is_none()
            || last_completed_at.is_some_and(|last_completed_at_value| {
                last_started_at_value > last_completed_at_value
            }))
    {
        return AccountAddressSyncStatus::Syncing;
    }

    if last_completed_at.is_some() {
        return match last_result {
            Some(TransactionSyncResult::Success) => AccountAddressSyncStatus::Synced,
            Some(TransactionSyncResult::Failure) => AccountAddressSyncStatus::Failed,
            None => AccountAddressSyncStatus::NotSynced,
        };
    }

    AccountAddressSyncStatus::NotSynced
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub(crate) enum AddressBackfillCursor {
    Mempool { cursor_txid: MempoolCursorTxid },
    Etherscan { end_block: EthereumBlockNumber },
}

impl AddressBackfillCursor {
    pub(crate) fn display_string(&self) -> String {
        match self {
            AddressBackfillCursor::Mempool { cursor_txid } => {
                format!("txid: {}", cursor_txid.as_str())
            }
            AddressBackfillCursor::Etherscan { end_block } => {
                format!("block: {}", end_block.value())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AddressBackfillState {
    pub cursor: AddressBackfillCursor,
    pub expected_tx_count: Option<TransactionCount>,
}

impl AddressBackfillState {
    pub(crate) fn new(
        cursor: AddressBackfillCursor,
        expected_tx_count: Option<TransactionCount>,
    ) -> Self {
        Self {
            cursor,
            expected_tx_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountBackfillProgress {
    pub state: AddressBackfillState,
    pub fetched_tx_count: Option<TransactionCount>,
    pub expected_tx_count_is_lower_bound: bool,
}

impl AccountBackfillProgress {
    pub(crate) fn new(
        state: AddressBackfillState,
        fetched_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: bool,
    ) -> Self {
        Self {
            state,
            fetched_tx_count,
            expected_tx_count_is_lower_bound,
        }
    }

    pub(crate) fn expected_tx_count(&self) -> Option<TransactionCount> {
        self.state.expected_tx_count
    }

    pub(crate) fn set_fetched_tx_count(&mut self, fetched_tx_count: Option<TransactionCount>) {
        self.fetched_tx_count = fetched_tx_count;
    }

    pub(crate) fn set_expected_tx_count(
        &mut self,
        expected_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: bool,
    ) {
        self.state.expected_tx_count = expected_tx_count;
        self.expected_tx_count_is_lower_bound = expected_tx_count_is_lower_bound;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EtherscanHistoryStatus {
    Continuous,
    RecentOnly,
    Gap,
}

impl EtherscanHistoryStatus {
    pub(crate) const fn as_db_value(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::RecentOnly => "recent_only",
            Self::Gap => "gap",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountIntegrationSyncSnapshot {
    pub integration_id: SyncIntegrationId,
    pub is_active: bool,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub last_result: Option<AggregateSyncResult>,
    pub last_error: Option<SyncErrorMessage>,
    pub backfill_progress: Option<AccountBackfillProgress>,
    pub etherscan_history_status: Option<EtherscanHistoryStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountSyncSnapshot {
    pub account_id: DigitalAssetAccountId,
    #[serde(skip_serializing, default)]
    pub sync_integration_id: Option<SyncIntegrationId>,
    pub addresses_total: AddressCount,
    pub addresses_never_synced: AddressCount,
    pub addresses_synced: AddressCount,
    pub addresses_failed: AddressCount,
    pub addresses_in_progress: AddressCount,
    pub max_consecutive_failures: ConsecutiveFailureCount,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub last_result: Option<AccountSyncResult>,
    pub last_error: Option<SyncErrorMessage>,
    pub backfill_progress: Option<AccountBackfillProgress>,
    pub etherscan_history_status: Option<EtherscanHistoryStatus>,
    pub integration_states: Vec<AccountIntegrationSyncSnapshot>,
}

impl AccountSyncSnapshot {
    pub(crate) fn is_running(&self) -> bool {
        self.addresses_in_progress.value() > 0
    }
}

pub(crate) fn derive_account_sync_result_from_integration_states(
    integration_states: &[AccountIntegrationSyncSnapshot],
) -> Option<AccountSyncResult> {
    if integration_states.iter().any(|state| state.is_active) {
        return Some(AccountSyncResult::InProgress);
    }

    let mut saw_result = false;
    let mut all_success = true;
    let mut all_failure = true;

    for state in integration_states {
        let Some(result) = state.last_result else {
            continue;
        };
        saw_result = true;
        match result {
            AggregateSyncResult::Success => {
                all_failure = false;
            }
            AggregateSyncResult::Failure => {
                all_success = false;
            }
            AggregateSyncResult::Partial => {
                all_success = false;
                all_failure = false;
            }
        }
    }

    if !saw_result {
        return None;
    }

    if all_success {
        Some(AccountSyncResult::Success)
    } else if all_failure {
        Some(AccountSyncResult::Failure)
    } else {
        Some(AccountSyncResult::Partial)
    }
}

pub(crate) fn derive_account_sync_result(
    snapshot: &AccountSyncSnapshot,
) -> Option<AccountSyncResult> {
    let address_result = derive_account_sync_result_from_address_counts(snapshot);

    if !snapshot.integration_states.is_empty() {
        let integration_result =
            derive_account_sync_result_from_integration_states(&snapshot.integration_states);
        return match integration_result {
            Some(AccountSyncResult::InProgress) => Some(AccountSyncResult::InProgress),
            _ if address_result == Some(AccountSyncResult::Success) => address_result,
            Some(AccountSyncResult::Success)
                if address_result != Some(AccountSyncResult::Success) =>
            {
                address_result
            }
            Some(result) => Some(result),
            None => address_result,
        };
    }

    address_result
}

fn derive_account_sync_result_from_address_counts(
    snapshot: &AccountSyncSnapshot,
) -> Option<AccountSyncResult> {
    if snapshot.addresses_in_progress.value() > 0 {
        return Some(AccountSyncResult::InProgress);
    }

    let synced = snapshot.addresses_synced.value();
    let failed = snapshot.addresses_failed.value();
    let never = snapshot.addresses_never_synced.value();

    if synced == 0 && failed == 0 {
        return None;
    }

    if failed == 0 {
        if never > 0 {
            Some(AccountSyncResult::InProgress)
        } else {
            Some(AccountSyncResult::Success)
        }
    } else if synced == 0 && never == 0 {
        Some(AccountSyncResult::Failure)
    } else {
        Some(AccountSyncResult::Partial)
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn compute_aggregate_sync_result_all_success() {
        let result =
            compute_aggregate_sync_result(AddressCount::from_u32(3), AddressCount::from_u32(0));
        assert_eq!(result, AggregateSyncResult::Success);
    }

    #[test]
    fn compute_aggregate_sync_result_all_failure() {
        let result =
            compute_aggregate_sync_result(AddressCount::from_u32(0), AddressCount::from_u32(3));
        assert_eq!(result, AggregateSyncResult::Failure);
    }

    #[test]
    fn compute_aggregate_sync_result_partial() {
        let result =
            compute_aggregate_sync_result(AddressCount::from_u32(2), AddressCount::from_u32(1));
        assert_eq!(result, AggregateSyncResult::Partial);
    }

    #[test]
    fn compute_aggregate_sync_result_zero_both_is_success() {
        let result =
            compute_aggregate_sync_result(AddressCount::from_u32(0), AddressCount::from_u32(0));
        assert_eq!(result, AggregateSyncResult::Success);
    }

    fn make_account_sync_snapshot(
        addresses_never_synced: u32,
        addresses_synced: u32,
        addresses_failed: u32,
        addresses_in_progress: u32,
    ) -> AccountSyncSnapshot {
        AccountSyncSnapshot {
            account_id: DigitalAssetAccountId::new(),
            sync_integration_id: None,
            addresses_total: AddressCount::from_u32(
                addresses_never_synced
                    .saturating_add(addresses_synced)
                    .saturating_add(addresses_failed),
            ),
            addresses_never_synced: AddressCount::from_u32(addresses_never_synced),
            addresses_synced: AddressCount::from_u32(addresses_synced),
            addresses_failed: AddressCount::from_u32(addresses_failed),
            addresses_in_progress: AddressCount::from_u32(addresses_in_progress),
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

    #[test]
    fn derive_account_sync_result_none_for_never_synced_accounts() {
        let snapshot = make_account_sync_snapshot(2, 0, 0, 0);
        assert_eq!(derive_account_sync_result(&snapshot), None);
    }

    #[test]
    fn derive_account_sync_result_success_for_all_successful_addresses() {
        let snapshot = make_account_sync_snapshot(0, 2, 0, 0);
        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::Success)
        );
    }

    #[test]
    fn derive_account_sync_result_in_progress_for_partially_synced_initial_account() {
        let snapshot = make_account_sync_snapshot(2, 3, 0, 0);
        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::InProgress)
        );
    }

    #[test]
    fn derive_account_sync_result_does_not_trust_integration_success_before_all_addresses_sync() {
        let now = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let mut snapshot = make_account_sync_snapshot(35, 5, 0, 0);
        snapshot
            .integration_states
            .push(AccountIntegrationSyncSnapshot {
                integration_id: SyncIntegrationId::Mempool,
                is_active: false,
                last_started_at: Some(now),
                last_completed_at: Some(now),
                last_result: Some(AggregateSyncResult::Success),
                last_error: None,
                backfill_progress: None,
                etherscan_history_status: None,
            });

        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::InProgress)
        );
    }

    #[test]
    fn derive_account_sync_result_does_not_trust_integration_success_over_address_failure() {
        let now = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let mut snapshot = make_account_sync_snapshot(0, 39, 1, 0);
        snapshot
            .integration_states
            .push(AccountIntegrationSyncSnapshot {
                integration_id: SyncIntegrationId::Mempool,
                is_active: false,
                last_started_at: Some(now),
                last_completed_at: Some(now),
                last_result: Some(AggregateSyncResult::Success),
                last_error: None,
                backfill_progress: None,
                etherscan_history_status: None,
            });

        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::Partial)
        );
    }

    #[test]
    fn derive_account_sync_result_trusts_all_successful_addresses_over_stale_integration_partial() {
        let now = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let mut snapshot = make_account_sync_snapshot(0, 40, 0, 0);
        snapshot
            .integration_states
            .push(AccountIntegrationSyncSnapshot {
                integration_id: SyncIntegrationId::Mempool,
                is_active: false,
                last_started_at: Some(now),
                last_completed_at: Some(now),
                last_result: Some(AggregateSyncResult::Partial),
                last_error: Some(SyncErrorMessage::sanitize("stale failure")),
                backfill_progress: None,
                etherscan_history_status: None,
            });

        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::Success)
        );
    }

    #[test]
    fn derive_account_sync_result_failure_for_all_failed_addresses() {
        let snapshot = make_account_sync_snapshot(0, 0, 2, 0);
        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::Failure)
        );
    }

    #[test]
    fn derive_account_sync_result_partial_for_mixed_outcomes() {
        let snapshot = make_account_sync_snapshot(0, 1, 1, 0);
        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::Partial)
        );
    }

    #[test]
    fn derive_account_sync_result_in_progress_overrides_other_states() {
        let snapshot = make_account_sync_snapshot(0, 1, 1, 1);
        assert_eq!(
            derive_account_sync_result(&snapshot),
            Some(AccountSyncResult::InProgress)
        );
        assert!(snapshot.is_running());
    }

    #[test]
    fn derive_account_sync_result_from_integration_states_is_order_independent() {
        let now = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let success = AccountIntegrationSyncSnapshot {
            integration_id: SyncIntegrationId::Mempool,
            is_active: false,
            last_started_at: Some(now),
            last_completed_at: Some(now),
            last_result: Some(AggregateSyncResult::Success),
            last_error: None,
            backfill_progress: None,
            etherscan_history_status: None,
        };
        let failure = AccountIntegrationSyncSnapshot {
            integration_id: SyncIntegrationId::Etherscan,
            is_active: false,
            last_started_at: Some(now),
            last_completed_at: Some(now),
            last_result: Some(AggregateSyncResult::Failure),
            last_error: Some(SyncErrorMessage::sanitize("boom")),
            backfill_progress: None,
            etherscan_history_status: None,
        };

        assert_eq!(
            derive_account_sync_result_from_integration_states(&[success.clone(), failure.clone()]),
            Some(AccountSyncResult::Partial)
        );
        assert_eq!(
            derive_account_sync_result_from_integration_states(&[failure, success]),
            Some(AccountSyncResult::Partial)
        );
    }

    #[test]
    fn derive_account_sync_result_from_integration_states_returns_none_when_empty() {
        assert_eq!(
            derive_account_sync_result_from_integration_states(&[]),
            None
        );
    }

    #[test]
    fn derive_account_address_sync_status_not_synced_when_no_sync_row_exists() {
        assert_eq!(
            derive_account_address_sync_status(None, None, None, false, false),
            AccountAddressSyncStatus::NotSynced
        );
    }

    #[test]
    fn derive_account_address_sync_status_syncing_when_started_without_completion() {
        let started_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                Some(started_at),
                None,
                Some(TransactionSyncResult::Success),
                false,
                false,
            ),
            AccountAddressSyncStatus::Syncing
        );
    }

    #[test]
    fn derive_account_address_sync_status_syncing_when_started_after_completion() {
        let started_at = "2026-03-14T12:01:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let completed_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                Some(started_at),
                Some(completed_at),
                Some(TransactionSyncResult::Success),
                false,
                false,
            ),
            AccountAddressSyncStatus::Syncing
        );
    }

    #[test]
    fn derive_account_address_sync_status_syncing_when_mempool_backfill_cursor_exists() {
        let started_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let completed_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                Some(started_at),
                Some(completed_at),
                Some(TransactionSyncResult::Success),
                true,
                false,
            ),
            AccountAddressSyncStatus::Syncing
        );
    }

    #[test]
    fn derive_account_address_sync_status_syncing_when_etherscan_backfill_block_exists() {
        let started_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let completed_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                Some(started_at),
                Some(completed_at),
                Some(TransactionSyncResult::Success),
                false,
                true,
            ),
            AccountAddressSyncStatus::Syncing
        );
    }

    #[test]
    fn derive_account_address_sync_status_synced_when_completed_success() {
        let completed_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                None,
                Some(completed_at),
                Some(TransactionSyncResult::Success),
                false,
                false,
            ),
            AccountAddressSyncStatus::Synced
        );
    }

    #[test]
    fn derive_account_address_sync_status_failed_when_completed_failure() {
        let completed_at = "2026-03-14T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        assert_eq!(
            derive_account_address_sync_status(
                None,
                Some(completed_at),
                Some(TransactionSyncResult::Failure),
                false,
                false,
            ),
            AccountAddressSyncStatus::Failed
        );
    }
}

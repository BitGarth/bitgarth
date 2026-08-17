#![cfg_attr(
    not(feature = "server"),
    expect(
        dead_code,
        reason = "Transaction sync domain types are primarily exercised on server paths"
    )
)]

use super::sync_state::*;
use super::types::*;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncEventType {
    Snapshot,
    Started,
    Completed,
    Failed,
    AccountSyncStarted,
    AccountSyncProgress,
    AccountSyncCompleted,
    AccountSyncFailed,
    AccountIntegrationSyncStarted,
    AccountIntegrationSyncProgress,
    AccountIntegrationSyncCompleted,
    AccountIntegrationSyncFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RateLimitedIntegration {
    pub integration: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TransactionSyncEvent {
    pub event_type: TransactionSyncEventType,
    pub run_id: Option<TransactionSyncRunId>,
    pub occurred_at: DateTime<Utc>,
    pub snapshot: Option<AggregateSyncSnapshot>,
    pub new_tx_count: Option<TransactionCount>,
    pub updated_tx_count: Option<TransactionCount>,
    pub addresses_synced: Option<AddressCount>,
    pub addresses_failed: Option<AddressCount>,
    pub addresses_skipped: Option<AddressCount>,
    pub rate_limited: Option<Vec<RateLimitedIntegration>>,
    pub error: Option<SyncErrorMessage>,
    pub account_id: Option<DigitalAssetAccountId>,
    pub is_first_sync: Option<bool>,
    pub fetched_tx_count: Option<TransactionCount>,
    pub expected_tx_count: Option<TransactionCount>,
    pub expected_tx_count_is_lower_bound: Option<bool>,
    pub addresses_total: Option<AddressCount>,
    /// Set only for integration-scoped events (AccountIntegrationSync*).
    pub integration_id: Option<SyncIntegrationId>,
    /// Set on AccountIntegrationSyncFailed when the integration is rate-limited and a
    /// retry-after time is known. Live-SSE-only; not persisted in the database.
    pub retry_after: Option<DateTime<Utc>>,
}

impl TransactionSyncEvent {
    pub(crate) fn event_name(&self) -> &'static str {
        match self.event_type {
            TransactionSyncEventType::Snapshot => "sync_snapshot",
            TransactionSyncEventType::Started => "sync_started",
            TransactionSyncEventType::Completed => "sync_completed",
            TransactionSyncEventType::Failed => "sync_failed",
            TransactionSyncEventType::AccountSyncStarted => "account_sync_started",
            TransactionSyncEventType::AccountSyncProgress => "account_sync_progress",
            TransactionSyncEventType::AccountSyncCompleted => "account_sync_completed",
            TransactionSyncEventType::AccountSyncFailed => "account_sync_failed",
            TransactionSyncEventType::AccountIntegrationSyncStarted => {
                "account_integration_sync_started"
            }
            TransactionSyncEventType::AccountIntegrationSyncProgress => {
                "account_integration_sync_progress"
            }
            TransactionSyncEventType::AccountIntegrationSyncCompleted => {
                "account_integration_sync_completed"
            }
            TransactionSyncEventType::AccountIntegrationSyncFailed => {
                "account_integration_sync_failed"
            }
        }
    }

    #[cfg(all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    ))]
    pub(crate) fn sync_snapshot(
        snapshot: AggregateSyncSnapshot,
        occurred_at: DateTime<Utc>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::Snapshot,
            run_id: None,
            occurred_at,
            snapshot: Some(snapshot),
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: None,
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn sync_started(run_id: TransactionSyncRunId, started_at: DateTime<Utc>) -> Self {
        Self {
            event_type: TransactionSyncEventType::Started,
            run_id: Some(run_id),
            occurred_at: started_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: None,
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sync_completed(
        run_id: TransactionSyncRunId,
        completed_at: DateTime<Utc>,
        new_tx_count: TransactionCount,
        updated_tx_count: TransactionCount,
        addresses_synced: AddressCount,
        addresses_failed: AddressCount,
        addresses_skipped: AddressCount,
        rate_limited: Vec<RateLimitedIntegration>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::Completed,
            run_id: Some(run_id),
            occurred_at: completed_at,
            snapshot: None,
            new_tx_count: Some(new_tx_count),
            updated_tx_count: Some(updated_tx_count),
            addresses_synced: Some(addresses_synced),
            addresses_failed: Some(addresses_failed),
            addresses_skipped: Some(addresses_skipped),
            rate_limited: if rate_limited.is_empty() {
                None
            } else {
                Some(rate_limited)
            },
            error: None,
            account_id: None,
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn sync_failed(
        run_id: TransactionSyncRunId,
        failed_at: DateTime<Utc>,
        error: SyncErrorMessage,
        addresses_failed: AddressCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::Failed,
            run_id: Some(run_id),
            occurred_at: failed_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: Some(addresses_failed),
            addresses_skipped: None,
            rate_limited: None,
            error: Some(error),
            account_id: None,
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_sync_started_single_address(
        run_id: TransactionSyncRunId,
        started_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        is_first_sync: bool,
        expected_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: Option<bool>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncStarted,
            run_id: Some(run_id),
            occurred_at: started_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: Some(is_first_sync),
            fetched_tx_count: None,
            expected_tx_count,
            expected_tx_count_is_lower_bound,
            addresses_total: Some(AddressCount::from_u32(1)),
            integration_id: None,
            retry_after: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn account_sync_progress_single_address(
        run_id: TransactionSyncRunId,
        progress_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        is_first_sync: bool,
        fetched_tx_count: TransactionCount,
        expected_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: Option<bool>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncProgress,
            run_id: Some(run_id),
            occurred_at: progress_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: Some(is_first_sync),
            fetched_tx_count: Some(fetched_tx_count),
            expected_tx_count,
            expected_tx_count_is_lower_bound,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_sync_started_hd_account(
        run_id: TransactionSyncRunId,
        started_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        addresses_total: AddressCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncStarted,
            run_id: Some(run_id),
            occurred_at: started_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: Some(addresses_total),
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_sync_progress_hd_account(
        run_id: TransactionSyncRunId,
        progress_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        addresses_synced: AddressCount,
        addresses_total: AddressCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncProgress,
            run_id: Some(run_id),
            occurred_at: progress_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: Some(addresses_synced),
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: Some(addresses_total),
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_sync_completed(
        run_id: TransactionSyncRunId,
        completed_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        new_tx_count: TransactionCount,
        updated_tx_count: TransactionCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncCompleted,
            run_id: Some(run_id),
            occurred_at: completed_at,
            snapshot: None,
            new_tx_count: Some(new_tx_count),
            updated_tx_count: Some(updated_tx_count),
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_sync_failed(
        run_id: TransactionSyncRunId,
        failed_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        error: SyncErrorMessage,
        rate_limited: Vec<RateLimitedIntegration>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountSyncFailed,
            run_id: Some(run_id),
            occurred_at: failed_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: if rate_limited.is_empty() {
                None
            } else {
                Some(rate_limited)
            },
            error: Some(error),
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: None,
            retry_after: None,
        }
    }

    pub(crate) fn account_integration_sync_started_single_address(
        run_id: TransactionSyncRunId,
        started_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        is_first_sync: bool,
        expected_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: Option<bool>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncStarted,
            run_id: Some(run_id),
            occurred_at: started_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: Some(is_first_sync),
            fetched_tx_count: None,
            expected_tx_count,
            expected_tx_count_is_lower_bound,
            addresses_total: Some(AddressCount::from_u32(1)),
            integration_id: Some(integration_id),
            retry_after: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn account_integration_sync_progress_single_address(
        run_id: TransactionSyncRunId,
        progress_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        is_first_sync: bool,
        fetched_tx_count: TransactionCount,
        expected_tx_count: Option<TransactionCount>,
        expected_tx_count_is_lower_bound: Option<bool>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncProgress,
            run_id: Some(run_id),
            occurred_at: progress_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: Some(is_first_sync),
            fetched_tx_count: Some(fetched_tx_count),
            expected_tx_count,
            expected_tx_count_is_lower_bound,
            addresses_total: None,
            integration_id: Some(integration_id),
            retry_after: None,
        }
    }

    pub(crate) fn account_integration_sync_started_hd_account(
        run_id: TransactionSyncRunId,
        started_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        addresses_total: AddressCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncStarted,
            run_id: Some(run_id),
            occurred_at: started_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: Some(addresses_total),
            integration_id: Some(integration_id),
            retry_after: None,
        }
    }

    pub(crate) fn account_integration_sync_progress_hd_account(
        run_id: TransactionSyncRunId,
        progress_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        addresses_synced: AddressCount,
        addresses_total: AddressCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncProgress,
            run_id: Some(run_id),
            occurred_at: progress_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: Some(addresses_synced),
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: Some(addresses_total),
            integration_id: Some(integration_id),
            retry_after: None,
        }
    }

    pub(crate) fn account_integration_sync_completed(
        run_id: TransactionSyncRunId,
        completed_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        new_tx_count: TransactionCount,
        updated_tx_count: TransactionCount,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncCompleted,
            run_id: Some(run_id),
            occurred_at: completed_at,
            snapshot: None,
            new_tx_count: Some(new_tx_count),
            updated_tx_count: Some(updated_tx_count),
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: None,
            error: None,
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: Some(integration_id),
            retry_after: None,
        }
    }

    pub(crate) fn account_integration_sync_failed(
        run_id: TransactionSyncRunId,
        failed_at: DateTime<Utc>,
        account_id: DigitalAssetAccountId,
        integration_id: SyncIntegrationId,
        error: SyncErrorMessage,
        rate_limited: Vec<RateLimitedIntegration>,
        retry_after: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            event_type: TransactionSyncEventType::AccountIntegrationSyncFailed,
            run_id: Some(run_id),
            occurred_at: failed_at,
            snapshot: None,
            new_tx_count: None,
            updated_tx_count: None,
            addresses_synced: None,
            addresses_failed: None,
            addresses_skipped: None,
            rate_limited: if rate_limited.is_empty() {
                None
            } else {
                Some(rate_limited)
            },
            error: Some(error),
            account_id: Some(account_id),
            is_first_sync: None,
            fetched_tx_count: None,
            expected_tx_count: None,
            expected_tx_count_is_lower_bound: None,
            addresses_total: None,
            integration_id: Some(integration_id),
            retry_after,
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn event_name_includes_account_level_variants() {
        let account_id = DigitalAssetAccountId::new();
        let run_id = TransactionSyncRunId::new();
        let occurred_at = "2026-03-01T14:30:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");

        let started = TransactionSyncEvent::account_sync_started_single_address(
            run_id,
            occurred_at,
            account_id,
            true,
            None,
            None,
        );
        let progress = TransactionSyncEvent::account_sync_progress_hd_account(
            run_id,
            occurred_at,
            account_id,
            AddressCount::from_u32(3),
            AddressCount::from_u32(10),
        );
        let completed = TransactionSyncEvent::account_sync_completed(
            run_id,
            occurred_at,
            account_id,
            TransactionCount::from_u32(5),
            TransactionCount::from_u32(2),
        );
        let failed = TransactionSyncEvent::account_sync_failed(
            run_id,
            occurred_at,
            account_id,
            SyncErrorMessage::sanitize("oops"),
            vec![RateLimitedIntegration {
                integration: "etherscan".to_string(),
            }],
        );

        assert_eq!(started.event_name(), "account_sync_started");
        assert_eq!(progress.event_name(), "account_sync_progress");
        assert_eq!(completed.event_name(), "account_sync_completed");
        assert_eq!(failed.event_name(), "account_sync_failed");
    }

    #[test]
    fn account_sync_started_single_address_sets_first_sync_flag() {
        let account_id = DigitalAssetAccountId::new();
        let run_id = TransactionSyncRunId::new();
        let occurred_at = "2026-03-01T14:30:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");

        let first_sync = TransactionSyncEvent::account_sync_started_single_address(
            run_id,
            occurred_at,
            account_id,
            true,
            Some(TransactionCount::from_u32(150)),
            Some(false),
        );
        let incremental = TransactionSyncEvent::account_sync_started_single_address(
            run_id,
            occurred_at,
            account_id,
            false,
            None,
            None,
        );

        assert_eq!(first_sync.is_first_sync, Some(true));
        assert_eq!(incremental.is_first_sync, Some(false));
        assert_eq!(first_sync.account_id, Some(account_id));
        assert_eq!(first_sync.addresses_total, Some(AddressCount::from_u32(1)));
        assert_eq!(
            first_sync.expected_tx_count,
            Some(TransactionCount::from_u32(150))
        );
    }

    #[test]
    fn account_sync_progress_hd_account_serializes_expected_counts() {
        let account_id = DigitalAssetAccountId::new();
        let run_id = TransactionSyncRunId::new();
        let occurred_at = "2026-03-01T14:30:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let event = TransactionSyncEvent::account_sync_progress_hd_account(
            run_id,
            occurred_at,
            account_id,
            AddressCount::from_u32(15),
            AddressCount::from_u32(42),
        );

        let json = serde_json::to_value(&event).expect("event should serialize");
        assert_eq!(json["event_type"], "account_sync_progress");
        assert_eq!(json["account_id"], account_id.to_string());
        assert_eq!(json["addresses_synced"], 15);
        assert_eq!(json["addresses_total"], 42);
    }

    #[test]
    fn account_sync_progress_single_address_omits_expected_count_for_incremental_sync() {
        let account_id = DigitalAssetAccountId::new();
        let run_id = TransactionSyncRunId::new();
        let occurred_at = "2026-03-01T14:30:05Z"
            .parse::<DateTime<Utc>>()
            .expect("valid timestamp");
        let event = TransactionSyncEvent::account_sync_progress_single_address(
            run_id,
            occurred_at,
            account_id,
            false,
            TransactionCount::from_u32(42),
            None,
            None,
        );

        assert_eq!(event.is_first_sync, Some(false));
        assert_eq!(event.fetched_tx_count, Some(TransactionCount::from_u32(42)));
        assert_eq!(event.expected_tx_count, None);
        assert_eq!(event.expected_tx_count_is_lower_bound, None);
    }
}

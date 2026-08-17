use crate::models::UserId;
use crate::tasks::publish_transaction_sync_event;
use crate::transactions::{
    RateLimitedIntegration, SyncErrorMessage, SyncIntegrationId, TransactionCount,
    TransactionSyncEvent, TransactionSyncRunId,
};
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};

/// Common context for account-level sync event publication.
pub(super) struct AccountEventContext {
    pub(super) user_id: UserId,
    pub(super) run_id: TransactionSyncRunId,
    pub(super) completed_at_utc: DateTime<Utc>,
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) integration_id: SyncIntegrationId,
}

/// Publish the paired account-sync and account-integration-sync started events
/// for a single-address (non-HD or manual-control) sync.
///
/// Event order: account sync started, then account integration sync started.
pub(super) fn publish_single_address_started_events(
    ctx: &AccountEventContext,
    started_at_utc: DateTime<Utc>,
    is_first_sync: bool,
    expected_tx_count: Option<TransactionCount>,
    expected_tx_count_is_lower_bound: Option<bool>,
) {
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_sync_started_single_address(
            ctx.run_id,
            started_at_utc,
            ctx.account_id,
            is_first_sync,
            expected_tx_count,
            expected_tx_count_is_lower_bound,
        ),
    );
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_integration_sync_started_single_address(
            ctx.run_id,
            started_at_utc,
            ctx.account_id,
            ctx.integration_id,
            is_first_sync,
            expected_tx_count,
            expected_tx_count_is_lower_bound,
        ),
    );
}

/// Publish the paired account-sync and account-integration-sync failed events.
///
/// Event order: account sync failed, then account integration sync failed.
pub(super) fn publish_account_sync_failed_events(
    ctx: &AccountEventContext,
    error: SyncErrorMessage,
    rate_limited: Vec<RateLimitedIntegration>,
    retry_after_utc: Option<DateTime<Utc>>,
) {
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_sync_failed(
            ctx.run_id,
            ctx.completed_at_utc,
            ctx.account_id,
            error.clone(),
            rate_limited.clone(),
        ),
    );
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_integration_sync_failed(
            ctx.run_id,
            ctx.completed_at_utc,
            ctx.account_id,
            ctx.integration_id,
            error,
            rate_limited,
            retry_after_utc,
        ),
    );
}

/// Publish the paired account-sync and account-integration-sync completed events.
///
/// Event order: account sync completed, then account integration sync completed.
pub(super) fn publish_account_sync_completed_events(
    ctx: &AccountEventContext,
    new_tx_count: TransactionCount,
    updated_tx_count: TransactionCount,
) {
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_sync_completed(
            ctx.run_id,
            ctx.completed_at_utc,
            ctx.account_id,
            new_tx_count,
            updated_tx_count,
        ),
    );
    publish_transaction_sync_event(
        ctx.user_id,
        TransactionSyncEvent::account_integration_sync_completed(
            ctx.run_id,
            ctx.completed_at_utc,
            ctx.account_id,
            ctx.integration_id,
            new_tx_count,
            updated_tx_count,
        ),
    );
}

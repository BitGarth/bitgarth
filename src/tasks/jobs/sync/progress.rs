use super::{
    AddressCount, RunContext, SingleAddressProgressPlan, SyncClients, TransactionSyncEvent,
    integration_for_asset, is_first_sync, is_rate_limited, publish_transaction_sync_event,
    to_address_count,
};
use crate::db::SyncAddress;
use crate::tasks::jobs::sync::integrations::{
    IntegrationEstimateContext,
    estimate_first_sync_tx_count as integration_estimate_first_sync_tx_count,
    unfinished_backfill_state as integration_unfinished_backfill_state,
};
use crate::transactions::SyncIntegrationId;
use crate::transactions::TxCountEstimate;
use dioxus::logger::tracing;

pub(crate) fn approximate_account_unsynced_count(
    reported_address_counts: impl IntoIterator<Item = crate::transactions::TransactionCount>,
    known_account_transactions: crate::transactions::TransactionCount,
) -> crate::transactions::TransactionCount {
    let reported_total = reported_address_counts
        .into_iter()
        .fold(0_u32, |total, count| total.saturating_add(count.value()));
    crate::transactions::TransactionCount::from_u32(
        reported_total.saturating_sub(known_account_transactions.value()),
    )
}

pub(super) fn publish_hd_account_progress_event(
    run: RunContext<'_>,
    account_id: crate::wallets::DigitalAssetAccountId,
    integration_id: SyncIntegrationId,
    addresses_synced: usize,
    addresses_total: u32,
) {
    crate::db::debug_assert_user_db_unlocked(run.user_id, "hd account progress publish");
    let now_utc = run.clock.utc_now();
    let addresses_synced_count = to_address_count(addresses_synced);
    let addresses_total_count = AddressCount::from_u32(addresses_total);
    publish_transaction_sync_event(
        run.user_id,
        TransactionSyncEvent::account_sync_progress_hd_account(
            run.run_id,
            now_utc,
            account_id,
            addresses_synced_count,
            addresses_total_count,
        ),
    );
    publish_transaction_sync_event(
        run.user_id,
        TransactionSyncEvent::account_integration_sync_progress_hd_account(
            run.run_id,
            now_utc,
            account_id,
            integration_id,
            addresses_synced_count,
            addresses_total_count,
        ),
    );
}

fn estimate_first_sync_tx_count(
    run: RunContext<'_>,
    address: &SyncAddress,
    clients: SyncClients<'_>,
    allow_transaction_history_estimate: bool,
) -> Option<TxCountEstimate> {
    if !allow_transaction_history_estimate {
        return None;
    }

    let integration = integration_for_asset(address.asset_id);
    let now = run.clock.instant_now();
    if is_rate_limited(run.user_id, integration, now) {
        return None;
    }

    match integration_estimate_first_sync_tx_count(IntegrationEstimateContext {
        run,
        address,
        clients,
    }) {
        Ok(estimate) => estimate,
        Err(error) => {
            tracing::warn!(
                user_id = %run.user_id,
                run_id = %run.run_id,
                address_id = %address.address_id,
                error = %error,
                "transactions sync: failed to load preflight estimate"
            );
            None
        }
    }
}

pub(super) fn build_single_address_progress_plan(
    run: RunContext<'_>,
    address: &SyncAddress,
    clients: SyncClients<'_>,
    allow_transaction_history_estimate: bool,
) -> Option<SingleAddressProgressPlan> {
    let account_id = address.account_id?;
    let unfinished_backfill = integration_unfinished_backfill_state(address);
    let has_unfinished_backfill = unfinished_backfill.is_some();
    let is_first = is_first_sync(address.last_tip_height);
    let estimate = if is_first {
        estimate_first_sync_tx_count(run, address, clients, allow_transaction_history_estimate)
            .or_else(|| {
                unfinished_backfill
                    .as_ref()
                    .and_then(|state| state.expected_tx_count)
                    .map(TxCountEstimate::Exact)
            })
    } else {
        unfinished_backfill
            .as_ref()
            .and_then(|state| state.expected_tx_count)
            .map(TxCountEstimate::Exact)
    };
    Some(SingleAddressProgressPlan {
        account_id,
        is_first_sync: is_first || has_unfinished_backfill,
        expected_tx_count: estimate.map(TxCountEstimate::transaction_count),
        expected_tx_count_is_lower_bound: estimate.map(TxCountEstimate::is_lower_bound),
    })
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::transactions::TransactionCount;

    #[test]
    fn mempool_history_proof_progress_uses_one_saturating_approximate_unsynced_count() {
        assert_eq!(
            approximate_account_unsynced_count(
                [
                    TransactionCount::from_u32(u32::MAX),
                    TransactionCount::from_u32(10),
                ],
                TransactionCount::from_u32(3),
            ),
            TransactionCount::from_u32(u32::MAX - 3)
        );
        assert_eq!(
            approximate_account_unsynced_count(
                [TransactionCount::from_u32(2)],
                TransactionCount::from_u32(5),
            ),
            TransactionCount::zero()
        );
    }

    #[test]
    fn cap_crossing_progress_uses_reported_confirmed_minus_known_not_stale_estimate() {
        let approximate = approximate_account_unsynced_count(
            [TransactionCount::from_u32(6), TransactionCount::from_u32(5)],
            TransactionCount::from_u32(7),
        );

        assert_eq!(approximate, TransactionCount::from_u32(4));
        assert_ne!(
            approximate,
            TransactionCount::from_u32(100_u32.saturating_sub(3)),
            "stale expected count minus configured cap is not sync progress"
        );
    }
}

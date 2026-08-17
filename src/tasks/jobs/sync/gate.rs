use super::client_config::fetch_mempool_chain_tip;
use super::context::{DEFAULT_INTER_ADDRESS_PACING_POLICY, is_successful_balance_refresh_fresh};
use super::error::preserve_iteration_error;
use super::{
    ADDRESS_SYNC_COOLDOWN, AddressSyncExecutionRequest, AddressSyncExecutor, ChainTipCache,
    FAILED_ADDRESS_SYNC_COOLDOWN, LABEL_ETHERSCAN, LABEL_MEMPOOL, RunContext,
    SingleAddressProgressPlan, SyncClients, SyncIterationResult, TransactionSyncResult,
    UserTransactionMonitorError, is_rate_limited, mark_sync_failure, record_rate_limit,
};
use crate::asset_capabilities::{SyncProviderId, default_sync_provider};
use crate::db::{
    AddressSyncSuccess, SyncAddress, mark_address_sync_completed_success, mark_address_sync_started,
};
use crate::tasks::TriggerSource;
use crate::transactions::{ChainTipHeight, TransactionCount};
use crate::wallets::DigitalAssetAddressId;
use crate::wallets::SyncedAssetId;
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::HashSet;
use std::time::Duration;

pub(super) fn default_api_provider_for_asset(asset_id: SyncedAssetId) -> SyncProviderId {
    default_sync_provider(asset_id)
}

pub(super) fn integration_for_asset(asset_id: SyncedAssetId) -> &'static str {
    integration_label_for_provider(default_api_provider_for_asset(asset_id))
}

pub(super) fn requires_provider(
    non_hd_addresses: &[SyncAddress],
    hd_bundles: &[crate::db::AccountSyncBundle],
    provider: SyncProviderId,
) -> bool {
    non_hd_addresses
        .iter()
        .any(|address| default_api_provider_for_asset(address.asset_id) == provider)
        || hd_bundles
            .iter()
            .any(|bundle| default_api_provider_for_asset(bundle.asset_id) == provider)
}

fn integration_label_for_provider(provider: SyncProviderId) -> &'static str {
    match provider {
        SyncProviderId::MempoolSpace => LABEL_MEMPOOL,
        SyncProviderId::Etherscan => LABEL_ETHERSCAN,
    }
}

pub(super) fn cooldown_for_last_result(
    asset_id: SyncedAssetId,
    last_result: Option<TransactionSyncResult>,
) -> Option<Duration> {
    let provider = default_api_provider_for_asset(asset_id);
    match (provider, last_result) {
        (SyncProviderId::MempoolSpace, Some(TransactionSyncResult::Success)) => {
            Some(ADDRESS_SYNC_COOLDOWN)
        }
        (SyncProviderId::MempoolSpace, Some(TransactionSyncResult::Failure)) => {
            Some(FAILED_ADDRESS_SYNC_COOLDOWN)
        }
        (SyncProviderId::Etherscan, Some(TransactionSyncResult::Success)) => {
            Some(super::ETHERSCAN_ADDRESS_SYNC_COOLDOWN)
        }
        (SyncProviderId::Etherscan, Some(TransactionSyncResult::Failure)) => {
            Some(super::ETHERSCAN_FAILED_ADDRESS_SYNC_COOLDOWN)
        }
        (_, None) => None,
    }
}

fn is_on_cooldown(address: &SyncAddress, now: DateTime<Utc>, source: TriggerSource) -> bool {
    let Some(last_completed_at) = address.last_completed_at else {
        return false;
    };
    let Some(required_cooldown) = cooldown_for_last_result(address.asset_id, address.last_result)
    else {
        return false;
    };
    if matches!(source, TriggerSource::AutoUpgrade)
        || (matches!(source, TriggerSource::ManualInternal)
            && manual_trigger_bypasses_cooldown(address))
    {
        return false;
    }
    if matches!(address.last_result, Some(TransactionSyncResult::Success))
        && super::integrations::unfinished_backfill_state(address).is_some()
    {
        return false;
    }
    let elapsed = now.signed_duration_since(last_completed_at);
    if elapsed.num_seconds() < 0 {
        return true;
    }
    match elapsed.to_std() {
        Ok(elapsed_std) => elapsed_std < required_cooldown,
        Err(_) => false,
    }
}

fn manual_trigger_bypasses_cooldown(address: &SyncAddress) -> bool {
    if matches!(address.last_result, Some(TransactionSyncResult::Failure)) {
        return true;
    }
    matches!(
        (
            default_api_provider_for_asset(address.asset_id),
            address.last_result,
            address.last_tip_height,
        ),
        (
            SyncProviderId::Etherscan,
            Some(TransactionSyncResult::Success),
            Some(_)
        )
    )
}

pub(super) fn mempool_history_requires_first_page_restart(address: &SyncAddress) -> bool {
    let Some(expected) = address.mempool_expected_tx_count else {
        return false;
    };
    match address.mempool_history_proof {
        None => expected.value() > 0,
        Some(proof) => expected.value() > proof.confirmed_tx_count.value(),
    }
}

fn apply_inter_address_sync_delay(
    clock: &dyn super::SyncClock,
    is_first_attempt: &mut bool,
    provider: SyncProviderId,
) {
    if *is_first_attempt {
        *is_first_attempt = false;
        return;
    }
    clock.sleep(DEFAULT_INTER_ADDRESS_PACING_POLICY.delay_for_provider(provider));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TipUnchangedGateDecision {
    ContinueSync,
    SkipTipUnchanged,
    RefreshPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MempoolHistoryPolicy {
    CurrentOnly,
    Normal { cap: TransactionCount },
    LegacyRepair,
}

impl MempoolHistoryPolicy {
    pub(super) fn normal(enabled: bool, cap: TransactionCount) -> Self {
        if enabled {
            Self::Normal { cap }
        } else {
            Self::CurrentOnly
        }
    }

    pub(super) fn permits_transaction_page(self, stored_count: TransactionCount) -> bool {
        match self {
            Self::CurrentOnly => false,
            Self::Normal { cap } => stored_count.value() < cap.value(),
            Self::LegacyRepair => true,
        }
    }
}

pub(super) fn load_account_transaction_count_for_history_policy(
    user_id: crate::models::UserId,
    account_id: crate::wallets::DigitalAssetAccountId,
    policy: MempoolHistoryPolicy,
) -> Result<TransactionCount, UserTransactionMonitorError> {
    match policy {
        MempoolHistoryPolicy::Normal { cap } => Ok(
            crate::db::load_canonical_account_transaction_count_bounded(user_id, account_id, cap)?,
        ),
        MempoolHistoryPolicy::CurrentOnly | MempoolHistoryPolicy::LegacyRepair => {
            Ok(TransactionCount::zero())
        }
    }
}

pub(super) fn decide_tip_unchanged_gate(
    last_tip: Option<ChainTipHeight>,
    current_tip: ChainTipHeight,
    has_pending_txs: bool,
    has_api_confirmed_balance: bool,
) -> TipUnchangedGateDecision {
    if !has_api_confirmed_balance {
        return TipUnchangedGateDecision::ContinueSync;
    }

    match last_tip {
        Some(last_tip_height) if last_tip_height == current_tip => {
            if has_pending_txs {
                TipUnchangedGateDecision::RefreshPending
            } else {
                TipUnchangedGateDecision::SkipTipUnchanged
            }
        }
        _ => TipUnchangedGateDecision::ContinueSync,
    }
}

#[cfg(test)]
use crate::transactions::MempoolCursorTxid;

#[cfg(test)]
pub(super) fn should_use_mempool_backfill_lane(
    last_tip_height: Option<ChainTipHeight>,
    mempool_backfill_cursor_txid: Option<&MempoolCursorTxid>,
    allow_known_confirmed_early_exit: bool,
) -> bool {
    super::is_first_sync(last_tip_height)
        || mempool_backfill_cursor_txid.is_some()
        || !allow_known_confirmed_early_exit
}

pub(super) fn integration_for_address(address: &SyncAddress) -> &'static str {
    integration_for_asset(address.asset_id)
}

pub(super) fn summary_has_activity(summary: &SyncIterationResult) -> bool {
    summary.observed_activity
        || summary.new_tx_count.value() > 0
        || summary.updated_tx_count.value() > 0
        || summary
            .api_confirmed_balance
            .is_some_and(|balance| balance.amount() != crate::amounts::UnsignedAmount::zero())
}

pub(super) struct SyncSingleAddressControlRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) address: &'a mut SyncAddress,
    pub(super) chain_tip_cache: &'a mut ChainTipCache,
    pub(super) pending_address_ids: &'a HashSet<DigitalAssetAddressId>,
    pub(super) clients: SyncClients<'a>,
    pub(super) executor: &'a mut dyn AddressSyncExecutor,
    pub(super) accumulator: &'a mut super::CycleAccumulator,
    pub(super) processed_for_account: &'a mut u32,
    pub(super) single_address_progress: Option<SingleAddressProgressPlan>,
    pub(super) mempool_history_policy: MempoolHistoryPolicy,
    pub(super) mempool_history_page_frontier: Option<crate::db::HdMempoolHistoryFrontierUpdate>,
}

pub(super) fn sync_single_address_with_controls(
    request: SyncSingleAddressControlRequest<'_>,
) -> Result<(bool, bool), UserTransactionMonitorError> {
    let SyncSingleAddressControlRequest {
        run,
        address,
        chain_tip_cache,
        pending_address_ids,
        clients,
        executor,
        accumulator,
        processed_for_account,
        single_address_progress,
        mempool_history_policy,
        mempool_history_page_frontier,
    } = request;
    let stored_count = match address.account_id {
        Some(account_id) => load_account_transaction_count_for_history_policy(
            run.user_id,
            account_id,
            mempool_history_policy,
        )?,
        None => TransactionCount::zero(),
    };
    let historical_backfill_enabled = mempool_history_policy.permits_transaction_page(stored_count);
    let now_utc = run.clock.utc_now();
    let now_instant = run.clock.instant_now();
    let mut allow_known_confirmed_early_exit = true;
    let provider = default_api_provider_for_asset(address.asset_id);

    if !historical_backfill_enabled
        && is_successful_balance_refresh_fresh(
            address.last_result,
            address.last_completed_at,
            now_utc,
        )
    {
        accumulator.add_skipped();
        return Ok((false, false));
    }

    if is_on_cooldown(address, now_utc, run.source)
        && (!matches!(mempool_history_policy, MempoolHistoryPolicy::LegacyRepair)
            || matches!(address.last_result, Some(TransactionSyncResult::Failure)))
    {
        accumulator.add_skipped();
        return Ok((false, false));
    }

    let integration = integration_for_address(address);
    if is_rate_limited(run.user_id, integration, now_instant) {
        accumulator.add_rate_limited(integration);
        accumulator.add_skipped();
        return Ok((false, true));
    }

    if provider == SyncProviderId::MempoolSpace
        && !matches!(mempool_history_policy, MempoolHistoryPolicy::LegacyRepair)
        && address.mempool_backfill_cursor_txid.is_none()
        && !mempool_history_requires_first_page_restart(address)
        && let Some(last_tip_height) = address.last_tip_height
    {
        let asset_id_for_err = address.asset_id;
        let current_tip = chain_tip_cache.get_or_fetch(
            address.asset_id,
            address.network,
            now_instant,
            now_utc,
            || {
                let client = clients.mempool_client.ok_or_else(|| {
                    UserTransactionMonitorError::Parse(format!(
                        "mempool client unavailable for {} sync",
                        asset_id_for_err.as_str()
                    ))
                })?;
                fetch_mempool_chain_tip(client)
            },
        )?;

        let has_pending_txs = pending_address_ids.contains(&address.address_id);
        allow_known_confirmed_early_exit = !has_pending_txs;

        match decide_tip_unchanged_gate(
            Some(last_tip_height),
            current_tip,
            has_pending_txs,
            address.has_api_confirmed_balance,
        ) {
            TipUnchangedGateDecision::SkipTipUnchanged => {
                tracing::debug!(
                    user_id = %run.user_id,
                    run_id = %run.run_id,
                    address_id = %address.address_id,
                    tip_height = current_tip.value(),
                    reason = "tip_unchanged",
                    "transactions sync: address skipped before fetch"
                );
                accumulator.add_skipped();
                accumulator.add_skipped_tip_unchanged();
                return Ok((false, false));
            }
            TipUnchangedGateDecision::RefreshPending => {
                tracing::debug!(
                    user_id = %run.user_id,
                    run_id = %run.run_id,
                    address_id = %address.address_id,
                    tip_height = current_tip.value(),
                    reason = "tip_unchanged_pending_refresh",
                    "transactions sync: address continuing for pending refresh"
                );
            }
            TipUnchangedGateDecision::ContinueSync => {}
        }
    }

    apply_inter_address_sync_delay(run.clock, &mut accumulator.is_first_attempt, provider);
    *processed_for_account = processed_for_account.saturating_add(1);
    mark_address_sync_started(run.user_id, address.address_id, run.run_id, run.started_at)?;

    let result = executor.sync_one_iteration(AddressSyncExecutionRequest {
        run,
        now_utc,
        now_instant,
        address,
        chain_tip_cache,
        clients,
        single_address_progress,
        allow_known_confirmed_early_exit,
        historical_backfill_enabled,
        legacy_mempool_history_repair: matches!(
            mempool_history_policy,
            MempoolHistoryPolicy::LegacyRepair
        ),
        mempool_history_page_frontier,
    });
    let result = result.and_then(|summary| {
        let success = AddressSyncSuccess {
            address_id: address.address_id,
            run_id: run.run_id,
            started_at: run.started_at,
            completed_at: summary.completed_at,
            last_tip_height: summary.tip_height,
            new_tx_count: summary.new_tx_count,
            updated_tx_count: summary.updated_tx_count,
            api_confirmed_balance: summary.api_confirmed_balance,
        };
        mark_address_sync_completed_success(run.user_id, &success)
            .map_err(|error| preserve_iteration_error(error, &summary))?;
        Ok(summary)
    });
    if let Err(error) = &result
        && let Some(targets) = error.coverage_invalidation()
    {
        let targets = targets.clone();
        crate::db::invalidate_mempool_history_coverage(run.user_id, &targets).map_err(|error| {
            let sanitized = crate::transactions::SyncErrorMessage::sanitize(error.to_string());
            UserTransactionMonitorError::Db(crate::db::DbError::new(sanitized.as_str()))
                .with_coverage_invalidation(targets.clone())
        })?;
        accumulator.mark_accounts_history_unavailable(&targets.account_ids);
    }

    match result {
        Ok(summary) => {
            accumulator.add_synced(&summary);
            if summary.early_exited {
                accumulator.add_early_exited();
            }
            if summary.ledger_rebuild_required
                && let Some(account_id) = address.account_id
            {
                accumulator.mark_account_dirty(account_id);
            }
            address.last_completed_at = Some(summary.completed_at);
            address.last_result = Some(TransactionSyncResult::Success);
            address.last_tip_height = Some(summary.tip_height);
            Ok((summary_has_activity(&summary), false))
        }
        Err(UserTransactionMonitorError::RateLimited {
            integration,
            message,
            retry_after,
        }) => {
            let limited_at = run.clock.instant_now();
            record_rate_limit(run.user_id, &integration, limited_at, retry_after);
            accumulator.add_rate_limited(&integration);
            accumulator.add_skipped();
            let err = UserTransactionMonitorError::RateLimited {
                integration,
                message,
                retry_after,
            };
            let completed_at = run.clock.utc_now();
            mark_sync_failure(run, address.address_id, &err, completed_at);
            address.last_completed_at = Some(completed_at);
            address.last_result = Some(TransactionSyncResult::Failure);
            Ok((false, true))
        }
        Err(err) => {
            tracing::error!(
                user_id = %run.user_id,
                run_id = %run.run_id,
                address_id = %address.address_id,
                error = %err,
                "transactions sync: per-address sync failed, continuing"
            );
            accumulator.add_failed(
                &err,
                matches!(mempool_history_policy, MempoolHistoryPolicy::LegacyRepair),
            );
            let completed_at = run.clock.utc_now();
            mark_sync_failure(run, address.address_id, &err, completed_at);
            address.last_completed_at = Some(completed_at);
            address.last_result = Some(TransactionSyncResult::Failure);
            Ok((false, true))
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::super::{
        ADDRESS_SYNC_COOLDOWN, ETHERSCAN_ADDRESS_SYNC_COOLDOWN, LABEL_ETHERSCAN, LABEL_MEMPOOL,
    };
    use super::*;
    use crate::asset_capabilities::SyncProviderId;
    use crate::db::SyncAddress;
    use crate::transactions::{
        ChainTipHeight, MempoolCursorTxid, TrackedAddress, TransactionCount, TransactionSyncResult,
    };
    use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};

    #[test]
    fn cooldown_for_last_result_uses_longer_window_for_etherscan_assets() {
        let ethereum_cooldown = cooldown_for_last_result(
            SyncedAssetId::Ethereum,
            Some(TransactionSyncResult::Success),
        );
        let bitcoin_cooldown =
            cooldown_for_last_result(SyncedAssetId::Bitcoin, Some(TransactionSyncResult::Success));

        assert_eq!(ethereum_cooldown, Some(ETHERSCAN_ADDRESS_SYNC_COOLDOWN));
        assert_eq!(bitcoin_cooldown, Some(ADDRESS_SYNC_COOLDOWN));
    }

    #[test]
    fn decide_tip_unchanged_gate_skips_when_tip_unchanged_and_no_pending() {
        let tip = ChainTipHeight::try_new(100).expect("tip should be valid");
        let decision = decide_tip_unchanged_gate(Some(tip), tip, false, true);
        assert_eq!(decision, TipUnchangedGateDecision::SkipTipUnchanged);
    }

    #[test]
    fn decide_tip_unchanged_gate_continues_when_api_balance_is_missing() {
        let tip = ChainTipHeight::try_new(100).expect("tip should be valid");
        let decision = decide_tip_unchanged_gate(Some(tip), tip, false, false);
        assert_eq!(decision, TipUnchangedGateDecision::ContinueSync);
    }

    #[test]
    fn decide_tip_unchanged_gate_refreshes_when_tip_unchanged_and_pending_exists() {
        let tip = ChainTipHeight::try_new(200).expect("tip should be valid");
        let decision = decide_tip_unchanged_gate(Some(tip), tip, true, true);
        assert_eq!(decision, TipUnchangedGateDecision::RefreshPending);
    }

    #[test]
    fn decide_tip_unchanged_gate_continues_when_tip_advanced() {
        let last_tip = ChainTipHeight::try_new(300).expect("tip should be valid");
        let current_tip = ChainTipHeight::try_new(301).expect("tip should be valid");
        let decision = decide_tip_unchanged_gate(Some(last_tip), current_tip, false, true);
        assert_eq!(decision, TipUnchangedGateDecision::ContinueSync);
    }

    #[test]
    fn decide_tip_unchanged_gate_continues_when_last_tip_missing() {
        let current_tip = ChainTipHeight::try_new(500).expect("tip should be valid");
        let decision = decide_tip_unchanged_gate(None, current_tip, true, true);
        assert_eq!(decision, TipUnchangedGateDecision::ContinueSync);
    }

    #[test]
    fn should_use_mempool_backfill_lane_covers_first_sync_cursor_and_pending_refresh() {
        let incremental_tip = Some(ChainTipHeight::try_new(1).expect("valid tip"));
        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");

        assert!(should_use_mempool_backfill_lane(None, None, true));
        assert!(should_use_mempool_backfill_lane(
            incremental_tip,
            Some(&cursor),
            true
        ));
        assert!(should_use_mempool_backfill_lane(
            incremental_tip,
            None,
            false
        ));
        assert!(!should_use_mempool_backfill_lane(
            incremental_tip,
            None,
            true
        ));
    }

    #[test]
    fn mempool_history_policy_enforces_normal_cap_per_page() {
        let current_only = MempoolHistoryPolicy::CurrentOnly;
        let zero_cap = MempoolHistoryPolicy::Normal {
            cap: TransactionCount::zero(),
        };
        let normal = MempoolHistoryPolicy::Normal {
            cap: TransactionCount::from_u32(100),
        };
        let repair = MempoolHistoryPolicy::LegacyRepair;

        assert!(!current_only.permits_transaction_page(TransactionCount::zero()));
        assert!(!zero_cap.permits_transaction_page(TransactionCount::zero()));
        assert!(normal.permits_transaction_page(TransactionCount::from_u32(99)));
        assert!(!normal.permits_transaction_page(TransactionCount::from_u32(100)));
        assert!(repair.permits_transaction_page(TransactionCount::from_u32(u32::MAX)));
    }

    #[test]
    fn bitcoin_history_full_resync_policy_is_cap_exempt() {
        assert!(
            MempoolHistoryPolicy::LegacyRepair
                .permits_transaction_page(TransactionCount::from_u32(u32::MAX))
        );
    }

    #[test]
    fn bitcoin_history_full_resync_failure_uses_normal_failure_backoff() {
        let now = Utc::now();
        let mut address = SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse("bc1qrepairfailurebackoff").expect("valid"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_id: None,
            derivation_change: None,
            derivation_index: None,
            address_scheme: None,
            last_completed_at: None,
            last_result: None,
            last_tip_height: None,
            mempool_backfill_cursor_txid: None,
            mempool_expected_tx_count: None,
            mempool_history_proof: None,
            mempool_history_scan_start_run_id: None,
            etherscan_backfill_end_block: None,
            etherscan_history_checkpoint_verified: false,
            has_api_confirmed_balance: false,
            consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::zero(),
        };
        address.last_result = Some(TransactionSyncResult::Failure);
        address.last_completed_at = Some(
            now - chrono::Duration::seconds(FAILED_ADDRESS_SYNC_COOLDOWN.as_secs() as i64 - 1),
        );

        assert!(is_on_cooldown(&address, now, TriggerSource::Schedule));
    }

    #[test]
    fn provider_mapping_returns_expected_for_known_assets() {
        assert_eq!(
            default_api_provider_for_asset(SyncedAssetId::Bitcoin),
            SyncProviderId::MempoolSpace
        );
        assert_eq!(
            default_api_provider_for_asset(SyncedAssetId::Ethereum),
            SyncProviderId::Etherscan
        );
    }

    #[test]
    fn integration_for_address_uses_capability_provider() {
        let bitcoin = SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse("bc1qtest").expect("valid"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_id: None,
            derivation_change: None,
            derivation_index: None,
            address_scheme: None,
            last_completed_at: None,
            last_result: None,
            last_tip_height: None,
            mempool_backfill_cursor_txid: None,
            mempool_expected_tx_count: None,
            mempool_history_proof: None,
            mempool_history_scan_start_run_id: None,
            etherscan_backfill_end_block: None,
            etherscan_history_checkpoint_verified: false,
            has_api_confirmed_balance: false,
            consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::zero(),
        };
        let ethereum = SyncAddress {
            address_id: DigitalAssetAddressId::new(),
            address: TrackedAddress::parse("0x1111111111111111111111111111111111111111")
                .expect("valid"),
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            account_id: None,
            derivation_change: None,
            derivation_index: None,
            address_scheme: None,
            last_completed_at: None,
            last_result: None,
            last_tip_height: None,
            mempool_backfill_cursor_txid: None,
            mempool_expected_tx_count: None,
            mempool_history_proof: None,
            mempool_history_scan_start_run_id: None,
            etherscan_backfill_end_block: None,
            etherscan_history_checkpoint_verified: false,
            has_api_confirmed_balance: false,
            consecutive_failure_count: crate::transactions::ConsecutiveFailureCount::zero(),
        };

        assert_eq!(integration_for_address(&bitcoin), LABEL_MEMPOOL);
        assert_eq!(integration_for_address(&ethereum), LABEL_ETHERSCAN);
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::tasks::jobs::sync::context::SyncClock;
    use crate::tasks::jobs::sync::test_support::{
        FakeAddressSyncExecutor, FakeClock, FakeSyncOutcome, make_run_context, make_sync_address,
        persist_sync_address_for_test, test_utc_now, with_rate_limiter_isolated,
    };
    use crate::transactions::{ChainTipHeight, MempoolCursorTxid};
    use crate::wallets::{Network, SyncedAssetId};
    use std::collections::HashSet;
    use std::time::Instant;

    // Private items from the sync parent module (accessible because tests is a descendant module).
    use super::super::chain_tip::CachedChainTip;
    use super::super::{
        ADDRESS_SYNC_COOLDOWN, ChainTipCache, CycleAccumulator, SyncClients, SyncHttpCounters,
        chain_tip_cache_key, record_rate_limit,
    };

    /// Helper: insert a tip directly into the cache without any DB or network call.
    fn seed_chain_tip_cache(
        cache: &mut ChainTipCache,
        asset_id: SyncedAssetId,
        network: Network,
        height: i64,
        fetched_at: Instant,
    ) {
        let tip = ChainTipHeight::try_new(height).expect("valid tip height in test");
        cache.tips.insert(
            chain_tip_cache_key(asset_id, network),
            CachedChainTip {
                height: tip,
                fetched_at,
            },
        );
    }

    #[test]
    fn sync_single_address_with_controls_skips_on_success_cooldown() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let mut run = make_run_context(&clock);
            run.source = TriggerSource::Schedule;
            let mut address = make_sync_address(
                "0x1111111111111111111111111111111111111111",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - chrono::Duration::seconds(10));
            address.last_result = Some(TransactionSyncResult::Success);
            address.last_tip_height =
                Some(ChainTipHeight::try_new(19_500_000).expect("valid tip height"));
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 1,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
                mempool_history_page_frontier: None,
            })
            .expect("cooldown skip should not error");

            assert_eq!(result, (false, false));
            assert_eq!(accumulator.addresses_skipped, 1);
            assert_eq!(processed_for_account, 0);
            assert!(executor.calls.is_empty());
            assert_eq!(clock.sleep_count(), 0);
        });
    }

    #[test]
    fn sync_single_address_with_controls_manual_trigger_bypasses_success_cooldown() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let mut address = make_sync_address(
                "0x1111111111111111111111111111111111111111",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - chrono::Duration::seconds(10));
            address.last_result = Some(TransactionSyncResult::Success);
            address.last_tip_height =
                Some(ChainTipHeight::try_new(19_500_000).expect("valid tip height"));
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 1,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("manual sync should bypass cooldown");

            assert_eq!(result, (true, false));
            assert_eq!(accumulator.addresses_synced, 1);
            assert_eq!(accumulator.addresses_skipped, 0);
            assert_eq!(processed_for_account, 1);
            assert_eq!(executor.calls.len(), 1);
        });
    }

    #[test]
    fn sync_failure_preserves_dirty_account_for_boundary_rebuild() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qcoveragefailure",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::FailureWithCoverage {
                    message: "post-invalidation persistence failed".to_string(),
                    account_id,
                }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("address-level failures should be accumulated");

            assert_eq!(result, (false, true));
            assert_eq!(accumulator.addresses_failed, 1);
            assert_eq!(accumulator.touched_accounts, HashSet::from([account_id]));
            assert_eq!(
                accumulator.coverage_invalidated_accounts,
                HashSet::from([account_id])
            );
        });
    }

    #[test]
    fn observed_activity_without_reconciliation_does_not_dirty_the_account() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qactivityonly",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::SuccessWithObservedActivity]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("activity-only success should be accumulated");

            assert_eq!(
                result,
                (true, false),
                "observed activity must still drive HD discovery and scheduling"
            );
            assert!(
                accumulator.touched_accounts.is_empty(),
                "observed activity alone must not dirty the ledger"
            );
        });
    }

    #[test]
    fn zero_count_reconciliation_still_dirties_the_account() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qzerocountreconcile",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            // Zero counts, no observed activity, no balance: only the transient
            // reconciliation signal can produce a dirty account here.
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }])
            .with_iteration_ledger_rebuild_required(true);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("zero-count reconciliation should be accumulated");

            assert_eq!(
                accumulator.touched_accounts,
                HashSet::from([account_id]),
                "a non-empty reconciliation must dirty the ledger even at zero counts"
            );
        });
    }

    #[test]
    fn success_persistence_failure_preserves_targets_and_retries_invalidation() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qsuccesspersistencefailure",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);
            crate::db::mark_address_sync_started(
                run.user_id,
                address.address_id,
                run.run_id,
                now - chrono::Duration::seconds(1),
            )
            .expect("sync state should persist");
            crate::db::publish_mempool_history_proof(
                run.user_id,
                address.address_id,
                crate::db::MempoolHistoryProof {
                    confirmed_tx_count: TransactionCount::from_u32(1),
                    complete_height: ChainTipHeight::try_new(1).expect("height should parse"),
                },
            )
            .expect("proof should publish");
            crate::db::with_user_db_mut(run.user_id, |conn| {
                conn.execute_batch(
                    "CREATE TRIGGER test_reject_address_success
                     BEFORE UPDATE OF last_result ON transaction_sync_state
                     WHEN NEW.last_result = 'success'
                     BEGIN
                       SELECT RAISE(ABORT, 'injected address success failure');
                     END;",
                )
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "Failed to install address success failure: {err}"
                    ))
                })
            })
            .expect("address success failure should install");

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::SuccessWithCoverage {
                    account_id,
                }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("success persistence failure should enter normal failure handling");

            assert_eq!(result, (false, true));
            assert_eq!(accumulator.addresses_failed, 1);
            assert_eq!(accumulator.touched_accounts, HashSet::from([account_id]));
            assert_eq!(
                accumulator.coverage_invalidated_accounts,
                HashSet::from([account_id])
            );
            assert_eq!(
                crate::db::get_sync_addresses_for_account(run.user_id, account_id)
                    .expect("address should reload")[0]
                    .mempool_history_proof,
                None
            );
        });
    }

    #[test]
    fn auto_upgrade_bypasses_success_cooldown() {
        let now = test_utc_now();
        let mut address = make_sync_address(
            "0x1111111111111111111111111111111111111111",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        address.last_completed_at = Some(now - chrono::Duration::seconds(10));
        address.last_result = Some(TransactionSyncResult::Success);

        assert!(!is_on_cooldown(&address, now, TriggerSource::AutoUpgrade));
    }

    #[test]
    fn sync_single_address_with_controls_runs_at_cooldown_boundary() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let cooldown = chrono::Duration::from_std(ADDRESS_SYNC_COOLDOWN)
                .expect("cooldown should convert to chrono duration");
            let mut address = make_sync_address(
                "bc1qcooldownboundary",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - cooldown);
            address.last_result = Some(TransactionSyncResult::Success);
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("cooldown boundary should sync");

            assert_eq!(result, (false, false));
            assert_eq!(accumulator.addresses_synced, 1);
            assert_eq!(accumulator.addresses_skipped, 0);
            assert_eq!(processed_for_account, 1);
            assert_eq!(executor.calls.len(), 1);
        });
    }

    #[test]
    fn sync_single_address_retries_failed_balance_after_failed_cooldown() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let failed_cooldown = chrono::Duration::from_std(FAILED_ADDRESS_SYNC_COOLDOWN)
                .expect("failed cooldown should convert to chrono duration");
            let mut address = make_sync_address(
                "bc1qfailedbalancefreshness",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - failed_cooldown);
            address.last_result = Some(TransactionSyncResult::Failure);
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
                mempool_history_page_frontier: None,
            })
            .expect("failed balance refresh should retry after failed cooldown");

            assert_eq!(result, (false, false));
            assert_eq!(accumulator.addresses_synced, 1);
            assert_eq!(accumulator.addresses_skipped, 0);
            assert_eq!(processed_for_account, 1);
            assert_eq!(executor.calls, vec![address.address_id]);
        });
    }

    #[test]
    fn sync_single_address_with_controls_bypasses_success_cooldown_for_backfill() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let mut address = make_sync_address(
                "bc1qmanualbackfillcooldown",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - chrono::Duration::seconds(10));
            address.last_result = Some(TransactionSyncResult::Success);
            address.mempool_backfill_cursor_txid = Some(
                MempoolCursorTxid::parse(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .expect("cursor should parse"),
            );
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("active backfill should bypass cooldown");

            assert_eq!(result, (false, false));
            assert_eq!(accumulator.addresses_synced, 1);
            assert_eq!(accumulator.addresses_skipped, 0);
            assert_eq!(processed_for_account, 1);
            assert_eq!(executor.calls, vec![address.address_id]);
        });
    }

    #[test]
    fn sync_single_address_with_controls_manual_path_retries_failed_balance_after_failed_cooldown()
    {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let failed_cooldown = chrono::Duration::from_std(FAILED_ADDRESS_SYNC_COOLDOWN)
                .expect("failed cooldown should convert to chrono duration");
            let mut address = make_sync_address(
                "bc1qmanualfailedbalancefreshness",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            address.last_completed_at = Some(now - failed_cooldown);
            address.last_result = Some(TransactionSyncResult::Failure);
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::CurrentOnly,
                mempool_history_page_frontier: None,
            })
            .expect("failed balance refresh should retry after failed cooldown");

            assert_eq!(result, (false, false));
            assert_eq!(accumulator.addresses_synced, 1);
            assert_eq!(accumulator.addresses_skipped, 0);
            assert_eq!(processed_for_account, 1);
            assert_eq!(executor.calls, vec![address.address_id]);
        });
    }

    #[test]
    fn sync_single_address_with_controls_short_circuits_on_rate_limit() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let mut address = make_sync_address(
                "0x2222222222222222222222222222222222222222",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            record_rate_limit(run.user_id, LABEL_ETHERSCAN, clock.instant_now(), None);

            let mut chain_tip_cache = ChainTipCache::default();
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 3,
                updated_tx_count: 1,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("rate-limited skip should not error");

            assert_eq!(result, (false, true));
            assert_eq!(accumulator.addresses_skipped, 1);
            assert_eq!(accumulator.rate_limited.len(), 1);
            assert_eq!(processed_for_account, 0);
            assert!(executor.calls.is_empty());
        });
    }

    #[test]
    fn tip_unchanged_gate_skips_bitcoin_when_no_backfill_cursor() {
        let now = test_utc_now();
        let clock = FakeClock::new(now);
        let run = make_run_context(&clock);
        let tip = ChainTipHeight::try_new(900_000).expect("valid tip");

        let mut address = make_sync_address(
            "bc1qunchangedtipnobackfill",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        // Last-seen tip equals current tip; no backfill cursor; no pending txs.
        address.last_tip_height = Some(tip);
        address.mempool_backfill_cursor_txid = None;
        address.has_api_confirmed_balance = true;
        address.last_completed_at =
            Some(now - chrono::Duration::from_std(ADDRESS_SYNC_COOLDOWN * 2).unwrap());
        address.last_result = Some(TransactionSyncResult::Success);

        let mut chain_tip_cache = ChainTipCache::default();
        seed_chain_tip_cache(
            &mut chain_tip_cache,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            tip.value(),
            clock.instant_now(),
        );

        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let mut executor = FakeAddressSyncExecutor::new(vec![]);
        let mut accumulator = CycleAccumulator::new(1);
        let mut processed_for_account = 0_u32;
        let pending_address_ids = HashSet::new();

        let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
            run,
            address: &mut address,
            chain_tip_cache: &mut chain_tip_cache,
            pending_address_ids: &pending_address_ids,
            clients,
            executor: &mut executor,
            accumulator: &mut accumulator,
            processed_for_account: &mut processed_for_account,
            single_address_progress: None,
            mempool_history_policy: MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(1),
            },
            mempool_history_page_frontier: None,
        })
        .expect("tip-unchanged skip should not error");

        assert_eq!(result, (false, false), "address should be skipped");
        assert_eq!(
            accumulator.addresses_skipped, 1,
            "skip counter should be incremented"
        );
        assert_eq!(
            accumulator.addresses_skipped_tip_unchanged, 1,
            "tip-unchanged counter should be incremented"
        );
        assert!(executor.calls.is_empty(), "executor should not be called");
    }

    #[test]
    fn tip_unchanged_gate_refreshes_when_pending_address_is_preloaded() {
        let now = test_utc_now();
        let clock = FakeClock::new(now);
        let run = make_run_context(&clock);
        let tip = ChainTipHeight::try_new(900_000).expect("valid tip");

        let mut address = make_sync_address(
            "bc1qpreloadedpendingtip",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        address.last_tip_height = Some(tip);
        address.last_completed_at =
            Some(now - chrono::Duration::from_std(ADDRESS_SYNC_COOLDOWN * 2).unwrap());
        address.last_result = Some(TransactionSyncResult::Success);
        persist_sync_address_for_test(run, &address);

        let mut chain_tip_cache = ChainTipCache::default();
        seed_chain_tip_cache(
            &mut chain_tip_cache,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            tip.value(),
            clock.instant_now(),
        );

        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
            new_tx_count: 1,
            updated_tx_count: 0,
        }]);
        let mut accumulator = CycleAccumulator::new(1);
        let mut processed_for_account = 0_u32;
        let pending_address_ids = HashSet::from([address.address_id]);

        let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
            run,
            address: &mut address,
            chain_tip_cache: &mut chain_tip_cache,
            pending_address_ids: &pending_address_ids,
            clients,
            executor: &mut executor,
            accumulator: &mut accumulator,
            processed_for_account: &mut processed_for_account,
            single_address_progress: None,
            mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
            mempool_history_page_frontier: None,
        })
        .expect("pending refresh should not error");

        assert_eq!(result, (true, false));
        assert_eq!(accumulator.addresses_skipped, 0);
        assert_eq!(accumulator.addresses_skipped_tip_unchanged, 0);
        assert_eq!(processed_for_account, 1);
        assert_eq!(executor.calls, vec![address.address_id]);
    }

    #[test]
    fn tip_unchanged_gate_does_not_skip_bitcoin_when_backfill_cursor_is_set() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let tip = ChainTipHeight::try_new(900_000).expect("valid tip");

            let mut address = make_sync_address(
                "bc1qunchangedtipwithbackfill",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            );
            // Same tip as last seen, but an active backfill cursor is present.
            address.last_tip_height = Some(tip);
            address.mempool_backfill_cursor_txid = Some(
                MempoolCursorTxid::parse(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .expect("cursor should parse"),
            );
            address.last_completed_at =
                Some(now - chrono::Duration::from_std(ADDRESS_SYNC_COOLDOWN * 2).unwrap());
            address.last_result = Some(TransactionSyncResult::Success);
            persist_sync_address_for_test(run, &address);

            let mut chain_tip_cache = ChainTipCache::default();
            // Pre-seed the same tip so no network call is made.
            seed_chain_tip_cache(
                &mut chain_tip_cache,
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                tip.value(),
                clock.instant_now(),
            );

            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 5,
                updated_tx_count: 0,
            }]);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let pending_address_ids = HashSet::new();

            let result = sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::LegacyRepair,
                mempool_history_page_frontier: None,
            })
            .expect("backfill-cursor bypass should not error");

            assert_eq!(
                accumulator.addresses_skipped, 0,
                "address with active backfill cursor must not be skipped"
            );
            assert_eq!(
                accumulator.addresses_skipped_tip_unchanged, 0,
                "tip-unchanged counter must stay zero when gate is bypassed"
            );
            assert_eq!(
                executor.calls.len(),
                1,
                "executor should be called to continue backfill"
            );
            // 5 new txs -> summary_has_activity is true -> first tuple element is true
            assert_eq!(result, (true, false));
        });
    }
}

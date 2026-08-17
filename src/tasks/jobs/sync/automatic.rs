use crate::asset_capabilities::SyncProviderId;
use crate::auth::session;
use crate::db::{
    AccountSyncBundle, SyncAddress, cleanup_raw_sync_history_with_compaction,
    get_hd_account_sync_bundles, get_non_hd_sync_addresses, load_account_labels,
    load_address_ids_with_activity, load_address_ids_with_pending_txs, load_settings,
    refresh_account_integration_sync_state,
};
use crate::models::{SyncHistoryRetentionDays, UserId};
use crate::payments::types::EntitlementTier;
use crate::tasks::{TriggerSource, publish_transaction_sync_event};
use crate::transactions::{
    AddressCount, AggregateSyncResult, RateLimitedIntegration, SyncErrorMessage, SyncIntegrationId,
    TransactionCount, TransactionSyncEvent, TransactionSyncResult, TransactionSyncRunId,
    TransactionSyncScope,
};
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::{HashMap, HashSet};

use super::account_events::{
    AccountEventContext, publish_account_sync_completed_events, publish_account_sync_failed_events,
    publish_single_address_started_events,
};
use super::chain_tip::ChainTipCache;
use super::context::{
    ADDRESS_FAILURE_THRESHOLD, RunContext, SyncClients, SyncClock, SyncHttpCounters,
    SyncRunPreload, SystemSyncClock, UserTransactionMonitorParams,
    UserTransactionMonitorScheduleHint, UserTransactionMonitorSchedulePolicyInput,
    UserTransactionMonitorSummary, compute_user_transaction_monitor_schedule_hint,
    default_user_transaction_monitor_schedule_hint, is_first_sync,
};
use super::cycle::{
    AccountSyncLogSummary, CycleAccumulator, CycleAccumulatorSnapshot, account_label_for_log,
    log_account_sync_completed,
};
use super::error::UserTransactionMonitorError;
use super::executor::{AddressSyncExecutor, recover_interrupted_mempool_account};
use super::gate::{
    MempoolHistoryPolicy, SyncSingleAddressControlRequest, default_api_provider_for_asset,
    integration_for_asset, load_account_transaction_count_for_history_policy, requires_provider,
    sync_single_address_with_controls,
};
use super::hd_scan::{AddressDerivationProvider, HdBundleScanRequest, run_hd_bundle_scan};
use super::integrations::unfinished_backfill_state;
use super::parent_cycle::{
    IntegrationChildOutcome, IntegrationChildRunner, IntegrationChildSummary,
    LiveIntegrationChildRunner, ParentSyncCycleRequest, ParentSyncCycleResult,
    reduce_parent_result, run_parent_sync_cycle_with_runner,
};
use super::planner::{
    PlannedSyncIteration, SyncPlannerInput, address_is_blocked_for_planner,
    ordered_active_mempool_history_address_ids, plan_next_iteration,
    sort_addresses_by_planner_priority, sort_hd_bundles_by_planner_priority,
};
use super::progress::build_single_address_progress_plan;
use super::rate_limit::{
    earliest_rate_limit_unblock_for_integrations, retry_after_utc_for_integration,
};

#[cfg(all(test, feature = "db-tests"))]
use super::parent_cycle::integration_child_summary_from_summary;

#[cfg(all(test, feature = "db-tests"))]
use super::rate_limit::earliest_rate_limit_unblock_for_user;

fn load_sync_run_preload(user_id: UserId) -> Result<SyncRunPreload, UserTransactionMonitorError> {
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())?;
    Ok(SyncRunPreload {
        settings: load_settings(user_id)?,
        historical_backfill_enabled: entitlements.historical_backfill_enabled,
        historical_backfill_transactions_per_account: entitlements
            .historical_backfill_transactions_per_account,
        account_labels: load_account_labels(user_id)?,
        known_activity_address_ids: load_address_ids_with_activity(user_id)?,
        pending_address_ids: load_address_ids_with_pending_txs(user_id)?,
        bitcoin_history_repair_account_ids: load_pending_bitcoin_history_repair_account_ids(
            user_id,
        )?,
    })
}

fn load_pending_bitcoin_history_repair_account_ids(
    user_id: UserId,
) -> Result<HashSet<DigitalAssetAccountId>, UserTransactionMonitorError> {
    if crate::db::load_user_data_repair_status(
        user_id,
        crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR,
    )? != Some(crate::db::UserDataRepairStatus::Pending)
    {
        return Ok(HashSet::new());
    }
    Ok(
        crate::db::load_unverified_bitcoin_history_repair_account_ids(user_id)?
            .into_iter()
            .collect(),
    )
}

fn mempool_history_policy_for_preload(preload: &SyncRunPreload) -> MempoolHistoryPolicy {
    if !preload.bitcoin_history_repair_account_ids.is_empty() {
        return MempoolHistoryPolicy::LegacyRepair;
    }
    normal_mempool_history_policy_for_preload(preload)
}

fn normal_mempool_history_policy_for_preload(preload: &SyncRunPreload) -> MempoolHistoryPolicy {
    MempoolHistoryPolicy::normal(
        preload.historical_backfill_enabled,
        TransactionCount::from_u32(preload.historical_backfill_transactions_per_account),
    )
}

fn mempool_history_policy_for_account(
    preload: &SyncRunPreload,
    account_id: Option<DigitalAssetAccountId>,
) -> MempoolHistoryPolicy {
    if account_id.is_some_and(|id| preload.bitcoin_history_repair_account_ids.contains(&id)) {
        MempoolHistoryPolicy::LegacyRepair
    } else {
        normal_mempool_history_policy_for_preload(preload)
    }
}

fn default_raw_sync_history_retention_days() -> SyncHistoryRetentionDays {
    SyncHistoryRetentionDays::default()
}

fn saturating_add_len(total: u32, len: usize) -> u32 {
    total.saturating_add(u32::try_from(len).unwrap_or(u32::MAX))
}

pub(super) fn total_sync_address_count(
    non_hd_addresses: &[SyncAddress],
    hd_bundles: &[AccountSyncBundle],
) -> u32 {
    let mut total = u32::try_from(non_hd_addresses.len()).unwrap_or(u32::MAX);
    for bundle in hd_bundles {
        total = saturating_add_len(total, bundle.external_addresses.len());
        total = saturating_add_len(total, bundle.internal_addresses.len());
    }
    total
}

fn total_sync_account_count(
    non_hd_addresses: &[SyncAddress],
    hd_bundles: &[AccountSyncBundle],
) -> u32 {
    let mut account_ids = HashSet::new();
    for address in non_hd_addresses {
        if let Some(account_id) = address.account_id {
            account_ids.insert(account_id);
        }
    }
    for bundle in hd_bundles {
        account_ids.insert(bundle.account_id);
    }
    u32::try_from(account_ids.len()).unwrap_or(u32::MAX)
}

fn planner_input_for_preload<'a>(
    preload: &'a SyncRunPreload,
    now_utc: DateTime<Utc>,
    account_transaction_counts: &'a HashMap<DigitalAssetAccountId, TransactionCount>,
    run_excluded_address_ids: &'a HashSet<crate::wallets::DigitalAssetAddressId>,
) -> SyncPlannerInput<'a> {
    SyncPlannerInput {
        now_utc,
        mempool_history_policy: mempool_history_policy_for_preload(preload),
        account_transaction_counts,
        pending_address_ids: &preload.pending_address_ids,
        known_activity_address_ids: &preload.known_activity_address_ids,
        bitcoin_history_repair_account_ids: &preload.bitcoin_history_repair_account_ids,
        run_excluded_address_ids,
    }
}

fn load_account_transaction_counts_for_workset(
    user_id: UserId,
    non_hd_addresses: &[SyncAddress],
    hd_bundles: &[AccountSyncBundle],
    policy: MempoolHistoryPolicy,
) -> Result<HashMap<DigitalAssetAccountId, TransactionCount>, UserTransactionMonitorError> {
    let mut account_ids = HashSet::new();
    for address in non_hd_addresses {
        if let Some(account_id) = address.account_id {
            account_ids.insert(account_id);
        }
    }
    for bundle in hd_bundles {
        account_ids.insert(bundle.account_id);
    }

    let mut counts = HashMap::new();
    for account_id in account_ids {
        counts.insert(
            account_id,
            load_account_transaction_count_for_history_policy(user_id, account_id, policy)?,
        );
    }
    Ok(counts)
}

struct HdMempoolHistoryBreadthRoundRequest<'a> {
    run: RunContext<'a>,
    clients: SyncClients<'a>,
    pending_address_ids: &'a HashSet<crate::wallets::DigitalAssetAddressId>,
    bundle: &'a mut AccountSyncBundle,
    known_activity: &'a HashSet<crate::wallets::DigitalAssetAddressId>,
    chain_tip_cache: &'a mut ChainTipCache,
    accumulator: &'a mut CycleAccumulator,
    sync_executor: &'a mut dyn AddressSyncExecutor,
    policy: MempoolHistoryPolicy,
}

fn run_hd_mempool_history_breadth_round(
    request: HdMempoolHistoryBreadthRoundRequest<'_>,
) -> Result<(bool, HashSet<crate::wallets::DigitalAssetAddressId>), UserTransactionMonitorError> {
    let HdMempoolHistoryBreadthRoundRequest {
        run,
        clients,
        pending_address_ids,
        bundle,
        known_activity,
        chain_tip_cache,
        accumulator,
        sync_executor,
        policy,
    } = request;
    let mut completed_address_ids = HashSet::new();
    if default_api_provider_for_asset(bundle.asset_id) != SyncProviderId::MempoolSpace {
        return Ok((false, completed_address_ids));
    }
    let ordered_address_ids = ordered_active_mempool_history_address_ids(
        bundle
            .external_addresses
            .iter()
            .chain(bundle.internal_addresses.iter()),
        known_activity,
        bundle
            .sync_state
            .as_ref()
            .and_then(|state| state.mempool_history_next_address_id),
    );
    let mut processed_for_account = 0_u32;

    for (index, address_id) in ordered_address_ids.iter().copied().enumerate() {
        if processed_for_account >= super::MAX_ADDRESSES_PER_ACCOUNT_PER_RUN {
            return Ok((true, completed_address_ids));
        }
        let processed_before = processed_for_account;
        let next_address_id = ordered_address_ids.get(index.saturating_add(1)).copied();
        let address = bundle
            .external_addresses
            .iter_mut()
            .chain(bundle.internal_addresses.iter_mut())
            .find(|address| address.address_id == address_id)
            .ok_or_else(|| {
                UserTransactionMonitorError::Parse(
                    "HD mempool history address missing from account bundle".to_string(),
                )
            })?;
        let (_, interrupted) =
            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address,
                chain_tip_cache,
                pending_address_ids,
                clients,
                executor: sync_executor,
                accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: policy,
                mempool_history_page_frontier: Some(crate::db::HdMempoolHistoryFrontierUpdate {
                    account_id: bundle.account_id,
                    next_address_id,
                }),
            })?;
        if processed_for_account > processed_before {
            completed_address_ids.insert(address_id);
        }
        if interrupted || matches!(address.last_result, Some(TransactionSyncResult::Failure)) {
            return Ok((true, completed_address_ids));
        }
        if processed_for_account == processed_before {
            return Ok((false, completed_address_ids));
        }
        let stored_count = load_account_transaction_count_for_history_policy(
            run.user_id,
            bundle.account_id,
            policy,
        )?;
        if !policy.permits_transaction_page(stored_count) {
            return Ok((false, completed_address_ids));
        }
    }

    Ok((false, completed_address_ids))
}

pub(super) struct SyncCycleRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) clients: SyncClients<'a>,
    pub(super) preload: &'a SyncRunPreload,
    pub(super) non_hd_addresses: Vec<SyncAddress>,
    pub(super) hd_bundles: Vec<AccountSyncBundle>,
    pub(super) known_activity: HashSet<crate::wallets::DigitalAssetAddressId>,
    pub(super) sync_executor: &'a mut dyn AddressSyncExecutor,
    pub(super) derivation_provider: &'a mut dyn AddressDerivationProvider,
}

pub(super) fn run_sync_cycle(
    request: SyncCycleRequest<'_>,
) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
    let SyncCycleRequest {
        run,
        clients,
        preload,
        mut non_hd_addresses,
        mut hd_bundles,
        mut known_activity,
        sync_executor,
        derivation_provider,
    } = request;
    let mempool_history_policy = mempool_history_policy_for_preload(preload);
    let account_transaction_counts = load_account_transaction_counts_for_workset(
        run.user_id,
        &non_hd_addresses,
        &hd_bundles,
        mempool_history_policy,
    )?;
    let run_excluded_address_ids = HashSet::new();
    let planner_input = planner_input_for_preload(
        preload,
        run.clock.utc_now(),
        &account_transaction_counts,
        &run_excluded_address_ids,
    );
    sort_addresses_by_planner_priority(&mut non_hd_addresses, &planner_input);
    sort_hd_bundles_by_planner_priority(&mut hd_bundles, &planner_input);
    let planned_iteration = plan_next_iteration(&non_hd_addresses, &hd_bundles, &planner_input);
    let total_addresses = total_sync_address_count(&non_hd_addresses, &hd_bundles);
    let mut accumulator = CycleAccumulator::new(total_addresses);
    if let PlannedSyncIteration::Stop { reason } = planned_iteration {
        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            reason = ?reason,
            "transactions sync: planner stopped automatic cycle"
        );
        return Ok(accumulator.into_summary(run.run_id));
    }
    let mut chain_tip_cache = ChainTipCache::default();
    let mut processed_non_hd = 0_u32;
    let mut non_hd_account_summaries =
        HashMap::<DigitalAssetAccountId, AccountSyncLogSummary>::new();
    let mut non_hd_account_order = Vec::<DigitalAssetAccountId>::new();

    for address in &mut non_hd_addresses {
        if address_is_blocked_for_planner(address, &planner_input) {
            accumulator.add_skipped();
            continue;
        }

        let account_id = address.account_id;
        let account_mempool_history_policy =
            mempool_history_policy_for_account(preload, account_id);
        let asset_id = address.asset_id;
        let network = address.network;
        let single_address_progress = build_single_address_progress_plan(
            run,
            address,
            clients,
            preload.historical_backfill_enabled,
        );
        if let Some(progress) = single_address_progress {
            let started_at_utc = run.clock.utc_now();
            let start = crate::db::mark_account_integration_sync_started(
                run.user_id,
                progress.account_id,
                SyncIntegrationId::for_asset(asset_id),
                started_at_utc,
            )?;
            let recovery = recover_interrupted_mempool_account(
                run.user_id,
                progress.account_id,
                asset_id,
                start,
            )?;
            accumulator.mark_accounts_history_unavailable(&recovery.account_ids);
            crate::db::debug_assert_user_db_unlocked(run.user_id, "single-address start publish");
            let integration_id_for_start = SyncIntegrationId::for_asset(asset_id);
            publish_single_address_started_events(
                &AccountEventContext {
                    user_id: run.user_id,
                    run_id: run.run_id,
                    completed_at_utc: started_at_utc,
                    account_id: progress.account_id,
                    integration_id: integration_id_for_start,
                },
                started_at_utc,
                progress.is_first_sync,
                progress.expected_tx_count,
                progress.expected_tx_count_is_lower_bound,
            );
        }
        let started_at = run.clock.instant_now();
        let counters_before = CycleAccumulatorSnapshot::from_accumulator(&accumulator);
        let (_has_activity, interrupted) =
            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &preload.pending_address_ids,
                clients,
                executor: sync_executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_non_hd,
                single_address_progress,
                mempool_history_policy: account_mempool_history_policy,
                mempool_history_page_frontier: None,
            })?;
        let completed_at = run.clock.instant_now();
        let counters_after = CycleAccumulatorSnapshot::from_accumulator(&accumulator);
        let delta = counters_after.delta_from(counters_before);
        let aborted_by_rate_limit = interrupted
            && accumulator
                .rate_limited
                .contains(integration_for_asset(asset_id));

        if let Some(account_id) = account_id {
            let completed_at_utc = run.clock.utc_now();
            accumulator.rebuild_account_if_touched(run.user_id, account_id, completed_at_utc)?;
            refresh_account_integration_sync_state(
                run.user_id,
                account_id,
                SyncIntegrationId::for_asset(asset_id),
                completed_at_utc,
            )?;
            let integration_id_for_event = SyncIntegrationId::for_asset(asset_id);
            if aborted_by_rate_limit || delta.addresses_failed > 0 {
                let integration = integration_for_asset(asset_id).to_string();
                let error = if aborted_by_rate_limit {
                    SyncErrorMessage::sanitize(format!(
                        "Rate limit reached for integration {integration}"
                    ))
                } else {
                    accumulator
                        .failure_error
                        .clone()
                        .unwrap_or_else(|| SyncErrorMessage::sanitize("Account sync failed"))
                };
                let rate_limited = if aborted_by_rate_limit {
                    vec![RateLimitedIntegration {
                        integration: integration.clone(),
                    }]
                } else {
                    Vec::new()
                };
                let retry_after_utc = if aborted_by_rate_limit {
                    retry_after_utc_for_integration(
                        run.user_id,
                        integration_id_for_event.as_db_value(),
                        run.clock.instant_now(),
                        completed_at_utc,
                    )
                } else {
                    None
                };
                crate::db::debug_assert_user_db_unlocked(
                    run.user_id,
                    "single-address failure publish",
                );
                publish_account_sync_failed_events(
                    &AccountEventContext {
                        user_id: run.user_id,
                        run_id: run.run_id,
                        completed_at_utc,
                        account_id,
                        integration_id: integration_id_for_event,
                    },
                    error,
                    rate_limited,
                    retry_after_utc,
                );
            } else if delta.addresses_synced + delta.addresses_skipped > 0 {
                crate::db::debug_assert_user_db_unlocked(
                    run.user_id,
                    "single-address completion publish",
                );
                publish_account_sync_completed_events(
                    &AccountEventContext {
                        user_id: run.user_id,
                        run_id: run.run_id,
                        completed_at_utc,
                        account_id,
                        integration_id: integration_id_for_event,
                    },
                    TransactionCount::from_u32(delta.new_tx_count),
                    TransactionCount::from_u32(delta.updated_tx_count),
                );
            }
        }

        if let Some(account_id) = account_id {
            match non_hd_account_summaries.get_mut(&account_id) {
                Some(existing) => existing.apply_delta(started_at, completed_at, delta),
                None => {
                    non_hd_account_order.push(account_id);
                    non_hd_account_summaries.insert(
                        account_id,
                        AccountSyncLogSummary::from_first_delta(
                            account_id,
                            account_label_for_log(&preload.account_labels, account_id),
                            asset_id,
                            network,
                            started_at,
                            completed_at,
                            delta,
                        ),
                    );
                }
            }
        }
    }

    for account_id in accumulator.touched_accounts.clone() {
        accumulator.rebuild_account_if_touched(run.user_id, account_id, run.clock.utc_now())?;
    }

    for account_id in non_hd_account_order {
        if let Some(summary) = non_hd_account_summaries.get(&account_id) {
            log_account_sync_completed(run, summary);
        }
    }

    for mut bundle in hd_bundles {
        tracing::info!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            account_id = %bundle.account_id,
            asset_id = %bundle.asset_id.as_str(),
            network = %bundle.network.as_str(),
            address_scheme = %bundle.address_scheme.as_str(),
            hd_key_len = bundle.hd_key_extended_pubkey.len(),
            external_addresses = bundle.external_addresses.len(),
            internal_addresses = bundle.internal_addresses.len(),
            "transactions sync: scanning HD account"
        );
        let started_at = run.clock.instant_now();
        let counters_before = CycleAccumulatorSnapshot::from_accumulator(&accumulator);
        let account_id = bundle.account_id;
        let account_mempool_history_policy =
            mempool_history_policy_for_account(preload, Some(account_id));
        let asset_id = bundle.asset_id;
        let network = bundle.network;
        let start = crate::db::mark_account_integration_sync_started(
            run.user_id,
            account_id,
            SyncIntegrationId::for_asset(asset_id),
            run.clock.utc_now(),
        )?;
        let recovery =
            recover_interrupted_mempool_account(run.user_id, account_id, asset_id, start)?;
        accumulator.mark_accounts_history_unavailable(&recovery.account_ids);
        let (history_interrupted, completed_address_ids) =
            run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                run,
                clients,
                pending_address_ids: &preload.pending_address_ids,
                bundle: &mut bundle,
                known_activity: &known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor,
                policy: account_mempool_history_policy,
            })?;
        if !history_interrupted {
            run_hd_bundle_scan(HdBundleScanRequest {
                run,
                clients,
                pending_address_ids: &preload.pending_address_ids,
                bundle,
                completed_address_ids,
                known_activity: &mut known_activity,
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor,
                derivation_provider,
                historical_backfill_enabled: matches!(
                    account_mempool_history_policy,
                    MempoolHistoryPolicy::LegacyRepair
                ),
            })?;
        }
        let completed_at = run.clock.instant_now();
        let counters_after = CycleAccumulatorSnapshot::from_accumulator(&accumulator);
        let delta = counters_after.delta_from(counters_before);
        let mut summary = AccountSyncLogSummary::from_first_delta(
            account_id,
            account_label_for_log(&preload.account_labels, account_id),
            asset_id,
            network,
            started_at,
            completed_at,
            delta,
        );
        summary.is_hd = true;
        summary.addresses_derived = delta.addresses_total;
        summary.hd_keys_scanned = delta
            .addresses_synced
            .saturating_add(delta.addresses_skipped)
            .saturating_add(delta.addresses_failed);
        let completed_at_utc = run.clock.utc_now();
        accumulator.rebuild_account_if_touched(run.user_id, account_id, completed_at_utc)?;
        refresh_account_integration_sync_state(
            run.user_id,
            account_id,
            SyncIntegrationId::for_asset(asset_id),
            completed_at_utc,
        )?;
        let hd_integration_id = SyncIntegrationId::for_asset(asset_id);
        if delta.addresses_failed > 0 && delta.addresses_synced + delta.addresses_skipped == 0 {
            let error = accumulator
                .failure_error
                .clone()
                .unwrap_or_else(|| SyncErrorMessage::sanitize("Account sync failed"));
            crate::db::debug_assert_user_db_unlocked(run.user_id, "hd account failure publish");
            publish_account_sync_failed_events(
                &AccountEventContext {
                    user_id: run.user_id,
                    run_id: run.run_id,
                    completed_at_utc,
                    account_id,
                    integration_id: hd_integration_id,
                },
                error,
                Vec::new(),
                None,
            );
        } else if delta.addresses_synced + delta.addresses_skipped + delta.addresses_failed > 0 {
            crate::db::debug_assert_user_db_unlocked(run.user_id, "hd account completion publish");
            publish_account_sync_completed_events(
                &AccountEventContext {
                    user_id: run.user_id,
                    run_id: run.run_id,
                    completed_at_utc,
                    account_id,
                    integration_id: hd_integration_id,
                },
                TransactionCount::from_u32(delta.new_tx_count),
                TransactionCount::from_u32(delta.updated_tx_count),
            );
        }
        log_account_sync_completed(run, &summary);
    }

    Ok(accumulator.into_summary(run.run_id))
}

fn user_has_unexpired_session(user_id: UserId, now: DateTime<Utc>) -> bool {
    match session::list_users_with_unexpired_sessions_at(now) {
        Ok(users) => users.contains(&user_id),
        Err(err) => {
            tracing::warn!(
                user_id = %user_id,
                error = %err,
                "transactions sync: unable to resolve active sessions; skipping sync"
            );
            false
        }
    }
}

pub(super) fn empty_sync_summary(
    run_id: TransactionSyncRunId,
    total_addresses: u32,
) -> UserTransactionMonitorSummary {
    UserTransactionMonitorSummary {
        run_id,
        new_tx_count: TransactionCount::zero(),
        updated_tx_count: TransactionCount::zero(),
        addresses_total: AddressCount::from_u32(total_addresses),
        addresses_synced: AddressCount::zero(),
        addresses_failed: AddressCount::zero(),
        addresses_skipped: AddressCount::zero(),
        addresses_skipped_tip_unchanged: AddressCount::zero(),
        addresses_early_exited: AddressCount::zero(),
        pagination_cache_hits: 0,
        total_api_calls: 0,
        rate_limited: Vec::new(),
        failure_error: None,
        bitcoin_history_repair_failure_error: None,
        schedule_hint: default_user_transaction_monitor_schedule_hint(),
    }
}

fn address_has_unfinished_work(
    address: &SyncAddress,
    bitcoin_history_repair_account_ids: &HashSet<DigitalAssetAccountId>,
    mempool_history_page_permitted: bool,
) -> bool {
    let repair_owned = address
        .account_id
        .is_some_and(|account_id| bitcoin_history_repair_account_ids.contains(&account_id));
    let has_unfinished_backfill = match default_api_provider_for_asset(address.asset_id) {
        SyncProviderId::MempoolSpace => {
            mempool_history_page_permitted && unfinished_backfill_state(address).is_some()
        }
        SyncProviderId::Etherscan => unfinished_backfill_state(address).is_some(),
    };

    (matches!(address.last_result, Some(TransactionSyncResult::Failure))
        && (repair_owned || address.consecutive_failure_count.value() < ADDRESS_FAILURE_THRESHOLD))
        || is_first_sync(address.last_tip_height)
        || has_unfinished_backfill
}

fn scope_matches_address(scope: TransactionSyncScope, address: &SyncAddress) -> bool {
    match scope {
        TransactionSyncScope::User => true,
        TransactionSyncScope::Account { account_id } => address.account_id == Some(account_id),
        TransactionSyncScope::Address { address_id } => address.address_id == address_id,
    }
}

fn filter_non_hd_addresses_for_scope(
    addresses: Vec<SyncAddress>,
    scope: TransactionSyncScope,
) -> Vec<SyncAddress> {
    addresses
        .into_iter()
        .filter(|address| scope_matches_address(scope, address))
        .collect()
}

fn filter_hd_bundles_for_scope(
    bundles: Vec<AccountSyncBundle>,
    scope: TransactionSyncScope,
) -> Vec<AccountSyncBundle> {
    match scope {
        TransactionSyncScope::User => bundles,
        TransactionSyncScope::Account { account_id } => bundles
            .into_iter()
            .filter(|bundle| bundle.account_id == account_id)
            .collect(),
        TransactionSyncScope::Address { address_id } => bundles
            .into_iter()
            .filter_map(|mut bundle| {
                bundle
                    .external_addresses
                    .retain(|address| address.address_id == address_id);
                bundle
                    .internal_addresses
                    .retain(|address| address.address_id == address_id);
                if bundle.external_addresses.is_empty() && bundle.internal_addresses.is_empty() {
                    None
                } else {
                    Some(bundle)
                }
            })
            .collect(),
    }
}

fn load_active_native_accounts_for_entitlements(
    user_id: UserId,
    entitlements: &crate::payments::types::FeatureEntitlements,
) -> Result<HashSet<DigitalAssetAccountId>, UserTransactionMonitorError> {
    Ok(
        crate::db::account_limits::sync_eligible_native_account_ids_for_user(
            user_id,
            usize::from(entitlements.sync_account_slots_limit),
            entitlements.tier == EntitlementTier::Free,
        )?,
    )
}

fn load_active_native_accounts(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<HashSet<DigitalAssetAccountId>, UserTransactionMonitorError> {
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)?;
    load_active_native_accounts_for_entitlements(user_id, &entitlements)
}

fn filter_non_hd_addresses_for_active_native_accounts(
    addresses: Vec<SyncAddress>,
    active_accounts: &HashSet<DigitalAssetAccountId>,
) -> Vec<SyncAddress> {
    addresses
        .into_iter()
        .filter(|address| {
            address
                .account_id
                .is_some_and(|account_id| active_accounts.contains(&account_id))
        })
        .collect()
}

fn filter_hd_bundles_for_active_native_accounts(
    bundles: Vec<AccountSyncBundle>,
    active_accounts: &HashSet<DigitalAssetAccountId>,
) -> Vec<AccountSyncBundle> {
    bundles
        .into_iter()
        .filter(|bundle| active_accounts.contains(&bundle.account_id))
        .collect()
}

fn reload_has_unfinished_sync_work(user_id: UserId) -> Result<bool, UserTransactionMonitorError> {
    Ok(!reload_unfinished_sync_integrations(user_id)?.is_empty())
}

fn load_mempool_history_page_permission(
    user_id: UserId,
    address: &SyncAddress,
    bitcoin_history_repair_account_ids: &HashSet<DigitalAssetAccountId>,
    normal_policy: MempoolHistoryPolicy,
    stored_counts: &mut HashMap<DigitalAssetAccountId, TransactionCount>,
) -> Result<bool, UserTransactionMonitorError> {
    if default_api_provider_for_asset(address.asset_id) != SyncProviderId::MempoolSpace
        || unfinished_backfill_state(address).is_none()
    {
        return Ok(false);
    }
    let Some(account_id) = address.account_id else {
        return Ok(false);
    };
    if bitcoin_history_repair_account_ids.contains(&account_id) {
        return Ok(true);
    }

    let stored_count = match stored_counts.get(&account_id).copied() {
        Some(stored_count) => stored_count,
        None => {
            let stored_count = load_account_transaction_count_for_history_policy(
                user_id,
                account_id,
                normal_policy,
            )?;
            stored_counts.insert(account_id, stored_count);
            stored_count
        }
    };
    Ok(normal_policy.permits_transaction_page(stored_count))
}

fn reload_unfinished_sync_integrations(
    user_id: UserId,
) -> Result<HashSet<SyncIntegrationId>, UserTransactionMonitorError> {
    let now = Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)?;
    let mut active_accounts = load_active_native_accounts_for_entitlements(user_id, &entitlements)?;
    let normal_policy = MempoolHistoryPolicy::normal(
        entitlements.historical_backfill_enabled,
        TransactionCount::from_u32(entitlements.historical_backfill_transactions_per_account),
    );
    let bitcoin_history_repair_account_ids =
        load_pending_bitcoin_history_repair_account_ids(user_id)?;
    active_accounts.extend(bitcoin_history_repair_account_ids.iter().copied());
    let non_hd_addresses = filter_non_hd_addresses_for_active_native_accounts(
        get_non_hd_sync_addresses(user_id)?,
        &active_accounts,
    );
    let hd_bundles = filter_hd_bundles_for_active_native_accounts(
        get_hd_account_sync_bundles(user_id)?,
        &active_accounts,
    );
    let mut integrations = HashSet::new();
    let mut stored_counts = HashMap::new();

    for address in &non_hd_addresses {
        let page_permitted = load_mempool_history_page_permission(
            user_id,
            address,
            &bitcoin_history_repair_account_ids,
            normal_policy,
            &mut stored_counts,
        )?;
        if address_has_unfinished_work(address, &bitcoin_history_repair_account_ids, page_permitted)
        {
            integrations.insert(SyncIntegrationId::for_asset(address.asset_id));
        }
    }

    for bundle in &hd_bundles {
        for address in bundle
            .external_addresses
            .iter()
            .chain(bundle.internal_addresses.iter())
        {
            let page_permitted = load_mempool_history_page_permission(
                user_id,
                address,
                &bitcoin_history_repair_account_ids,
                normal_policy,
                &mut stored_counts,
            )?;
            if address_has_unfinished_work(
                address,
                &bitcoin_history_repair_account_ids,
                page_permitted,
            ) {
                integrations.insert(SyncIntegrationId::for_asset(bundle.asset_id));
                break;
            }
        }
    }

    Ok(integrations)
}

#[cfg(all(test, feature = "db-tests"))]
fn schedule_hint_for_run(
    run: RunContext<'_>,
    source: TriggerSource,
    summary: &UserTransactionMonitorSummary,
    has_unfinished_work: bool,
) -> UserTransactionMonitorScheduleHint {
    let blocked_for = earliest_rate_limit_unblock_for_user(run.user_id, run.clock.instant_now())
        .map(|blocked_until| blocked_until.saturating_duration_since(run.clock.instant_now()));
    let is_idle = !has_unfinished_work
        && summary.new_tx_count.value() == 0
        && summary.updated_tx_count.value() == 0
        && summary.addresses_failed.value() == 0;

    compute_user_transaction_monitor_schedule_hint(UserTransactionMonitorSchedulePolicyInput {
        source,
        has_unfinished_work,
        is_idle,
        blocked_for,
    })
}

fn schedule_hint_for_parent_run(
    run: RunContext<'_>,
    source: TriggerSource,
    summary: &UserTransactionMonitorSummary,
    child_summaries: &[IntegrationChildSummary],
    unfinished_integrations: &HashSet<SyncIntegrationId>,
) -> UserTransactionMonitorScheduleHint {
    let blocked_integrations = child_summaries
        .iter()
        .filter(|child| matches!(child.outcome, IntegrationChildOutcome::Blocked))
        .map(|child| child.integration_id)
        .collect::<HashSet<_>>();
    let has_unfinished_work = !unfinished_integrations.is_empty();
    let has_unblocked_unfinished_work = unfinished_integrations
        .iter()
        .any(|integration_id| !blocked_integrations.contains(integration_id));
    let blocked_for = if blocked_integrations.is_empty() || has_unblocked_unfinished_work {
        None
    } else {
        earliest_rate_limit_unblock_for_integrations(
            run.user_id,
            run.clock.instant_now(),
            &blocked_integrations,
        )
        .map(|blocked_until| blocked_until.saturating_duration_since(run.clock.instant_now()))
    };
    let is_idle = !has_unfinished_work
        && summary.new_tx_count.value() == 0
        && summary.updated_tx_count.value() == 0
        && summary.addresses_failed.value() == 0;

    compute_user_transaction_monitor_schedule_hint(UserTransactionMonitorSchedulePolicyInput {
        source,
        has_unfinished_work,
        is_idle,
        blocked_for,
    })
}

#[derive(Clone, Copy, Default)]
struct ChildCompletionCounts {
    total: usize,
    success: usize,
    blocked: usize,
    failed: usize,
}

#[derive(Clone, Copy)]
struct SyncCompletionContext<'a> {
    run: RunContext<'a>,
    source: TriggerSource,
    scope: TransactionSyncScope,
    completed_at: DateTime<Utc>,
    aggregate_result: AggregateSyncResult,
    children: ChildCompletionCounts,
}

fn publish_and_log_sync_completed(
    context: SyncCompletionContext<'_>,
    summary: &UserTransactionMonitorSummary,
) {
    crate::db::debug_assert_user_db_unlocked(context.run.user_id, "parent sync completion publish");
    publish_transaction_sync_event(
        context.run.user_id,
        TransactionSyncEvent::sync_completed(
            context.run.run_id,
            context.completed_at,
            summary.new_tx_count,
            summary.updated_tx_count,
            summary.addresses_synced,
            summary.addresses_failed,
            summary.addresses_skipped,
            summary.rate_limited.clone(),
        ),
    );

    tracing::info!(
        user_id = %context.run.user_id,
        run_id = %context.run.run_id,
        source = ?context.source,
        scope = ?context.scope,
        addresses_total = summary.addresses_total.value(),
        addresses_synced = summary.addresses_synced.value(),
        addresses_failed = summary.addresses_failed.value(),
        addresses_skipped = summary.addresses_skipped.value(),
        addresses_skipped_tip_unchanged = summary.addresses_skipped_tip_unchanged.value(),
        addresses_early_exited = summary.addresses_early_exited.value(),
        pagination_cache_hits = summary.pagination_cache_hits,
        total_api_calls = summary.total_api_calls,
        rate_limited_integrations = summary.rate_limited.len(),
        new_tx_count = summary.new_tx_count.value(),
        updated_tx_count = summary.updated_tx_count.value(),
        child_workers_total = context.children.total,
        child_workers_success = context.children.success,
        child_workers_blocked = context.children.blocked,
        child_workers_failed = context.children.failed,
        aggregate_result = ?context.aggregate_result,
        next_due_in_seconds = summary.schedule_hint.interval.as_secs(),
        next_due_reason = ?summary.schedule_hint.reason,
        schedule_urgency = ?summary.schedule_hint.urgency,
        next_due_at_utc = ?summary.schedule_hint.next_due_at_utc(context.completed_at),
        "sync_completed"
    );
}

struct AutomaticRunFailure {
    error: UserTransactionMonitorError,
    message: SyncErrorMessage,
    addresses_failed: AddressCount,
}

struct AutomaticRunSuccess {
    summary: UserTransactionMonitorSummary,
    completed_at: DateTime<Utc>,
    aggregate_result: AggregateSyncResult,
    children: ChildCompletionCounts,
}

impl From<UserTransactionMonitorError> for AutomaticRunFailure {
    fn from(error: UserTransactionMonitorError) -> Self {
        let message = SyncErrorMessage::sanitize(error.to_string());
        Self {
            error,
            message,
            addresses_failed: AddressCount::zero(),
        }
    }
}

impl From<crate::db::DbError> for AutomaticRunFailure {
    fn from(error: crate::db::DbError) -> Self {
        UserTransactionMonitorError::from(error).into()
    }
}

fn finish_automatic_run(
    run: RunContext<'_>,
    source: TriggerSource,
    scope: TransactionSyncScope,
    result: Result<AutomaticRunSuccess, AutomaticRunFailure>,
) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
    match result {
        Ok(success) => {
            publish_and_log_sync_completed(
                SyncCompletionContext {
                    run,
                    source,
                    scope,
                    completed_at: success.completed_at,
                    aggregate_result: success.aggregate_result,
                    children: success.children,
                },
                &success.summary,
            );
            Ok(success.summary)
        }
        Err(failure) => {
            crate::db::debug_assert_user_db_unlocked(run.user_id, "parent sync failure publish");
            publish_transaction_sync_event(
                run.user_id,
                TransactionSyncEvent::sync_failed(
                    run.run_id,
                    run.clock.utc_now(),
                    failure.message,
                    failure.addresses_failed,
                ),
            );
            Err(failure.error)
        }
    }
}

fn run_started_automatic_with(
    run: RunContext<'_>,
    source: TriggerSource,
    scope: TransactionSyncScope,
    execute: impl FnOnce(
        RunContext<'_>,
        TriggerSource,
        TransactionSyncScope,
    ) -> Result<AutomaticRunSuccess, AutomaticRunFailure>,
) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
    crate::db::debug_assert_user_db_unlocked(run.user_id, "parent sync start publish");
    publish_transaction_sync_event(
        run.user_id,
        TransactionSyncEvent::sync_started(run.run_id, run.started_at),
    );
    finish_automatic_run(run, source, scope, execute(run, source, scope))
}

pub(crate) fn run(
    user_id: UserId,
    source: TriggerSource,
    params: UserTransactionMonitorParams,
) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
    let clock = SystemSyncClock;
    let run_id = params.run_id;
    let started_at = clock.utc_now();
    let run = RunContext {
        user_id,
        run_id,
        source,
        started_at,
        clock: &clock,
    };
    let scope = params.scope;
    tracing::debug!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        source = ?source,
        scope = ?scope,
        "tasks: user transaction monitor started"
    );

    if !user_has_unexpired_session(run.user_id, run.started_at) {
        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            source = ?source,
            scope = ?scope,
            "tasks: user transaction monitor skipped (no active session)"
        );
        let mut summary = empty_sync_summary(run.run_id, 0);
        summary.schedule_hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source,
                has_unfinished_work: false,
                is_idle: true,
                blocked_for: None,
            },
        );
        return Ok(summary);
    }

    run_started_automatic_with(run, source, scope, run_automatic_inner)
}

fn run_automatic_inner(
    run: RunContext<'_>,
    source: TriggerSource,
    scope: TransactionSyncScope,
) -> Result<AutomaticRunSuccess, AutomaticRunFailure> {
    run_automatic_inner_with_runner(run, source, scope, &LiveIntegrationChildRunner)
}

fn run_automatic_inner_with_runner(
    run: RunContext<'_>,
    source: TriggerSource,
    scope: TransactionSyncScope,
    child_runner: &dyn IntegrationChildRunner,
) -> Result<AutomaticRunSuccess, AutomaticRunFailure> {
    let mut user_sync_lease =
        Some(crate::sync_execution_lease::UserSyncExecutionLease::acquire(run.user_id));
    let http_counters = SyncHttpCounters::new();

    let preload = load_sync_run_preload(run.user_id)?;
    if preload.bitcoin_history_repair_account_ids.is_empty() {
        drop(user_sync_lease.take());
    }
    let _repair_user_sync_lease = user_sync_lease;
    let mut active_accounts = load_active_native_accounts(run.user_id, run.started_at)?;
    active_accounts.extend(preload.bitcoin_history_repair_account_ids.iter().copied());
    let scoped_non_hd =
        filter_non_hd_addresses_for_scope(get_non_hd_sync_addresses(run.user_id)?, scope);
    let scoped_hd = filter_hd_bundles_for_scope(get_hd_account_sync_bundles(run.user_id)?, scope);
    if scope != TransactionSyncScope::User
        && scoped_non_hd
            .iter()
            .filter_map(|address| address.account_id)
            .chain(scoped_hd.iter().map(|bundle| bundle.account_id))
            .any(|account_id| {
                preload
                    .bitcoin_history_repair_account_ids
                    .contains(&account_id)
            })
    {
        return Err(
            crate::db::DbError::new("Bitcoin history correctness repair is in progress").into(),
        );
    }
    let non_hd_addresses =
        filter_non_hd_addresses_for_active_native_accounts(scoped_non_hd, &active_accounts);
    let hd_bundles = filter_hd_bundles_for_active_native_accounts(scoped_hd, &active_accounts);
    let total_addresses = total_sync_address_count(&non_hd_addresses, &hd_bundles);
    let accounts_total = total_sync_account_count(&non_hd_addresses, &hd_bundles);
    let hd_accounts = u32::try_from(hd_bundles.len()).unwrap_or(u32::MAX);

    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        source = ?source,
        scope = ?scope,
        accounts_total,
        addresses_total = total_addresses,
        hd_accounts,
        "sync_started"
    );

    if total_addresses == 0 {
        let completed_at = run.clock.utc_now();
        crate::db::complete_bitcoin_history_full_resync_if_satisfied(run.user_id, completed_at)?;
        let mut summary = empty_sync_summary(run.run_id, total_addresses);
        summary.pagination_cache_hits = http_counters.pagination_cache_hits();
        summary.total_api_calls = http_counters.total_api_calls();
        let has_unfinished_work = reload_has_unfinished_sync_work(run.user_id)?;
        summary.schedule_hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source,
                has_unfinished_work,
                is_idle: !has_unfinished_work,
                blocked_for: None,
            },
        );

        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            source = ?source,
            scope = ?scope,
            "tasks: user transaction monitor skipped (no addresses)"
        );
        return Ok(AutomaticRunSuccess {
            summary,
            completed_at,
            aggregate_result: AggregateSyncResult::Success,
            children: ChildCompletionCounts::default(),
        });
    }

    let has_mempool_provider_assets =
        requires_provider(&non_hd_addresses, &hd_bundles, SyncProviderId::MempoolSpace);
    let has_etherscan_provider_assets =
        requires_provider(&non_hd_addresses, &hd_bundles, SyncProviderId::Etherscan);
    let mempool_client = if has_mempool_provider_assets {
        let (mempool_base_url, base_url_source) =
            super::client_config::resolve_mempool_base_url_from_settings(&preload.settings)?;
        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            mempool_base_url = %mempool_base_url.as_str(),
            mempool_base_url_source = ?base_url_source,
            address_count = total_addresses,
            "transactions sync: resolved mempool base URL for mempool-provider sync"
        );
        Some(super::client_config::build_mempool_client(
            run.user_id,
            &mempool_base_url,
            base_url_source,
            &http_counters,
        )?)
    } else {
        None
    };

    let etherscan_api_key = if has_etherscan_provider_assets {
        let key = super::client_config::resolve_etherscan_api_key_from_settings(&preload.settings);
        tracing::debug!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            has_api_key = key.is_some(),
            "transactions sync: resolved Etherscan API key for etherscan-provider sync"
        );
        key
    } else {
        None
    };

    let etherscan_base_url = if has_etherscan_provider_assets {
        super::client_config::resolve_etherscan_base_url_from_settings(&preload.settings)?
    } else {
        None
    };

    let clients = SyncClients {
        mempool_client: mempool_client.as_ref(),
        etherscan_api_key: etherscan_api_key.as_ref(),
        etherscan_base_url: etherscan_base_url.as_ref(),
        http_counters: &http_counters,
    };
    let ParentSyncCycleResult {
        mut summary,
        aggregate_result,
        child_summaries,
    } = run_parent_sync_cycle_with_runner(
        ParentSyncCycleRequest {
            run,
            clients,
            preload: &preload,
            non_hd_addresses,
            hd_bundles,
        },
        child_runner,
    );
    debug_assert_eq!(reduce_parent_result(&child_summaries), aggregate_result);
    if !preload.bitcoin_history_repair_account_ids.is_empty()
        && let Some(error) = child_summaries
            .iter()
            .filter(|child| {
                child.integration_id == SyncIntegrationId::Mempool
                    && matches!(child.outcome, IntegrationChildOutcome::Failure)
            })
            .find_map(|child| child.summary.bitcoin_history_repair_failure_error.as_ref())
    {
        crate::db::record_user_data_repair_failure(
            run.user_id,
            crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR,
            run.clock.utc_now(),
            error.as_str(),
        )?;
    }
    crate::db::complete_bitcoin_history_full_resync_if_satisfied(run.user_id, run.clock.utc_now())?;
    let child_worker_count = child_summaries.len();
    let child_success_count = child_summaries
        .iter()
        .filter(|child| matches!(child.outcome, IntegrationChildOutcome::Success))
        .count();
    let child_blocked_count = child_summaries
        .iter()
        .filter(|child| matches!(child.outcome, IntegrationChildOutcome::Blocked))
        .count();
    let child_failure_count = child_summaries
        .iter()
        .filter(|child| matches!(child.outcome, IntegrationChildOutcome::Failure))
        .count();
    summary.pagination_cache_hits = http_counters.pagination_cache_hits();
    summary.total_api_calls = http_counters.total_api_calls();
    let unfinished_integrations = reload_unfinished_sync_integrations(run.user_id)?;
    summary.schedule_hint = schedule_hint_for_parent_run(
        run,
        source,
        &summary,
        &child_summaries,
        &unfinished_integrations,
    );
    let completed_at = run.clock.utc_now();
    let raw_sync_history_retention_days = default_raw_sync_history_retention_days();

    let cleanup_report = cleanup_raw_sync_history_with_compaction(
        run.user_id,
        completed_at,
        raw_sync_history_retention_days,
    )?;
    let cleanup_stats = cleanup_report.deletion;
    let compaction_stats = cleanup_report.compaction;
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        retention_days = raw_sync_history_retention_days.value(),
        deleted_sync_runs = cleanup_stats.deleted_sync_runs,
        deleted_request_attempts = cleanup_stats.deleted_request_attempts,
        deleted_raw_observation_sets = cleanup_stats.deleted_raw_observation_sets,
        deleted_raw_parse_attempts = cleanup_stats.deleted_raw_parse_attempts,
        deleted_raw_mempool_transaction_observations = cleanup_stats.deleted_raw_mempool_transaction_observations,
        deleted_raw_etherscan_normal_transaction_observations = cleanup_stats.deleted_raw_etherscan_normal_transaction_observations,
        deleted_raw_etherscan_internal_transaction_observations = cleanup_stats.deleted_raw_etherscan_internal_transaction_observations,
        "raw_sync_history_cleanup_completed"
    );
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        retention_days = raw_sync_history_retention_days.value(),
        auto_vacuum_mode = compaction_stats.auto_vacuum_mode.as_str(),
        freelist_pages_before_cleanup = compaction_stats.freelist_pages_before_cleanup,
        freelist_pages_after_cleanup = compaction_stats.freelist_pages_after_cleanup,
        freelist_pages_after_compaction = compaction_stats.freelist_pages_after_compaction,
        pages_freed_by_cleanup = compaction_stats.pages_freed_by_cleanup,
        incremental_vacuum_pages_requested = compaction_stats.incremental_vacuum_pages_requested,
        page_count_before_compaction = compaction_stats.page_count_before_compaction,
        page_count_after_compaction = compaction_stats.page_count_after_compaction,
        pages_reclaimed_by_compaction = compaction_stats.pages_reclaimed_by_compaction,
        "raw_sync_history_compaction_completed"
    );

    if matches!(aggregate_result, AggregateSyncResult::Failure)
        && summary.addresses_failed.value() > 0
    {
        tracing::info!(
            user_id = %run.user_id,
            run_id = %run.run_id,
            source = ?source,
            addresses_total = summary.addresses_total.value(),
            addresses_synced = summary.addresses_synced.value(),
            addresses_failed = summary.addresses_failed.value(),
            addresses_skipped = summary.addresses_skipped.value(),
            addresses_skipped_tip_unchanged = summary.addresses_skipped_tip_unchanged.value(),
            addresses_early_exited = summary.addresses_early_exited.value(),
            pagination_cache_hits = summary.pagination_cache_hits,
            total_api_calls = summary.total_api_calls,
            rate_limited_integrations = summary.rate_limited.len(),
            new_tx_count = summary.new_tx_count.value(),
            updated_tx_count = summary.updated_tx_count.value(),
            child_workers_total = child_worker_count,
            child_workers_success = child_success_count,
            child_workers_blocked = child_blocked_count,
            child_workers_failed = child_failure_count,
            aggregate_result = ?aggregate_result,
            next_due_in_seconds = summary.schedule_hint.interval.as_secs(),
            next_due_reason = ?summary.schedule_hint.reason,
            schedule_urgency = ?summary.schedule_hint.urgency,
            next_due_at_utc = ?summary.schedule_hint.next_due_at_utc(completed_at),
            "sync_completed"
        );
    }

    // Only return Err if all addresses failed (causes task manager to log error)
    if matches!(
        aggregate_result,
        crate::transactions::AggregateSyncResult::Failure
    ) && summary.addresses_failed.value() > 0
    {
        let error_msg = summary.failure_error.unwrap_or_else(|| {
            SyncErrorMessage::sanitize(format!(
                "All {} addresses failed to sync",
                summary.addresses_failed.value()
            ))
        });
        Err(AutomaticRunFailure {
            error: UserTransactionMonitorError::Http(error_msg.as_str().to_string()),
            message: error_msg,
            addresses_failed: summary.addresses_failed,
        })
    } else {
        Ok(AutomaticRunSuccess {
            summary,
            completed_at,
            aggregate_result,
            children: ChildCompletionCounts {
                total: child_worker_count,
                success: child_success_count,
                blocked: child_blocked_count,
                failed: child_failure_count,
            },
        })
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) fn empty_sync_run_preload() -> SyncRunPreload {
    SyncRunPreload {
        settings: crate::models::UserSettings::default(),
        historical_backfill_enabled: true,
        historical_backfill_transactions_per_account: u32::MAX,
        account_labels: HashMap::new(),
        known_activity_address_ids: HashSet::new(),
        pending_address_ids: HashSet::new(),
        bitcoin_history_repair_account_ids: HashSet::new(),
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) fn make_summary_for_test(
    run_id: TransactionSyncRunId,
    total: u32,
    synced: u32,
    failed: u32,
    skipped: u32,
    rate_limited: &[&str],
    error: Option<&str>,
) -> UserTransactionMonitorSummary {
    let mut summary = empty_sync_summary(run_id, total);
    summary.addresses_synced = AddressCount::from_u32(synced);
    summary.addresses_failed = AddressCount::from_u32(failed);
    summary.addresses_skipped = AddressCount::from_u32(skipped);
    summary.rate_limited = rate_limited
        .iter()
        .map(|integration| RateLimitedIntegration {
            integration: (*integration).to_string(),
        })
        .collect();
    summary.failure_error = error.map(SyncErrorMessage::sanitize);
    summary
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::super::chain_tip::{CachedChainTip, chain_tip_cache_key};
    use super::super::context::{LABEL_ETHERSCAN, LABEL_MEMPOOL};
    use super::super::executor::LiveAddressSyncExecutor;
    use super::super::integrations::mempool::tests::start_historical_sync_mempool_server;
    use super::super::parent_cycle::{IntegrationChildCycleRequest, IntegrationChildRunner};
    use super::super::rate_limit::record_rate_limit;
    use super::super::test_support::{
        FakeAddressDerivationProvider, FakeAddressSyncExecutor, FakeClock, FakeSyncOutcome,
        make_derived_sync_address, make_run_context, make_sync_address,
        persist_sync_addresses_for_test, test_utc_now, with_rate_limiter_isolated,
    };
    use super::super::{AddressSyncExecutionRequest, SyncIterationResult};
    use super::*;
    use crate::db::AccountSyncBundle;
    use crate::integrations::mempool::MempoolClient;
    use crate::traces::client::{IntegrationLabel, TracedBlockingClient};
    use crate::transactions::{
        AddressBackfillCursor, ChainTipHeight, ChainTransactionStatus, ConsecutiveFailureCount,
        MempoolCursorTxid, TransactionCount, TransactionSyncEventType, TransactionSyncResult,
        TxHash,
    };
    use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
    use rusqlite::OptionalExtension;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use url::Url;

    fn live_mempool_client(
        user_id: UserId,
        base_url: &str,
        http_counters: &SyncHttpCounters,
    ) -> MempoolClient {
        let traced_client =
            TracedBlockingClient::builder(IntegrationLabel::new(LABEL_MEMPOOL), user_id)
                .configure(|builder| builder.timeout(Duration::from_secs(2)))
                .build_for_tests_with_tracing(false)
                .expect("traced mempool client should build");
        MempoolClient::new(
            traced_client,
            Url::parse(base_url).expect("test mempool URL should parse"),
        )
        .with_total_api_call_counter(http_counters.total_api_calls_counter())
    }

    fn mempool_stats_json(confirmed_count: u32) -> String {
        format!(
            r#"{{"chain_stats":{{"tx_count":{confirmed_count},"funded_txo_sum":50000,"spent_txo_sum":0}},"mempool_stats":{{"tx_count":0}}}}"#
        )
    }

    fn mempool_page_json(address: &SyncAddress, tx_hash: &str) -> String {
        format!(
            r#"[{{"txid":"{tx_hash}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block","block_time":1}}}}]"#,
            address.address.as_str()
        )
    }

    fn mempool_page_json_with_count(address: &SyncAddress, count: u32) -> String {
        let transactions = (1..=count)
            .map(|index| {
                let tx_hash = format!("{index:064x}");
                format!(
                    r#"{{"txid":"{tx_hash}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":1}}],"fee":0,"status":{{"confirmed":true,"block_height":1,"block_hash":"block","block_time":1}}}}"#,
                    address.address.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("[{transactions}]")
    }

    fn run_live_single_address_cycle(
        run: RunContext<'_>,
        clock: &FakeClock,
        address: &mut SyncAddress,
        response_bodies: Vec<String>,
        policy: MempoolHistoryPolicy,
    ) -> Vec<String> {
        let server = start_historical_sync_mempool_server(response_bodies);
        let http_counters = SyncHttpCounters::new();
        let client = live_mempool_client(run.user_id, &server.base_url, &http_counters);
        let mut chain_tip_cache = cached_bitcoin_chain_tip(clock);
        let mut executor = LiveAddressSyncExecutor::new();
        let mut accumulator = CycleAccumulator::new(1);
        let mut processed_for_account = 0_u32;
        let pending_address_ids = HashSet::new();
        let account_id = address
            .account_id
            .expect("test address should have an account");

        crate::db::mark_account_integration_sync_started(
            run.user_id,
            account_id,
            SyncIntegrationId::Mempool,
            run.started_at,
        )
        .expect("account integration start should persist");
        let (_, interrupted) = sync_single_address_with_controls(SyncSingleAddressControlRequest {
            run,
            address,
            chain_tip_cache: &mut chain_tip_cache,
            pending_address_ids: &pending_address_ids,
            clients: SyncClients {
                mempool_client: Some(&client),
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            },
            executor: &mut executor,
            accumulator: &mut accumulator,
            processed_for_account: &mut processed_for_account,
            single_address_progress: None,
            mempool_history_policy: policy,
            mempool_history_page_frontier: None,
        })
        .expect("controlled address cycle should succeed");
        assert!(!interrupted);
        refresh_account_integration_sync_state(
            run.user_id,
            account_id,
            SyncIntegrationId::Mempool,
            run.clock.utc_now(),
        )
        .expect("account integration state should refresh");
        server.join()
    }

    fn cached_bitcoin_chain_tip(clock: &FakeClock) -> ChainTipCache {
        let mut cache = ChainTipCache::default();
        cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(100).expect("tip should parse"),
                fetched_at: clock.instant_now(),
            },
        );
        cache
    }

    fn seed_account_history_frontier(
        run: RunContext<'_>,
        account_id: DigitalAssetAccountId,
        frontier_address_id: DigitalAssetAddressId,
        last_derived_external_index: u32,
    ) {
        crate::db::upsert_account_sync_state(
            run.user_id,
            account_id,
            crate::wallets::BIP44_GAP_LIMIT,
            Some(last_derived_external_index),
            None,
            run.started_at,
        )
        .expect("account sync state should seed");
        crate::db::with_user_db_mut(run.user_id, |conn| {
            conn.execute(
                "UPDATE account_sync_state
                 SET mempool_history_next_address_id = ?1
                 WHERE account_id = ?2",
                rusqlite::params![frontier_address_id.to_string(), account_id.to_string()],
            )
            .map_err(|error| {
                crate::db::DbError::new(format!("frontier fixture update failed: {error}"))
            })?;
            Ok::<(), crate::db::DbError>(())
        })
        .expect("account frontier should seed");
    }

    fn load_account_history_frontier(
        user_id: UserId,
        account_id: DigitalAssetAccountId,
    ) -> Option<DigitalAssetAddressId> {
        crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT mempool_history_next_address_id
                 FROM account_sync_state
                 WHERE account_id = ?1",
                [account_id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| {
                crate::db::DbError::new(format!("frontier fixture load failed: {error}"))
            })
        })
        .expect("account frontier should load")
        .map(|raw| raw.parse().expect("frontier address id should parse"))
    }

    fn load_test_hd_bundle(
        run: RunContext<'_>,
        account_id: DigitalAssetAccountId,
    ) -> AccountSyncBundle {
        let frontier = load_account_history_frontier(run.user_id, account_id);
        let addresses = crate::db::get_sync_addresses_for_account(run.user_id, account_id)
            .expect("HD addresses should load");
        let (internal_addresses, external_addresses): (Vec<_>, Vec<_>) = addresses
            .into_iter()
            .partition(|address| address.derivation_change == Some(1));
        AccountSyncBundle {
            account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            hd_key_extended_pubkey: "xpub-test".to_string(),
            address_scheme: crate::wallets::AddressScheme::NativeSegwit,
            sync_state: Some(crate::db::AccountSyncStateRow {
                account_id,
                last_scanned_time: None,
                gap_limit: crate::wallets::BIP44_GAP_LIMIT,
                last_derived_external_index: Some(12),
                last_derived_internal_index: None,
                mempool_history_next_address_id: frontier,
            }),
            external_addresses,
            internal_addresses,
        }
    }

    fn next_run_for_user<'a>(clock: &'a FakeClock, user_id: UserId) -> RunContext<'a> {
        RunContext {
            user_id,
            run_id: TransactionSyncRunId::new(),
            source: TriggerSource::ManualInternal,
            started_at: clock.utc_now(),
            clock,
        }
    }

    fn assert_automatic_event_sequence(
        receiver: &mut tokio::sync::broadcast::Receiver<TransactionSyncEvent>,
        run_id: TransactionSyncRunId,
        terminal_event: TransactionSyncEventType,
    ) {
        let events = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter(|event| {
                matches!(
                    event.event_type,
                    TransactionSyncEventType::Started
                        | TransactionSyncEventType::Completed
                        | TransactionSyncEventType::Failed
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, TransactionSyncEventType::Started);
        assert_eq!(events[0].run_id, Some(run_id));
        assert_eq!(events[1].event_type, terminal_event);
        assert_eq!(events[1].run_id, Some(run_id));
    }

    fn successful_automatic_run(
        summary: UserTransactionMonitorSummary,
        completed_at: DateTime<Utc>,
        children: ChildCompletionCounts,
    ) -> AutomaticRunSuccess {
        AutomaticRunSuccess {
            summary,
            completed_at,
            aggregate_result: AggregateSyncResult::Success,
            children,
        }
    }

    struct PersistFirstCanonicalTxRunner {
        runtime_context: Arc<crate::runtime_context::RuntimeContext>,
        address: SyncAddress,
    }

    impl IntegrationChildRunner for PersistFirstCanonicalTxRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let _runtime_context_guard = crate::runtime_context::push_default_runtime_context(
                Arc::clone(&self.runtime_context),
            );
            persist_confirmed_output_for_account_count(
                request.run,
                &self.address,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
            Ok(make_summary_for_test(
                request.run.run_id,
                1,
                1,
                0,
                0,
                &[],
                None,
            ))
        }
    }

    #[derive(Default)]
    struct RecordingChildRunner {
        account_ids: Mutex<Vec<DigitalAssetAccountId>>,
    }

    impl IntegrationChildRunner for RecordingChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let account_ids = request
                .workset
                .non_hd_addresses
                .iter()
                .filter_map(|address| address.account_id)
                .collect::<Vec<_>>();
            self.account_ids
                .lock()
                .expect("recording child runner should lock")
                .extend(account_ids);
            let total = u32::try_from(request.workset.non_hd_addresses.len()).unwrap_or(u32::MAX);
            Ok(make_summary_for_test(
                request.run.run_id,
                total,
                total,
                0,
                0,
                &[],
                None,
            ))
        }
    }

    struct FailingRepairExecutor;

    impl AddressSyncExecutor for FailingRepairExecutor {
        fn sync_one_iteration(
            &mut self,
            _request: AddressSyncExecutionRequest<'_>,
        ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
            Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
                "Strict Mempool history scan count mismatch",
            )))
        }
    }

    struct FailingRepairChildRunner {
        runtime_context: Arc<crate::runtime_context::RuntimeContext>,
    }

    impl IntegrationChildRunner for FailingRepairChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let _runtime_context_guard = crate::runtime_context::push_default_runtime_context(
                Arc::clone(&self.runtime_context),
            );
            let mut executor = FailingRepairExecutor;
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            run_sync_cycle(SyncCycleRequest {
                run: request.run,
                clients: request.clients,
                preload: request.preload,
                non_hd_addresses: request.workset.non_hd_addresses,
                hd_bundles: request.workset.hd_bundles,
                known_activity: request.preload.known_activity_address_ids.clone(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
        }
    }

    struct EtherscanFailureChildRunner;

    impl IntegrationChildRunner for EtherscanFailureChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let total = u32::try_from(request.workset.non_hd_addresses.len()).unwrap_or(u32::MAX);
            if request.workset.integration_id == SyncIntegrationId::Etherscan {
                return Ok(make_summary_for_test(
                    request.run.run_id,
                    total,
                    0,
                    total,
                    0,
                    &[],
                    Some("unrelated Etherscan failure"),
                ));
            }
            Ok(make_summary_for_test(
                request.run.run_id,
                total,
                total,
                0,
                0,
                &[],
                None,
            ))
        }
    }

    struct MixedMempoolAccountFailureExecutor {
        repair_account_id: DigitalAssetAccountId,
        repair_success: FakeAddressSyncExecutor,
    }

    impl MixedMempoolAccountFailureExecutor {
        fn new(repair_account_id: DigitalAssetAccountId) -> Self {
            Self {
                repair_account_id,
                repair_success: FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                }]),
            }
        }
    }

    impl AddressSyncExecutor for MixedMempoolAccountFailureExecutor {
        fn sync_one_iteration(
            &mut self,
            request: AddressSyncExecutionRequest<'_>,
        ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
            if request.address.account_id == Some(self.repair_account_id) {
                return self.repair_success.sync_one_iteration(request);
            }
            Err(UserTransactionMonitorError::Http(
                "ordinary Bitcoin account failed".to_string(),
            ))
        }
    }

    struct MixedMempoolAccountFailureChildRunner {
        repair_account_id: DigitalAssetAccountId,
        runtime_context: Arc<crate::runtime_context::RuntimeContext>,
    }

    impl IntegrationChildRunner for MixedMempoolAccountFailureChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let _runtime_context_guard = crate::runtime_context::push_default_runtime_context(
                Arc::clone(&self.runtime_context),
            );
            let mut executor = MixedMempoolAccountFailureExecutor::new(self.repair_account_id);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            run_sync_cycle(SyncCycleRequest {
                run: request.run,
                clients: request.clients,
                preload: request.preload,
                non_hd_addresses: request.workset.non_hd_addresses,
                hd_bundles: request.workset.hd_bundles,
                known_activity: request.preload.known_activity_address_ids.clone(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
        }
    }

    struct RestartingStrictMismatchExecutor {
        inner: FakeAddressSyncExecutor,
        observed_cursors: Vec<Option<String>>,
        restart_on_next_call: bool,
    }

    impl RestartingStrictMismatchExecutor {
        fn new() -> Self {
            Self {
                inner: FakeAddressSyncExecutor::new(vec![
                    FakeSyncOutcome::Failure {
                        message: "Strict Mempool history scan count mismatch".to_string(),
                    },
                    FakeSyncOutcome::Success {
                        new_tx_count: 0,
                        updated_tx_count: 0,
                    },
                ]),
                observed_cursors: Vec::new(),
                restart_on_next_call: true,
            }
        }
    }

    impl AddressSyncExecutor for RestartingStrictMismatchExecutor {
        fn sync_one_iteration(
            &mut self,
            request: AddressSyncExecutionRequest<'_>,
        ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
            self.observed_cursors.push(
                request
                    .address
                    .mempool_backfill_cursor_txid
                    .as_ref()
                    .map(|cursor| cursor.as_str().to_string()),
            );
            if self.restart_on_next_call {
                self.restart_on_next_call = false;
                crate::db::restart_strict_mempool_history_scan(
                    request.run.user_id,
                    request.address.address_id,
                )?;
            }
            self.inner.sync_one_iteration(request)
        }
    }

    #[derive(Default)]
    struct LeaseProbeChildRunner {
        acquired_during_child: Mutex<Option<bool>>,
    }

    impl IntegrationChildRunner for LeaseProbeChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let lease = crate::sync_execution_lease::UserSyncExecutionLease::try_acquire(
                request.run.user_id,
            );
            *self
                .acquired_during_child
                .lock()
                .expect("lease probe should lock") = Some(lease.is_some());
            drop(lease);
            Ok(make_summary_for_test(
                request.run.run_id,
                1,
                1,
                0,
                0,
                &[],
                None,
            ))
        }
    }

    fn seed_free_sync_account_limit(total: u16) {
        let capabilities = crate::payments::free_tier::free_tier_capabilities_for_test(total);
        crate::db::upsert_free_tier_entitlement_cache(
            &crate::payments::free_tier::FreeTierObservation {
                observed_at: DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
                    .expect("entitlement time should parse")
                    .with_timezone(&Utc),
                capability_schema_version: crate::payments::types::CAPABILITY_SCHEMA_VERSION_V3,
                capabilities,
            },
        )
        .expect("free account limit should persist");
    }

    fn run_fake_non_hd_cycle(
        run: RunContext<'_>,
        raw_address: &str,
        outcome: FakeSyncOutcome,
    ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
        let address = make_sync_address(
            raw_address,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(crate::wallets::DigitalAssetAccountId::new()),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            Some(0),
            Some(0),
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        let mut executor = FakeAddressSyncExecutor::new(vec![outcome]);
        let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
        let http_counters = SyncHttpCounters::new();
        let preload = empty_sync_run_preload();

        run_sync_cycle(SyncCycleRequest {
            run,
            clients: SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            },
            preload: &preload,
            non_hd_addresses: vec![address],
            hd_bundles: Vec::new(),
            known_activity: HashSet::new(),
            sync_executor: &mut executor,
            derivation_provider: &mut derivation_provider,
        })
    }

    #[test]
    fn empty_automatic_run_reports_started_then_one_completion_without_provider_setup() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
            .expect("sync event subscription should succeed");
        let summary = empty_sync_summary(run.run_id, 0);
        let expected_schedule_hint = summary.schedule_hint;

        let returned = run_started_automatic_with(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            |_, _, _| {
                Ok(successful_automatic_run(
                    summary,
                    clock.utc_now(),
                    ChildCompletionCounts::default(),
                ))
            },
        )
        .expect("empty automatic run should succeed");

        assert_automatic_event_sequence(
            &mut receiver,
            run.run_id,
            TransactionSyncEventType::Completed,
        );
        assert_eq!(returned.schedule_hint, expected_schedule_hint);
    }

    #[test]
    fn bitcoin_history_full_resync_completion_waits_for_post_scheduler_eligibility() {
        with_rate_limiter_isolated(|| {
            let runtime =
                crate::db::acquire_test_runtime().expect("test runtime should initialize");
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                None,
                None,
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let runner = PersistFirstCanonicalTxRunner {
                runtime_context: runtime.runtime_context(),
                address,
            };

            if let Err(failure) = run_automatic_inner_with_runner(
                run,
                TriggerSource::Schedule,
                TransactionSyncScope::User,
                &runner,
            ) {
                panic!(
                    "normal scheduler pass should succeed: {}",
                    failure.message.as_str()
                );
            }
            assert_eq!(
                crate::db::load_user_data_repair_status(
                    run.user_id,
                    crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR,
                )
                .expect("repair status should load"),
                Some(crate::db::UserDataRepairStatus::Pending)
            );
            assert_eq!(
                crate::db::load_unverified_bitcoin_history_repair_account_ids(run.user_id)
                    .expect("unverified accounts should load"),
                vec![account_id]
            );
        });
    }

    #[test]
    fn bitcoin_history_full_resync_holds_user_lease_during_repair_work() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let account_id = DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qrepairlease000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        persist_confirmed_output_for_account_count(
            run,
            &address,
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        );
        let runner = LeaseProbeChildRunner::default();

        run_automatic_inner_with_runner(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            &runner,
        )
        .unwrap_or_else(|failure| {
            panic!(
                "automatic repair should succeed: {}",
                failure.message.as_str()
            )
        });
        assert_eq!(
            *runner
                .acquired_during_child
                .lock()
                .expect("lease probe should lock"),
            Some(false),
            "repair child must execute while the scheduler owns the shared user lease"
        );
    }

    #[test]
    fn automatic_current_only_releases_user_lease_before_child_work() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let address = make_sync_address(
            "bc1qcurrentonlylease000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(DigitalAssetAccountId::new()),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        let runner = LeaseProbeChildRunner::default();

        run_automatic_inner_with_runner(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            &runner,
        )
        .unwrap_or_else(|failure| {
            panic!(
                "current-only automatic sync should succeed: {}",
                failure.message.as_str()
            )
        });
        assert_eq!(
            *runner
                .acquired_during_child
                .lock()
                .expect("lease probe should lock"),
            Some(true),
            "normal current-only work must not retain the repair serialization lease"
        );
    }

    #[test]
    fn bitcoin_history_full_resync_bypasses_inactive_account_filter() {
        with_rate_limiter_isolated(|| {
            let _runtime =
                crate::db::acquire_test_runtime().expect("test runtime should initialize");
            seed_free_sync_account_limit(0);
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                None,
                None,
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            persist_confirmed_output_for_account_count(
                run,
                &address,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            );
            assert!(
                !load_active_native_accounts(run.user_id, run.started_at)
                    .expect("active accounts should load")
                    .contains(&account_id)
            );
            let runner = RecordingChildRunner::default();

            if let Err(failure) = run_automatic_inner_with_runner(
                run,
                TriggerSource::Schedule,
                TransactionSyncScope::User,
                &runner,
            ) {
                panic!(
                    "repair scheduler pass should succeed: {}",
                    failure.message.as_str()
                );
            }
            assert_eq!(
                *runner
                    .account_ids
                    .lock()
                    .expect("recorded accounts should lock"),
                vec![account_id]
            );
        });
    }

    #[test]
    fn bitcoin_history_full_resync_empty_scheduler_pass_completes_without_provider_work() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let runner = RecordingChildRunner::default();

        let result = run_automatic_inner_with_runner(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            &runner,
        );
        if let Err(failure) = result {
            panic!(
                "empty scheduler pass should succeed: {}",
                failure.message.as_str()
            );
        }
        assert!(
            runner
                .account_ids
                .lock()
                .expect("recorded accounts should lock")
                .is_empty()
        );
        assert_eq!(
            crate::db::load_user_data_repair_status(
                run.user_id,
                crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR,
            )
            .expect("repair status should load"),
            Some(crate::db::UserDataRepairStatus::Completed)
        );
    }

    #[test]
    fn repair_in_progress_rejects_address_scoped_sync() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let account_id = DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qscopedrepair0000000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        persist_confirmed_output_for_account_count(
            run,
            &address,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        );
        let runner = RecordingChildRunner::default();

        let failure = match run_automatic_inner_with_runner(
            run,
            TriggerSource::ManualInternal,
            TransactionSyncScope::Address {
                address_id: address.address_id,
            },
            &runner,
        ) {
            Ok(_) => panic!("repair-owned address scope must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error.to_string(),
            "Bitcoin history correctness repair is in progress"
        );
        assert!(
            runner
                .account_ids
                .lock()
                .expect("recorded accounts should lock")
                .is_empty()
        );
    }

    #[test]
    fn bitcoin_history_full_resync_mismatch_records_repair_failure() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let account_id = DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qmismatchrepair00000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
        persist_confirmed_output_for_account_count(
            run,
            &address,
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        );

        assert!(
            run_automatic_inner_with_runner(
                run,
                TriggerSource::Schedule,
                TransactionSyncScope::User,
                &FailingRepairChildRunner {
                    runtime_context: crate::runtime_context::current_runtime_context()
                        .expect("test runtime context should exist"),
                },
            )
            .is_err()
        );
        let (status, attempted_at, last_error) = crate::db::with_user_db(run.user_id, |conn| {
            conn.query_row(
                "SELECT status, last_attempted_at, last_error
                     FROM user_data_repairs
                     WHERE repair_key = ?1",
                [crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(|error| {
                crate::db::DbError::new(format!("repair failure fixture should load: {error}"))
            })
        })
        .expect("repair failure should load");
        assert_eq!(status, "pending");
        assert!(attempted_at.is_some());
        assert_eq!(
            last_error.as_deref(),
            Some("Strict Mempool history scan count mismatch")
        );
    }

    #[test]
    fn bitcoin_history_full_resync_ignores_unrelated_etherscan_failure_diagnostics() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let bitcoin_account_id = DigitalAssetAccountId::new();
        let bitcoin = make_sync_address(
            "bc1qrepairdiagnostics000000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(bitcoin_account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        let ethereum = make_sync_address(
            "0x1111111111111111111111111111111111111111",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(DigitalAssetAccountId::new()),
            None,
            None,
            None,
        );
        persist_sync_addresses_for_test(run, &[bitcoin.clone(), ethereum]);
        persist_confirmed_output_for_account_count(
            run,
            &bitcoin,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );

        let result = run_automatic_inner_with_runner(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            &EtherscanFailureChildRunner,
        )
        .unwrap_or_else(|failure| {
            panic!("partial sync should succeed: {}", failure.message.as_str())
        });
        assert_eq!(
            result
                .summary
                .failure_error
                .as_ref()
                .map(SyncErrorMessage::as_str),
            Some("unrelated Etherscan failure")
        );
        let repair_error = crate::db::with_user_db(run.user_id, |conn| {
            conn.query_row(
                "SELECT last_error
                 FROM user_data_repairs
                 WHERE repair_key = ?1",
                [crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| {
                crate::db::DbError::new(format!("repair diagnostics should load: {error}"))
            })
        })
        .expect("repair diagnostics should load");
        assert_eq!(repair_error, None);
    }

    #[test]
    fn bitcoin_history_full_resync_ignores_ordinary_mempool_account_failure_diagnostics() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let repair_account_id = DigitalAssetAccountId::new();
        let repair_address = make_sync_address(
            "bc1qrepairmixeddiagnostics000000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(repair_account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        let ordinary_address = make_sync_address(
            "bc1qordinarymixeddiagnostics0000000000000000000000000000000",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(DigitalAssetAccountId::new()),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_addresses_for_test(run, &[repair_address.clone(), ordinary_address.clone()]);
        persist_confirmed_output_for_account_count(
            run,
            &repair_address,
            "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
        );

        let failure = match run_automatic_inner_with_runner(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            &MixedMempoolAccountFailureChildRunner {
                repair_account_id,
                runtime_context: crate::runtime_context::current_runtime_context()
                    .expect("test runtime context should exist"),
            },
        ) {
            Ok(_) => panic!("ordinary Mempool failure should fail the only child"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.message.as_str(),
            "Sync HTTP request failed: ordinary Bitcoin account failed"
        );
        let repair_error = crate::db::with_user_db(run.user_id, |conn| {
            conn.query_row(
                "SELECT last_error
                 FROM user_data_repairs
                 WHERE repair_key = ?1",
                [crate::db::BITCOIN_HISTORY_FULL_RESYNC_REPAIR],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| {
                crate::db::DbError::new(format!("repair diagnostics should load: {error}"))
            })
        })
        .expect("repair diagnostics should load");
        assert_eq!(repair_error, None);
    }

    #[test]
    fn non_empty_automatic_run_reports_one_terminal_completion() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
                .expect("sync event subscription should succeed");
            let summary = run_started_automatic_with(
                run,
                TriggerSource::Schedule,
                TransactionSyncScope::User,
                |run, _, _| {
                    let summary = run_fake_non_hd_cycle(
                        run,
                        "bc1qnonemptycompletion",
                        FakeSyncOutcome::Success {
                            new_tx_count: 0,
                            updated_tx_count: 0,
                        },
                    )?;
                    Ok(successful_automatic_run(
                        summary,
                        clock.utc_now(),
                        ChildCompletionCounts {
                            total: 1,
                            success: 1,
                            blocked: 0,
                            failed: 0,
                        },
                    ))
                },
            )
            .expect("non-empty automatic run should succeed");

            assert_automatic_event_sequence(
                &mut receiver,
                run.run_id,
                TransactionSyncEventType::Completed,
            );
            assert_eq!(summary.addresses_synced.value(), 1);
            assert_eq!(summary.addresses_failed.value(), 0);
        });
    }

    fn assert_injected_automatic_error_reports_one_failure(
        execute: impl FnOnce(
            RunContext<'_>,
            TriggerSource,
            TransactionSyncScope,
        ) -> Result<AutomaticRunSuccess, AutomaticRunFailure>,
    ) {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
            .expect("sync event subscription should succeed");

        let result = run_started_automatic_with(
            run,
            TriggerSource::Schedule,
            TransactionSyncScope::User,
            execute,
        );
        result.expect_err("injected automatic run error should be returned");

        assert_automatic_event_sequence(
            &mut receiver,
            run.run_id,
            TransactionSyncEventType::Failed,
        );
    }

    #[test]
    fn pre_cycle_infrastructure_error_reports_one_terminal_failure() {
        assert_injected_automatic_error_reports_one_failure(|_, _, _| {
            Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
                "injected preload failure",
            ))
            .into())
        });
    }

    #[test]
    fn post_cycle_infrastructure_error_reports_one_terminal_failure() {
        assert_injected_automatic_error_reports_one_failure(|run, _, _| {
            let summary = run_fake_non_hd_cycle(
                run,
                "bc1qpostcyclefailure",
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            )?;
            assert_eq!(summary.addresses_synced.value(), 1);
            Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
                "injected cleanup failure",
            ))
            .into())
        });
    }

    #[test]
    fn all_addresses_failed_reports_started_then_one_terminal_failure() {
        assert_injected_automatic_error_reports_one_failure(|run, _, _| {
            let summary = run_fake_non_hd_cycle(
                run,
                "bc1qalladdressesfailed",
                FakeSyncOutcome::Failure {
                    message: "injected address failure".to_string(),
                },
            )?;
            assert_eq!(summary.addresses_synced.value(), 0);
            assert_eq!(summary.addresses_failed.value(), 1);
            let message = summary
                .failure_error
                .expect("failed cycle should retain its error");
            Err(AutomaticRunFailure {
                error: UserTransactionMonitorError::Http(message.as_str().to_string()),
                message,
                addresses_failed: summary.addresses_failed,
            })
        });
    }

    #[test]
    fn default_raw_sync_history_retention_is_30_days() {
        assert_eq!(default_raw_sync_history_retention_days().value(), 14);
    }

    #[test]
    fn mempool_history_policy_automatic_does_not_use_provider_count_as_admission() {
        let mut preload = empty_sync_run_preload();

        preload.historical_backfill_enabled = false;
        assert_eq!(
            mempool_history_policy_for_preload(&preload),
            MempoolHistoryPolicy::CurrentOnly,
        );

        preload.historical_backfill_enabled = true;
        preload.historical_backfill_transactions_per_account = 10_000;
        let policy = mempool_history_policy_for_preload(&preload);
        assert_eq!(
            policy,
            MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(10_000),
            },
        );
        assert!(policy.permits_transaction_page(TransactionCount::from_u32(9_999)));
    }

    #[test]
    fn breadth_cap_boundary_resumes_hd_suffix_and_advances_frontier_per_page() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut addresses = (1..=4)
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qbreadth{index}"),
                        SyncedAssetId::Bitcoin,
                        Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(index % 2),
                        Some(index),
                    )
                })
                .collect::<Vec<_>>();
            addresses.sort_by_key(|address| {
                (
                    address.derivation_index,
                    address.derivation_change,
                    address.address_id.to_string(),
                )
            });
            persist_sync_addresses_for_test(run, &addresses);
            let expected = vec![addresses[2].address_id, addresses[3].address_id];
            let mut bundle = AccountSyncBundle {
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "xpub-test".to_string(),
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                sync_state: Some(crate::db::AccountSyncStateRow {
                    account_id,
                    last_scanned_time: None,
                    gap_limit: crate::wallets::BIP44_GAP_LIMIT,
                    last_derived_external_index: Some(4),
                    last_derived_internal_index: None,
                    mempool_history_next_address_id: Some(addresses[2].address_id),
                }),
                external_addresses: addresses,
                internal_addresses: Vec::new(),
            };
            let known_activity = bundle
                .external_addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<HashSet<_>>();
            let pending = HashSet::new();
            let mut chain_tip_cache = ChainTipCache::default();
            let mut accumulator = CycleAccumulator::new(4);
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let http_counters = SyncHttpCounters::new();

            let (interrupted, _) =
                run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                    run,
                    clients: SyncClients {
                        mempool_client: None,
                        etherscan_api_key: None,
                        etherscan_base_url: None,
                        http_counters: &http_counters,
                    },
                    pending_address_ids: &pending,
                    bundle: &mut bundle,
                    known_activity: &known_activity,
                    chain_tip_cache: &mut chain_tip_cache,
                    accumulator: &mut accumulator,
                    sync_executor: &mut executor,
                    policy: MempoolHistoryPolicy::Normal {
                        cap: TransactionCount::from_u32(100),
                    },
                })
                .expect("breadth round should finish");

            assert!(!interrupted);
            assert_eq!(executor.calls, expected);
            assert_eq!(
                executor.mempool_history_frontier_calls,
                vec![
                    Some(crate::db::HdMempoolHistoryFrontierUpdate {
                        account_id,
                        next_address_id: Some(bundle.external_addresses[3].address_id),
                    }),
                    Some(crate::db::HdMempoolHistoryFrontierUpdate {
                        account_id,
                        next_address_id: None,
                    }),
                ],
            );
        });
    }

    #[test]
    fn task5_persisted_breadth_crossing_cap_resumes_frontier_suffix() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let first_run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let addresses = [1_u32, 2, 3, 12]
                .map(|index| {
                    make_sync_address(
                        &format!("bc1qtask5breadth{index}"),
                        SyncedAssetId::Bitcoin,
                        Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                })
                .to_vec();
            persist_sync_addresses_for_test(first_run, &addresses);
            seed_account_history_frontier(first_run, account_id, addresses[1].address_id, 12);
            let known_activity = addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<HashSet<_>>();
            let pending = HashSet::new();
            let first_server = start_historical_sync_mempool_server(vec![
                mempool_stats_json(2),
                mempool_page_json(
                    &addresses[1],
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ),
            ]);
            let first_http_counters = SyncHttpCounters::new();
            let first_client = live_mempool_client(
                first_run.user_id,
                &first_server.base_url,
                &first_http_counters,
            );
            let mut first_bundle = load_test_hd_bundle(first_run, account_id);
            let mut first_chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut first_accumulator = CycleAccumulator::new(4);
            let mut first_executor = LiveAddressSyncExecutor::new();

            let (first_interrupted, _) =
                run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                    run: first_run,
                    clients: SyncClients {
                        mempool_client: Some(&first_client),
                        etherscan_api_key: None,
                        etherscan_base_url: None,
                        http_counters: &first_http_counters,
                    },
                    pending_address_ids: &pending,
                    bundle: &mut first_bundle,
                    known_activity: &known_activity,
                    chain_tip_cache: &mut first_chain_tip_cache,
                    accumulator: &mut first_accumulator,
                    sync_executor: &mut first_executor,
                    policy: MempoolHistoryPolicy::Normal {
                        cap: TransactionCount::from_u32(1),
                    },
                })
                .expect("crossing breadth page should succeed");
            let first_requests = first_server.join();

            assert!(!first_interrupted);
            assert_eq!(
                first_requests,
                vec![
                    format!(
                        "GET /api/address/{} HTTP/1.1",
                        addresses[1].address.as_str()
                    ),
                    format!(
                        "GET /api/address/{}/txs HTTP/1.1",
                        addresses[1].address.as_str()
                    ),
                ]
            );
            assert!(
                first_requests
                    .iter()
                    .all(|request| !request.contains(addresses[3].address.as_str()))
            );
            let first_persisted =
                crate::db::get_sync_addresses_for_account(first_run.user_id, account_id)
                    .expect("first-run addresses should load");
            assert_eq!(
                first_persisted
                    .iter()
                    .find(|address| address.address_id == addresses[1].address_id)
                    .expect("address 2 should persist")
                    .mempool_backfill_cursor_txid
                    .as_ref()
                    .map(MempoolCursorTxid::as_str),
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            );
            assert_eq!(
                load_account_history_frontier(first_run.user_id, account_id),
                Some(addresses[2].address_id)
            );
            assert_eq!(
                crate::db::load_canonical_confirmed_account_transaction_count(
                    first_run.user_id,
                    account_id,
                )
                .expect("crossing confirmed count should load"),
                TransactionCount::from_u32(1)
            );

            let second_run = next_run_for_user(&clock, first_run.user_id);
            let second_server = start_historical_sync_mempool_server(vec![
                mempool_stats_json(1),
                mempool_page_json(
                    &addresses[2],
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ),
            ]);
            let second_http_counters = SyncHttpCounters::new();
            let second_client = live_mempool_client(
                second_run.user_id,
                &second_server.base_url,
                &second_http_counters,
            );
            let mut second_bundle = load_test_hd_bundle(second_run, account_id);
            let mut second_chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut second_accumulator = CycleAccumulator::new(4);
            let mut second_executor = LiveAddressSyncExecutor::new();

            run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                run: second_run,
                clients: SyncClients {
                    mempool_client: Some(&second_client),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &second_http_counters,
                },
                pending_address_ids: &pending,
                bundle: &mut second_bundle,
                known_activity: &known_activity,
                chain_tip_cache: &mut second_chain_tip_cache,
                accumulator: &mut second_accumulator,
                sync_executor: &mut second_executor,
                policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(2),
                },
            })
            .expect("raised-cap breadth page should succeed");
            let second_requests = second_server.join();

            assert_eq!(
                second_requests.first(),
                Some(&format!(
                    "GET /api/address/{} HTTP/1.1",
                    addresses[2].address.as_str()
                ))
            );
            assert!(
                second_requests
                    .iter()
                    .all(|request| !request.contains(addresses[0].address.as_str()))
            );
        });
    }

    #[test]
    fn hd_breadth_rounds_rebuild_only_for_reconciliation() {
        for (label, outcomes, expected_rebuild_count) in [
            (
                "completed",
                vec![
                    FakeSyncOutcome::SuccessWithObservedActivity,
                    FakeSyncOutcome::Success {
                        new_tx_count: 0,
                        updated_tx_count: 0,
                    },
                ],
                1,
            ),
            (
                "yielded",
                vec![
                    FakeSyncOutcome::SuccessWithObservedActivity,
                    FakeSyncOutcome::RateLimited {
                        integration: LABEL_MEMPOOL.to_string(),
                    },
                ],
                0,
            ),
        ] {
            with_rate_limiter_isolated(|| {
                let clock = FakeClock::new(test_utc_now());
                let run = make_run_context(&clock);
                let account_id = DigitalAssetAccountId::new();
                let addresses = [0_u32, 1].map(|index| {
                    make_sync_address(
                        &format!("bc1qrebuild{label}{index}"),
                        SyncedAssetId::Bitcoin,
                        Network::Mainnet,
                        Some(account_id),
                        Some(crate::wallets::AddressScheme::NativeSegwit),
                        Some(0),
                        Some(index),
                    )
                });
                persist_sync_addresses_for_test(run, &addresses);
                persist_confirmed_output_for_account_count(
                    run,
                    &addresses[0],
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                );
                crate::db::rebuild_account_transaction_ledger_with_unknown_bitcoin_basis(
                    run.user_id,
                    account_id,
                    run.started_at,
                )
                .expect("fixture ledger should rebuild");
                crate::db::with_user_db_mut(run.user_id, |conn| {
                    conn.execute_batch(&format!(
                        "CREATE TABLE test_hd_rebuild_audit (
                             rebuild_count INTEGER NOT NULL
                         );
                         INSERT INTO test_hd_rebuild_audit (rebuild_count) VALUES (0);
                         CREATE TRIGGER test_count_hd_rebuild
                         AFTER INSERT ON account_transaction_ledger
                         WHEN NEW.account_id = '{account_id}'
                         BEGIN
                           UPDATE test_hd_rebuild_audit
                           SET rebuild_count = rebuild_count + 1;
                         END;"
                    ))
                    .map_err(|error| {
                        crate::db::DbError::new(format!(
                            "failed to install HD rebuild audit: {error}"
                        ))
                    })
                })
                .expect("HD rebuild audit should install");
                let bundle = AccountSyncBundle {
                    account_id,
                    asset_id: SyncedAssetId::Bitcoin,
                    network: Network::Mainnet,
                    hd_key_extended_pubkey: format!("xpub-{label}"),
                    address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                    sync_state: None,
                    external_addresses: addresses.to_vec(),
                    internal_addresses: Vec::new(),
                };
                let known_activity = addresses
                    .iter()
                    .map(|address| address.address_id)
                    .collect::<HashSet<_>>();
                let mut executor = FakeAddressSyncExecutor::new(outcomes);
                let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
                let http_counters = SyncHttpCounters::new();
                let preload = empty_sync_run_preload();

                run_sync_cycle(SyncCycleRequest {
                    run,
                    clients: SyncClients {
                        mempool_client: None,
                        etherscan_api_key: None,
                        etherscan_base_url: None,
                        http_counters: &http_counters,
                    },
                    preload: &preload,
                    non_hd_addresses: Vec::new(),
                    hd_bundles: vec![bundle],
                    known_activity,
                    sync_executor: &mut executor,
                    derivation_provider: &mut derivation_provider,
                })
                .expect("HD breadth cycle should finish");

                let rebuild_count = crate::db::with_user_db(run.user_id, |conn| {
                    conn.query_row(
                        "SELECT rebuild_count FROM test_hd_rebuild_audit",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| {
                        crate::db::DbError::new(format!("failed to load HD rebuild count: {error}"))
                    })
                })
                .expect("HD rebuild count should load");
                assert_eq!(
                    rebuild_count, expected_rebuild_count,
                    "{label} breadth round"
                );
            });
        }
    }

    #[test]
    fn task5_persisted_capped_stats_restart_from_first_page_when_cap_rises() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let capped_run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qtask5cappedrestart",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(1),
            );
            persist_sync_addresses_for_test(capped_run, std::slice::from_ref(&address));
            seed_account_history_frontier(capped_run, account_id, address.address_id, 1);
            persist_confirmed_output_for_account_count(
                capped_run,
                &address,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
            crate::db::mark_address_sync_started(
                capped_run.user_id,
                address.address_id,
                capped_run.run_id,
                capped_run.started_at,
            )
            .expect("address sync state should seed");
            let old_proof = crate::db::MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(1),
                complete_height: ChainTipHeight::try_new(10).expect("old height should parse"),
            };
            crate::db::publish_mempool_history_proof(
                capped_run.user_id,
                address.address_id,
                old_proof,
            )
            .expect("older proof should seed");
            let stale_cursor = MempoolCursorTxid::parse(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .expect("stale cursor should parse");
            crate::db::update_address_mempool_backfill_cursor(
                capped_run.user_id,
                address.address_id,
                Some(&stale_cursor),
            )
            .expect("stale cursor should seed");
            let known_activity = HashSet::from([address.address_id]);
            let pending = HashSet::new();
            let capped_server = start_historical_sync_mempool_server(vec![mempool_stats_json(2)]);
            let capped_http_counters = SyncHttpCounters::new();
            let capped_client = live_mempool_client(
                capped_run.user_id,
                &capped_server.base_url,
                &capped_http_counters,
            );
            let mut capped_bundle = load_test_hd_bundle(capped_run, account_id);
            let mut capped_chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut capped_accumulator = CycleAccumulator::new(1);
            let mut capped_executor = LiveAddressSyncExecutor::new();

            run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                run: capped_run,
                clients: SyncClients {
                    mempool_client: Some(&capped_client),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &capped_http_counters,
                },
                pending_address_ids: &pending,
                bundle: &mut capped_bundle,
                known_activity: &known_activity,
                chain_tip_cache: &mut capped_chain_tip_cache,
                accumulator: &mut capped_accumulator,
                sync_executor: &mut capped_executor,
                policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(1),
                },
            })
            .expect("capped statistics visit should succeed");
            let capped_requests = capped_server.join();

            assert_eq!(
                capped_requests,
                vec![format!(
                    "GET /api/address/{} HTTP/1.1",
                    address.address.as_str()
                )]
            );
            let capped_address =
                crate::db::get_sync_addresses_for_account(capped_run.user_id, account_id)
                    .expect("capped address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("capped address should persist");
            assert_eq!(capped_address.mempool_history_proof, Some(old_proof));
            assert_eq!(capped_address.mempool_backfill_cursor_txid, None);
            assert_eq!(
                capped_address.mempool_expected_tx_count,
                Some(TransactionCount::from_u32(2))
            );

            clock.sleep(Duration::from_secs(91));
            let raised_run = next_run_for_user(&clock, capped_run.user_id);
            let raised_server = start_historical_sync_mempool_server(vec![
                mempool_stats_json(2),
                mempool_page_json(
                    &address,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ),
            ]);
            let raised_http_counters = SyncHttpCounters::new();
            let raised_client = live_mempool_client(
                raised_run.user_id,
                &raised_server.base_url,
                &raised_http_counters,
            );
            let mut raised_bundle = load_test_hd_bundle(raised_run, account_id);
            let mut raised_chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut raised_accumulator = CycleAccumulator::new(1);
            let mut raised_executor = LiveAddressSyncExecutor::new();

            run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                run: raised_run,
                clients: SyncClients {
                    mempool_client: Some(&raised_client),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &raised_http_counters,
                },
                pending_address_ids: &pending,
                bundle: &mut raised_bundle,
                known_activity: &known_activity,
                chain_tip_cache: &mut raised_chain_tip_cache,
                accumulator: &mut raised_accumulator,
                sync_executor: &mut raised_executor,
                policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(2),
                },
            })
            .expect("raised-cap restart should succeed");
            assert_eq!(
                raised_accumulator.addresses_skipped_tip_unchanged, 0,
                "proof/count restart work must bypass the unchanged-tip gate"
            );
            let raised_requests = raised_server.join();

            assert_eq!(
                raised_requests,
                vec![
                    format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                    format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str()),
                ]
            );
        });
    }

    #[test]
    fn auto_upgrade_ingests_unproven_expected_bitcoin_history_after_free_balance_sync() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let base_run = make_run_context(&clock);
            let run = RunContext {
                source: TriggerSource::AutoUpgrade,
                ..base_run
            };
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qunprovenupgradehistory",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            seed_account_history_frontier(run, account_id, address.address_id, 0);
            crate::db::mark_address_sync_started(
                run.user_id,
                address.address_id,
                run.run_id,
                run.started_at,
            )
            .expect("free-tier sync state should seed");
            crate::db::persist_mempool_address_observation_success(
                run.user_id,
                crate::db::MempoolAddressObservationSuccess {
                    address_id: address.address_id,
                    confirmed_tx_count: TransactionCount::from_u32(1),
                    confirmed_balance: Some(
                        crate::transactions::ApiConfirmedBalance::from_smallest_unit_i64(1)
                            .expect("balance should parse"),
                    ),
                    tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
                    observed_at: run.started_at,
                },
            )
            .expect("free-tier provider observation should seed");
            crate::db::update_address_mempool_expected_tx_count(
                run.user_id,
                address.address_id,
                Some(TransactionCount::from_u32(1)),
            )
            .expect("free-tier expected count should seed");

            let server = start_historical_sync_mempool_server(vec![
                mempool_stats_json(1),
                mempool_page_json(
                    &address,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ),
            ]);
            let http_counters = SyncHttpCounters::new();
            let client = live_mempool_client(run.user_id, &server.base_url, &http_counters);
            let mut bundle = load_test_hd_bundle(run, account_id);
            let mut chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut accumulator = CycleAccumulator::new(1);
            let mut executor = LiveAddressSyncExecutor::new();
            let pending = HashSet::new();

            run_hd_mempool_history_breadth_round(HdMempoolHistoryBreadthRoundRequest {
                run,
                clients: SyncClients {
                    mempool_client: Some(&client),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                pending_address_ids: &pending,
                bundle: &mut bundle,
                known_activity: &HashSet::new(),
                chain_tip_cache: &mut chain_tip_cache,
                accumulator: &mut accumulator,
                sync_executor: &mut executor,
                policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(1),
                },
            })
            .expect("upgrade history breadth round should succeed");
            let requests = server.join();

            assert_eq!(
                requests,
                vec![
                    format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                    format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str()),
                ]
            );
            assert_eq!(
                crate::db::load_canonical_confirmed_account_transaction_count(
                    run.user_id,
                    account_id,
                )
                .expect("canonical transaction count should load"),
                TransactionCount::from_u32(1)
            );
        });
    }

    #[test]
    fn task5_persisted_single_address_page_uses_no_account_frontier() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qtask5singleaddress",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                None,
                None,
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let server = start_historical_sync_mempool_server(vec![
                mempool_stats_json(2),
                mempool_page_json(
                    &address,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ),
            ]);
            let http_counters = SyncHttpCounters::new();
            let client = live_mempool_client(run.user_id, &server.base_url, &http_counters);
            let mut chain_tip_cache = cached_bitcoin_chain_tip(&clock);
            let mut accumulator = CycleAccumulator::new(1);
            let mut processed_for_account = 0_u32;
            let mut executor = LiveAddressSyncExecutor::new();
            let pending = HashSet::new();

            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address: &mut address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending,
                clients: SyncClients {
                    mempool_client: Some(&client),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                executor: &mut executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(10),
                },
                mempool_history_page_frontier: None,
            })
            .expect("single-address page should succeed");
            let requests = server.join();

            assert_eq!(
                requests,
                vec![
                    format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                    format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str()),
                ]
            );
            let persisted = crate::db::get_sync_addresses_for_account(run.user_id, account_id)
                .expect("single address should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("single address should persist");
            assert_eq!(
                persisted
                    .mempool_backfill_cursor_txid
                    .as_ref()
                    .map(MempoolCursorTxid::as_str),
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            );
            let account_sync_state_count = crate::db::with_user_db(run.user_id, |conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM account_sync_state WHERE account_id = ?1",
                    [account_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| {
                    crate::db::DbError::new(format!(
                        "single-address account-state count failed: {error}"
                    ))
                })
            })
            .expect("single-address account state count should load");
            assert_eq!(account_sync_state_count, 0);
        });
    }

    #[test]
    fn capped_mempool_cursor_settles_and_resumes_after_cap_raise() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let first_run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qcappedcursortransition",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                None,
                None,
            );
            persist_sync_addresses_for_test(first_run, std::slice::from_ref(&address));
            let capped_policy = MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(5),
            };
            let first_page = mempool_page_json_with_count(&address, 6);
            let expected_cursor = MempoolCursorTxid::parse(&format!("{:064x}", 6))
                .expect("fixture cursor should parse");

            let first_requests = run_live_single_address_cycle(
                first_run,
                &clock,
                &mut address,
                vec![mempool_stats_json(100), first_page],
                capped_policy,
            );

            assert_eq!(
                first_requests,
                vec![
                    format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                    format!("GET /api/address/{}/txs HTTP/1.1", address.address.as_str()),
                ]
            );
            let first_completed =
                crate::db::get_sync_addresses_for_account(first_run.user_id, account_id)
                    .expect("first-page address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("first-page address should exist");
            assert_eq!(
                first_completed.mempool_backfill_cursor_txid.as_ref(),
                Some(&expected_cursor)
            );
            assert_eq!(
                first_completed.last_result,
                Some(TransactionSyncResult::Success)
            );
            assert_eq!(
                crate::db::load_canonical_confirmed_account_transaction_count(
                    first_run.user_id,
                    account_id,
                )
                .expect("canonical count should load"),
                TransactionCount::from_u32(6)
            );

            clock.sleep(Duration::from_secs(30 * 60 + 1));
            let capped_run = next_run_for_user(&clock, first_run.user_id);
            let mut capped_address =
                crate::db::get_sync_addresses_for_account(capped_run.user_id, account_id)
                    .expect("capped address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("capped address should exist");
            let capped_requests = run_live_single_address_cycle(
                capped_run,
                &clock,
                &mut capped_address,
                vec![mempool_stats_json(100)],
                capped_policy,
            );

            assert_eq!(
                capped_requests,
                vec![format!(
                    "GET /api/address/{} HTTP/1.1",
                    address.address.as_str()
                )]
            );
            let capped_address =
                crate::db::get_sync_addresses_for_account(capped_run.user_id, account_id)
                    .expect("capped address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("capped address should exist");
            assert_eq!(
                capped_address.mempool_backfill_cursor_txid.as_ref(),
                Some(&expected_cursor)
            );
            assert_eq!(
                capped_address.last_result,
                Some(TransactionSyncResult::Success)
            );

            let snapshot = crate::db::load_account_sync_snapshots(capped_run.user_id)
                .expect("snapshot should load")
                .into_iter()
                .find(|snapshot| snapshot.account_id == account_id)
                .expect("account snapshot should exist");
            assert_eq!(snapshot.addresses_in_progress.value(), 0);
            assert_eq!(
                snapshot.last_result,
                Some(crate::transactions::AccountSyncResult::Success)
            );
            assert!(
                snapshot
                    .integration_states
                    .iter()
                    .all(|integration| !integration.is_active)
            );
            assert_eq!(
                snapshot
                    .backfill_progress
                    .as_ref()
                    .expect("retained progress should remain")
                    .state
                    .cursor,
                AddressBackfillCursor::Mempool {
                    cursor_txid: expected_cursor.clone(),
                }
            );
            let mempool_integration = snapshot
                .integration_states
                .first()
                .expect("mempool integration should exist");
            assert_eq!(
                mempool_integration.last_result,
                Some(AggregateSyncResult::Success)
            );

            let mut stored_counts = HashMap::new();
            let page_permitted = load_mempool_history_page_permission(
                capped_run.user_id,
                &capped_address,
                &HashSet::new(),
                capped_policy,
                &mut stored_counts,
            )
            .expect("capped permission should load");
            assert!(!page_permitted);
            assert!(!address_has_unfinished_work(
                &capped_address,
                &HashSet::new(),
                page_permitted,
            ));

            stored_counts.clear();
            let raised_policy = MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(7),
            };
            let page_permitted = load_mempool_history_page_permission(
                capped_run.user_id,
                &capped_address,
                &HashSet::new(),
                raised_policy,
                &mut stored_counts,
            )
            .expect("raised-cap permission should load");
            assert!(page_permitted);
            assert!(address_has_unfinished_work(
                &capped_address,
                &HashSet::new(),
                page_permitted,
            ));

            clock.sleep(Duration::from_secs(91));
            let raised_run = next_run_for_user(&clock, first_run.user_id);
            let mut raised_address =
                crate::db::get_sync_addresses_for_account(raised_run.user_id, account_id)
                    .expect("raised-cap address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("raised-cap address should exist");
            let raised_requests = run_live_single_address_cycle(
                raised_run,
                &clock,
                &mut raised_address,
                vec![mempool_stats_json(100), "[]".to_string()],
                raised_policy,
            );

            assert_eq!(
                raised_requests,
                vec![
                    format!("GET /api/address/{} HTTP/1.1", address.address.as_str()),
                    format!(
                        "GET /api/address/{}/txs/chain/{} HTTP/1.1",
                        address.address.as_str(),
                        expected_cursor.as_str(),
                    ),
                ]
            );
            let resumed_address =
                crate::db::get_sync_addresses_for_account(raised_run.user_id, account_id)
                    .expect("resumed address should load")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("resumed address should exist");
            assert_eq!(
                resumed_address.mempool_backfill_cursor_txid.as_ref(),
                Some(&expected_cursor)
            );
        });
    }

    #[test]
    fn schedule_hint_for_run_retries_unfinished_work_on_shorter_interval() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let summary = empty_sync_summary(run.run_id, 1);

        let hint = schedule_hint_for_run(run, TriggerSource::Schedule, &summary, true);

        assert_eq!(
            hint.interval,
            super::super::context::USER_TRANSACTION_MONITOR_MIN_INTERVAL.as_duration()
        );
        assert_eq!(
            hint.reason,
            super::super::context::UserTransactionMonitorScheduleReason::UnfinishedWork
        );
    }

    #[test]
    fn schedule_hint_for_run_backs_off_idle_users() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let summary = empty_sync_summary(run.run_id, 3);

        let hint = schedule_hint_for_run(run, TriggerSource::Schedule, &summary, false);

        assert_eq!(
            hint.interval,
            super::super::context::USER_TRANSACTION_MONITOR_IDLE_INTERVAL.as_duration()
        );
        assert_eq!(
            hint.reason,
            super::super::context::UserTransactionMonitorScheduleReason::Idle
        );
    }

    #[test]
    fn schedule_hint_for_run_does_not_schedule_before_rate_limit_unblock() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let summary = empty_sync_summary(run.run_id, 1);

            record_rate_limit(
                run.user_id,
                LABEL_MEMPOOL,
                clock.instant_now(),
                Some(Duration::from_secs(120)),
            );

            let hint = schedule_hint_for_run(run, TriggerSource::Schedule, &summary, false);

            assert_eq!(hint.interval, Duration::from_secs(120));
            assert_eq!(
                hint.reason,
                super::super::context::UserTransactionMonitorScheduleReason::RateLimited
            );
        });
    }

    #[test]
    fn schedule_hint_for_parent_run_waits_for_blocked_integration_when_only_blocked_work_remains() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let summary = empty_sync_summary(run.run_id, 1);
            let child_summaries = vec![integration_child_summary_from_summary(
                SyncIntegrationId::Etherscan,
                make_summary_for_test(run.run_id, 1, 0, 0, 1, &[LABEL_ETHERSCAN], None),
            )];
            let unfinished_integrations = HashSet::from([SyncIntegrationId::Etherscan]);

            record_rate_limit(
                run.user_id,
                LABEL_ETHERSCAN,
                clock.instant_now(),
                Some(Duration::from_secs(120)),
            );

            let hint = schedule_hint_for_parent_run(
                run,
                TriggerSource::Schedule,
                &summary,
                &child_summaries,
                &unfinished_integrations,
            );

            assert_eq!(hint.interval, super::super::ETHERSCAN_RATE_LIMIT_BACKOFF);
            assert_eq!(
                hint.reason,
                super::super::context::UserTransactionMonitorScheduleReason::RateLimited
            );
        });
    }

    #[test]
    fn schedule_hint_for_parent_run_retries_when_unblocked_work_is_still_unfinished() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let summary = empty_sync_summary(run.run_id, 2);
            let child_summaries = vec![
                integration_child_summary_from_summary(
                    SyncIntegrationId::Mempool,
                    make_summary_for_test(run.run_id, 1, 1, 0, 0, &[], None),
                ),
                integration_child_summary_from_summary(
                    SyncIntegrationId::Etherscan,
                    make_summary_for_test(run.run_id, 1, 0, 0, 1, &[LABEL_ETHERSCAN], None),
                ),
            ];
            let unfinished_integrations =
                HashSet::from([SyncIntegrationId::Mempool, SyncIntegrationId::Etherscan]);

            record_rate_limit(
                run.user_id,
                LABEL_ETHERSCAN,
                clock.instant_now(),
                Some(Duration::from_secs(120)),
            );

            let hint = schedule_hint_for_parent_run(
                run,
                TriggerSource::Schedule,
                &summary,
                &child_summaries,
                &unfinished_integrations,
            );

            assert_eq!(
                hint.interval,
                super::super::context::USER_TRANSACTION_MONITOR_MIN_INTERVAL.as_duration()
            );
            assert_eq!(
                hint.reason,
                super::super::context::UserTransactionMonitorScheduleReason::UnfinishedWork
            );
        });
    }

    #[test]
    fn run_sync_cycle_uses_planner_order_for_mixed_asset_accounts() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_a = crate::wallets::DigitalAssetAccountId::new();
            let account_b = crate::wallets::DigitalAssetAccountId::new();
            let non_hd_addresses = vec![
                make_sync_address(
                    "bc1qorder000",
                    SyncedAssetId::Bitcoin,
                    Network::Mainnet,
                    Some(account_a),
                    Some(crate::wallets::AddressScheme::NativeSegwit),
                    Some(0),
                    Some(0),
                ),
                make_sync_address(
                    "0x3333333333333333333333333333333333333333",
                    SyncedAssetId::Ethereum,
                    Network::Mainnet,
                    Some(account_b),
                    Some(crate::wallets::AddressScheme::Standard),
                    Some(0),
                    Some(0),
                ),
                make_sync_address(
                    "bc1qorder001",
                    SyncedAssetId::Bitcoin,
                    Network::Mainnet,
                    Some(account_a),
                    Some(crate::wallets::AddressScheme::NativeSegwit),
                    Some(0),
                    Some(1),
                ),
            ];
            persist_sync_addresses_for_test(run, &non_hd_addresses);
            let mut expected_call_order = non_hd_addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<DigitalAssetAddressId>>();
            expected_call_order.sort_by_key(|address_id| address_id.to_string());
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let http_counters = SyncHttpCounters::new();
            let preload = empty_sync_run_preload();
            let summary = run_sync_cycle(SyncCycleRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                preload: &preload,
                non_hd_addresses,
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("sync cycle should succeed");

            assert_eq!(summary.addresses_total.value(), 3);
            assert_eq!(summary.addresses_synced.value(), 3);
            assert_eq!(summary.addresses_failed.value(), 0);
            assert_eq!(summary.addresses_skipped.value(), 0);
            assert_eq!(summary.addresses_skipped_tip_unchanged.value(), 0);
            assert_eq!(summary.addresses_early_exited.value(), 0);
            assert_eq!(summary.new_tx_count.value(), 0);
            assert_eq!(summary.updated_tx_count.value(), 0);
            assert_eq!(executor.calls, expected_call_order);
            assert!(
                executor
                    .observed_lock_free
                    .iter()
                    .all(|is_lock_free| *is_lock_free),
                "executor dispatch should observe no user-db locks"
            );
            assert!(derivation_provider.requests.is_empty());
            assert_eq!(clock.sleep_count(), 2);
        });
    }

    fn persist_confirmed_output_for_account_count(
        run: RunContext<'_>,
        address: &SyncAddress,
        tx_hash: &str,
    ) {
        let record = crate::db::SyncTransactionRecord {
            tx_hash: TxHash::parse(tx_hash).expect("tx hash should parse"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("test-block".to_string()),
            block_time: Some(run.started_at),
            fee_amount: Some(0),
            inputs: Vec::new(),
            outputs: vec![crate::db::SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(address.address.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 1,
            }],
        };
        crate::db::reconcile_address_transactions(
            run.user_id,
            address.asset_id,
            address.network,
            &[record],
            run.started_at,
        )
        .expect("canonical count fixture should persist");
    }

    #[test]
    fn run_sync_cycle_stops_when_planner_has_only_blocked_addresses() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qblocked000",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            address.consecutive_failure_count =
                ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let http_counters = SyncHttpCounters::new();
            let preload = empty_sync_run_preload();

            let summary = run_sync_cycle(SyncCycleRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                preload: &preload,
                non_hd_addresses: vec![address],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("blocked cycle should stop cleanly");

            assert_eq!(summary.addresses_total.value(), 1);
            assert_eq!(summary.addresses_synced.value(), 0);
            assert_eq!(summary.addresses_failed.value(), 0);
            assert!(executor.calls.is_empty());
        });
    }

    #[test]
    fn repair_owned_threshold_failure_runs_after_failure_cooldown() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qrepairthresholdretry",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            address.last_tip_height = Some(ChainTipHeight::try_new(40).expect("tip should parse"));
            address.last_result = Some(TransactionSyncResult::Failure);
            address.last_completed_at = Some(
                run.started_at
                    - chrono::Duration::from_std(super::super::FAILED_ADDRESS_SYNC_COOLDOWN)
                        .expect("cooldown should convert"),
            );
            address.consecutive_failure_count =
                ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let http_counters = SyncHttpCounters::new();
            let mut preload = empty_sync_run_preload();
            preload
                .bitcoin_history_repair_account_ids
                .insert(account_id);

            let summary = run_sync_cycle(SyncCycleRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                preload: &preload,
                non_hd_addresses: vec![address],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("repair-owned threshold failure should retry");

            assert_eq!(summary.addresses_synced.value(), 1);
            assert_eq!(summary.addresses_skipped.value(), 0);
            assert_eq!(executor.calls.len(), 1);
        });
    }

    #[test]
    fn accumulated_strict_mismatch_failure_retries_first_page_after_backoff() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let first_run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qrepairmismatchbackoff",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            address.last_tip_height = Some(ChainTipHeight::try_new(40).expect("tip should parse"));
            address.last_result = Some(TransactionSyncResult::Failure);
            address.last_completed_at = Some(
                first_run.started_at
                    - chrono::Duration::from_std(super::super::FAILED_ADDRESS_SYNC_COOLDOWN)
                        .expect("cooldown should convert"),
            );
            address.consecutive_failure_count =
                ConsecutiveFailureCount::try_new(2).expect("failure count should parse");
            persist_sync_addresses_for_test(first_run, std::slice::from_ref(&address));
            crate::db::mark_address_sync_started(
                first_run.user_id,
                address.address_id,
                first_run.run_id,
                first_run.started_at,
            )
            .expect("sync state should seed");
            crate::db::with_user_db_mut(first_run.user_id, |conn| {
                let changed = conn
                    .execute(
                        "UPDATE transaction_sync_state
                     SET consecutive_failure_count = 2
                     WHERE address_id = ?1",
                        rusqlite::params![address.address_id.to_string()],
                    )
                    .map_err(|error| {
                        crate::db::DbError::new(format!(
                            "failure-count fixture update failed: {error}"
                        ))
                    })?;
                assert_eq!(changed, 1, "failure-count fixture should update one row");
                Ok::<(), crate::db::DbError>(())
            })
            .expect("failure count should seed");
            persist_confirmed_output_for_account_count(
                first_run,
                &address,
                "abababababababababababababababababababababababababababababababab",
            );
            let mut preload = empty_sync_run_preload();
            preload
                .bitcoin_history_repair_account_ids
                .insert(account_id);
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = RestartingStrictMismatchExecutor::new();
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());

            let first_summary = run_sync_cycle(SyncCycleRequest {
                run: first_run,
                clients,
                preload: &preload,
                non_hd_addresses: vec![address],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("threshold repair attempt should run and record mismatch");
            assert_eq!(first_summary.addresses_failed.value(), 1);
            let failed = crate::db::get_sync_addresses_for_account(first_run.user_id, account_id)
                .expect("failed address should reload")
                .into_iter()
                .next()
                .expect("failed address should exist");
            assert_eq!(failed.consecutive_failure_count.value(), 3);
            assert_eq!(failed.mempool_backfill_cursor_txid, None);

            let cooldown_run = RunContext {
                source: TriggerSource::Schedule,
                ..next_run_for_user(&clock, first_run.user_id)
            };
            let cooldown_summary = run_sync_cycle(SyncCycleRequest {
                run: cooldown_run,
                clients,
                preload: &preload,
                non_hd_addresses: vec![failed],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("cooldown pass should skip without failing");
            assert_eq!(cooldown_summary.addresses_synced.value(), 0);
            assert_eq!(cooldown_summary.addresses_failed.value(), 0);
            assert_eq!(executor.observed_cursors, vec![None]);

            clock.sleep(super::super::FAILED_ADDRESS_SYNC_COOLDOWN + Duration::from_secs(1));
            let retry_run = RunContext {
                source: TriggerSource::Schedule,
                ..next_run_for_user(&clock, first_run.user_id)
            };
            let retry_address =
                crate::db::get_sync_addresses_for_account(retry_run.user_id, account_id)
                    .expect("retry address should reload")
                    .into_iter()
                    .next()
                    .expect("retry address should exist");
            let retry_summary = run_sync_cycle(SyncCycleRequest {
                run: retry_run,
                clients,
                preload: &preload,
                non_hd_addresses: vec![retry_address],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("repair should retry after failure cooldown");

            assert_eq!(retry_summary.addresses_synced.value(), 1);
            assert_eq!(executor.observed_cursors, vec![None, None]);
        });
    }

    #[test]
    fn run_sync_cycle_uses_balance_refresh_when_account_is_at_transaction_cap() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = crate::wallets::DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qcap000",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run, std::slice::from_ref(&address));
            persist_confirmed_output_for_account_count(
                run,
                &address,
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            );
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(Vec::new());
            let http_counters = SyncHttpCounters::new();
            let mut preload = empty_sync_run_preload();
            preload.historical_backfill_transactions_per_account = 1;

            let summary = run_sync_cycle(SyncCycleRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                preload: &preload,
                non_hd_addresses: vec![address.clone()],
                hd_bundles: Vec::new(),
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("cap-limited cycle should use balance refresh");

            assert_eq!(summary.addresses_synced.value(), 1);
            assert_eq!(executor.calls, vec![address.address_id]);
            assert_eq!(executor.historical_backfill_enabled_calls, vec![false]);
        });
    }

    #[test]
    fn bitcoin_history_full_resync_selects_cap_exempt_policy_only_for_owned_account() {
        let repair_account_id = DigitalAssetAccountId::new();
        let normal_account_id = DigitalAssetAccountId::new();
        let mut preload = empty_sync_run_preload();
        preload.historical_backfill_enabled = true;
        preload.historical_backfill_transactions_per_account = 1;
        preload
            .bitcoin_history_repair_account_ids
            .insert(repair_account_id);

        assert_eq!(
            mempool_history_policy_for_account(&preload, Some(repair_account_id)),
            MempoolHistoryPolicy::LegacyRepair
        );
        assert_eq!(
            mempool_history_policy_for_account(&preload, Some(normal_account_id)),
            MempoolHistoryPolicy::Normal {
                cap: TransactionCount::from_u32(1)
            }
        );
    }

    #[test]
    fn prioritize_non_hd_addresses_orders_by_urgency_tier() {
        let mut unfinished = make_sync_address(
            "bc1qpriorityunfinished",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        unfinished.last_tip_height = Some(ChainTipHeight::try_new(10).expect("valid tip"));
        unfinished.mempool_backfill_cursor_txid = Some(
            MempoolCursorTxid::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("cursor should parse"),
        );
        let unfinished_id = unfinished.address_id;

        let mut pending = make_sync_address(
            "0x4444444444444444444444444444444444444444",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        pending.last_tip_height = Some(ChainTipHeight::try_new(20).expect("valid tip"));
        let pending_id = pending.address_id;

        let first_sync = make_sync_address(
            "0x5555555555555555555555555555555555555555",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        let first_sync_id = first_sync.address_id;

        let mut recent = make_sync_address(
            "bc1qpriorityrecent",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        recent.last_tip_height = Some(ChainTipHeight::try_new(30).expect("valid tip"));
        let recent_id = recent.address_id;

        let mut cold = make_sync_address(
            "bc1qprioritycold",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        cold.last_tip_height = Some(ChainTipHeight::try_new(40).expect("valid tip"));
        let cold_id = cold.address_id;

        let mut addresses = vec![cold, first_sync, recent, pending, unfinished];
        let mut preload = empty_sync_run_preload();
        preload.pending_address_ids.insert(pending_id);
        preload.known_activity_address_ids.insert(recent_id);

        let counts = HashMap::new();
        let excluded = HashSet::new();
        let planner_input = planner_input_for_preload(&preload, test_utc_now(), &counts, &excluded);
        sort_addresses_by_planner_priority(&mut addresses, &planner_input);

        assert_eq!(
            addresses
                .iter()
                .map(|address| address.address_id)
                .collect::<Vec<_>>(),
            vec![unfinished_id, pending_id, first_sync_id, recent_id, cold_id]
        );
    }

    #[test]
    fn failed_address_has_unfinished_work_for_retry_scheduling() {
        let mut address = make_sync_address(
            "bc1qfailedunfinishedwork",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        address.last_tip_height = Some(ChainTipHeight::try_new(40).expect("valid tip"));
        address.last_result = Some(TransactionSyncResult::Failure);

        assert!(address_has_unfinished_work(
            &address,
            &HashSet::new(),
            false
        ));
    }

    #[test]
    fn failed_address_at_threshold_is_not_unfinished_retry_work() {
        let mut address = make_sync_address(
            "bc1qfailedthresholdunfinished",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            None,
            None,
            None,
            None,
        );
        address.last_tip_height = Some(ChainTipHeight::try_new(40).expect("valid tip"));
        address.last_result = Some(TransactionSyncResult::Failure);
        address.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("valid count");

        assert!(!address_has_unfinished_work(
            &address,
            &HashSet::new(),
            false
        ));
    }

    #[test]
    fn repair_owned_failed_address_at_threshold_remains_unfinished_retry_work() {
        let account_id = DigitalAssetAccountId::new();
        let mut address = make_sync_address(
            "bc1qrepairfailedthresholdunfinished",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        address.last_tip_height = Some(ChainTipHeight::try_new(40).expect("valid tip"));
        address.last_result = Some(TransactionSyncResult::Failure);
        address.consecutive_failure_count =
            ConsecutiveFailureCount::try_new(2).expect("valid count");

        assert!(address_has_unfinished_work(
            &address,
            &HashSet::from([account_id]),
            false,
        ));
    }

    #[test]
    fn capped_mempool_cursor_is_unfinished_only_when_a_page_is_permitted() {
        let account_id = DigitalAssetAccountId::new();
        let mut address = make_sync_address(
            "bc1qcappedcursorunfinished",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(account_id),
            None,
            None,
            None,
        );
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("valid tip"));
        address.last_result = Some(TransactionSyncResult::Success);
        address.mempool_backfill_cursor_txid = Some(
            MempoolCursorTxid::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .expect("cursor should parse"),
        );

        assert!(!address_has_unfinished_work(
            &address,
            &HashSet::new(),
            false
        ));
        assert!(address_has_unfinished_work(&address, &HashSet::new(), true));
    }

    #[test]
    fn etherscan_cursor_remains_unfinished_without_mempool_permission() {
        let mut address = make_sync_address(
            "0x7777777777777777777777777777777777777777",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(DigitalAssetAccountId::new()),
            None,
            None,
            None,
        );
        address.last_tip_height = Some(ChainTipHeight::try_new(100).expect("valid tip"));
        address.last_result = Some(TransactionSyncResult::Success);
        address.etherscan_backfill_end_block =
            Some(crate::transactions::EthereumBlockNumber::try_new(50).expect("valid block"));

        assert!(address_has_unfinished_work(
            &address,
            &HashSet::new(),
            false
        ));
    }

    #[test]
    fn filter_non_hd_addresses_for_account_scope_keeps_only_matching_account() {
        let target_account_id = DigitalAssetAccountId::new();
        let other_account_id = DigitalAssetAccountId::new();
        let matching = make_sync_address(
            "0x7777777777777777777777777777777777777777",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(target_account_id),
            None,
            None,
            None,
        );
        let matching_id = matching.address_id;
        let other = make_sync_address(
            "0x8888888888888888888888888888888888888888",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(other_account_id),
            None,
            None,
            None,
        );

        let filtered = filter_non_hd_addresses_for_scope(
            vec![other, matching],
            TransactionSyncScope::Account {
                account_id: target_account_id,
            },
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].address_id, matching_id);
    }

    #[test]
    fn filter_hd_bundles_for_address_scope_keeps_only_matching_address() {
        let account_id = DigitalAssetAccountId::new();
        let matching = make_sync_address(
            "0x9999999999999999999999999999999999999999",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::Standard),
            Some(0),
            Some(5),
        );
        let matching_id = matching.address_id;
        let other_external = make_sync_address(
            "0xaAaAaAaaAaAaAaaAaAAAAAAAAaaaAaAaAaaAaaAa",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::Standard),
            Some(0),
            Some(6),
        );
        let other_internal = make_sync_address(
            "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::Standard),
            Some(1),
            Some(0),
        );

        let filtered = filter_hd_bundles_for_scope(
            vec![AccountSyncBundle {
                account_id,
                asset_id: SyncedAssetId::Ethereum,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "zpub-test".to_string(),
                address_scheme: crate::wallets::AddressScheme::Standard,
                sync_state: None,
                external_addresses: vec![other_external, matching],
                internal_addresses: vec![other_internal],
            }],
            TransactionSyncScope::Address {
                address_id: matching_id,
            },
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].external_addresses.len(), 1);
        assert_eq!(filtered[0].external_addresses[0].address_id, matching_id);
        assert!(filtered[0].internal_addresses.is_empty());
    }

    #[test]
    fn run_sync_cycle_derives_empty_hd_bundle() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let bundle = AccountSyncBundle {
                account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                hd_key_extended_pubkey: "zpub-empty-bootstrap".to_string(),
                address_scheme: crate::wallets::AddressScheme::NativeSegwit,
                sync_state: None,
                external_addresses: Vec::new(),
                internal_addresses: Vec::new(),
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);
            let mut derivation_provider = FakeAddressDerivationProvider::new(vec![
                vec![make_derived_sync_address("bc1qderived000", 0, 0)],
                vec![make_derived_sync_address("bc1qderived100", 1, 0)],
                vec![make_derived_sync_address("bc1qderived001", 0, 1)],
                vec![make_derived_sync_address("bc1qderived101", 1, 1)],
            ]);
            let http_counters = SyncHttpCounters::new();
            let preload = empty_sync_run_preload();

            let summary = run_sync_cycle(SyncCycleRequest {
                run,
                clients: SyncClients {
                    mempool_client: None,
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &http_counters,
                },
                preload: &preload,
                non_hd_addresses: Vec::new(),
                hd_bundles: vec![bundle],
                known_activity: HashSet::new(),
                sync_executor: &mut executor,
                derivation_provider: &mut derivation_provider,
            })
            .expect("sync cycle should derive empty HD bundle");

            assert_eq!(summary.addresses_failed.value(), 0);
            assert!(
                derivation_provider.requests.len() >= 2,
                "empty HD bundle should trigger at least 2 derivation requests, got {}",
                derivation_provider.requests.len()
            );
            for request in &derivation_provider.requests {
                assert_eq!(request.account_id, account_id);
                assert!(request.count > 0, "derivation count should be positive");
            }
            let external_derivation = derivation_provider
                .requests
                .iter()
                .any(|r| r.derivation_change == 0);
            let internal_derivation = derivation_provider
                .requests
                .iter()
                .any(|r| r.derivation_change == 1);
            assert!(external_derivation, "planner should derive external chain");
            assert!(internal_derivation, "planner should derive internal chain");
            assert!(
                executor.calls.len() >= 2,
                "empty HD bundle should trigger at least 2 sync executor calls, got {}",
                executor.calls.len()
            );
        });
    }

    fn btc_stats_json(tx_count: u32, funded: u64, spent: u64) -> String {
        format!(
            r#"{{"chain_stats":{{"tx_count":{tx_count},"funded_txo_sum":{funded},"spent_txo_sum":{spent}}},"mempool_stats":{{"tx_count":0}}}}"#
        )
    }

    /// Newest-first page. Each entry is (txid, value_sat, block_height).
    fn btc_page_json(address: &SyncAddress, entries: &[(&str, u64, i64)]) -> String {
        let items = entries
            .iter()
            .map(|(txid, value, height)| {
                format!(
                    r#"{{"txid":"{txid}","vin":[],"vout":[{{"scriptpubkey":"00","scriptpubkey_address":"{}","value":{value}}}],"fee":0,"status":{{"confirmed":true,"block_height":{height},"block_hash":"block{height}","block_time":{}}}}}"#,
                    address.address.as_str(),
                    1_700_000_000_i64 + height
                )
            })
            .collect::<Vec<String>>()
            .join(",");
        format!("[{items}]")
    }

    fn bitcoin_chain_tip_at(clock: &FakeClock, height: i64) -> ChainTipCache {
        let mut cache = ChainTipCache::default();
        cache.tips.insert(
            chain_tip_cache_key(SyncedAssetId::Bitcoin, Network::Mainnet),
            CachedChainTip {
                height: ChainTipHeight::try_new(height).expect("tip should parse"),
                fetched_at: clock.instant_now(),
            },
        );
        cache
    }

    fn closing_balance_lo_for(
        user_id: UserId,
        account_id: DigitalAssetAccountId,
        tx_hash: &str,
    ) -> Option<i64> {
        crate::db::with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1 AND tx_hash = ?2",
                rusqlite::params![account_id.to_string(), tx_hash],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|err| {
                crate::db::DbError::new(format!("Failed to read closing balance: {err}"))
            })
            .map(|value| value.flatten())
        })
        .expect("closing balance query should succeed")
    }

    const T2_TXID: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const T3_TXID: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    #[test]
    fn capped_stats_only_refresh_preserves_existing_closing_balances() {
        with_rate_limiter_isolated(|| {
            let clock = FakeClock::new(test_utc_now());
            let run_b = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qcappedledgerstability",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            );
            persist_sync_addresses_for_test(run_b, std::slice::from_ref(&address));

            // Block B: provider reports 3 confirmed transactions totalling 123 sat.
            // The cap of 2 admits only the newest page (T3, T2). T1 stays unsynced.
            let server_b = start_historical_sync_mempool_server(vec![
                btc_stats_json(3, 123, 0),
                btc_page_json(&address, &[(T3_TXID, 3, 3), (T2_TXID, 20, 2)]),
            ]);
            let counters_b = SyncHttpCounters::new();
            let client_b = live_mempool_client(run_b.user_id, &server_b.base_url, &counters_b);
            let mut tip_cache_b = bitcoin_chain_tip_at(&clock, 100);
            let mut accumulator_b = CycleAccumulator::new(1);
            let mut executor_b = LiveAddressSyncExecutor::new();
            let mut address_b = address.clone();
            let mut processed_b = 0_u32;
            let pending = HashSet::new();

            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run: run_b,
                address: &mut address_b,
                chain_tip_cache: &mut tip_cache_b,
                pending_address_ids: &pending,
                clients: SyncClients {
                    mempool_client: Some(&client_b),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &counters_b,
                },
                executor: &mut executor_b,
                accumulator: &mut accumulator_b,
                processed_for_account: &mut processed_b,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(2),
                },
                mempool_history_page_frontier: None,
            })
            .expect("block B sync should succeed");
            accumulator_b
                .rebuild_account_if_touched(run_b.user_id, account_id, clock.utc_now())
                .expect("block B finalization should succeed");
            server_b.join();

            assert_eq!(
                closing_balance_lo_for(run_b.user_id, account_id, T2_TXID),
                Some(120),
                "block B must establish T2 closing balance of 120 sat"
            );
            assert_eq!(
                closing_balance_lo_for(run_b.user_id, account_id, T3_TXID),
                Some(123),
                "block B must establish T3 closing balance of 123 sat"
            );

            crate::db::with_user_db_mut(run_b.user_id, |conn| {
                conn.execute_batch(&format!(
                    "CREATE TABLE test_rebuild_audit (rebuild_count INTEGER NOT NULL);
                     INSERT INTO test_rebuild_audit (rebuild_count) VALUES (0);
                     CREATE TRIGGER test_count_capped_rebuild
                     AFTER INSERT ON account_transaction_ledger
                     WHEN NEW.account_id = '{account_id}'
                     BEGIN
                       UPDATE test_rebuild_audit
                       SET rebuild_count = rebuild_count + 1;
                     END;"
                ))
                .map_err(|err| {
                    crate::db::DbError::new(format!("Failed to install rebuild audit: {err}"))
                })
            })
            .expect("rebuild audit should install");

            // Block B+1: T4 (+7 sat) arrives. The account is already at its cap,
            // so sync must fetch statistics only and must not touch the ledger.
            // Both preconditions matter: the balance refresh must not be fresh
            // (BALANCE_REFRESH_TTL is 30 minutes) and the tip must advance.
            clock.sleep(Duration::from_secs(31 * 60));
            let run_next = next_run_for_user(&clock, run_b.user_id);
            let server_next = start_historical_sync_mempool_server(vec![btc_stats_json(4, 130, 0)]);
            let counters_next = SyncHttpCounters::new();
            let client_next =
                live_mempool_client(run_next.user_id, &server_next.base_url, &counters_next);
            let mut tip_cache_next = bitcoin_chain_tip_at(&clock, 101);
            let mut accumulator_next = CycleAccumulator::new(1);
            let mut executor_next = LiveAddressSyncExecutor::new();
            let mut address_next =
                crate::db::get_sync_addresses_for_account(run_next.user_id, account_id)
                    .expect("address should reload")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("address should persist");
            let mut processed_next = 0_u32;

            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run: run_next,
                address: &mut address_next,
                chain_tip_cache: &mut tip_cache_next,
                pending_address_ids: &pending,
                clients: SyncClients {
                    mempool_client: Some(&client_next),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &counters_next,
                },
                executor: &mut executor_next,
                accumulator: &mut accumulator_next,
                processed_for_account: &mut processed_next,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(2),
                },
                mempool_history_page_frontier: None,
            })
            .expect("block B+1 stats-only refresh should succeed");
            accumulator_next
                .rebuild_account_if_touched(run_next.user_id, account_id, clock.utc_now())
                .expect("block B+1 finalization should succeed");
            let requests_next = server_next.join();

            assert_eq!(
                requests_next,
                vec![format!(
                    "GET /api/address/{} HTTP/1.1",
                    address.address.as_str()
                )],
                "at the cap the provider must receive only the statistics request"
            );

            // Existing balances first: this is the reported defect.
            assert_eq!(
                closing_balance_lo_for(run_next.user_id, account_id, T2_TXID),
                Some(120),
                "T2 closing balance must not shift after a balance-only refresh"
            );
            assert_eq!(
                closing_balance_lo_for(run_next.user_id, account_id, T3_TXID),
                Some(123),
                "T3 closing balance must not shift after a balance-only refresh"
            );

            let rebuild_count = crate::db::with_user_db(run_next.user_id, |conn| {
                conn.query_row("SELECT rebuild_count FROM test_rebuild_audit", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|err| {
                    crate::db::DbError::new(format!("Failed to read rebuild count: {err}"))
                })
            })
            .expect("rebuild count should load");
            assert_eq!(
                rebuild_count, 0,
                "a stats-only capped refresh must not rebuild the ledger"
            );

            let stored_count = crate::db::load_canonical_account_transaction_count_bounded(
                run_next.user_id,
                account_id,
                TransactionCount::from_u32(10),
            )
            .expect("stored count should load");
            assert_eq!(
                stored_count,
                TransactionCount::from_u32(2),
                "T4 must not be admitted beyond the cap"
            );

            // A second balance-only refresh must be equally inert. This guards the
            // durable invariant against a future change that reintroduces a rebuild
            // later in the normal sync cycle.
            clock.sleep(Duration::from_secs(31 * 60));
            let run_third = next_run_for_user(&clock, run_next.user_id);
            let server_third =
                start_historical_sync_mempool_server(vec![btc_stats_json(4, 130, 0)]);
            let counters_third = SyncHttpCounters::new();
            let client_third =
                live_mempool_client(run_third.user_id, &server_third.base_url, &counters_third);
            let mut tip_cache_third = bitcoin_chain_tip_at(&clock, 102);
            let mut accumulator_third = CycleAccumulator::new(1);
            let mut executor_third = LiveAddressSyncExecutor::new();
            let mut address_third =
                crate::db::get_sync_addresses_for_account(run_third.user_id, account_id)
                    .expect("address should reload")
                    .into_iter()
                    .find(|candidate| candidate.address_id == address.address_id)
                    .expect("address should persist");
            let mut processed_third = 0_u32;

            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run: run_third,
                address: &mut address_third,
                chain_tip_cache: &mut tip_cache_third,
                pending_address_ids: &pending,
                clients: SyncClients {
                    mempool_client: Some(&client_third),
                    etherscan_api_key: None,
                    etherscan_base_url: None,
                    http_counters: &counters_third,
                },
                executor: &mut executor_third,
                accumulator: &mut accumulator_third,
                processed_for_account: &mut processed_third,
                single_address_progress: None,
                mempool_history_policy: MempoolHistoryPolicy::Normal {
                    cap: TransactionCount::from_u32(2),
                },
                mempool_history_page_frontier: None,
            })
            .expect("repeated balance-only refresh should succeed");
            accumulator_third
                .rebuild_account_if_touched(run_third.user_id, account_id, clock.utc_now())
                .expect("third finalization should succeed");
            server_third.join();

            assert_eq!(
                closing_balance_lo_for(run_third.user_id, account_id, T2_TXID),
                Some(120),
                "repeated balance-only refreshes must not shift T2"
            );
            assert_eq!(
                closing_balance_lo_for(run_third.user_id, account_id, T3_TXID),
                Some(123),
                "repeated balance-only refreshes must not shift T3"
            );
        });
    }
}

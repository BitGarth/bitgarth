use std::collections::{HashMap, HashSet};

use crate::db::{
    SyncAddress, cleanup_raw_sync_history_with_compaction, refresh_account_integration_sync_state,
};
use crate::models::SyncHistoryRetentionDays;
use crate::payments::types::EntitlementTier;
use crate::transactions::{
    RateLimitedIntegration, SyncErrorMessage, SyncIntegrationId, TransactionCount,
    TransactionSyncRunId,
};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};

use super::account_events::{
    AccountEventContext, publish_account_sync_completed_events, publish_account_sync_failed_events,
    publish_single_address_started_events,
};
use super::chain_tip::ChainTipCache;
use super::client_config::{
    build_mempool_client, resolve_etherscan_base_url_from_settings,
    resolve_mempool_base_url_from_settings,
};
use super::context::{RunContext, SyncClients, SyncClock, SyncHttpCounters, SystemSyncClock};
use super::cycle::CycleAccumulator;
use super::error::UserTransactionMonitorError;
use super::executor::{
    AddressSyncExecutor, LiveAddressSyncExecutor, recover_interrupted_mempool_account,
};
use super::gate::{
    MempoolHistoryPolicy, SyncSingleAddressControlRequest, integration_for_asset,
    load_account_transaction_count_for_history_policy, sync_single_address_with_controls,
};
use super::integrations::unfinished_backfill_state;
use super::planner::{SyncPlannerInput, pick_next_address_index};
use super::progress::build_single_address_progress_plan;
use super::rate_limit::retry_after_utc_for_integration;
use crate::tasks::TriggerSource;

fn ensure_manual_history_sync_allowed(
    repair_owns_account: bool,
) -> Result<(), UserTransactionMonitorError> {
    if repair_owns_account {
        return Err(UserTransactionMonitorError::Db(crate::db::DbError::new(
            "Bitcoin history correctness repair is in progress",
        )));
    }
    Ok(())
}

#[cfg(feature = "server")]
struct ManualSyncControlRunRequest<'a> {
    run: RunContext<'a>,
    native_account_id: DigitalAssetAccountId,
    iteration_budget: u32,
    addresses: Vec<SyncAddress>,
    pending_address_ids: HashSet<DigitalAssetAddressId>,
    known_activity_address_ids: HashSet<DigitalAssetAddressId>,
    clients: SyncClients<'a>,
    executor: &'a mut dyn AddressSyncExecutor,
    historical_backfill_enabled: bool,
    historical_backfill_transactions_per_account: u32,
}

#[cfg(feature = "server")]
fn run_manual_sync_control_with_executor(
    request: ManualSyncControlRunRequest<'_>,
) -> Result<crate::transactions::SyncControlInvocationResponse, UserTransactionMonitorError> {
    let ManualSyncControlRunRequest {
        run,
        native_account_id,
        iteration_budget,
        mut addresses,
        pending_address_ids,
        known_activity_address_ids,
        clients,
        executor,
        historical_backfill_enabled,
        historical_backfill_transactions_per_account,
    } = request;

    if addresses.is_empty() {
        return Ok(crate::transactions::SyncControlInvocationResponse {
            iterations_requested: iteration_budget,
            iterations_completed: 0,
            addresses_touched: 0,
            total_new_transactions: 0,
            total_updated_transactions: 0,
            backfill_continuing: false,
            stopped_early: false,
            error_message: None,
        });
    }

    let mut chain_tip_cache = ChainTipCache::default();
    let mut iterations_completed = 0_u32;
    let mut total_new = 0_u32;
    let mut total_updated = 0_u32;
    let mut addresses_touched = HashSet::<DigitalAssetAddressId>::new();
    let bitcoin_history_repair_account_ids = HashSet::new();
    let mut stopped_early = false;
    let mut error_message = None::<String>;
    let mempool_history_policy = MempoolHistoryPolicy::normal(
        historical_backfill_enabled,
        TransactionCount::from_u32(historical_backfill_transactions_per_account),
    );

    for _ in 0..iteration_budget {
        let mut accumulator = CycleAccumulator::new(1);
        let account_transaction_count = load_account_transaction_count_for_history_policy(
            run.user_id,
            native_account_id,
            mempool_history_policy,
        )?;
        let account_transaction_counts =
            HashMap::from([(native_account_id, account_transaction_count)]);
        let run_excluded_address_ids = HashSet::new();
        let planner_input = SyncPlannerInput {
            now_utc: run.clock.utc_now(),
            mempool_history_policy,
            account_transaction_counts: &account_transaction_counts,
            pending_address_ids: &pending_address_ids,
            known_activity_address_ids: &known_activity_address_ids,
            bitcoin_history_repair_account_ids: &bitcoin_history_repair_account_ids,
            run_excluded_address_ids: &run_excluded_address_ids,
        };
        let Some(idx) = pick_next_address_index(&addresses, &planner_input) else {
            break;
        };

        let address = &mut addresses[idx];
        let asset_id = address.asset_id;
        let integration_id = SyncIntegrationId::for_asset(asset_id);
        let single_address_progress =
            build_single_address_progress_plan(run, address, clients, historical_backfill_enabled);
        if let Some(progress) = single_address_progress {
            let started_at_utc = run.clock.utc_now();
            let start = crate::db::mark_account_integration_sync_started(
                run.user_id,
                progress.account_id,
                integration_id,
                started_at_utc,
            )?;
            let recovery = recover_interrupted_mempool_account(
                run.user_id,
                progress.account_id,
                asset_id,
                start,
            )?;
            accumulator.mark_accounts_history_unavailable(&recovery.account_ids);
            crate::db::debug_assert_user_db_unlocked(
                run.user_id,
                "manual single-address start publish",
            );
            let ctx = AccountEventContext {
                user_id: run.user_id,
                run_id: run.run_id,
                completed_at_utc: started_at_utc,
                account_id: progress.account_id,
                integration_id,
            };
            publish_single_address_started_events(
                &ctx,
                started_at_utc,
                progress.is_first_sync,
                progress.expected_tx_count,
                progress.expected_tx_count_is_lower_bound,
            );
        }

        let mut processed_for_account = 0_u32;
        let (had_activity, interrupted) =
            sync_single_address_with_controls(SyncSingleAddressControlRequest {
                run,
                address,
                chain_tip_cache: &mut chain_tip_cache,
                pending_address_ids: &pending_address_ids,
                clients,
                executor,
                accumulator: &mut accumulator,
                processed_for_account: &mut processed_for_account,
                single_address_progress,
                mempool_history_policy,
                mempool_history_page_frontier: None,
            })?;
        let delta = super::cycle::CycleAccumulatorSnapshot::from_accumulator(&accumulator);
        let aborted_by_rate_limit = interrupted
            && accumulator
                .rate_limited
                .contains(integration_for_asset(asset_id));
        let completed_at_utc = run.clock.utc_now();

        iterations_completed = iterations_completed.saturating_add(1);
        total_new = total_new.saturating_add(delta.new_tx_count);
        total_updated = total_updated.saturating_add(delta.updated_tx_count);

        if had_activity {
            addresses_touched.insert(address.address_id);
        }
        accumulator.rebuild_account_if_touched(run.user_id, native_account_id, completed_at_utc)?;
        refresh_account_integration_sync_state(
            run.user_id,
            native_account_id,
            integration_id,
            completed_at_utc,
        )?;

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
                    integration_id.as_db_value(),
                    run.clock.instant_now(),
                    completed_at_utc,
                )
            } else {
                None
            };
            crate::db::debug_assert_user_db_unlocked(
                run.user_id,
                "manual single-address failure publish",
            );
            let ctx = AccountEventContext {
                user_id: run.user_id,
                run_id: run.run_id,
                completed_at_utc,
                account_id: native_account_id,
                integration_id,
            };
            publish_account_sync_failed_events(&ctx, error.clone(), rate_limited, retry_after_utc);
            stopped_early = true;
            error_message = Some(error.as_str().to_string());
            break;
        }

        if delta.addresses_synced + delta.addresses_skipped > 0 {
            crate::db::debug_assert_user_db_unlocked(
                run.user_id,
                "manual single-address completion publish",
            );
            let ctx = AccountEventContext {
                user_id: run.user_id,
                run_id: run.run_id,
                completed_at_utc,
                account_id: native_account_id,
                integration_id,
            };
            publish_account_sync_completed_events(
                &ctx,
                TransactionCount::from_u32(delta.new_tx_count),
                TransactionCount::from_u32(delta.updated_tx_count),
            );
        }

        addresses = crate::db::get_sync_addresses_for_account(run.user_id, native_account_id)?;
    }

    Ok(crate::transactions::SyncControlInvocationResponse {
        iterations_requested: iteration_budget,
        iterations_completed,
        addresses_touched: u32::try_from(addresses_touched.len()).unwrap_or(u32::MAX),
        total_new_transactions: total_new,
        total_updated_transactions: total_updated,
        backfill_continuing: addresses
            .iter()
            .any(|address| unfinished_backfill_state(address).is_some()),
        stopped_early,
        error_message,
    })
}

#[cfg(feature = "server")]
pub(crate) fn run_manual_sync_control(
    user_id: crate::models::UserId,
    native_account_id: DigitalAssetAccountId,
    iteration_budget: u32,
) -> Result<crate::transactions::SyncControlInvocationResponse, UserTransactionMonitorError> {
    use crate::db::{
        get_sync_addresses_for_account, load_address_ids_with_activity,
        load_address_ids_with_pending_txs, load_settings,
    };

    ensure_manual_history_sync_allowed(crate::db::bitcoin_history_repair_owns_account(
        user_id,
        native_account_id,
    )?)?;

    let clock = SystemSyncClock;
    let now_utc = clock.utc_now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now_utc)?;
    let active_accounts = crate::db::account_limits::sync_eligible_native_account_ids_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
        entitlements.tier == EntitlementTier::Free,
    )?;
    let addresses = if active_accounts.contains(&native_account_id) {
        get_sync_addresses_for_account(user_id, native_account_id)?
    } else {
        Vec::new()
    };

    let run_id = TransactionSyncRunId::new();

    let run = RunContext {
        user_id,
        run_id,
        source: TriggerSource::ManualInternal,
        started_at: now_utc,
        clock: &clock,
    };

    let settings = load_settings(user_id)?;
    let http_counters = SyncHttpCounters::new();

    let (mempool_base_url, mempool_source) = resolve_mempool_base_url_from_settings(&settings)?;

    let mempool_client =
        build_mempool_client(user_id, &mempool_base_url, mempool_source, &http_counters).ok();

    let etherscan_base_url = resolve_etherscan_base_url_from_settings(&settings)?;

    let clients = SyncClients {
        mempool_client: mempool_client.as_ref(),
        etherscan_api_key: settings.etherscan_api_key.as_ref(),
        etherscan_base_url: etherscan_base_url.as_ref(),
        http_counters: &http_counters,
    };

    let pending_address_ids = load_address_ids_with_pending_txs(user_id)?;
    let known_activity_address_ids = load_address_ids_with_activity(user_id)?;
    let mut executor = LiveAddressSyncExecutor::new();
    let result = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
        run,
        native_account_id,
        iteration_budget,
        addresses,
        pending_address_ids,
        known_activity_address_ids,
        clients,
        executor: &mut executor,
        historical_backfill_enabled: entitlements.historical_backfill_enabled,
        historical_backfill_transactions_per_account: entitlements
            .historical_backfill_transactions_per_account,
    });

    let completed_at = clock.utc_now();
    let retention = SyncHistoryRetentionDays::default();
    let cleanup_result = cleanup_raw_sync_history_with_compaction(user_id, completed_at, retention);
    match &cleanup_result {
        Ok(cleanup_report) => tracing::info!(
            user_id = %user_id,
            retention_days = retention.value(),
            deleted_sync_runs = cleanup_report.deletion.deleted_sync_runs,
            "manual_sync_history_cleanup_completed"
        ),
        Err(err) => tracing::warn!(
            user_id = %user_id,
            retention_days = retention.value(),
            error = %err,
            "manual_sync_history_cleanup_failed"
        ),
    }

    result
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::ensure_manual_history_sync_allowed;

    #[test]
    fn repair_in_progress_rejects_manual_sync_without_provider_work() {
        let error = ensure_manual_history_sync_allowed(true)
            .expect_err("repair-owned account must reject manual history sync");
        assert_eq!(
            error.to_string(),
            "Bitcoin history correctness repair is in progress"
        );
        assert!(ensure_manual_history_sync_allowed(false).is_ok());
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::super::context::{RunContext, SyncClients, SyncHttpCounters};
    use super::super::test_support::{
        FakeAddressSyncExecutor, FakeClock, FakeSyncOutcome, make_run_context, make_sync_address,
        persist_sync_address_for_test, test_utc_now, with_rate_limiter_isolated,
    };
    #[cfg(feature = "server")]
    use super::{
        ManualSyncControlRunRequest, run_manual_sync_control, run_manual_sync_control_with_executor,
    };
    use crate::db::SyncAddress;
    use crate::transactions::{
        ApiConfirmedBalance, ChainTransactionStatus, MempoolCursorTxid, TransactionCount,
        TransactionSyncEvent, TxHash,
    };
    use crate::wallets::{DigitalAssetAccountId, SyncedAssetId};
    use std::collections::HashSet;

    fn collect_sync_event_types(
        receiver: &mut tokio::sync::broadcast::Receiver<TransactionSyncEvent>,
    ) -> Vec<crate::transactions::TransactionSyncEventType> {
        let mut event_types = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(event) => event_types.push(event.event_type),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        event_types
    }

    fn persist_confirmed_outputs_for_account_count(
        run: RunContext<'_>,
        address: &SyncAddress,
        count: u32,
    ) {
        for index in 0..count {
            let tx_hash = format!("{:064x}", u64::from(index) + 1);
            let record = crate::db::SyncTransactionRecord {
                tx_hash: TxHash::parse(&tx_hash).expect("tx hash should parse"),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(i64::from(index) + 1),
                block_hash: Some(format!("test-block-{index}")),
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
    }

    #[test]
    #[cfg(feature = "server")]
    fn repair_in_progress_rejects_manual_sync() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let account_id = DigitalAssetAccountId::new();
        let address = make_sync_address(
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            SyncedAssetId::Bitcoin,
            crate::wallets::Network::Mainnet,
            Some(account_id),
            Some(crate::wallets::AddressScheme::NativeSegwit),
            None,
            None,
        );
        persist_sync_address_for_test(run, &address);
        persist_confirmed_outputs_for_account_count(run, &address, 1);

        let error = run_manual_sync_control(run.user_id, account_id, 1)
            .expect_err("repair-owned account must reject manual sync");
        assert_eq!(
            error.to_string(),
            "Bitcoin history correctness repair is in progress"
        );
    }

    fn persist_covered_bitcoin_fixture(
        run: RunContext<'_>,
        address: &SyncAddress,
        account_id: DigitalAssetAccountId,
    ) {
        persist_sync_address_for_test(run, address);
        persist_confirmed_outputs_for_account_count(run, address, 1);
        crate::db::mark_address_sync_started(
            run.user_id,
            address.address_id,
            run.run_id,
            run.started_at - chrono::Duration::seconds(1),
        )
        .expect("address sync state should persist");
        let complete = crate::db::publish_bitcoin_account_completion(
            run.user_id,
            crate::db::BitcoinAccountCompletionPublication {
                account_id,
                final_address_proof: Some(crate::db::BitcoinAddressProofPublication {
                    address_id: address.address_id,
                    proof: crate::db::MempoolHistoryProof {
                        confirmed_tx_count: TransactionCount::from_u32(1),
                        complete_height: crate::transactions::ChainTipHeight::try_new(1)
                            .expect("height should parse"),
                    },
                    scan_start_run_id: None,
                }),
                completed_hd_discovery: None,
                observed_at: run.started_at,
            },
        )
        .expect("history proof should publish atomically");
        assert!(complete, "fixture account should become complete");
    }

    #[test]
    #[cfg(feature = "server")]
    fn mempool_history_policy_manual_uses_canonical_count_for_history_cap() {
        for (canonical_count, cap, expected_history_enabled) in [
            (0_u32, 1_u32, true),
            (1_u32, 1_u32, false),
            (2_u32, 1_u32, false),
        ] {
            with_rate_limiter_isolated(|| {
                let clock = FakeClock::new(test_utc_now());
                let run = make_run_context(&clock);
                let account_id = DigitalAssetAccountId::new();
                let address = make_sync_address(
                    "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                    SyncedAssetId::Bitcoin,
                    crate::wallets::Network::Mainnet,
                    Some(account_id),
                    None,
                    None,
                    None,
                );
                persist_sync_address_for_test(run, &address);
                persist_confirmed_outputs_for_account_count(run, &address, canonical_count);
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

                run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                    run,
                    native_account_id: account_id,
                    iteration_budget: 1,
                    addresses: vec![address],
                    pending_address_ids: HashSet::new(),
                    known_activity_address_ids: HashSet::new(),
                    clients,
                    executor: &mut executor,
                    historical_backfill_enabled: true,
                    historical_backfill_transactions_per_account: cap,
                })
                .expect("manual sync should finish");

                assert_eq!(
                    executor.historical_backfill_enabled_calls,
                    vec![expected_history_enabled]
                );
            });
        }
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_recovers_interrupted_mempool_history_before_success() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);
            persist_confirmed_outputs_for_account_count(run, &address, 1);
            crate::db::mark_address_sync_started(
                run.user_id,
                address.address_id,
                run.run_id,
                now - chrono::Duration::seconds(1),
            )
            .expect("address sync state should persist");
            let complete = crate::db::publish_bitcoin_account_completion(
                run.user_id,
                crate::db::BitcoinAccountCompletionPublication {
                    account_id,
                    final_address_proof: Some(crate::db::BitcoinAddressProofPublication {
                        address_id: address.address_id,
                        proof: crate::db::MempoolHistoryProof {
                            confirmed_tx_count: TransactionCount::from_u32(1),
                            complete_height: crate::transactions::ChainTipHeight::try_new(1)
                                .expect("height should parse"),
                        },
                        scan_start_run_id: None,
                    }),
                    completed_hd_discovery: None,
                    observed_at: now,
                },
            )
            .expect("history proof should publish atomically");
            assert!(complete, "fixture account should become complete");
            crate::db::mark_account_integration_sync_started(
                run.user_id,
                account_id,
                crate::transactions::SyncIntegrationId::Mempool,
                now - chrono::Duration::seconds(1),
            )
            .expect("interrupted integration start should persist");

            crate::db::with_user_db_mut(run.user_id, |conn| {
                conn.execute_batch(&format!(
                    "CREATE TABLE test_rebuild_audit (rebuild_count INTEGER NOT NULL);
                     INSERT INTO test_rebuild_audit (rebuild_count) VALUES (0);
                     CREATE TRIGGER test_count_recovery_rebuild
                     AFTER INSERT ON account_transaction_ledger
                     WHEN NEW.account_id = '{account_id}'
                     BEGIN
                       UPDATE test_rebuild_audit
                       SET rebuild_count = rebuild_count + 1;
                     END;
                     CREATE TRIGGER test_guard_integration_success
                     BEFORE UPDATE OF last_result ON account_integration_sync_state
                     WHEN NEW.account_id = '{account_id}'
                       AND NEW.integration_id = 'mempool'
                       AND NEW.last_result = 'success'
                       AND (SELECT rebuild_count FROM test_rebuild_audit) != 1
                     BEGIN
                       SELECT RAISE(ABORT, 'integration success before one complete rebuild');
                     END;"
                ))
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "Failed to install recovery ordering assertions: {err}"
                    ))
                })
            })
            .expect("recovery ordering assertions should install");

            let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
                .expect("sync event subscription should succeed");
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

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 1,
                addresses: vec![address.clone()],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("interrupted manual sync should recover");

            assert!(!response.stopped_early);
            assert_eq!(response.total_new_transactions, 0);
            assert_eq!(response.total_updated_transactions, 0);
            assert_eq!(
                crate::db::get_sync_addresses_for_account(run.user_id, account_id)
                    .expect("address should reload")[0]
                    .mempool_history_proof,
                None
            );
            let (rebuild_count, null_closing_count, integration_result) =
                crate::db::with_user_db(run.user_id, |conn| {
                    let rebuild_count = conn
                        .query_row("SELECT rebuild_count FROM test_rebuild_audit", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load recovery rebuild count: {err}"
                            ))
                        })?;
                    let null_closing_count = conn
                        .query_row(
                            "SELECT COUNT(*)
                             FROM account_transaction_ledger
                             WHERE account_id = ?1
                               AND (
                                 closing_balance_hi IS NULL
                                 OR closing_balance_lo IS NULL
                               )",
                            [account_id.to_string()],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load recovery closing balances: {err}"
                            ))
                        })?;
                    let integration_result = conn
                        .query_row(
                            "SELECT last_result
                             FROM account_integration_sync_state
                             WHERE account_id = ?1 AND integration_id = 'mempool'",
                            [account_id.to_string()],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load recovery integration result: {err}"
                            ))
                        })?;
                    Ok::<_, crate::db::DbError>((
                        rebuild_count,
                        null_closing_count,
                        integration_result,
                    ))
                })
                .expect("recovery state should load");
            assert_eq!(rebuild_count, 1);
            assert_eq!(null_closing_count, 1);
            assert_eq!(integration_result.as_deref(), Some("success"));
            assert!(collect_sync_event_types(&mut receiver).contains(
                &crate::transactions::TransactionSyncEventType::AccountIntegrationSyncCompleted
            ));
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_retries_carried_invalidation_before_failure_completion() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qretrycoveragefailure",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_covered_bitcoin_fixture(run, &address, account_id);
            crate::db::with_user_db_mut(run.user_id, |conn| {
                conn.execute_batch(&format!(
                    "CREATE TABLE test_retry_rebuild_audit (rebuild_count INTEGER NOT NULL);
                     INSERT INTO test_retry_rebuild_audit (rebuild_count) VALUES (0);
                     CREATE TRIGGER test_count_retry_rebuild
                     AFTER INSERT ON account_transaction_ledger
                     WHEN NEW.account_id = '{account_id}'
                     BEGIN
                       UPDATE test_retry_rebuild_audit
                       SET rebuild_count = rebuild_count + 1;
                     END;
                     CREATE TRIGGER test_guard_failure_completion
                     BEFORE UPDATE OF last_result ON account_integration_sync_state
                     WHEN NEW.account_id = '{account_id}'
                       AND NEW.integration_id = 'mempool'
                       AND NEW.last_result = 'failure'
                       AND (
                         (SELECT rebuild_count FROM test_retry_rebuild_audit) != 1
                         OR EXISTS (
                           SELECT 1
                           FROM transaction_sync_state
                           WHERE address_id = '{}'
                             AND mempool_history_complete_tx_count IS NOT NULL
                         )
                       )
                     BEGIN
                       SELECT RAISE(ABORT, 'failure completion before invalidation rebuild');
                     END;",
                    address.address_id
                ))
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "Failed to install retry ordering assertions: {err}"
                    ))
                })
            })
            .expect("retry ordering assertions should install");

            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::FailureWithCoverage {
                    message: "first invalidation write failed".to_string(),
                    account_id,
                }]);

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 1,
                addresses: vec![address.clone()],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("gate retry should allow normal failure completion");

            assert!(response.stopped_early);
            let (proof_count, rebuild_count, integration_result) =
                crate::db::with_user_db(run.user_id, |conn| {
                    let proof_count = conn
                        .query_row(
                            "SELECT COUNT(*)
                             FROM transaction_sync_state
                             WHERE address_id = ?1
                               AND mempool_history_complete_tx_count IS NOT NULL",
                            [address.address_id.to_string()],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load retried proof state: {err}"
                            ))
                        })?;
                    let rebuild_count = conn
                        .query_row(
                            "SELECT rebuild_count FROM test_retry_rebuild_audit",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load retry rebuild count: {err}"
                            ))
                        })?;
                    let integration_result = conn
                        .query_row(
                            "SELECT last_result
                             FROM account_integration_sync_state
                             WHERE account_id = ?1 AND integration_id = 'mempool'",
                            [account_id.to_string()],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load retry integration result: {err}"
                            ))
                        })?;
                    Ok::<_, crate::db::DbError>((proof_count, rebuild_count, integration_result))
                })
                .expect("retry state should load");
            assert_eq!(proof_count, 0);
            assert_eq!(rebuild_count, 1);
            assert_eq!(integration_result.as_deref(), Some("failure"));
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_persistent_invalidation_failure_leaves_recovery_armed() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qpersistentcoveragefailure",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_covered_bitcoin_fixture(run, &address, account_id);
            crate::db::with_user_db_mut(run.user_id, |conn| {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER test_reject_coverage_invalidation
                     BEFORE UPDATE OF mempool_history_complete_tx_count
                     ON transaction_sync_state
                     WHEN OLD.address_id = '{}'
                       AND OLD.mempool_history_complete_tx_count IS NOT NULL
                       AND NEW.mempool_history_complete_tx_count IS NULL
                     BEGIN
                       SELECT RAISE(ABORT, 'persistent invalidation failure');
                     END;",
                    address.address_id
                ))
                .map_err(|err| {
                    crate::db::DbError::new(format!(
                        "Failed to install persistent invalidation failure: {err}"
                    ))
                })
            })
            .expect("persistent invalidation failure should install");

            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor =
                FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::FailureWithCoverage {
                    message: "first invalidation write failed".to_string(),
                    account_id,
                }]);

            let error = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 1,
                addresses: vec![address.clone()],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect_err("persistent invalidation failure should abort account completion");
            assert!(
                error
                    .to_string()
                    .contains("persistent invalidation failure")
            );

            let (proof_count, null_closing_count, integration_state) =
                crate::db::with_user_db(run.user_id, |conn| {
                    let proof_count = conn
                        .query_row(
                            "SELECT COUNT(*)
                             FROM transaction_sync_state
                             WHERE address_id = ?1
                               AND mempool_history_complete_tx_count IS NOT NULL",
                            [address.address_id.to_string()],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load persistent proof state: {err}"
                            ))
                        })?;
                    let null_closing_count = conn
                        .query_row(
                            "SELECT COUNT(*)
                             FROM account_transaction_ledger
                             WHERE account_id = ?1
                               AND (
                                 closing_balance_hi IS NULL
                                 OR closing_balance_lo IS NULL
                               )",
                            [account_id.to_string()],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load persistent ledger state: {err}"
                            ))
                        })?;
                    let integration_state = conn
                        .query_row(
                            "SELECT last_started_at, last_completed_at, last_result
                             FROM account_integration_sync_state
                             WHERE account_id = ?1 AND integration_id = 'mempool'",
                            [account_id.to_string()],
                            |row| {
                                Ok((
                                    row.get::<_, Option<String>>(0)?,
                                    row.get::<_, Option<String>>(1)?,
                                    row.get::<_, Option<String>>(2)?,
                                ))
                            },
                        )
                        .map_err(|err| {
                            crate::db::DbError::new(format!(
                                "Failed to load persistent integration state: {err}"
                            ))
                        })?;
                    Ok::<_, crate::db::DbError>((
                        proof_count,
                        null_closing_count,
                        integration_state,
                    ))
                })
                .expect("persistent failure state should load");
            assert_eq!(proof_count, 1);
            assert_eq!(null_closing_count, 0);
            assert!(integration_state.0.is_some());
            assert_eq!(integration_state.1, None);
            assert_eq!(integration_state.2, None);
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_control_stops_after_address_failure() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "0x6666666666666666666666666666666666666666",
                SyncedAssetId::Ethereum,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
                .expect("sync event subscription should succeed");
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::Failure {
                    message: "provider failed".to_string(),
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 1,
                    updated_tx_count: 0,
                },
            ]);

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 2,
                addresses: vec![address],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("manual sync control should return a partial summary");

            assert_eq!(response.iterations_requested, 2);
            assert_eq!(response.iterations_completed, 1);
            assert_eq!(response.addresses_touched, 0);
            assert_eq!(response.total_new_transactions, 0);
            assert_eq!(response.total_updated_transactions, 0);
            assert!(!response.backfill_continuing);
            assert!(response.stopped_early);
            assert_eq!(
                response.error_message.as_deref(),
                Some("Sync HTTP request failed: provider failed")
            );
            assert_eq!(
                executor.calls.len(),
                1,
                "manual sync should stop after failure"
            );

            let event_types = collect_sync_event_types(&mut receiver);
            assert_eq!(
                event_types,
                vec![
                    crate::transactions::TransactionSyncEventType::AccountSyncStarted,
                    crate::transactions::TransactionSyncEventType::AccountIntegrationSyncStarted,
                    crate::transactions::TransactionSyncEventType::AccountSyncFailed,
                    crate::transactions::TransactionSyncEventType::AccountIntegrationSyncFailed,
                ]
            );
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_control_stops_after_rate_limit() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "0x7777777777777777777777777777777777777778",
                SyncedAssetId::Ethereum,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let mut receiver = crate::tasks::subscribe_transaction_sync_events(run.user_id)
                .expect("sync event subscription should succeed");
            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::RateLimited {
                    integration: "etherscan".to_string(),
                },
                FakeSyncOutcome::Success {
                    new_tx_count: 1,
                    updated_tx_count: 0,
                },
            ]);

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 2,
                addresses: vec![address],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("manual sync control should return a partial summary");

            assert_eq!(response.iterations_completed, 1);
            assert!(response.stopped_early);
            assert_eq!(
                response.error_message.as_deref(),
                Some("Rate limit reached for integration etherscan")
            );
            assert_eq!(executor.calls.len(), 1, "rate limit should stop the loop");

            let event_types = collect_sync_event_types(&mut receiver);
            assert_eq!(
                event_types,
                vec![
                    crate::transactions::TransactionSyncEventType::AccountSyncStarted,
                    crate::transactions::TransactionSyncEventType::AccountIntegrationSyncStarted,
                    crate::transactions::TransactionSyncEventType::AccountSyncFailed,
                    crate::transactions::TransactionSyncEventType::AccountIntegrationSyncFailed,
                ]
            );
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_control_iteration_persists_api_confirmed_balance() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let address = make_sync_address(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            persist_sync_address_for_test(run, &address);

            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let observed_balance = ApiConfirmedBalance::from_smallest_unit_i64(321_000)
                .expect("test balance should be valid");
            let mut executor = FakeAddressSyncExecutor::new(vec![FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            }])
            .with_iteration_ledger_rebuild_required(false)
            .with_iteration_api_confirmed_balance(Some(observed_balance));

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 1,
                addresses: vec![address.clone()],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("manual sync control should succeed");

            assert_eq!(response.iterations_completed, 1);

            let stored_balance = crate::db::with_user_db(run.user_id, |conn| {
                conn.query_row(
                    "SELECT api_confirmed_balance_hi, api_confirmed_balance_lo
                     FROM transaction_sync_state
                     WHERE scope = 'address' AND address_id = ?1",
                    rusqlite::params![address.address_id.to_string()],
                    |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .map_err(|err| {
                    crate::db::DbError::new(format!("Failed to load stored balance: {err}"))
                })
            })
            .expect("stored balance should load");
            assert_eq!(stored_balance, (Some(0), Some(321_000)));
        });
    }

    #[test]
    #[cfg(feature = "server")]
    fn manual_sync_control_reports_backfill_complete_after_cursor_recovery() {
        with_rate_limiter_isolated(|| {
            let now = test_utc_now();
            let clock = FakeClock::new(now);
            let run = make_run_context(&clock);
            let account_id = DigitalAssetAccountId::new();
            let mut address = make_sync_address(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                SyncedAssetId::Bitcoin,
                crate::wallets::Network::Mainnet,
                Some(account_id),
                None,
                None,
                None,
            );
            let cursor = MempoolCursorTxid::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("cursor should parse");
            address.mempool_backfill_cursor_txid = Some(cursor);
            address.mempool_expected_tx_count = Some(TransactionCount::from_u32(1));
            persist_sync_address_for_test(run, &address);

            let http_counters = SyncHttpCounters::new();
            let clients = SyncClients {
                mempool_client: None,
                etherscan_api_key: None,
                etherscan_base_url: None,
                http_counters: &http_counters,
            };
            let mut executor = FakeAddressSyncExecutor::new(vec![
                FakeSyncOutcome::SuccessClearingMempoolBackfill {
                    new_tx_count: 0,
                    updated_tx_count: 0,
                },
            ]);

            let response = run_manual_sync_control_with_executor(ManualSyncControlRunRequest {
                run,
                native_account_id: account_id,
                iteration_budget: 1,
                addresses: vec![address.clone()],
                pending_address_ids: HashSet::new(),
                known_activity_address_ids: HashSet::new(),
                clients,
                executor: &mut executor,
                historical_backfill_enabled: true,
                historical_backfill_transactions_per_account: u32::MAX,
            })
            .expect("manual sync control should succeed");

            assert_eq!(response.iterations_completed, 1);
            assert!(!response.backfill_continuing);
            assert_eq!(executor.calls, vec![address.address_id]);

            let stored_address = crate::db::get_sync_addresses_for_account(run.user_id, account_id)
                .expect("sync addresses should load")
                .into_iter()
                .find(|candidate| candidate.address_id == address.address_id)
                .expect("address should still exist");
            assert_eq!(stored_address.mempool_backfill_cursor_txid, None);
            assert_eq!(stored_address.mempool_expected_tx_count, None);
        });
    }
}

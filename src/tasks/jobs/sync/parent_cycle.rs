use crate::db::{AccountSyncBundle, SyncAddress};
use crate::transactions::{
    AddressCount, AggregateSyncResult, RateLimitedIntegration, SyncErrorMessage, SyncIntegrationId,
    TransactionSyncRunId,
};
use dioxus::logger::tracing;
use std::collections::{BTreeMap, HashSet};

use super::automatic::{
    SyncCycleRequest, empty_sync_summary, run_sync_cycle, total_sync_address_count,
};
use super::context::{RunContext, SyncClients, SyncRunPreload, UserTransactionMonitorSummary};
use super::error::UserTransactionMonitorError;
use super::executor::LiveAddressSyncExecutor;
use super::hd_scan::LiveAddressDerivationProvider;

#[cfg(all(test, feature = "db-tests"))]
use super::automatic::{empty_sync_run_preload, make_summary_for_test};

#[cfg(all(test, feature = "db-tests"))]
use super::context::SyncHttpCounters;

#[derive(Debug, Clone)]
pub(super) struct IntegrationWorkset {
    pub(super) integration_id: SyncIntegrationId,
    pub(super) non_hd_addresses: Vec<SyncAddress>,
    pub(super) hd_bundles: Vec<AccountSyncBundle>,
}

impl IntegrationWorkset {
    fn total_addresses(&self) -> u32 {
        total_sync_address_count(&self.non_hd_addresses, &self.hd_bundles)
    }

    fn hd_derivation_pending_accounts(&self) -> usize {
        self.hd_bundles
            .iter()
            .filter(|bundle| {
                bundle.external_addresses.is_empty() || bundle.internal_addresses.is_empty()
            })
            .count()
    }

    fn has_work(&self) -> bool {
        self.total_addresses() > 0 || self.hd_derivation_pending_accounts() > 0
    }
}

fn partition_sync_worksets(
    non_hd_addresses: Vec<SyncAddress>,
    hd_bundles: Vec<AccountSyncBundle>,
) -> Vec<IntegrationWorkset> {
    let mut worksets = BTreeMap::<SyncIntegrationId, IntegrationWorkset>::new();

    for address in non_hd_addresses {
        let integration_id = SyncIntegrationId::for_asset(address.asset_id);
        worksets
            .entry(integration_id)
            .or_insert_with(|| IntegrationWorkset {
                integration_id,
                non_hd_addresses: Vec::new(),
                hd_bundles: Vec::new(),
            })
            .non_hd_addresses
            .push(address);
    }

    for bundle in hd_bundles {
        let integration_id = SyncIntegrationId::for_asset(bundle.asset_id);
        worksets
            .entry(integration_id)
            .or_insert_with(|| IntegrationWorkset {
                integration_id,
                non_hd_addresses: Vec::new(),
                hd_bundles: Vec::new(),
            })
            .hd_bundles
            .push(bundle);
    }

    worksets
        .into_values()
        .filter(IntegrationWorkset::has_work)
        .collect()
}

pub(super) struct IntegrationChildCycleRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) clients: SyncClients<'a>,
    pub(super) preload: &'a SyncRunPreload,
    pub(super) workset: IntegrationWorkset,
}

pub(super) trait IntegrationChildRunner: Sync {
    fn run_child_cycle(
        &self,
        request: IntegrationChildCycleRequest<'_>,
    ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError>;
}

pub(super) struct LiveIntegrationChildRunner;

impl IntegrationChildRunner for LiveIntegrationChildRunner {
    fn run_child_cycle(
        &self,
        request: IntegrationChildCycleRequest<'_>,
    ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
        let mut sync_executor = LiveAddressSyncExecutor::new();
        let mut derivation_provider = LiveAddressDerivationProvider;
        run_sync_cycle(SyncCycleRequest {
            run: request.run,
            clients: request.clients,
            preload: request.preload,
            non_hd_addresses: request.workset.non_hd_addresses,
            hd_bundles: request.workset.hd_bundles,
            known_activity: request.preload.known_activity_address_ids.clone(),
            sync_executor: &mut sync_executor,
            derivation_provider: &mut derivation_provider,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IntegrationChildFailureKind {
    FatalExecution,
    NonFatal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IntegrationChildOutcome {
    Success,
    Blocked,
    Failure,
}

#[derive(Clone, Debug)]
pub(super) struct IntegrationChildSummary {
    pub(super) integration_id: SyncIntegrationId,
    pub(super) summary: UserTransactionMonitorSummary,
    pub(super) outcome: IntegrationChildOutcome,
    pub(super) failure_kind: Option<IntegrationChildFailureKind>,
}

pub(super) struct ParentSyncCycleRequest<'a> {
    pub(super) run: RunContext<'a>,
    pub(super) clients: SyncClients<'a>,
    pub(super) preload: &'a SyncRunPreload,
    pub(super) non_hd_addresses: Vec<SyncAddress>,
    pub(super) hd_bundles: Vec<AccountSyncBundle>,
}

pub(super) struct ParentSyncCycleResult {
    pub(super) summary: UserTransactionMonitorSummary,
    pub(super) aggregate_result: AggregateSyncResult,
    pub(super) child_summaries: Vec<IntegrationChildSummary>,
}

fn normalize_child_outcome(summary: &UserTransactionMonitorSummary) -> IntegrationChildOutcome {
    if summary.addresses_failed.value() > 0 {
        IntegrationChildOutcome::Failure
    } else if !summary.rate_limited.is_empty() {
        IntegrationChildOutcome::Blocked
    } else {
        IntegrationChildOutcome::Success
    }
}

pub(super) fn integration_child_summary_from_summary(
    integration_id: SyncIntegrationId,
    summary: UserTransactionMonitorSummary,
) -> IntegrationChildSummary {
    let outcome = normalize_child_outcome(&summary);
    let failure_kind = matches!(outcome, IntegrationChildOutcome::Failure)
        .then_some(IntegrationChildFailureKind::NonFatal);
    IntegrationChildSummary {
        integration_id,
        summary,
        outcome,
        failure_kind,
    }
}

fn synthetic_failed_child_summary(
    run_id: TransactionSyncRunId,
    integration_id: SyncIntegrationId,
    addresses_total: u32,
    error: SyncErrorMessage,
    failure_kind: IntegrationChildFailureKind,
) -> IntegrationChildSummary {
    let mut summary = empty_sync_summary(run_id, addresses_total);
    summary.addresses_failed = AddressCount::from_u32(addresses_total);
    summary.failure_error = Some(error);
    IntegrationChildSummary {
        integration_id,
        summary,
        outcome: IntegrationChildOutcome::Failure,
        failure_kind: Some(failure_kind),
    }
}

pub(super) fn reduce_parent_result(
    child_summaries: &[IntegrationChildSummary],
) -> AggregateSyncResult {
    if child_summaries
        .iter()
        .all(|child| matches!(child.outcome, IntegrationChildOutcome::Success))
    {
        AggregateSyncResult::Success
    } else if child_summaries
        .iter()
        .any(|child| !matches!(child.outcome, IntegrationChildOutcome::Failure))
    {
        AggregateSyncResult::Partial
    } else {
        AggregateSyncResult::Failure
    }
}

fn debug_assert_child_summary_invariants(
    workset_count: usize,
    child_summaries: &[IntegrationChildSummary],
) {
    debug_assert_eq!(
        child_summaries.len(),
        workset_count,
        "each started integration workset must reduce to exactly one child summary"
    );

    for window in child_summaries.windows(2) {
        debug_assert!(
            window[0].integration_id < window[1].integration_id,
            "child summaries must be kept in stable integration order without duplicates"
        );
    }
}

fn log_child_worker_started(run: RunContext<'_>, workset: &IntegrationWorkset) {
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        integration = %workset.integration_id,
        addresses_total = workset.total_addresses(),
        non_hd_addresses = workset.non_hd_addresses.len(),
        hd_accounts = workset.hd_bundles.len(),
        hd_derivation_pending_accounts = workset.hd_derivation_pending_accounts(),
        "sync_child_started"
    );
}

fn log_child_worker_terminal(run: RunContext<'_>, child_summary: &IntegrationChildSummary) {
    tracing::info!(
        user_id = %run.user_id,
        run_id = %run.run_id,
        integration = %child_summary.integration_id,
        outcome = ?child_summary.outcome,
        failure_kind = ?child_summary.failure_kind,
        addresses_total = child_summary.summary.addresses_total.value(),
        addresses_synced = child_summary.summary.addresses_synced.value(),
        addresses_failed = child_summary.summary.addresses_failed.value(),
        addresses_skipped = child_summary.summary.addresses_skipped.value(),
        rate_limited_integrations = child_summary.summary.rate_limited.len(),
        "sync_child_completed"
    );
}

fn child_failure_rank(kind: Option<IntegrationChildFailureKind>) -> u8 {
    match kind {
        Some(IntegrationChildFailureKind::FatalExecution) => 0,
        Some(IntegrationChildFailureKind::NonFatal) | None => 1,
    }
}

fn select_child_failure_error(
    child_summaries: &[IntegrationChildSummary],
) -> Option<SyncErrorMessage> {
    let mut failures = child_summaries
        .iter()
        .filter(|child| matches!(child.outcome, IntegrationChildOutcome::Failure))
        .collect::<Vec<_>>();
    failures.sort_by_key(|child| (child_failure_rank(child.failure_kind), child.integration_id));
    failures.first().map(|child| {
        child
            .summary
            .failure_error
            .clone()
            .unwrap_or_else(|| match child.failure_kind {
                Some(IntegrationChildFailureKind::FatalExecution) => SyncErrorMessage::sanitize(
                    format!("fatal {} sync worker failure", child.integration_id),
                ),
                Some(IntegrationChildFailureKind::NonFatal) | None => {
                    SyncErrorMessage::sanitize(format!("{} sync failed", child.integration_id))
                }
            })
    })
}

fn aggregate_child_summaries(
    run_id: TransactionSyncRunId,
    child_summaries: &[IntegrationChildSummary],
) -> UserTransactionMonitorSummary {
    let total_addresses = child_summaries.iter().fold(0_u32, |total, child| {
        total.saturating_add(child.summary.addresses_total.value())
    });
    let mut summary = empty_sync_summary(run_id, total_addresses);
    let mut rate_limited = HashSet::<String>::new();

    for child in child_summaries {
        summary.new_tx_count = summary
            .new_tx_count
            .saturating_add(child.summary.new_tx_count);
        summary.updated_tx_count = summary
            .updated_tx_count
            .saturating_add(child.summary.updated_tx_count);
        summary.addresses_synced = AddressCount::from_u32(
            summary
                .addresses_synced
                .value()
                .saturating_add(child.summary.addresses_synced.value()),
        );
        summary.addresses_failed = AddressCount::from_u32(
            summary
                .addresses_failed
                .value()
                .saturating_add(child.summary.addresses_failed.value()),
        );
        summary.addresses_skipped = AddressCount::from_u32(
            summary
                .addresses_skipped
                .value()
                .saturating_add(child.summary.addresses_skipped.value()),
        );
        summary.addresses_skipped_tip_unchanged = AddressCount::from_u32(
            summary
                .addresses_skipped_tip_unchanged
                .value()
                .saturating_add(child.summary.addresses_skipped_tip_unchanged.value()),
        );
        summary.addresses_early_exited = AddressCount::from_u32(
            summary
                .addresses_early_exited
                .value()
                .saturating_add(child.summary.addresses_early_exited.value()),
        );
        for integration in &child.summary.rate_limited {
            rate_limited.insert(integration.integration.clone());
        }
    }

    let mut ordered_rate_limited = rate_limited
        .into_iter()
        .map(|integration| RateLimitedIntegration { integration })
        .collect::<Vec<_>>();
    ordered_rate_limited.sort_by(|left, right| left.integration.cmp(&right.integration));
    summary.rate_limited = ordered_rate_limited;
    summary.failure_error = select_child_failure_error(child_summaries);
    summary
}

pub(super) fn run_parent_sync_cycle_with_runner(
    request: ParentSyncCycleRequest<'_>,
    child_runner: &dyn IntegrationChildRunner,
) -> ParentSyncCycleResult {
    let worksets = partition_sync_worksets(request.non_hd_addresses, request.hd_bundles);
    let workset_count = worksets.len();
    let mut child_summaries = Vec::<IntegrationChildSummary>::new();

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for workset in worksets {
            let integration_id = workset.integration_id;
            let total_addresses = workset.total_addresses();
            handles.push((
                integration_id,
                total_addresses,
                scope.spawn(move || {
                    log_child_worker_started(request.run, &workset);
                    child_runner.run_child_cycle(IntegrationChildCycleRequest {
                        run: request.run,
                        clients: request.clients,
                        preload: request.preload,
                        workset,
                    })
                }),
            ));
        }

        for (integration_id, total_addresses, handle) in handles {
            let child_summary = match handle.join() {
                Ok(Ok(summary)) => integration_child_summary_from_summary(integration_id, summary),
                Ok(Err(error)) => synthetic_failed_child_summary(
                    request.run.run_id,
                    integration_id,
                    total_addresses,
                    SyncErrorMessage::sanitize(format!(
                        "fatal {} sync worker failure: {error}",
                        integration_id
                    )),
                    IntegrationChildFailureKind::FatalExecution,
                ),
                Err(_) => synthetic_failed_child_summary(
                    request.run.run_id,
                    integration_id,
                    total_addresses,
                    SyncErrorMessage::sanitize(format!(
                        "fatal {} sync worker failure: child thread panicked",
                        integration_id
                    )),
                    IntegrationChildFailureKind::FatalExecution,
                ),
            };
            log_child_worker_terminal(request.run, &child_summary);
            child_summaries.push(child_summary);
        }
    });

    child_summaries.sort_by_key(|child| child.integration_id);
    debug_assert_child_summary_invariants(workset_count, &child_summaries);
    let aggregate_result = reduce_parent_result(&child_summaries);
    let summary = aggregate_child_summaries(request.run.run_id, &child_summaries);

    ParentSyncCycleResult {
        summary,
        aggregate_result,
        child_summaries,
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::super::context::{LABEL_ETHERSCAN, LABEL_MEMPOOL};
    use super::super::test_support::{
        FakeClock, lock_or_recover, make_run_context, make_sync_address, test_utc_now,
    };
    use super::*;
    use crate::db::AccountSyncBundle;
    use crate::wallets::{DigitalAssetAccountId, Network, SyncedAssetId};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Sender};
    use std::time::Duration;

    fn bitcoin_hd_bundle(
        account_id: DigitalAssetAccountId,
        external_addresses: Vec<SyncAddress>,
        internal_addresses: Vec<SyncAddress>,
    ) -> AccountSyncBundle {
        AccountSyncBundle {
            account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            hd_key_extended_pubkey: "zpub-test-parent-cycle".to_string(),
            address_scheme: crate::wallets::AddressScheme::NativeSegwit,
            sync_state: None,
            external_addresses,
            internal_addresses,
        }
    }

    #[derive(Clone, Debug)]
    enum TestChildRunnerBehavior {
        Summary {
            synced: u32,
            failed: u32,
            skipped: u32,
            rate_limited: Vec<&'static str>,
            error: Option<&'static str>,
        },
        Error(&'static str),
        Panic,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct TestChildRunnerState {
        starts: Vec<SyncIntegrationId>,
        completions: Vec<SyncIntegrationId>,
        in_flight: usize,
        max_in_flight: usize,
    }

    struct InFlightGuard<'a> {
        state: &'a Mutex<TestChildRunnerState>,
    }

    impl Drop for InFlightGuard<'_> {
        fn drop(&mut self) {
            let mut state = lock_or_recover(self.state);
            state.in_flight = state.in_flight.saturating_sub(1);
        }
    }

    struct StepChildRunner {
        behaviors: HashMap<SyncIntegrationId, TestChildRunnerBehavior>,
        release_receivers: Mutex<HashMap<SyncIntegrationId, mpsc::Receiver<()>>>,
        started_tx: Sender<SyncIntegrationId>,
        ready_tx: Option<Sender<SyncIntegrationId>>,
        completed_tx: Option<Sender<SyncIntegrationId>>,
        state: Mutex<TestChildRunnerState>,
    }

    impl StepChildRunner {
        fn new(
            behaviors: HashMap<SyncIntegrationId, TestChildRunnerBehavior>,
            release_receivers: HashMap<SyncIntegrationId, mpsc::Receiver<()>>,
            started_tx: Sender<SyncIntegrationId>,
            completed_tx: Option<Sender<SyncIntegrationId>>,
        ) -> Self {
            Self {
                behaviors,
                release_receivers: Mutex::new(release_receivers),
                started_tx,
                ready_tx: None,
                completed_tx,
                state: Mutex::new(TestChildRunnerState::default()),
            }
        }

        fn with_ready_tx(mut self, ready_tx: Sender<SyncIntegrationId>) -> Self {
            self.ready_tx = Some(ready_tx);
            self
        }

        fn snapshot(&self) -> TestChildRunnerState {
            lock_or_recover(&self.state).clone()
        }
    }

    impl IntegrationChildRunner for StepChildRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            let integration_id = request.workset.integration_id;
            {
                let mut state = lock_or_recover(&self.state);
                state.starts.push(integration_id);
                state.in_flight += 1;
                state.max_in_flight = state.max_in_flight.max(state.in_flight);
            }
            self.started_tx
                .send(integration_id)
                .expect("test start notification should be delivered");
            let _in_flight_guard = InFlightGuard { state: &self.state };

            let release_rx = lock_or_recover(&self.release_receivers).remove(&integration_id);
            if let Some(release_rx) = release_rx {
                if let Some(ready_tx) = &self.ready_tx {
                    ready_tx
                        .send(integration_id)
                        .expect("test ready notification should be delivered");
                }
                release_rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("test release signal should be delivered");
            }

            let behavior = self.behaviors.get(&integration_id).cloned().unwrap_or(
                TestChildRunnerBehavior::Summary {
                    synced: request.workset.total_addresses(),
                    failed: 0,
                    skipped: 0,
                    rate_limited: Vec::new(),
                    error: None,
                },
            );
            let result = match behavior {
                TestChildRunnerBehavior::Summary {
                    synced,
                    failed,
                    skipped,
                    rate_limited,
                    error,
                } => Ok(make_summary_for_test(
                    request.run.run_id,
                    request.workset.total_addresses(),
                    synced,
                    failed,
                    skipped,
                    &rate_limited,
                    error,
                )),
                TestChildRunnerBehavior::Error(message) => {
                    Err(UserTransactionMonitorError::Http(message.to_string()))
                }
                TestChildRunnerBehavior::Panic => {
                    panic!("panic requested for integration {integration_id}")
                }
            };

            {
                let mut state = lock_or_recover(&self.state);
                state.completions.push(integration_id);
            }
            if let Some(completed_tx) = &self.completed_tx {
                completed_tx
                    .send(integration_id)
                    .expect("test completion notification should be delivered");
            }
            result
        }
    }

    struct ThreadRecordingRunner {
        thread_ids: Mutex<Vec<std::thread::ThreadId>>,
    }

    impl IntegrationChildRunner for ThreadRecordingRunner {
        fn run_child_cycle(
            &self,
            request: IntegrationChildCycleRequest<'_>,
        ) -> Result<UserTransactionMonitorSummary, UserTransactionMonitorError> {
            self.thread_ids
                .lock()
                .expect("thread recorder lock should not be poisoned")
                .push(std::thread::current().id());
            Ok(make_summary_for_test(
                request.run.run_id,
                request.workset.total_addresses(),
                1,
                0,
                0,
                &[],
                None,
            ))
        }
    }

    #[test]
    fn zero_worksets_return_success_without_child() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let runner = ThreadRecordingRunner {
            thread_ids: Mutex::new(Vec::new()),
        };

        let result = run_parent_sync_cycle_with_runner(
            ParentSyncCycleRequest {
                run,
                clients,
                preload: &preload,
                non_hd_addresses: Vec::new(),
                hd_bundles: Vec::new(),
            },
            &runner,
        );

        assert_eq!(result.aggregate_result, AggregateSyncResult::Success);
        assert!(result.child_summaries.is_empty());
        assert!(
            runner
                .thread_ids
                .lock()
                .expect("thread recorder lock should not be poisoned")
                .is_empty()
        );
    }

    #[test]
    fn single_workset_runs_in_scoped_child_thread() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let runner = ThreadRecordingRunner {
            thread_ids: Mutex::new(Vec::new()),
        };

        let result = run_parent_sync_cycle_with_runner(
            ParentSyncCycleRequest {
                run,
                clients,
                preload: &preload,
                non_hd_addresses: vec![make_sync_address(
                    "bc1qsingleworkset",
                    SyncedAssetId::Bitcoin,
                    Network::Mainnet,
                    None,
                    None,
                    None,
                    None,
                )],
                hd_bundles: Vec::new(),
            },
            &runner,
        );

        let thread_ids = runner
            .thread_ids
            .lock()
            .expect("thread recorder lock should not be poisoned");
        assert_eq!(result.aggregate_result, AggregateSyncResult::Success);
        assert_eq!(result.child_summaries.len(), 1);
        assert_eq!(thread_ids.len(), 1);
        assert_ne!(thread_ids[0], std::thread::current().id());
    }

    #[test]
    fn partition_sync_worksets_groups_work_by_integration() {
        let bitcoin_account_id = DigitalAssetAccountId::new();
        let ethereum_account_id = DigitalAssetAccountId::new();
        let bitcoin_non_hd = make_sync_address(
            "bc1qpartitionbtc",
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            Some(bitcoin_account_id),
            None,
            None,
            None,
        );
        let ethereum_non_hd = make_sync_address(
            "0x1234567890123456789012345678901234567890",
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            Some(ethereum_account_id),
            None,
            None,
            None,
        );
        let bitcoin_bundle = AccountSyncBundle {
            account_id: bitcoin_account_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            hd_key_extended_pubkey: "zpub-partition-btc".to_string(),
            address_scheme: crate::wallets::AddressScheme::NativeSegwit,
            sync_state: None,
            external_addresses: vec![make_sync_address(
                "bc1qpartitionhd0",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                Some(bitcoin_account_id),
                Some(crate::wallets::AddressScheme::NativeSegwit),
                Some(0),
                Some(0),
            )],
            internal_addresses: Vec::new(),
        };
        let ethereum_bundle = AccountSyncBundle {
            account_id: ethereum_account_id,
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            hd_key_extended_pubkey: "zpub-partition-eth".to_string(),
            address_scheme: crate::wallets::AddressScheme::Standard,
            sync_state: None,
            external_addresses: vec![make_sync_address(
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                Some(ethereum_account_id),
                Some(crate::wallets::AddressScheme::Standard),
                Some(0),
                Some(0),
            )],
            internal_addresses: Vec::new(),
        };

        let worksets = partition_sync_worksets(
            vec![ethereum_non_hd, bitcoin_non_hd],
            vec![ethereum_bundle, bitcoin_bundle],
        );

        assert_eq!(
            worksets
                .iter()
                .map(|workset| workset.integration_id)
                .collect::<Vec<_>>(),
            vec![SyncIntegrationId::Mempool, SyncIntegrationId::Etherscan]
        );
        assert_eq!(worksets[0].non_hd_addresses.len(), 1);
        assert_eq!(worksets[0].hd_bundles.len(), 1);
        assert_eq!(worksets[1].non_hd_addresses.len(), 1);
        assert_eq!(worksets[1].hd_bundles.len(), 1);
    }

    #[test]
    fn run_parent_sync_cycle_with_runner_overlaps_child_integrations() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let non_hd_addresses = vec![
            make_sync_address(
                "bc1qchildoverlapbtc",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
            make_sync_address(
                "0x1111111111111111111111111111111111111111",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
        ];
        let (started_tx, started_rx) = mpsc::channel();
        let (mempool_release_tx, mempool_release_rx) = mpsc::channel();
        let (etherscan_release_tx, etherscan_release_rx) = mpsc::channel();
        let runner = StepChildRunner::new(
            HashMap::from([
                (
                    SyncIntegrationId::Mempool,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
                (
                    SyncIntegrationId::Etherscan,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
            ]),
            HashMap::from([
                (SyncIntegrationId::Mempool, mempool_release_rx),
                (SyncIntegrationId::Etherscan, etherscan_release_rx),
            ]),
            started_tx,
            None,
        );

        std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                run_parent_sync_cycle_with_runner(
                    ParentSyncCycleRequest {
                        run,
                        clients,
                        preload: &preload,
                        non_hd_addresses,
                        hd_bundles: Vec::new(),
                    },
                    &runner,
                )
            });

            let first = started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("first child should start");
            let second = started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("second child should start");
            assert_eq!(
                HashSet::from([first, second]),
                HashSet::from([SyncIntegrationId::Mempool, SyncIntegrationId::Etherscan])
            );
            assert_eq!(runner.snapshot().max_in_flight, 2);

            mempool_release_tx
                .send(())
                .expect("mempool child should be releasable");
            etherscan_release_tx
                .send(())
                .expect("etherscan child should be releasable");

            let result = handle.join().expect("parent run should complete");
            assert_eq!(result.aggregate_result, AggregateSyncResult::Success);
            assert_eq!(result.child_summaries.len(), 2);
            assert_eq!(runner.snapshot().in_flight, 0);
        });
    }

    #[test]
    fn run_parent_sync_cycle_with_runner_waits_for_every_child_before_returning() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let non_hd_addresses = vec![
            make_sync_address(
                "bc1qchildwaitbtc",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
            make_sync_address(
                "0x2222222222222222222222222222222222222222",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
        ];
        let (started_tx, started_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (mempool_release_tx, mempool_release_rx) = mpsc::channel();
        let (etherscan_release_tx, etherscan_release_rx) = mpsc::channel();
        let runner = StepChildRunner::new(
            HashMap::from([
                (
                    SyncIntegrationId::Mempool,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
                (
                    SyncIntegrationId::Etherscan,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
            ]),
            HashMap::from([
                (SyncIntegrationId::Mempool, mempool_release_rx),
                (SyncIntegrationId::Etherscan, etherscan_release_rx),
            ]),
            started_tx,
            Some(completed_tx),
        )
        .with_ready_tx(ready_tx);

        std::thread::scope(|scope| {
            let (result_tx, result_rx) = mpsc::channel();
            scope.spawn(move || {
                let result = run_parent_sync_cycle_with_runner(
                    ParentSyncCycleRequest {
                        run,
                        clients,
                        preload: &preload,
                        non_hd_addresses,
                        hd_bundles: Vec::new(),
                    },
                    &runner,
                );
                result_tx
                    .send(result)
                    .expect("parent result should be sent");
            });

            started_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("first child should start");
            started_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("second child should start");

            // Wait until both children are actually blocking on their release
            // receivers before sending any release signals. This avoids a race
            // where the test sends a release signal before the child thread has
            // been scheduled to call recv_timeout.
            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("first child should be ready");
            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("second child should be ready");

            mempool_release_tx
                .send(())
                .expect("mempool child should be releasable");
            let completed_integration = completed_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("released child should complete");
            assert_eq!(completed_integration, SyncIntegrationId::Mempool);
            assert!(
                matches!(result_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                "parent must still be waiting on the blocked child"
            );

            etherscan_release_tx
                .send(())
                .expect("etherscan child should be releasable");
            let result = result_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("parent should finish after all children complete");
            assert_eq!(result.aggregate_result, AggregateSyncResult::Success);
        });
    }

    #[test]
    fn run_parent_sync_cycle_with_runner_reduces_success_and_blocked_to_partial() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let non_hd_addresses = vec![
            make_sync_address(
                "bc1qblockedpartialbtc",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
            make_sync_address(
                "0x3333333333333333333333333333333333333333",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
        ];
        let (started_tx, _started_rx) = mpsc::channel();
        let runner = StepChildRunner::new(
            HashMap::from([
                (
                    SyncIntegrationId::Mempool,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
                (
                    SyncIntegrationId::Etherscan,
                    TestChildRunnerBehavior::Summary {
                        synced: 0,
                        failed: 0,
                        skipped: 1,
                        rate_limited: vec![LABEL_ETHERSCAN],
                        error: None,
                    },
                ),
            ]),
            HashMap::new(),
            started_tx,
            None,
        );

        let result = run_parent_sync_cycle_with_runner(
            ParentSyncCycleRequest {
                run,
                clients,
                preload: &preload,
                non_hd_addresses,
                hd_bundles: Vec::new(),
            },
            &runner,
        );

        assert_eq!(result.aggregate_result, AggregateSyncResult::Partial);
        assert_eq!(result.summary.failure_error, None);
        assert_eq!(result.summary.addresses_synced, AddressCount::from_u32(1));
        assert_eq!(result.summary.addresses_failed, AddressCount::zero());
        assert_eq!(
            result.summary.rate_limited,
            vec![RateLimitedIntegration {
                integration: LABEL_ETHERSCAN.to_string(),
            }]
        );
    }

    #[test]
    fn run_parent_sync_cycle_with_runner_reduces_success_and_panic_to_partial() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let non_hd_addresses = vec![
            make_sync_address(
                "bc1qpanicpartialbtc",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
            make_sync_address(
                "0x4444444444444444444444444444444444444444",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
        ];
        let (started_tx, started_rx) = mpsc::channel();
        let (mempool_release_tx, mempool_release_rx) = mpsc::channel();
        let (etherscan_release_tx, etherscan_release_rx) = mpsc::channel();
        let runner = StepChildRunner::new(
            HashMap::from([
                (
                    SyncIntegrationId::Mempool,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
                (SyncIntegrationId::Etherscan, TestChildRunnerBehavior::Panic),
            ]),
            HashMap::from([
                (SyncIntegrationId::Mempool, mempool_release_rx),
                (SyncIntegrationId::Etherscan, etherscan_release_rx),
            ]),
            started_tx,
            None,
        );

        std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                run_parent_sync_cycle_with_runner(
                    ParentSyncCycleRequest {
                        run,
                        clients,
                        preload: &preload,
                        non_hd_addresses,
                        hd_bundles: Vec::new(),
                    },
                    &runner,
                )
            });

            started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("first child should start");
            started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("second child should start");
            mempool_release_tx
                .send(())
                .expect("mempool child should be releasable");
            etherscan_release_tx
                .send(())
                .expect("etherscan child should be releasable");

            let result = handle
                .join()
                .expect("parent run should normalize child panic");
            assert_eq!(result.aggregate_result, AggregateSyncResult::Partial);
            assert_eq!(runner.snapshot().in_flight, 0);
            assert_eq!(
                result.summary.failure_error,
                Some(SyncErrorMessage::sanitize(
                    "fatal etherscan sync worker failure: child thread panicked"
                ))
            );
            assert_eq!(result.child_summaries.len(), 2);
            assert_eq!(
                result.child_summaries[1].failure_kind,
                Some(IntegrationChildFailureKind::FatalExecution)
            );
        });
    }

    #[test]
    fn run_parent_sync_cycle_with_runner_reduces_success_and_child_error_to_partial() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let non_hd_addresses = vec![
            make_sync_address(
                "bc1qerrorpartialbtc",
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
            make_sync_address(
                "0x5555555555555555555555555555555555555555",
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                None,
                None,
                None,
                None,
            ),
        ];
        let (started_tx, _started_rx) = mpsc::channel();
        let runner = StepChildRunner::new(
            HashMap::from([
                (
                    SyncIntegrationId::Mempool,
                    TestChildRunnerBehavior::Summary {
                        synced: 1,
                        failed: 0,
                        skipped: 0,
                        rate_limited: Vec::new(),
                        error: None,
                    },
                ),
                (
                    SyncIntegrationId::Etherscan,
                    TestChildRunnerBehavior::Error("synthetic child runner failure"),
                ),
            ]),
            HashMap::new(),
            started_tx,
            None,
        );

        let result = run_parent_sync_cycle_with_runner(
            ParentSyncCycleRequest {
                run,
                clients,
                preload: &preload,
                non_hd_addresses,
                hd_bundles: Vec::new(),
            },
            &runner,
        );

        assert_eq!(result.aggregate_result, AggregateSyncResult::Partial);
        assert_eq!(
            result.summary.failure_error,
            Some(SyncErrorMessage::sanitize(
                "fatal etherscan sync worker failure: Sync HTTP request failed: synthetic child runner failure"
            ))
        );
    }

    #[test]
    fn partition_sync_worksets_keeps_empty_hd_bundle_for_derivation() {
        let account_id = DigitalAssetAccountId::new();
        let empty_bundle = bitcoin_hd_bundle(account_id, Vec::new(), Vec::new());

        let worksets = partition_sync_worksets(Vec::new(), vec![empty_bundle]);

        assert_eq!(worksets.len(), 1);
        assert_eq!(worksets[0].integration_id, SyncIntegrationId::Mempool);
        assert_eq!(worksets[0].total_addresses(), 0);
        assert_eq!(worksets[0].hd_bundles.len(), 1);
    }

    #[test]
    fn parent_sync_cycle_starts_child_for_empty_hd_bundle() {
        let clock = FakeClock::new(test_utc_now());
        let run = make_run_context(&clock);
        let preload = empty_sync_run_preload();
        let http_counters = SyncHttpCounters::new();
        let clients = SyncClients {
            mempool_client: None,
            etherscan_api_key: None,
            etherscan_base_url: None,
            http_counters: &http_counters,
        };
        let account_id = DigitalAssetAccountId::new();
        let empty_bundle = bitcoin_hd_bundle(account_id, Vec::new(), Vec::new());
        let (started_tx, started_rx) = mpsc::channel();
        let runner = StepChildRunner::new(HashMap::new(), HashMap::new(), started_tx, None);

        let result = run_parent_sync_cycle_with_runner(
            ParentSyncCycleRequest {
                run,
                clients,
                preload: &preload,
                non_hd_addresses: Vec::new(),
                hd_bundles: vec![empty_bundle],
            },
            &runner,
        );

        let started = started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("empty HD bundle should start a child worker");
        assert_eq!(started, SyncIntegrationId::Mempool);
        assert_eq!(result.child_summaries.len(), 1);
        assert_eq!(
            result.child_summaries[0].integration_id,
            SyncIntegrationId::Mempool
        );
        assert_eq!(result.summary.addresses_total.value(), 0);
    }

    #[test]
    fn parent_error_reduction_is_order_independent() {
        let run_id = TransactionSyncRunId::new();
        let blocked = integration_child_summary_from_summary(
            SyncIntegrationId::Mempool,
            make_summary_for_test(run_id, 1, 0, 0, 1, &[LABEL_MEMPOOL], None),
        );
        let fatal = synthetic_failed_child_summary(
            run_id,
            SyncIntegrationId::Etherscan,
            1,
            SyncErrorMessage::sanitize("fatal etherscan failure"),
            IntegrationChildFailureKind::FatalExecution,
        );

        let forward = vec![blocked.clone(), fatal.clone()];
        let reverse = vec![fatal, blocked];

        assert_eq!(reduce_parent_result(&forward), AggregateSyncResult::Partial);
        assert_eq!(
            reduce_parent_result(&forward),
            reduce_parent_result(&reverse)
        );
        assert_eq!(
            select_child_failure_error(&forward),
            select_child_failure_error(&reverse)
        );
        assert_eq!(
            aggregate_child_summaries(run_id, &forward).failure_error,
            aggregate_child_summaries(run_id, &reverse).failure_error
        );
    }
}

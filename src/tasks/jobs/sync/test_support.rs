use crate::db::SyncAddress;
use crate::models::UserId;
use crate::tasks::TriggerSource;
use crate::transactions::{
    ApiConfirmedBalance, ChainTipHeight, TrackedAddress, TransactionCount, TransactionSyncRunId,
};
use crate::wallets::{
    AddressScheme, DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId,
};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::context::{RunContext, SyncClock, SyncIterationResult};
use super::error::UserTransactionMonitorError;
use super::executor::{AddressSyncExecutionRequest, AddressSyncExecutor};
use super::hd_scan::{AddressDerivationProvider, AddressDerivationRequest, DerivedSyncAddress};
use super::rate_limit::global_rate_limiter;

pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct FakeClock {
    pub(crate) utc_now: Mutex<DateTime<Utc>>,
    pub(crate) instant_now: Mutex<Instant>,
    pub(crate) sleep_calls: Mutex<Vec<Duration>>,
}

impl FakeClock {
    pub(crate) fn new(utc_now: DateTime<Utc>) -> Self {
        Self {
            utc_now: Mutex::new(utc_now),
            instant_now: Mutex::new(Instant::now()),
            sleep_calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn sleep_count(&self) -> usize {
        lock_or_recover(&self.sleep_calls).len()
    }
}

impl SyncClock for FakeClock {
    fn utc_now(&self) -> DateTime<Utc> {
        *lock_or_recover(&self.utc_now)
    }

    fn instant_now(&self) -> Instant {
        *lock_or_recover(&self.instant_now)
    }

    fn sleep(&self, duration: Duration) {
        lock_or_recover(&self.sleep_calls).push(duration);
        let mut instant_now = lock_or_recover(&self.instant_now);
        *instant_now += duration;
        if let Ok(delta) = chrono::Duration::from_std(duration) {
            let mut utc_now = lock_or_recover(&self.utc_now);
            *utc_now += delta;
        }
    }
}

pub(crate) enum FakeSyncOutcome {
    Success {
        new_tx_count: u32,
        updated_tx_count: u32,
    },
    SuccessClearingMempoolBackfill {
        new_tx_count: u32,
        updated_tx_count: u32,
    },
    SuccessWithObservedActivity,
    SuccessWithCoverage {
        account_id: DigitalAssetAccountId,
    },
    Failure {
        message: String,
    },
    FailureWithCoverage {
        message: String,
        account_id: DigitalAssetAccountId,
    },
    RateLimited {
        integration: String,
    },
}

pub(crate) struct FakeAddressSyncExecutor {
    pub(crate) outcomes: VecDeque<FakeSyncOutcome>,
    pub(crate) calls: Vec<DigitalAssetAddressId>,
    pub(crate) historical_backfill_enabled_calls: Vec<bool>,
    pub(crate) legacy_mempool_history_repair_calls: Vec<bool>,
    pub(crate) mempool_history_frontier_calls:
        Vec<Option<crate::db::HdMempoolHistoryFrontierUpdate>>,
    pub(crate) observed_lock_free: Vec<bool>,
    pub(crate) iteration_api_confirmed_balance: Option<ApiConfirmedBalance>,
    pub(crate) iteration_ledger_rebuild_required: bool,
}

impl FakeAddressSyncExecutor {
    pub(crate) fn new(outcomes: Vec<FakeSyncOutcome>) -> Self {
        Self {
            outcomes: VecDeque::from(outcomes),
            calls: Vec::new(),
            historical_backfill_enabled_calls: Vec::new(),
            legacy_mempool_history_repair_calls: Vec::new(),
            mempool_history_frontier_calls: Vec::new(),
            observed_lock_free: Vec::new(),
            iteration_api_confirmed_balance: None,
            iteration_ledger_rebuild_required: true,
        }
    }

    pub(crate) fn with_iteration_api_confirmed_balance(
        mut self,
        balance: Option<ApiConfirmedBalance>,
    ) -> Self {
        self.iteration_api_confirmed_balance = balance;
        self
    }

    pub(crate) fn with_iteration_ledger_rebuild_required(mut self, required: bool) -> Self {
        self.iteration_ledger_rebuild_required = required;
        self
    }
}

impl AddressSyncExecutor for FakeAddressSyncExecutor {
    fn sync_one_iteration(
        &mut self,
        request: AddressSyncExecutionRequest<'_>,
    ) -> Result<SyncIterationResult, UserTransactionMonitorError> {
        crate::db::debug_assert_user_db_unlocked(
            request.run.user_id,
            "fake executor iteration dispatch",
        );
        let lock_state = crate::db::user_db_lock_state_for_test(request.run.user_id);
        self.observed_lock_free
            .push(lock_state.read_locks == 0 && lock_state.write_locks == 0);
        self.calls.push(request.address.address_id);
        self.historical_backfill_enabled_calls
            .push(request.historical_backfill_enabled);
        self.legacy_mempool_history_repair_calls
            .push(request.legacy_mempool_history_repair);
        self.mempool_history_frontier_calls
            .push(request.mempool_history_page_frontier);
        let outcome = self
            .outcomes
            .pop_front()
            .unwrap_or(FakeSyncOutcome::Success {
                new_tx_count: 0,
                updated_tx_count: 0,
            });
        let clear_mempool_backfill = matches!(
            &outcome,
            FakeSyncOutcome::SuccessClearingMempoolBackfill { .. }
        );
        match outcome {
            FakeSyncOutcome::Success {
                new_tx_count,
                updated_tx_count,
            }
            | FakeSyncOutcome::SuccessClearingMempoolBackfill {
                new_tx_count,
                updated_tx_count,
            } => {
                if clear_mempool_backfill {
                    crate::db::update_address_mempool_backfill_cursor(
                        request.run.user_id,
                        request.address.address_id,
                        None,
                    )?;
                    crate::db::update_address_mempool_expected_tx_count(
                        request.run.user_id,
                        request.address.address_id,
                        None,
                    )?;
                }
                Ok(SyncIterationResult {
                    new_tx_count: TransactionCount::from_u32(new_tx_count),
                    updated_tx_count: TransactionCount::from_u32(updated_tx_count),
                    coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                    tip_height: ChainTipHeight::try_new(100).expect("valid tip height"),
                    completed_at: request.run.clock.utc_now(),
                    has_more_work: false,
                    early_exited: false,
                    observed_activity: false,
                    ledger_rebuild_required: self.iteration_ledger_rebuild_required,
                    raw_run_summary_json: None,
                    api_confirmed_balance: self.iteration_api_confirmed_balance,
                })
            }
            FakeSyncOutcome::SuccessWithObservedActivity => Ok(SyncIterationResult {
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
                tip_height: ChainTipHeight::try_new(100).expect("valid tip height"),
                completed_at: request.run.clock.utc_now(),
                has_more_work: false,
                early_exited: false,
                observed_activity: true,
                ledger_rebuild_required: false,
                raw_run_summary_json: None,
                api_confirmed_balance: None,
            }),
            FakeSyncOutcome::SuccessWithCoverage { account_id } => Ok(SyncIterationResult {
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                coverage_invalidation: crate::db::CoverageInvalidationTargets {
                    address_ids: std::collections::HashSet::from([request.address.address_id]),
                    account_ids: std::collections::HashSet::from([account_id]),
                },
                tip_height: ChainTipHeight::try_new(100).expect("valid tip height"),
                completed_at: request.run.clock.utc_now(),
                has_more_work: false,
                early_exited: false,
                observed_activity: false,
                ledger_rebuild_required: false,
                raw_run_summary_json: None,
                api_confirmed_balance: None,
            }),
            FakeSyncOutcome::Failure { message } => Err(UserTransactionMonitorError::Http(message)),
            FakeSyncOutcome::FailureWithCoverage {
                message,
                account_id,
            } => Err(
                UserTransactionMonitorError::Http(message).with_coverage_invalidation(
                    crate::db::CoverageInvalidationTargets {
                        address_ids: std::collections::HashSet::from([request.address.address_id]),
                        account_ids: std::collections::HashSet::from([account_id]),
                    },
                ),
            ),
            FakeSyncOutcome::RateLimited { integration } => {
                Err(UserTransactionMonitorError::RateLimited {
                    integration,
                    message: "fake rate limited".to_string(),
                    retry_after: None,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DerivationRequestLog {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) derivation_change: u32,
    pub(crate) count: u32,
}

pub(crate) struct FakeAddressDerivationProvider {
    pub(crate) batches: VecDeque<Vec<DerivedSyncAddress>>,
    pub(crate) requests: Vec<DerivationRequestLog>,
}

impl FakeAddressDerivationProvider {
    pub(crate) fn new(batches: Vec<Vec<DerivedSyncAddress>>) -> Self {
        Self {
            batches: VecDeque::from(batches),
            requests: Vec::new(),
        }
    }
}

impl AddressDerivationProvider for FakeAddressDerivationProvider {
    fn derive_next_addresses(
        &mut self,
        request: AddressDerivationRequest,
    ) -> Result<Vec<DerivedSyncAddress>, UserTransactionMonitorError> {
        self.requests.push(DerivationRequestLog {
            account_id: request.account_id,
            derivation_change: request.derivation_change,
            count: request.count,
        });
        let derived_batch = self.batches.pop_front().unwrap_or_default();
        for derived_address in &derived_batch {
            let sync_address = SyncAddress {
                address_id: derived_address.address_id,
                address: derived_address.address.clone(),
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                account_id: Some(request.account_id),
                derivation_change: Some(derived_address.derivation_change),
                derivation_index: Some(derived_address.derivation_index),
                address_scheme: Some(request.address_scheme),
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
            crate::db::persist_sync_address_fixture(request.user_id, &sync_address, request.now)
                .map_err(UserTransactionMonitorError::Db)?;
        }
        Ok(derived_batch)
    }
}

pub(crate) fn test_utc_now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0)
        .single()
        .expect("valid timestamp")
}

pub(crate) fn make_run_context<'a>(clock: &'a dyn SyncClock) -> RunContext<'a> {
    let user_id = UserId::new();
    crate::db::enable_user_test_mode();
    crate::db::initialize_user_db_for_test(user_id).expect("test user db should initialize");
    RunContext {
        user_id,
        run_id: TransactionSyncRunId::new(),
        source: TriggerSource::ManualInternal,
        started_at: clock.utc_now(),
        clock,
    }
}

pub(crate) fn make_sync_address(
    address: &str,
    asset_id: SyncedAssetId,
    network: Network,
    account_id: Option<DigitalAssetAccountId>,
    address_scheme: Option<AddressScheme>,
    derivation_change: Option<u32>,
    derivation_index: Option<u32>,
) -> SyncAddress {
    SyncAddress {
        address_id: DigitalAssetAddressId::new(),
        address: TrackedAddress::parse(address).expect("valid test address"),
        asset_id,
        network,
        account_id,
        derivation_change,
        derivation_index,
        address_scheme,
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
    }
}

pub(crate) fn make_derived_sync_address(
    address: &str,
    derivation_change: u32,
    derivation_index: u32,
) -> DerivedSyncAddress {
    DerivedSyncAddress {
        address_id: DigitalAssetAddressId::new(),
        address: TrackedAddress::parse(address).expect("valid derived address"),
        derivation_change,
        derivation_index,
    }
}

pub(crate) fn persist_sync_address_for_test(run: RunContext<'_>, address: &SyncAddress) {
    crate::db::persist_sync_address_fixture(run.user_id, address, run.started_at)
        .expect("sync address fixture should persist");
}

pub(crate) fn persist_sync_addresses_for_test(run: RunContext<'_>, addresses: &[SyncAddress]) {
    for address in addresses {
        persist_sync_address_for_test(run, address);
    }
}

pub(crate) fn clear_rate_limiter_for_test() {
    let mut guard = match global_rate_limiter().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
}

pub(crate) fn with_rate_limiter_isolated<T>(f: impl FnOnce() -> T) -> T {
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = TEST_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    clear_rate_limiter_for_test();
    let result = f();
    clear_rate_limiter_for_test();
    result
}

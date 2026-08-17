use crate::asset_capabilities::SyncProviderId;
use crate::db::raw_ingestion::SyncRunTriggerKind as RawSyncRunTriggerKind;
use crate::db::raw_ingestion::{OpaqueJsonText, SourceConnectionId, SyncRunId};
use crate::integrations::mempool::MempoolClient;
use crate::models::{EtherscanBaseUrl, RawEtherscanApiKey, UserId, UserSettings};
use crate::tasks::TriggerSource;
use crate::transactions::{
    AddressCount, ApiConfirmedBalance, ChainTipHeight, RateLimitedIntegration, SyncErrorMessage,
    TransactionCount, TransactionSyncResult, TransactionSyncRunId, TransactionSyncScope,
};
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub(crate) const LABEL_MEMPOOL: &str = "mempool";
pub(crate) const LABEL_ETHERSCAN: &str = "etherscan";
pub(crate) const BALANCE_REFRESH_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) use crate::transactions::ADDRESS_FAILURE_THRESHOLD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncIterationStopReason {
    NoEligibleAction,
    OnlyBlockedActions,
    BalanceRefreshesFresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SyncPlannerPriorityTier {
    ActiveUnfinishedBackfill,
    PendingTransactionRefresh,
    NeverAttemptedFirstSync,
    RetryableFailedAttempt,
    KnownActivityRefresh,
    BalanceRefresh,
    ColdRefresh,
    HdDerivation,
}

pub(crate) fn is_balance_refresh_fresh(
    last_completed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    let Some(last_completed_at) = last_completed_at else {
        return false;
    };
    let elapsed = now.signed_duration_since(last_completed_at);
    if elapsed.num_seconds() < 0 {
        return true;
    }
    match elapsed.to_std() {
        Ok(elapsed_std) => elapsed_std < BALANCE_REFRESH_TTL,
        Err(_) => false,
    }
}

pub(crate) fn is_successful_balance_refresh_fresh(
    last_result: Option<TransactionSyncResult>,
    last_completed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    matches!(last_result, Some(TransactionSyncResult::Success))
        && is_balance_refresh_fresh(last_completed_at, now)
}

pub(crate) trait SyncClock: Send + Sync {
    fn utc_now(&self) -> DateTime<Utc>;
    fn instant_now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub(super) struct SystemSyncClock;

impl SyncClock for SystemSyncClock {
    fn utc_now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn instant_now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RunContext<'a> {
    pub(crate) user_id: UserId,
    pub(crate) run_id: TransactionSyncRunId,
    pub(crate) source: TriggerSource,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) clock: &'a dyn SyncClock,
}

#[derive(Debug, Clone)]
pub(crate) struct SyncHttpCounters {
    pub(super) total_api_calls: Arc<AtomicU64>,
    pub(super) pagination_cache_hits: Arc<AtomicU64>,
}

impl SyncHttpCounters {
    pub(super) fn new() -> Self {
        Self {
            total_api_calls: Arc::new(AtomicU64::new(0)),
            pagination_cache_hits: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn total_api_calls(&self) -> u64 {
        self.total_api_calls.load(Ordering::Relaxed)
    }

    pub(super) fn pagination_cache_hits(&self) -> u64 {
        self.pagination_cache_hits.load(Ordering::Relaxed)
    }

    pub(crate) fn total_api_calls_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.total_api_calls)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SyncClients<'a> {
    pub(crate) mempool_client: Option<&'a MempoolClient>,
    pub(crate) etherscan_api_key: Option<&'a RawEtherscanApiKey>,
    pub(crate) etherscan_base_url: Option<&'a EtherscanBaseUrl>,
    pub(crate) http_counters: &'a SyncHttpCounters,
}

#[derive(Debug, Clone)]
pub(crate) struct SyncRunPreload {
    pub(crate) settings: UserSettings,
    pub(crate) historical_backfill_enabled: bool,
    pub(crate) historical_backfill_transactions_per_account: u32,
    pub(crate) account_labels: HashMap<DigitalAssetAccountId, String>,
    pub(crate) known_activity_address_ids: HashSet<DigitalAssetAddressId>,
    pub(crate) pending_address_ids: HashSet<DigitalAssetAddressId>,
    pub(crate) bitcoin_history_repair_account_ids: HashSet<DigitalAssetAccountId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InterAddressPacingPolicy {
    mempool_delay: Duration,
    etherscan_delay: Duration,
}

impl InterAddressPacingPolicy {
    pub(crate) const fn new(mempool_delay: Duration, etherscan_delay: Duration) -> Self {
        Self {
            mempool_delay,
            etherscan_delay,
        }
    }

    pub(crate) const fn delay_for_provider(self, provider: SyncProviderId) -> Duration {
        match provider {
            SyncProviderId::MempoolSpace => self.mempool_delay,
            SyncProviderId::Etherscan => self.etherscan_delay,
        }
    }
}

pub(crate) const DEFAULT_INTER_ADDRESS_PACING_POLICY: InterAddressPacingPolicy =
    InterAddressPacingPolicy::new(Duration::from_millis(250), Duration::from_millis(500));

#[derive(Debug, Clone, Copy)]
pub(crate) struct SingleAddressProgressPlan {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) is_first_sync: bool,
    pub(crate) expected_tx_count: Option<TransactionCount>,
    pub(crate) expected_tx_count_is_lower_bound: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncIterationResult {
    pub(crate) new_tx_count: TransactionCount,
    pub(crate) updated_tx_count: TransactionCount,
    pub(crate) coverage_invalidation: crate::db::CoverageInvalidationTargets,
    pub(crate) tip_height: ChainTipHeight,
    pub(crate) completed_at: DateTime<Utc>,
    pub(crate) has_more_work: bool,
    pub(crate) early_exited: bool,
    pub(crate) observed_activity: bool,
    /// Transient, never persisted. `true` only after this iteration successfully
    /// reconciled a non-empty transaction batch. Provider/address activity and
    /// the current provider balance are deliberately not inputs: they can change
    /// while the local transaction set does not.
    pub(crate) ledger_rebuild_required: bool,
    pub(crate) raw_run_summary_json: Option<OpaqueJsonText>,
    pub(crate) api_confirmed_balance: Option<ApiConfirmedBalance>,
}

impl SyncIterationResult {
    pub(crate) fn exhausted(tip_height: ChainTipHeight, completed_at: DateTime<Utc>) -> Self {
        Self {
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            coverage_invalidation: crate::db::CoverageInvalidationTargets::default(),
            tip_height,
            completed_at,
            has_more_work: false,
            early_exited: false,
            observed_activity: false,
            ledger_rebuild_required: false,
            raw_run_summary_json: None,
            api_confirmed_balance: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IntegrationSyncPlan {
    pub(crate) is_backfill_active: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct IntegrationIterationContext<'a> {
    pub(crate) run: RunContext<'a>,
    pub(crate) now_utc: DateTime<Utc>,
    pub(crate) now_instant: Instant,
    pub(crate) address: &'a crate::db::SyncAddress,
    pub(crate) clients: SyncClients<'a>,
    pub(crate) single_address_progress: Option<SingleAddressProgressPlan>,
    pub(crate) allow_known_confirmed_early_exit: bool,
    pub(crate) chain_tip: Option<ChainTipHeight>,
    pub(crate) raw_sync_run_id: SyncRunId,
    pub(crate) source_connection_id: &'a SourceConnectionId,
    pub(crate) is_backfill_active: bool,
    pub(crate) historical_backfill_enabled: bool,
    pub(crate) legacy_mempool_history_repair: bool,
    pub(crate) mempool_history_page_frontier: Option<crate::db::HdMempoolHistoryFrontierUpdate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserTransactionMonitorInterval(Duration);

impl UserTransactionMonitorInterval {
    const fn from_seconds(seconds: u64) -> Self {
        Self(Duration::from_secs(seconds))
    }

    pub(crate) const fn as_duration(self) -> Duration {
        self.0
    }
}

pub(crate) const USER_TRANSACTION_MONITOR_INTERVAL: UserTransactionMonitorInterval =
    UserTransactionMonitorInterval::from_seconds(300);
pub(crate) const USER_TRANSACTION_MONITOR_MIN_INTERVAL: UserTransactionMonitorInterval =
    UserTransactionMonitorInterval::from_seconds(60);
pub(crate) const USER_TRANSACTION_MONITOR_IDLE_INTERVAL: UserTransactionMonitorInterval =
    UserTransactionMonitorInterval::from_seconds(900);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserTransactionMonitorScheduleUrgency {
    Blocked,
    High,
    Normal,
    Low,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserTransactionMonitorScheduleReason {
    RateLimited,
    UnfinishedWork,
    Default,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserTransactionMonitorScheduleHint {
    pub(crate) interval: Duration,
    pub(crate) urgency: UserTransactionMonitorScheduleUrgency,
    pub(crate) reason: UserTransactionMonitorScheduleReason,
}

impl UserTransactionMonitorScheduleHint {
    pub(crate) fn next_due_at_utc(self, now_utc: DateTime<Utc>) -> Option<DateTime<Utc>> {
        chrono::Duration::from_std(self.interval)
            .ok()
            .and_then(|delta| now_utc.checked_add_signed(delta))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserTransactionMonitorSchedulePolicyInput {
    pub(crate) source: TriggerSource,
    pub(crate) has_unfinished_work: bool,
    pub(crate) is_idle: bool,
    pub(crate) blocked_for: Option<Duration>,
}

pub(crate) fn default_user_transaction_monitor_schedule_hint() -> UserTransactionMonitorScheduleHint
{
    UserTransactionMonitorScheduleHint {
        interval: USER_TRANSACTION_MONITOR_INTERVAL.as_duration(),
        urgency: UserTransactionMonitorScheduleUrgency::Normal,
        reason: UserTransactionMonitorScheduleReason::Default,
    }
}

pub(crate) fn compute_user_transaction_monitor_schedule_hint(
    input: UserTransactionMonitorSchedulePolicyInput,
) -> UserTransactionMonitorScheduleHint {
    if let Some(blocked_for) = input.blocked_for {
        return UserTransactionMonitorScheduleHint {
            interval: blocked_for,
            urgency: UserTransactionMonitorScheduleUrgency::Blocked,
            reason: UserTransactionMonitorScheduleReason::RateLimited,
        };
    }

    if input.has_unfinished_work {
        return UserTransactionMonitorScheduleHint {
            interval: USER_TRANSACTION_MONITOR_MIN_INTERVAL.as_duration(),
            urgency: UserTransactionMonitorScheduleUrgency::High,
            reason: UserTransactionMonitorScheduleReason::UnfinishedWork,
        };
    }

    if input.is_idle && !matches!(input.source, TriggerSource::ManualInternal) {
        return UserTransactionMonitorScheduleHint {
            interval: USER_TRANSACTION_MONITOR_IDLE_INTERVAL.as_duration(),
            urgency: UserTransactionMonitorScheduleUrgency::Low,
            reason: UserTransactionMonitorScheduleReason::Idle,
        };
    }

    default_user_transaction_monitor_schedule_hint()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserTransactionMonitorParams {
    pub(crate) run_id: TransactionSyncRunId,
    pub(crate) scope: TransactionSyncScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UserTransactionMonitorSummary {
    pub(crate) run_id: TransactionSyncRunId,
    pub(crate) new_tx_count: TransactionCount,
    pub(crate) updated_tx_count: TransactionCount,
    pub(crate) addresses_total: AddressCount,
    pub(crate) addresses_synced: AddressCount,
    pub(crate) addresses_failed: AddressCount,
    pub(crate) addresses_skipped: AddressCount,
    pub(crate) addresses_skipped_tip_unchanged: AddressCount,
    pub(crate) addresses_early_exited: AddressCount,
    pub(crate) pagination_cache_hits: u64,
    pub(crate) total_api_calls: u64,
    pub(crate) rate_limited: Vec<RateLimitedIntegration>,
    pub(crate) failure_error: Option<SyncErrorMessage>,
    pub(crate) bitcoin_history_repair_failure_error: Option<SyncErrorMessage>,
    pub(crate) schedule_hint: UserTransactionMonitorScheduleHint,
}

pub(super) fn to_address_count(value: usize) -> AddressCount {
    match u32::try_from(value) {
        Ok(as_u32) => AddressCount::from_u32(as_u32),
        Err(_) => AddressCount::from_u32(u32::MAX),
    }
}

pub(crate) fn is_first_sync(last_tip_height: Option<ChainTipHeight>) -> bool {
    match last_tip_height {
        None => true,
        Some(height) => height.value() <= 0_i64,
    }
}

pub(crate) fn raw_sync_run_trigger_kind(
    source: TriggerSource,
    is_backfill_active: bool,
) -> RawSyncRunTriggerKind {
    if is_backfill_active {
        return RawSyncRunTriggerKind::Backfill;
    }

    match source {
        TriggerSource::Schedule => RawSyncRunTriggerKind::Scheduled,
        TriggerSource::ManualInternal => RawSyncRunTriggerKind::Manual,
        TriggerSource::AutoAdd
        | TriggerSource::AutoUpgrade
        | TriggerSource::AutoSessionRestore
        | TriggerSource::AutoFreshness => RawSyncRunTriggerKind::Scheduled,
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn schedule_hint_prioritizes_rate_limit_block_deadline() {
        let hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source: TriggerSource::Schedule,
                has_unfinished_work: true,
                is_idle: false,
                blocked_for: Some(Duration::from_secs(37)),
            },
        );

        assert_eq!(hint.interval, Duration::from_secs(37));
        assert_eq!(hint.urgency, UserTransactionMonitorScheduleUrgency::Blocked);
        assert_eq!(
            hint.reason,
            UserTransactionMonitorScheduleReason::RateLimited
        );
    }

    #[test]
    fn schedule_hint_retries_unfinished_work_on_minimum_interval() {
        let hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source: TriggerSource::Schedule,
                has_unfinished_work: true,
                is_idle: false,
                blocked_for: None,
            },
        );

        assert_eq!(
            hint.interval,
            USER_TRANSACTION_MONITOR_MIN_INTERVAL.as_duration()
        );
        assert_eq!(hint.urgency, UserTransactionMonitorScheduleUrgency::High);
        assert_eq!(
            hint.reason,
            UserTransactionMonitorScheduleReason::UnfinishedWork
        );
    }

    #[test]
    fn schedule_hint_backs_off_idle_scheduled_users() {
        let hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source: TriggerSource::Schedule,
                has_unfinished_work: false,
                is_idle: true,
                blocked_for: None,
            },
        );

        assert_eq!(
            hint.interval,
            USER_TRANSACTION_MONITOR_IDLE_INTERVAL.as_duration()
        );
        assert_eq!(hint.urgency, UserTransactionMonitorScheduleUrgency::Low);
        assert_eq!(hint.reason, UserTransactionMonitorScheduleReason::Idle);
    }

    #[test]
    fn schedule_hint_keeps_manual_idle_runs_on_default_cadence() {
        let hint = compute_user_transaction_monitor_schedule_hint(
            UserTransactionMonitorSchedulePolicyInput {
                source: TriggerSource::ManualInternal,
                has_unfinished_work: false,
                is_idle: true,
                blocked_for: None,
            },
        );

        assert_eq!(hint, default_user_transaction_monitor_schedule_hint());
    }

    #[test]
    fn schedule_hint_computes_next_due_timestamp() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 13, 12, 0, 0)
            .single()
            .expect("test time should be valid");
        let hint = UserTransactionMonitorScheduleHint {
            interval: Duration::from_secs(90),
            urgency: UserTransactionMonitorScheduleUrgency::High,
            reason: UserTransactionMonitorScheduleReason::UnfinishedWork,
        };

        let next_due = hint
            .next_due_at_utc(now)
            .expect("next due timestamp should be representable");

        assert_eq!(
            next_due,
            Utc.with_ymd_and_hms(2026, 3, 13, 12, 1, 30)
                .single()
                .expect("test time should be valid")
        );
    }

    #[test]
    fn inter_address_pacing_policy_returns_provider_specific_delays() {
        let policy = DEFAULT_INTER_ADDRESS_PACING_POLICY;

        assert_eq!(
            policy.delay_for_provider(SyncProviderId::MempoolSpace),
            Duration::from_millis(250)
        );
        assert_eq!(
            policy.delay_for_provider(SyncProviderId::Etherscan),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn balance_refresh_freshness_uses_configured_ttl() {
        let now = Utc
            .with_ymd_and_hms(2026, 5, 4, 12, 0, 0)
            .single()
            .expect("test time should be valid");

        assert!(is_balance_refresh_fresh(
            Some(now - chrono::Duration::from_std(BALANCE_REFRESH_TTL / 2).unwrap()),
            now
        ));
        assert!(!is_balance_refresh_fresh(
            Some(now - chrono::Duration::from_std(BALANCE_REFRESH_TTL).unwrap()),
            now
        ));
        assert!(!is_balance_refresh_fresh(None, now));
    }

    #[test]
    fn is_first_sync_treats_none_and_zero_tip_as_first_sync() {
        assert!(is_first_sync(None));
        assert!(is_first_sync(Some(
            ChainTipHeight::try_new(0).expect("zero tip should be valid")
        )));
    }

    #[test]
    fn is_first_sync_treats_positive_tip_as_incremental_sync() {
        assert!(!is_first_sync(Some(
            ChainTipHeight::try_new(1).expect("positive tip should be valid")
        )));
    }

    #[test]
    fn sync_http_counters_track_api_calls_and_cache_hits() {
        let counters = SyncHttpCounters::new();
        counters.total_api_calls.fetch_add(7, Ordering::Relaxed);
        counters
            .pagination_cache_hits
            .fetch_add(3, Ordering::Relaxed);

        assert_eq!(counters.total_api_calls(), 7);
        assert_eq!(counters.pagination_cache_hits(), 3);
    }
}

//! Task scheduling for background jobs.
//!
//! User transaction monitoring eligibility is based on "open user DB" status.
//! An "open user DB" means the user database has been initialized with a valid
//! `UserDbOpenMode` (encrypted with DEK, unencrypted dev, or plaintext test).
//! For encrypted databases, this implies the DEK is available in memory and
//! the database is unlocked.

pub(crate) mod automatic_sync;
mod jobs;

#[cfg(feature = "server")]
pub(crate) use jobs::sync::approximate_account_unsynced_count;
#[cfg(feature = "server")]
pub(crate) use jobs::sync::integrations::etherscan::map_etherscan_transactions;
#[cfg(feature = "server")]
pub(crate) use jobs::sync::integrations::unfinished_backfill_state;
#[cfg(feature = "server")]
pub(crate) use jobs::sync::run_manual_sync_control;

use self::automatic_sync::{
    AutomaticSyncDecision, AutomaticSyncEligibility, AutomaticSyncEnqueueReason,
    AutomaticSyncFreshnessSummary, AutomaticSyncKeepScheduledReason, AutomaticSyncSkipReason,
    automatic_sync_block_state, automatic_sync_decision, automatic_sync_eligibility,
    is_automatic_sync_suppressed, summarize_automatic_sync_freshness,
};
use self::jobs::inactive_user_cleanup::{
    INACTIVE_USER_CLEANUP_INTERVAL, InactiveUserCleanupParams,
};
use self::jobs::price_history::PriceHistoryReconciliationParams;
use self::jobs::session_cleanup::{SESSION_CLEANUP_INTERVAL, SessionCleanupParams};
#[cfg(all(test, not(bitgarth_db_unit_only)))]
use self::jobs::sync::{
    UserTransactionMonitorScheduleReason, UserTransactionMonitorScheduleUrgency,
};
use self::jobs::trace_cleanup::{TRACE_CLEANUP_INTERVAL, TraceCleanupParams};
use self::jobs::user_transaction_monitor::{
    USER_TRANSACTION_MONITOR_INTERVAL, UserTransactionMonitorScheduleHint,
};
use crate::auth::session;
use crate::channel::Channel;
use crate::db::{
    list_open_user_db_users, load_account_ids_with_pending_txs, load_account_sync_snapshots,
};
use crate::models::UserId;
use crate::transactions::{
    SyncIntegrationId, TransactionSyncEvent, TransactionSyncRunId, TransactionSyncScope,
};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Instant as StdInstant;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Duration, Instant, MissedTickBehavior};

pub(crate) use self::jobs::price_history::PriceHistoryReconciliationReason;
pub(crate) use self::jobs::raw_ingestion_executor;
pub(crate) use self::jobs::user_transaction_monitor::UserTransactionMonitorError;
pub(crate) use self::jobs::user_transaction_monitor::UserTransactionMonitorParams;

const MANAGER_CHANNEL_CAPACITY: usize = 256;
const MANAGER_LOOP_TICK: Duration = Duration::from_secs(1);
const USER_SYNC_EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum JobId {
    SessionCleanup,
    TraceCleanup,
    InactiveUserCleanup,
    UserTransactionMonitor,
    PriceHistoryReconciliation,
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobId::SessionCleanup => write!(f, "SessionCleanup"),
            JobId::TraceCleanup => write!(f, "TraceCleanup"),
            JobId::InactiveUserCleanup => write!(f, "InactiveUserCleanup"),
            JobId::UserTransactionMonitor => write!(f, "UserTransactionMonitor"),
            JobId::PriceHistoryReconciliation => write!(f, "PriceHistoryReconciliation"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum JobKey {
    App { job_id: JobId },
    User { job_id: JobId, user_id: UserId },
}

impl JobKey {
    const fn app(job_id: JobId) -> Self {
        JobKey::App { job_id }
    }

    const fn job_id(self) -> JobId {
        match self {
            JobKey::App { job_id } => job_id,
            JobKey::User { job_id, .. } => job_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriggerSource {
    Schedule,
    ManualInternal,
    AutoAdd,
    AutoUpgrade,
    AutoSessionRestore,
    AutoFreshness,
}

impl std::fmt::Display for TriggerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerSource::Schedule => write!(f, "Schedule"),
            TriggerSource::ManualInternal => write!(f, "ManualInternal"),
            TriggerSource::AutoAdd => write!(f, "AutoAdd"),
            TriggerSource::AutoUpgrade => write!(f, "AutoUpgrade"),
            TriggerSource::AutoSessionRestore => write!(f, "AutoSessionRestore"),
            TriggerSource::AutoFreshness => write!(f, "AutoFreshness"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriggerParams {
    SessionCleanup(SessionCleanupParams),
    TraceCleanup(TraceCleanupParams),
    InactiveUserCleanup(InactiveUserCleanupParams),
    UserTransactionMonitor(UserTransactionMonitorParams),
    PriceHistoryReconciliation(PriceHistoryReconciliationParams),
}

impl TriggerParams {
    fn for_scheduled_job(job_id: JobId) -> Self {
        match job_id {
            JobId::SessionCleanup => TriggerParams::SessionCleanup(SessionCleanupParams),
            JobId::TraceCleanup => TriggerParams::TraceCleanup(TraceCleanupParams {
                retention: jobs::trace_cleanup::TRACE_RETENTION_HOURS,
            }),
            JobId::InactiveUserCleanup => {
                TriggerParams::InactiveUserCleanup(InactiveUserCleanupParams::default())
            }
            JobId::UserTransactionMonitor => {
                TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
                    run_id: TransactionSyncRunId::new(),
                    scope: TransactionSyncScope::User,
                })
            }
            JobId::PriceHistoryReconciliation => {
                unreachable!("price history reconciliation is not scheduled")
            }
        }
    }

    fn matches_job(self, job_id: JobId) -> bool {
        matches!(
            (job_id, self),
            (JobId::SessionCleanup, TriggerParams::SessionCleanup(_))
                | (JobId::TraceCleanup, TriggerParams::TraceCleanup(_))
                | (
                    JobId::InactiveUserCleanup,
                    TriggerParams::InactiveUserCleanup(_)
                )
                | (
                    JobId::UserTransactionMonitor,
                    TriggerParams::UserTransactionMonitor(_)
                )
                | (
                    JobId::PriceHistoryReconciliation,
                    TriggerParams::PriceHistoryReconciliation(_)
                )
        )
    }
}

fn transaction_sync_run_id_for_params(params: TriggerParams) -> Option<TransactionSyncRunId> {
    match params {
        TriggerParams::UserTransactionMonitor(params) => Some(params.run_id),
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TriggerRequest {
    pub(crate) key: JobKey,
    pub(crate) source: TriggerSource,
    pub(crate) params: TriggerParams,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriggerEnqueueResult {
    AcceptedStarted {
        run_id: Option<TransactionSyncRunId>,
    },
    AcceptedQueued {
        run_id: Option<TransactionSyncRunId>,
    },
    RejectedInvalidKey,
    RejectedShuttingDown,
}

#[derive(Debug)]
pub(crate) enum TaskStartupError {
    StartupLockPoisoned,
    ThreadSpawn(String),
    RuntimeBuild(String),
    StartupSignalReceive(String),
    AlreadyStarted,
}

impl std::fmt::Display for TaskStartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStartupError::StartupLockPoisoned => {
                write!(f, "Task startup lock was poisoned")
            }
            TaskStartupError::ThreadSpawn(err) => write!(f, "Failed to spawn task thread: {err}"),
            TaskStartupError::RuntimeBuild(err) => {
                write!(f, "Failed to build task runtime: {err}")
            }
            TaskStartupError::StartupSignalReceive(err) => {
                write!(f, "Failed to receive startup signal: {err}")
            }
            TaskStartupError::AlreadyStarted => write!(f, "Task manager was already started"),
        }
    }
}

impl std::error::Error for TaskStartupError {}

#[derive(Clone, Copy, Debug)]
struct JobDefinition {
    interval: Duration,
    scheduled: bool,
}

#[derive(Clone, Copy, Debug)]
struct JobState {
    definition: JobDefinition,
    running: bool,
    pending: Option<TriggerRequest>,
    next_due_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct TriggerTransition {
    should_start_now: bool,
    running_after: bool,
    pending_after: Option<TriggerRequest>,
    enqueue_result: TriggerEnqueueResult,
}

fn canonicalize_pending_request(request: TriggerRequest) -> TriggerRequest {
    match request.params {
        TriggerParams::UserTransactionMonitor(params) => TriggerRequest {
            params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
                run_id: params.run_id,
                scope: TransactionSyncScope::User,
            }),
            ..request
        },
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => request,
    }
}

fn compute_trigger_transition(
    running: bool,
    pending: Option<TriggerRequest>,
    request: TriggerRequest,
) -> TriggerTransition {
    if running {
        let pending_request = canonicalize_pending_request(pending.unwrap_or(request));
        return TriggerTransition {
            should_start_now: false,
            running_after: true,
            pending_after: Some(pending_request),
            enqueue_result: TriggerEnqueueResult::AcceptedQueued {
                run_id: transaction_sync_run_id_for_params(pending_request.params),
            },
        };
    }

    TriggerTransition {
        should_start_now: true,
        running_after: true,
        pending_after: None,
        enqueue_result: TriggerEnqueueResult::AcceptedStarted {
            run_id: transaction_sync_run_id_for_params(request.params),
        },
    }
}

enum ManagerCommand {
    Enqueue {
        request: TriggerRequest,
        result_tx: oneshot::Sender<TriggerEnqueueResult>,
    },
    JobCompleted(JobCompletion),
}

#[derive(Clone, Debug)]
struct JobExecutionSuccess {
    summary: String,
    next_due_hint: Option<UserTransactionMonitorScheduleHint>,
}

#[derive(Clone, Debug)]
struct JobCompletion {
    request: TriggerRequest,
    result: Result<JobExecutionSuccess, String>,
    elapsed: Duration,
}

struct TaskManager {
    command_tx: mpsc::Sender<ManagerCommand>,
    command_rx: mpsc::Receiver<ManagerCommand>,
    jobs: HashMap<JobKey, JobState>,
}

fn users_eligible_for_transaction_monitoring(
    open_user_ids: HashSet<UserId>,
    logged_in_user_ids: HashSet<UserId>,
) -> HashSet<UserId> {
    open_user_ids
        .union(&logged_in_user_ids)
        .copied()
        .filter(|user_id| {
            matches!(
                automatic_sync_eligibility(
                    open_user_ids.contains(user_id),
                    logged_in_user_ids.contains(user_id)
                ),
                AutomaticSyncEligibility::Eligible
            )
        })
        .collect()
}

fn automatic_sync_freshness_summary(
    user_id: UserId,
    now_utc: DateTime<Utc>,
) -> Result<AutomaticSyncFreshnessSummary, String> {
    let account_snapshots = load_account_sync_snapshots(user_id)
        .map_err(|err| format!("load_account_sync_snapshots failed: {err}"))?;
    let pending_account_ids = load_account_ids_with_pending_txs(user_id)
        .map_err(|err| format!("load_account_ids_with_pending_txs failed: {err}"))?;

    Ok(summarize_automatic_sync_freshness(
        now_utc,
        &account_snapshots,
        &pending_account_ids,
    ))
}

fn automatic_sync_request(user_id: UserId, source: TriggerSource) -> TriggerRequest {
    TriggerRequest {
        key: JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        },
        source,
        params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id: TransactionSyncRunId::new(),
            scope: TransactionSyncScope::User,
        }),
    }
}

fn price_history_request(
    user_id: UserId,
    source: TriggerSource,
    reason: PriceHistoryReconciliationReason,
) -> TriggerRequest {
    TriggerRequest {
        key: JobKey::User {
            job_id: JobId::PriceHistoryReconciliation,
            user_id,
        },
        source,
        params: TriggerParams::PriceHistoryReconciliation(PriceHistoryReconciliationParams {
            reason,
        }),
    }
}

fn blocked_integrations_for_summary(
    user_id: UserId,
    summary: &AutomaticSyncFreshnessSummary,
    now: Instant,
) -> HashSet<SyncIntegrationId> {
    if summary.stale_integrations().is_empty() {
        return HashSet::new();
    }

    self::jobs::sync::blocked_integrations_for_user(
        user_id,
        now.into(),
        summary.stale_integrations(),
    )
}

fn automatic_sync_decision_for_user(
    user_id: UserId,
    now: Instant,
    now_utc: DateTime<Utc>,
) -> (AutomaticSyncFreshnessSummary, AutomaticSyncDecision) {
    if is_automatic_sync_suppressed(crate::sync_control::sync_control_mode()) {
        return (
            AutomaticSyncFreshnessSummary::empty(),
            AutomaticSyncDecision::Skip {
                reason: AutomaticSyncSkipReason::DisabledBySyncControl,
            },
        );
    }

    let freshness = match automatic_sync_freshness_summary(user_id, now_utc) {
        Ok(summary) => summary,
        Err(err) => {
            tracing::warn!(
                user_id = %user_id,
                error = %err,
                "tasks: failed to inspect automatic sync freshness"
            );
            return (
                AutomaticSyncFreshnessSummary::empty(),
                AutomaticSyncDecision::KeepScheduled {
                    reason: AutomaticSyncKeepScheduledReason::InspectionFailed,
                    next_due_in: USER_TRANSACTION_MONITOR_INTERVAL.as_duration(),
                },
            );
        }
    };

    let blocked_integrations = blocked_integrations_for_summary(user_id, &freshness, now);
    let blocked_retry_in = self::jobs::sync::earliest_rate_limit_unblock_for_integrations_public(
        user_id,
        now.into(),
        freshness.stale_integrations(),
    )
    .map(|blocked_until| blocked_until.saturating_duration_since(now.into()));
    let block_state =
        automatic_sync_block_state(&freshness, &blocked_integrations, blocked_retry_in);
    let decision =
        automatic_sync_decision(AutomaticSyncEligibility::Eligible, &freshness, block_state);

    (freshness, decision)
}

fn log_automatic_sync_decision(
    user_id: UserId,
    source: TriggerSource,
    freshness: &AutomaticSyncFreshnessSummary,
    decision: AutomaticSyncDecision,
) {
    match decision {
        AutomaticSyncDecision::EnqueueNow { reason } => {
            tracing::info!(
                user_id = %user_id,
                source = %source,
                reason = ?reason,
                urgent_accounts = freshness.urgent_accounts,
                stale_warm_accounts = freshness.stale_warm_accounts,
                stale_cold_accounts = freshness.stale_cold_accounts,
                "tasks: automatic sync enqueued"
            );
        }
        AutomaticSyncDecision::KeepScheduled {
            reason,
            next_due_in,
        } => {
            tracing::debug!(
                user_id = %user_id,
                source = %source,
                reason = ?reason,
                next_due_in_seconds = next_due_in.as_secs(),
                urgent_accounts = freshness.urgent_accounts,
                stale_warm_accounts = freshness.stale_warm_accounts,
                stale_cold_accounts = freshness.stale_cold_accounts,
                fresh_accounts = freshness.fresh_accounts,
                "tasks: automatic sync kept scheduled"
            );
        }
        AutomaticSyncDecision::Skip { reason } => {
            tracing::debug!(
                user_id = %user_id,
                source = %source,
                reason = ?reason,
                "tasks: automatic sync skipped"
            );
        }
    }
}

fn register_scheduled_app_job(
    jobs: &mut HashMap<JobKey, JobState>,
    job_id: JobId,
    interval: Duration,
    now: Instant,
    now_utc: DateTime<Utc>,
) {
    let key = JobKey::app(job_id);
    jobs.insert(
        key,
        JobState {
            definition: JobDefinition {
                interval,
                scheduled: true,
            },
            running: false,
            pending: None,
            next_due_at: now + interval,
        },
    );
    log_job_registration(key, interval, now_utc);
}

fn register_static_app_jobs(
    jobs: &mut HashMap<JobKey, JobState>,
    channel: Channel,
    now: Instant,
    now_utc: DateTime<Utc>,
) {
    register_scheduled_app_job(
        jobs,
        JobId::SessionCleanup,
        SESSION_CLEANUP_INTERVAL.as_duration(),
        now,
        now_utc,
    );
    register_scheduled_app_job(
        jobs,
        JobId::TraceCleanup,
        TRACE_CLEANUP_INTERVAL.as_duration(),
        now,
        now_utc,
    );

    if channel == Channel::Hosted {
        register_scheduled_app_job(
            jobs,
            JobId::InactiveUserCleanup,
            INACTIVE_USER_CLEANUP_INTERVAL.as_duration(),
            now,
            now_utc,
        );
    }
}

impl TaskManager {
    fn new(
        command_tx: mpsc::Sender<ManagerCommand>,
        command_rx: mpsc::Receiver<ManagerCommand>,
    ) -> Self {
        let now = Instant::now();
        let now_utc = Utc::now();
        let mut jobs = HashMap::new();
        register_static_app_jobs(&mut jobs, crate::channel::channel(), now, now_utc);

        Self {
            command_tx,
            command_rx,
            jobs,
        }
    }

    async fn run(mut self) {
        tracing::info!("tasks: task manager loop started");
        let mut ticker = tokio::time::interval(MANAGER_LOOP_TICK);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.handle_schedule_tick();
                }
                maybe_command = self.command_rx.recv() => {
                    let Some(command) = maybe_command else {
                        tracing::warn!("tasks: manager command channel closed, task manager loop stopping");
                        break;
                    };
                    self.handle_command(command);
                }
            }
        }
    }

    fn insert_user_transaction_job_if_missing(
        &mut self,
        user_id: UserId,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) -> bool {
        let key = JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        };
        if self.jobs.contains_key(&key) {
            return false;
        }

        let interval = USER_TRANSACTION_MONITOR_INTERVAL.as_duration();
        self.jobs.insert(
            key,
            JobState {
                definition: JobDefinition {
                    interval,
                    scheduled: true,
                },
                running: false,
                pending: None,
                next_due_at: now + interval,
            },
        );
        log_job_registration(key, interval, now_utc);
        true
    }

    fn insert_price_history_reconciliation_job_if_missing(
        &mut self,
        user_id: UserId,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) -> bool {
        let key = JobKey::User {
            job_id: JobId::PriceHistoryReconciliation,
            user_id,
        };
        if self.jobs.contains_key(&key) {
            return false;
        }

        let interval = USER_TRANSACTION_MONITOR_INTERVAL.as_duration();
        self.jobs.insert(
            key,
            JobState {
                definition: JobDefinition {
                    interval,
                    scheduled: false,
                },
                running: false,
                pending: None,
                next_due_at: now + interval,
            },
        );
        log_job_registration(key, interval, now_utc);
        true
    }

    fn sync_user_transaction_jobs(
        &mut self,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) -> Vec<TriggerRequest> {
        let open_user_ids: HashSet<UserId> = match list_open_user_db_users() {
            Ok(users) => users.into_iter().collect(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "tasks: failed to list open user databases for transaction monitor scheduling"
                );
                HashSet::new()
            }
        };

        let logged_in_user_ids: HashSet<UserId> =
            match session::list_users_with_unexpired_sessions_at(now_utc) {
                Ok(users) => users.into_iter().collect(),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "tasks: failed to list users with unexpired sessions for transaction monitor scheduling"
                    );
                    HashSet::new()
                }
            };

        let eligible_users =
            users_eligible_for_transaction_monitoring(open_user_ids, logged_in_user_ids);
        let mut automatic_requests = Vec::new();

        for user_id in &eligible_users {
            let inserted = self.insert_user_transaction_job_if_missing(*user_id, now, now_utc);
            if !inserted {
                continue;
            }

            let key = JobKey::User {
                job_id: JobId::UserTransactionMonitor,
                user_id: *user_id,
            };
            let (freshness, decision) = automatic_sync_decision_for_user(*user_id, now, now_utc);
            match decision {
                AutomaticSyncDecision::EnqueueNow {
                    reason:
                        AutomaticSyncEnqueueReason::UrgentWork | AutomaticSyncEnqueueReason::StaleWork,
                } => {
                    automatic_requests.push(automatic_sync_request(
                        *user_id,
                        TriggerSource::AutoSessionRestore,
                    ));
                }
                AutomaticSyncDecision::KeepScheduled {
                    reason:
                        AutomaticSyncKeepScheduledReason::FreshUntilNextWindow
                        | AutomaticSyncKeepScheduledReason::BlockedUntilRetry
                        | AutomaticSyncKeepScheduledReason::InspectionFailed,
                    next_due_in,
                } => {
                    if let Some(state) = self.jobs.get_mut(&key) {
                        state.next_due_at = now + next_due_in;
                    }
                }
                AutomaticSyncDecision::Skip { .. } => {}
            }
            log_automatic_sync_decision(
                *user_id,
                TriggerSource::AutoSessionRestore,
                &freshness,
                decision,
            );
        }

        self.jobs.retain(|key, state| match key {
            JobKey::User {
                job_id: JobId::UserTransactionMonitor,
                user_id,
            } => eligible_users.contains(user_id) || state.running || state.pending.is_some(),
            _ => true,
        });
        automatic_requests
    }

    fn ensure_dynamic_job_exists(&mut self, key: JobKey) -> bool {
        if self.jobs.contains_key(&key) {
            return true;
        }

        let now = Instant::now();
        let now_utc = Utc::now();
        match key {
            JobKey::User {
                job_id: JobId::UserTransactionMonitor,
                user_id,
            } => {
                self.insert_user_transaction_job_if_missing(user_id, now, now_utc);
                self.jobs.contains_key(&key)
            }
            JobKey::User {
                job_id: JobId::PriceHistoryReconciliation,
                user_id,
            } => {
                self.insert_price_history_reconciliation_job_if_missing(user_id, now, now_utc);
                self.jobs.contains_key(&key)
            }
            _ => false,
        }
    }

    fn handle_schedule_tick(&mut self) {
        let now = Instant::now();
        let now_utc = Utc::now();
        let automatic_requests = self.sync_user_transaction_jobs(now, now_utc);
        for request in automatic_requests {
            let _ = self.apply_trigger(request);
        }
        let mut due_requests = Vec::new();
        let due_keys = self
            .jobs
            .iter()
            .filter_map(|(key, state)| {
                (state.definition.scheduled && now >= state.next_due_at).then_some(*key)
            })
            .collect::<Vec<_>>();

        for key in due_keys {
            let Some(state) = self.jobs.get_mut(&key) else {
                continue;
            };

            state.next_due_at = now + state.definition.interval;

            match key {
                JobKey::User {
                    job_id: JobId::UserTransactionMonitor,
                    user_id,
                } => {
                    let (freshness, decision) =
                        automatic_sync_decision_for_user(user_id, now, now_utc);
                    match decision {
                        AutomaticSyncDecision::EnqueueNow {
                            reason:
                                AutomaticSyncEnqueueReason::UrgentWork
                                | AutomaticSyncEnqueueReason::StaleWork,
                        } => {
                            due_requests.push(automatic_sync_request(
                                user_id,
                                TriggerSource::AutoFreshness,
                            ));
                        }
                        AutomaticSyncDecision::KeepScheduled {
                            reason:
                                AutomaticSyncKeepScheduledReason::FreshUntilNextWindow
                                | AutomaticSyncKeepScheduledReason::BlockedUntilRetry
                                | AutomaticSyncKeepScheduledReason::InspectionFailed,
                            next_due_in,
                        } => {
                            state.next_due_at = now + next_due_in;
                        }
                        AutomaticSyncDecision::Skip { .. } => {}
                    }
                    log_automatic_sync_decision(
                        user_id,
                        TriggerSource::AutoFreshness,
                        &freshness,
                        decision,
                    );
                }
                JobKey::App { .. } | JobKey::User { .. } => {
                    due_requests.push(TriggerRequest {
                        key,
                        source: TriggerSource::Schedule,
                        params: TriggerParams::for_scheduled_job(key.job_id()),
                    });
                }
            }
        }

        for request in due_requests {
            let _ = self.apply_trigger(request);
        }
    }

    fn handle_command(&mut self, command: ManagerCommand) {
        match command {
            ManagerCommand::Enqueue { request, result_tx } => {
                let result = self.apply_trigger(request);
                let _ = result_tx.send(result);
            }
            ManagerCommand::JobCompleted(completion) => {
                self.handle_job_completion(completion);
            }
        }
    }

    fn apply_trigger(&mut self, request: TriggerRequest) -> TriggerEnqueueResult {
        if !self.ensure_dynamic_job_exists(request.key) {
            return TriggerEnqueueResult::RejectedInvalidKey;
        }

        let Some(state) = self.jobs.get_mut(&request.key) else {
            return TriggerEnqueueResult::RejectedInvalidKey;
        };

        if !request.params.matches_job(request.key.job_id()) {
            return TriggerEnqueueResult::RejectedInvalidKey;
        }

        let was_running = state.running;
        let had_pending = state.pending.is_some();
        let transition = compute_trigger_transition(state.running, state.pending, request);
        state.running = transition.running_after;
        state.pending = transition.pending_after;

        if was_running
            && let TriggerEnqueueResult::AcceptedQueued {
                run_id: Some(run_id),
            } = transition.enqueue_result
        {
            tracing::debug!(
                key = ?request.key,
                run_id = %run_id,
                pending_action = if had_pending { "coalesced" } else { "created" },
                queued_scope = ?state.pending.and_then(|pending| match pending.params {
                    TriggerParams::UserTransactionMonitor(params) => Some(params.scope),
                    TriggerParams::SessionCleanup(_)
                    | TriggerParams::TraceCleanup(_)
                    | TriggerParams::InactiveUserCleanup(_)
                    | TriggerParams::PriceHistoryReconciliation(_) => None,
                }),
                "tasks: trigger queued into canonical pending rerun"
            );
        }

        if transition.should_start_now {
            self.start_job_run(request);
        }

        transition.enqueue_result
    }

    fn handle_job_completion(&mut self, completion: JobCompletion) {
        let Some(state) = self.jobs.get_mut(&completion.request.key) else {
            log_unknown_job_completion_key(completion.request.key);
            return;
        };

        state.running = false;

        match completion.result {
            Ok(success) => {
                apply_completion_schedule_hint(
                    state,
                    completion.request.key,
                    success.next_due_hint,
                    Instant::now(),
                    Utc::now(),
                );
                log_job_completed(completion.request, &success.summary, completion.elapsed);
            }
            Err(error) => {
                log_job_failed(completion.request, &error, completion.elapsed);
            }
        }

        if let Some(pending) = state.pending.take() {
            state.running = true;
            self.start_job_run(pending);
        }
    }

    fn start_job_run(&self, request: TriggerRequest) {
        let command_tx = self.command_tx.clone();
        tokio::spawn(async move {
            run_job_and_report_completion(request, command_tx).await;
        });
    }
}

fn apply_completion_schedule_hint(
    state: &mut JobState,
    key: JobKey,
    next_due_hint: Option<UserTransactionMonitorScheduleHint>,
    now: Instant,
    now_utc: DateTime<Utc>,
) {
    let Some(next_due_hint) = next_due_hint else {
        return;
    };

    state.next_due_at = now + next_due_hint.interval;
    log_job_rescheduled(key, next_due_hint, now_utc);
}

fn execute_job(request: TriggerRequest) -> Result<JobExecutionSuccess, String> {
    match (request.key, request.params) {
        (
            JobKey::App {
                job_id: JobId::SessionCleanup,
            },
            TriggerParams::SessionCleanup(params),
        ) => jobs::session_cleanup::run(params)
            .map(|summary| JobExecutionSuccess {
                summary: format!("{summary:?}"),
                next_due_hint: None,
            })
            .map_err(|err| err.to_string()),
        (
            JobKey::App {
                job_id: JobId::TraceCleanup,
            },
            TriggerParams::TraceCleanup(params),
        ) => jobs::trace_cleanup::run(params)
            .map(|summary| JobExecutionSuccess {
                summary: format!("{summary:?}"),
                next_due_hint: None,
            })
            .map_err(|err| err.to_string()),
        (
            JobKey::App {
                job_id: JobId::InactiveUserCleanup,
            },
            TriggerParams::InactiveUserCleanup(params),
        ) => jobs::inactive_user_cleanup::run(params)
            .map(|summary| JobExecutionSuccess {
                summary: format!("{summary:?}"),
                next_due_hint: None,
            })
            .map_err(|err| err.to_string()),
        (
            JobKey::User {
                job_id: JobId::UserTransactionMonitor,
                user_id,
            },
            TriggerParams::UserTransactionMonitor(params),
        ) => jobs::user_transaction_monitor::run(user_id, request.source, params)
            .map(|summary| JobExecutionSuccess {
                summary: format!("{summary:?}"),
                next_due_hint: Some(summary.schedule_hint),
            })
            .map_err(|err| err.to_string()),
        (
            JobKey::User {
                job_id: JobId::PriceHistoryReconciliation,
                user_id,
            },
            TriggerParams::PriceHistoryReconciliation(params),
        ) => {
            jobs::price_history::run_price_history_reconciliation(user_id, params).map(|summary| {
                JobExecutionSuccess {
                    summary,
                    next_due_hint: None,
                }
            })
        }
        (key, _) => Err(format!("Mismatched trigger params for job key {key:?}")),
    }
}

async fn run_job_and_report_completion(
    request: TriggerRequest,
    command_tx: mpsc::Sender<ManagerCommand>,
) {
    log_job_started(request);

    let started_at = StdInstant::now();
    let result = tokio::task::spawn_blocking(move || execute_job(request))
        .await
        .map_err(|err| format!("Job join failure: {err}"))
        .and_then(|inner| inner);
    let elapsed = started_at.elapsed();

    let completion = JobCompletion {
        request,
        result,
        elapsed,
    };
    let _ = command_tx
        .send(ManagerCommand::JobCompleted(completion))
        .await;
}

fn next_run_at_utc(now_utc: DateTime<Utc>, interval: Duration) -> Option<DateTime<Utc>> {
    chrono::Duration::from_std(interval)
        .ok()
        .and_then(|delta| now_utc.checked_add_signed(delta))
}

fn log_job_registration(key: JobKey, interval: Duration, now_utc: DateTime<Utc>) {
    let next_run_at_utc = next_run_at_utc(now_utc, interval);
    match key {
        JobKey::App { job_id } => tracing::info!(
            job_id = %job_id,
            interval_seconds = interval.as_secs(),
            next_run_at_utc = ?next_run_at_utc,
            "tasks: registered background job"
        ),
        JobKey::User { job_id, user_id } => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            interval_seconds = interval.as_secs(),
            next_run_at_utc = ?next_run_at_utc,
            "tasks: registered background job"
        ),
    }
}

fn log_job_rescheduled(
    key: JobKey,
    next_due_hint: UserTransactionMonitorScheduleHint,
    now_utc: DateTime<Utc>,
) {
    let next_run_at_utc = next_due_hint.next_due_at_utc(now_utc);
    match key {
        JobKey::App { job_id } => tracing::info!(
            job_id = %job_id,
            next_due_in_seconds = next_due_hint.interval.as_secs(),
            next_due_reason = ?next_due_hint.reason,
            schedule_urgency = ?next_due_hint.urgency,
            next_run_at_utc = ?next_run_at_utc,
            "tasks: background job rescheduled"
        ),
        JobKey::User { job_id, user_id } => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            next_due_in_seconds = next_due_hint.interval.as_secs(),
            next_due_reason = ?next_due_hint.reason,
            schedule_urgency = ?next_due_hint.urgency,
            next_run_at_utc = ?next_run_at_utc,
            "tasks: background job rescheduled"
        ),
    }
}

fn log_job_started(request: TriggerRequest) {
    let run_id = transaction_sync_run_id_for_params(request.params);
    match (request.key, run_id) {
        (JobKey::App { job_id }, None) => tracing::info!(
            job_id = %job_id,
            source = %request.source,
            "tasks: job started"
        ),
        (JobKey::User { job_id, user_id }, None) => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            "tasks: job started"
        ),
        (JobKey::App { job_id }, Some(run_id)) => tracing::info!(
            job_id = %job_id,
            source = %request.source,
            run_id = %run_id,
            "tasks: job started"
        ),
        (JobKey::User { job_id, user_id }, Some(run_id)) => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            run_id = %run_id,
            "tasks: job started"
        ),
    }
}

fn log_job_completed(request: TriggerRequest, summary: &str, elapsed: Duration) {
    let run_id = transaction_sync_run_id_for_params(request.params);
    match (request.key, run_id) {
        (JobKey::App { job_id }, None) => tracing::info!(
            job_id = %job_id,
            source = %request.source,
            summary = %summary,
            duration_ms = elapsed.as_millis(),
            "tasks: job completed"
        ),
        (JobKey::User { job_id, user_id }, None) => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            summary = %summary,
            duration_ms = elapsed.as_millis(),
            "tasks: job completed"
        ),
        (JobKey::App { job_id }, Some(run_id)) => tracing::info!(
            job_id = %job_id,
            source = %request.source,
            run_id = %run_id,
            summary = %summary,
            duration_ms = elapsed.as_millis(),
            "tasks: job completed"
        ),
        (JobKey::User { job_id, user_id }, Some(run_id)) => tracing::info!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            run_id = %run_id,
            summary = %summary,
            duration_ms = elapsed.as_millis(),
            "tasks: job completed"
        ),
    }
}

fn log_job_failed(request: TriggerRequest, error: &str, elapsed: Duration) {
    let run_id = transaction_sync_run_id_for_params(request.params);
    match (request.key, run_id) {
        (JobKey::App { job_id }, None) => tracing::error!(
            job_id = %job_id,
            source = %request.source,
            error = %error,
            duration_ms = elapsed.as_millis(),
            "tasks: job failed"
        ),
        (JobKey::User { job_id, user_id }, None) => tracing::error!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            error = %error,
            duration_ms = elapsed.as_millis(),
            "tasks: job failed"
        ),
        (JobKey::App { job_id }, Some(run_id)) => tracing::error!(
            job_id = %job_id,
            source = %request.source,
            run_id = %run_id,
            error = %error,
            duration_ms = elapsed.as_millis(),
            "tasks: job failed"
        ),
        (JobKey::User { job_id, user_id }, Some(run_id)) => tracing::error!(
            job_id = %job_id,
            user_id = %user_id,
            source = %request.source,
            run_id = %run_id,
            error = %error,
            duration_ms = elapsed.as_millis(),
            "tasks: job failed"
        ),
    }
}

fn log_unknown_job_completion_key(key: JobKey) {
    match key {
        JobKey::App { job_id } => tracing::warn!(
            job_id = %job_id,
            "tasks: job completion received for unknown key"
        ),
        JobKey::User { job_id, user_id } => tracing::warn!(
            job_id = %job_id,
            user_id = %user_id,
            "tasks: job completion received for unknown key"
        ),
    }
}

fn user_sync_event_senders()
-> &'static RwLock<HashMap<UserId, broadcast::Sender<TransactionSyncEvent>>> {
    static SENDERS: OnceLock<RwLock<HashMap<UserId, broadcast::Sender<TransactionSyncEvent>>>> =
        OnceLock::new();
    SENDERS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn user_sync_event_sender(user_id: UserId) -> Option<broadcast::Sender<TransactionSyncEvent>> {
    let senders = user_sync_event_senders();
    let mut write_guard = match senders.write() {
        Ok(guard) => guard,
        Err(_) => {
            tracing::error!(
                user_id = %user_id,
                "tasks: user sync event sender lock poisoned"
            );
            return None;
        }
    };

    let sender = write_guard.entry(user_id).or_insert_with(|| {
        let (sender, _) = broadcast::channel(USER_SYNC_EVENT_CHANNEL_CAPACITY);
        sender
    });
    Some(sender.clone())
}

fn should_warn_transaction_sync_publish_send_failure(active_receiver_count: usize) -> bool {
    active_receiver_count > 0
}

pub(crate) fn publish_transaction_sync_event(user_id: UserId, event: TransactionSyncEvent) {
    let event_name = event.event_name();
    let run_id = event.run_id;
    let run_id_display = run_id.map(|id| id.to_string()).unwrap_or_default();
    let Some(sender) = user_sync_event_sender(user_id) else {
        tracing::error!(
            user_id = %user_id,
            event = event_name,
            run_id = %run_id_display,
            "tasks: failed to publish transaction sync event (sender unavailable)"
        );
        return;
    };

    let active_receiver_count = sender.receiver_count();
    if !should_warn_transaction_sync_publish_send_failure(active_receiver_count) {
        tracing::debug!(
            user_id = %user_id,
            event = event_name,
            run_id = %run_id_display,
            "tasks: skipped transaction sync event publish because no receivers are subscribed"
        );
        return;
    }

    match sender.send(event) {
        Ok(receiver_count) => {
            tracing::debug!(
                user_id = %user_id,
                event = event_name,
                run_id = %run_id_display,
                receiver_count,
                "tasks: published transaction sync event"
            );
        }
        Err(err) => {
            let active_receiver_count = sender.receiver_count();
            if should_warn_transaction_sync_publish_send_failure(active_receiver_count) {
                tracing::warn!(
                    user_id = %user_id,
                    event = event_name,
                    run_id = %run_id_display,
                    active_receiver_count,
                    error = %err,
                    "tasks: failed to publish transaction sync event"
                );
            } else {
                tracing::debug!(
                    user_id = %user_id,
                    event = event_name,
                    run_id = %run_id_display,
                    error = %err,
                    "tasks: skipped transaction sync event publish because receivers disconnected"
                );
            }
        }
    }
}

#[cfg(all(feature = "server", any(test, not(feature = "desktop"))))]
pub(crate) fn subscribe_transaction_sync_events(
    user_id: UserId,
) -> Result<broadcast::Receiver<TransactionSyncEvent>, String> {
    user_sync_event_sender(user_id)
        .map(|sender| sender.subscribe())
        .ok_or_else(|| "unable to subscribe to transaction sync events".to_string())
}

static STARTUP_LOCK: Mutex<()> = Mutex::new(());
static TASK_MANAGER_COMMAND_TX: OnceLock<mpsc::Sender<ManagerCommand>> = OnceLock::new();
static TASK_MANAGER_THREAD_HANDLE: OnceLock<thread::JoinHandle<()>> = OnceLock::new();

fn spawn_task_manager_thread(
    manager: TaskManager,
) -> Result<thread::JoinHandle<()>, TaskStartupError> {
    let (startup_tx, startup_rx) = std::sync::mpsc::channel();
    let thread_handle = thread::Builder::new()
        .name("bitgarth-task-manager".to_string())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => {
                    let _ = startup_tx.send(Ok::<(), TaskStartupError>(()));
                    runtime.block_on(manager.run());
                }
                Err(err) => {
                    let _ = startup_tx.send(Err(TaskStartupError::RuntimeBuild(err.to_string())));
                }
            }
        })
        .map_err(|err| TaskStartupError::ThreadSpawn(err.to_string()))?;

    match startup_rx.recv() {
        Ok(Ok(())) => Ok(thread_handle),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(TaskStartupError::StartupSignalReceive(err.to_string())),
    }
}

pub(crate) fn ensure_started() -> Result<(), TaskStartupError> {
    if TASK_MANAGER_COMMAND_TX.get().is_some() {
        return Ok(());
    }

    let _startup_guard = STARTUP_LOCK
        .lock()
        .map_err(|_| TaskStartupError::StartupLockPoisoned)?;
    if TASK_MANAGER_COMMAND_TX.get().is_some() {
        return Ok(());
    }

    let (command_tx, command_rx) = mpsc::channel(MANAGER_CHANNEL_CAPACITY);
    let manager = TaskManager::new(command_tx.clone(), command_rx);

    #[cfg(not(test))]
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(manager.run());
    } else {
        let thread_handle = spawn_task_manager_thread(manager)?;
        let _ = TASK_MANAGER_THREAD_HANDLE.set(thread_handle);
    }

    #[cfg(test)]
    {
        // Current-thread test runtimes are dropped between tests. Keep the task manager on its
        // own runtime thread in tests so the global sender stays usable for the full suite.
        let thread_handle = spawn_task_manager_thread(manager)?;
        let _ = TASK_MANAGER_THREAD_HANDLE.set(thread_handle);
    }

    TASK_MANAGER_COMMAND_TX
        .set(command_tx)
        .map_err(|_| TaskStartupError::AlreadyStarted)?;
    tracing::info!("tasks: task manager started");
    Ok(())
}

pub(crate) async fn enqueue_trigger(request: TriggerRequest) -> TriggerEnqueueResult {
    let Some(command_tx) = TASK_MANAGER_COMMAND_TX.get() else {
        return TriggerEnqueueResult::RejectedShuttingDown;
    };

    let (result_tx, result_rx) = oneshot::channel();
    let send_result = command_tx
        .send(ManagerCommand::Enqueue { request, result_tx })
        .await;
    if send_result.is_err() {
        return TriggerEnqueueResult::RejectedShuttingDown;
    }

    match result_rx.await {
        Ok(result) => result,
        Err(_) => TriggerEnqueueResult::RejectedShuttingDown,
    }
}

pub(crate) async fn enqueue_price_history_reconciliation(
    user_id: UserId,
    reason: PriceHistoryReconciliationReason,
) -> TriggerEnqueueResult {
    enqueue_trigger(price_history_request(
        user_id,
        TriggerSource::ManualInternal,
        reason,
    ))
    .await
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn user_sync_key(user_id: UserId) -> JobKey {
        JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        }
    }

    fn make_user_sync_job_state(now: Instant) -> JobState {
        let interval = USER_TRANSACTION_MONITOR_INTERVAL.as_duration();
        JobState {
            definition: JobDefinition {
                interval,
                scheduled: true,
            },
            running: false,
            pending: None,
            next_due_at: now + interval,
        }
    }

    fn session_request(source: TriggerSource) -> TriggerRequest {
        TriggerRequest {
            key: JobKey::app(JobId::SessionCleanup),
            source,
            params: TriggerParams::SessionCleanup(SessionCleanupParams),
        }
    }

    fn user_sync_request(
        user_id: UserId,
        run_id: TransactionSyncRunId,
        scope: TransactionSyncScope,
    ) -> TriggerRequest {
        TriggerRequest {
            key: user_sync_key(user_id),
            source: TriggerSource::ManualInternal,
            params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
                run_id,
                scope,
            }),
        }
    }

    #[test]
    fn compute_trigger_transition_starts_when_not_running() {
        let request = session_request(TriggerSource::ManualInternal);
        let transition = compute_trigger_transition(false, None, request);

        assert!(transition.should_start_now);
        assert!(transition.running_after);
        assert!(transition.pending_after.is_none());
        assert_eq!(
            transition.enqueue_result,
            TriggerEnqueueResult::AcceptedStarted { run_id: None }
        );
    }

    #[test]
    fn compute_trigger_transition_promotes_first_queued_sync_request_to_user_scope() {
        let user_id = UserId::new();
        let queued_run_id = TransactionSyncRunId::new();
        let request = user_sync_request(
            user_id,
            queued_run_id,
            TransactionSyncScope::Account {
                account_id: crate::wallets::DigitalAssetAccountId::new(),
            },
        );
        let transition = compute_trigger_transition(true, None, request);

        assert!(!transition.should_start_now);
        assert!(transition.running_after);
        assert_eq!(
            transition.pending_after,
            Some(user_sync_request(
                user_id,
                queued_run_id,
                TransactionSyncScope::User,
            ))
        );
        assert_eq!(
            transition.enqueue_result,
            TriggerEnqueueResult::AcceptedQueued {
                run_id: Some(queued_run_id),
            }
        );
    }

    #[test]
    fn compute_trigger_transition_reuses_existing_pending_sync_run_id_when_running() {
        let user_id = UserId::new();
        let existing_run_id = TransactionSyncRunId::new();
        let existing = user_sync_request(user_id, existing_run_id, TransactionSyncScope::User);
        let newest = user_sync_request(
            user_id,
            TransactionSyncRunId::new(),
            TransactionSyncScope::Address {
                address_id: crate::wallets::DigitalAssetAddressId::new(),
            },
        );
        let transition = compute_trigger_transition(true, Some(existing), newest);

        assert!(!transition.should_start_now);
        assert!(transition.running_after);
        assert_eq!(transition.pending_after, Some(existing));
        assert_eq!(
            transition.enqueue_result,
            TriggerEnqueueResult::AcceptedQueued {
                run_id: Some(existing_run_id),
            }
        );
    }

    #[test]
    fn price_history_request_queues_when_job_is_running() {
        let user_id = UserId::new();
        let request = price_history_request(
            user_id,
            TriggerSource::ManualInternal,
            jobs::price_history::PriceHistoryReconciliationReason::Login,
        );

        let transition = compute_trigger_transition(true, None, request);

        assert!(!transition.should_start_now);
        assert!(transition.running_after);
        assert!(matches!(
            transition.enqueue_result,
            TriggerEnqueueResult::AcceptedQueued { .. }
        ));
    }

    #[tokio::test]
    async fn price_history_dynamic_job_is_not_started_by_schedule_tick() {
        crate::db::enable_test_mode();
        crate::db::reset_test_db();
        let (command_tx, command_rx) = mpsc::channel(MANAGER_CHANNEL_CAPACITY);
        let mut manager = TaskManager::new(command_tx, command_rx);
        let user_id = UserId::new();
        let now = Instant::now();
        let key = JobKey::User {
            job_id: JobId::PriceHistoryReconciliation,
            user_id,
        };
        manager.insert_price_history_reconciliation_job_if_missing(user_id, now, Utc::now());
        manager
            .jobs
            .get_mut(&key)
            .expect("price history job should be registered")
            .next_due_at = now;

        manager.handle_schedule_tick();

        let state = manager
            .jobs
            .get(&key)
            .expect("price history job should remain registered");
        assert!(!state.running);
        assert!(state.pending.is_none());
    }

    #[test]
    fn trigger_params_must_match_job_id() {
        assert!(
            TriggerParams::SessionCleanup(SessionCleanupParams).matches_job(JobId::SessionCleanup)
        );
        assert!(
            !TriggerParams::SessionCleanup(SessionCleanupParams).matches_job(JobId::TraceCleanup)
        );
        assert!(
            TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
                run_id: TransactionSyncRunId::new(),
                scope: TransactionSyncScope::User,
            })
            .matches_job(JobId::UserTransactionMonitor)
        );
    }

    #[test]
    fn static_app_jobs_include_inactive_cleanup_only_on_hosted_channel() {
        let now = Instant::now();
        let now_utc = Utc::now();
        let mut hosted_jobs = HashMap::new();
        register_static_app_jobs(&mut hosted_jobs, Channel::Hosted, now, now_utc);

        assert!(hosted_jobs.contains_key(&JobKey::app(JobId::SessionCleanup)));
        assert!(hosted_jobs.contains_key(&JobKey::app(JobId::TraceCleanup)));
        assert!(hosted_jobs.contains_key(&JobKey::app(JobId::InactiveUserCleanup)));

        let mut docker_jobs = HashMap::new();
        register_static_app_jobs(&mut docker_jobs, Channel::Docker, now, now_utc);

        assert!(docker_jobs.contains_key(&JobKey::app(JobId::SessionCleanup)));
        assert!(docker_jobs.contains_key(&JobKey::app(JobId::TraceCleanup)));
        assert!(!docker_jobs.contains_key(&JobKey::app(JobId::InactiveUserCleanup)));
    }

    #[test]
    fn inactive_cleanup_trigger_params_match_only_inactive_cleanup_job() {
        assert!(
            TriggerParams::InactiveUserCleanup(InactiveUserCleanupParams::default())
                .matches_job(JobId::InactiveUserCleanup)
        );
        assert!(
            !TriggerParams::InactiveUserCleanup(InactiveUserCleanupParams::default())
                .matches_job(JobId::SessionCleanup)
        );
    }

    #[test]
    fn job_key_hash_distinguishes_users_for_future_user_jobs() {
        let user_a = UserId::new();
        let user_b = UserId::new();
        let key_a = user_sync_key(user_a);
        let key_b = user_sync_key(user_b);

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn transaction_sync_publish_send_failure_only_warns_with_active_receivers() {
        assert!(!should_warn_transaction_sync_publish_send_failure(0));
        assert!(should_warn_transaction_sync_publish_send_failure(1));
    }

    #[test]
    fn apply_trigger_reuses_canonical_pending_sync_run_id() {
        let (command_tx, command_rx) = mpsc::channel(MANAGER_CHANNEL_CAPACITY);
        let mut manager = TaskManager::new(command_tx, command_rx);
        let now = Instant::now();
        let user_id = UserId::new();
        let key = user_sync_key(user_id);
        manager.jobs.insert(
            key,
            JobState {
                running: true,
                ..make_user_sync_job_state(now)
            },
        );

        let first_run_id = TransactionSyncRunId::new();
        let first_result = manager.apply_trigger(user_sync_request(
            user_id,
            first_run_id,
            TransactionSyncScope::Account {
                account_id: crate::wallets::DigitalAssetAccountId::new(),
            },
        ));
        let second_result = manager.apply_trigger(user_sync_request(
            user_id,
            TransactionSyncRunId::new(),
            TransactionSyncScope::Address {
                address_id: crate::wallets::DigitalAssetAddressId::new(),
            },
        ));

        assert_eq!(
            first_result,
            TriggerEnqueueResult::AcceptedQueued {
                run_id: Some(first_run_id),
            }
        );
        assert_eq!(
            second_result,
            TriggerEnqueueResult::AcceptedQueued {
                run_id: Some(first_run_id),
            }
        );
        assert_eq!(
            manager.jobs.get(&key).and_then(|state| state.pending),
            Some(user_sync_request(
                user_id,
                first_run_id,
                TransactionSyncScope::User,
            ))
        );
    }

    #[test]
    fn apply_completion_schedule_hint_shortens_next_due_for_unfinished_work() {
        let now = Instant::now();
        let mut state = make_user_sync_job_state(now);

        apply_completion_schedule_hint(
            &mut state,
            user_sync_key(UserId::new()),
            Some(UserTransactionMonitorScheduleHint {
                interval: Duration::from_secs(60),
                urgency: UserTransactionMonitorScheduleUrgency::High,
                reason: UserTransactionMonitorScheduleReason::UnfinishedWork,
            }),
            now,
            Utc::now(),
        );

        assert_eq!(state.next_due_at, now + Duration::from_secs(60));
    }

    #[test]
    fn apply_completion_schedule_hint_delays_next_due_for_idle_user() {
        let now = Instant::now();
        let mut state = make_user_sync_job_state(now);

        apply_completion_schedule_hint(
            &mut state,
            user_sync_key(UserId::new()),
            Some(UserTransactionMonitorScheduleHint {
                interval: Duration::from_secs(900),
                urgency: UserTransactionMonitorScheduleUrgency::Low,
                reason: UserTransactionMonitorScheduleReason::Idle,
            }),
            now,
            Utc::now(),
        );

        assert_eq!(state.next_due_at, now + Duration::from_secs(900));
    }

    #[test]
    fn users_eligible_for_transaction_monitoring_uses_open_and_logged_in_intersection() {
        let open_user_a = UserId::new();
        let open_user_b = UserId::new();
        let logged_in_user_b = open_user_b;
        let logged_in_user_c = UserId::new();

        let open_users = HashSet::from([open_user_a, open_user_b]);
        let logged_in_users = HashSet::from([logged_in_user_b, logged_in_user_c]);

        let eligible = users_eligible_for_transaction_monitoring(open_users, logged_in_users);
        assert_eq!(eligible, HashSet::from([open_user_b]));
    }

    #[test]
    fn display_values_are_human_readable() {
        assert_eq!(
            JobId::UserTransactionMonitor.to_string(),
            "UserTransactionMonitor"
        );
        assert_eq!(TriggerSource::ManualInternal.to_string(), "ManualInternal");
        assert_eq!(TriggerSource::AutoAdd.to_string(), "AutoAdd");
        assert_eq!(TriggerSource::AutoUpgrade.to_string(), "AutoUpgrade");
        assert_eq!(
            TriggerSource::AutoSessionRestore.to_string(),
            "AutoSessionRestore"
        );
        assert_eq!(TriggerSource::AutoFreshness.to_string(), "AutoFreshness");
    }

    #[test]
    fn transaction_sync_run_id_for_params_returns_sync_run_id() {
        let run_id = TransactionSyncRunId::new();
        let params = TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id,
            scope: TransactionSyncScope::User,
        });

        assert_eq!(transaction_sync_run_id_for_params(params), Some(run_id));
        assert_eq!(
            transaction_sync_run_id_for_params(TriggerParams::SessionCleanup(SessionCleanupParams)),
            None
        );
    }
}

use crate::tasks::publish_transaction_sync_event;
use crate::transactions::{AddressCount, TransactionSyncEvent, TransactionSyncResult};
use std::time::Duration;

mod account_events;
mod automatic;
mod chain_tip;
mod client_config;
mod context;
mod cycle;
mod error;
mod executor;
mod gate;
mod hd_scan;
pub(crate) mod integrations;
mod manual_control;
mod parent_cycle;
mod planner;
mod progress;
mod rate_limit;
#[cfg(all(test, feature = "db-tests"))]
mod test_support;

pub(crate) use self::automatic::run;
#[cfg(feature = "server")]
pub(crate) use self::manual_control::run_manual_sync_control;

pub(crate) use self::context::{
    IntegrationIterationContext, IntegrationSyncPlan, LABEL_ETHERSCAN, LABEL_MEMPOOL, RunContext,
    SingleAddressProgressPlan, SyncClients, SyncClock, SyncHttpCounters, SyncIterationResult,
    USER_TRANSACTION_MONITOR_INTERVAL, UserTransactionMonitorParams,
    UserTransactionMonitorScheduleHint, UserTransactionMonitorSummary, is_first_sync,
    raw_sync_run_trigger_kind,
};
#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(crate) use self::context::{
    UserTransactionMonitorScheduleReason, UserTransactionMonitorScheduleUrgency,
};
pub(crate) use self::error::UserTransactionMonitorError;
pub(crate) use self::progress::approximate_account_unsynced_count;
pub(crate) use self::rate_limit::blocked_integrations_for_user;
pub(crate) use self::rate_limit::earliest_rate_limit_unblock_for_integrations as earliest_rate_limit_unblock_for_integrations_public;

// Private re-exports consumed by existing sibling modules via `use super::`.
use self::chain_tip::{ChainTipCache, chain_tip_cache_key};
use self::context::{default_user_transaction_monitor_schedule_hint, to_address_count};
use self::cycle::{CycleAccumulator, CycleAccumulatorSnapshot, mark_sync_failure};
use self::executor::{AddressSyncExecutionRequest, AddressSyncExecutor};
use self::gate::{
    SyncSingleAddressControlRequest, default_api_provider_for_asset, integration_for_asset,
    sync_single_address_with_controls,
};
use self::rate_limit::{is_rate_limited, record_rate_limit};

const MEMPOOL_REQUEST_TIMEOUT_SECONDS: u64 = 10;
const ADDRESS_SYNC_COOLDOWN: Duration = Duration::from_secs(90);
const FAILED_ADDRESS_SYNC_COOLDOWN: Duration = Duration::from_secs(30);
const ETHERSCAN_ADDRESS_SYNC_COOLDOWN: Duration = Duration::from_secs(15 * 60);
const ETHERSCAN_FAILED_ADDRESS_SYNC_COOLDOWN: Duration = Duration::from_secs(15 * 60);
pub(super) const ETHERSCAN_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(15 * 60);
pub(super) const MEMPOOL_RATE_LIMIT_BACKOFF_BASE: Duration = Duration::from_secs(60);
pub(super) const MEMPOOL_RATE_LIMIT_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const CHAIN_TIP_CACHE_TTL_MIN: Duration = Duration::from_secs(30);
const MAX_ADDRESSES_PER_ACCOUNT_PER_RUN: u32 = 200;

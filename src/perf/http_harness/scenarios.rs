use super::PerfError;
use super::datasets::{
    HEAVY_SYNC_LEDGER_REBUILD_ITERATIONS, HEAVY_SYNC_OVERLAP_CONFIRMED_COUNT,
    HEAVY_SYNC_OVERLAP_PENDING_COUNT, LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
    LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT, PerfSyncWorkloadKind, PerfSyncWorkloadProfile,
    SYNC_LEDGER_REBUILD_ITERATIONS,
};
use super::measurement::PerfBudget;

pub(super) const DEFAULT_WARMUP_ITERATIONS: u32 = 2;
pub(super) const DEFAULT_MEASURED_ITERATIONS: u32 = 8;
pub(super) const DEFAULT_CONCURRENCY: u32 = 1;
pub(super) const DEFAULT_SYNC_WARMUP_ITERATIONS: u32 = 0;
pub(super) const DEFAULT_SYNC_OVERLAP_CONCURRENCY: u32 = 6;
pub(super) const HEAVY_SYNC_OVERLAP_CONCURRENCY: u32 = 8;

pub(super) const SCENARIO_WALLETS_EMPTY_READ: &str = "wallets-empty-read";
pub(super) const SCENARIO_WALLETS_LARGE_READ: &str = "wallets-large-read";
pub(super) const SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ: &str = "account-transactions-large-read";
pub(super) const SCENARIO_UTXO_TRANSACTIONS_LARGE_READ: &str = "utxo-transactions-large-read";
pub(super) const SCENARIO_SYNC_STATE_LARGE_READ: &str = "sync-state-large-read";
pub(super) const SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD: &str =
    "auth-restore-during-user-db-load";
pub(super) const SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES: &str =
    "auth-restore-during-app-db-writes";
pub(super) const SCENARIO_READS_DURING_EXPORT: &str = "reads-during-export";
pub(super) const SCENARIO_READS_DURING_SYNC: &str = "reads-during-sync";
pub(super) const SCENARIO_READS_DURING_HEAVY_SYNC: &str = "reads-during-heavy-sync";
pub(super) const SCENARIO_SETTINGS_WRITES_DURING_SYNC: &str = "settings-writes-during-sync";
pub(super) const SCENARIO_UTXO_READS_DURING_SYNC: &str = "utxo-reads-during-sync";

pub(super) const DATASET_AUTH_RESTORE_CROSS_USER: &str = "auth-restore-cross-user-sync-heavy";
pub(super) const DATASET_SYNC_MOCK_HEAVY: &str = "sync-mock-heavy";
pub(super) const DATASET_SYNC_OVERLAP_HEAVY: &str = "sync-overlap-heavy";
pub(super) const DATASET_WALLETS_MANY_ACCOUNTS: &str = "wallets-many-accounts";
pub(super) const DATASET_ACCOUNT_TRANSACTIONS_HEAVY: &str = "account-transactions-heavy";
pub(super) const DATASET_UTXO_TRANSACTIONS_HEAVY: &str = "utxo-transactions-heavy";
pub(super) const DATASET_SYNC_STATE_MANY_ADDRESSES: &str = "sync-state-many-addresses";

pub(super) const DATASET_SHAPE_AUTH_RESTORE_CROSS_USER: &str =
    "auth-restore-primary-empty-secondary-sync-heavy";
pub(super) const DATASET_SHAPE_WALLETS_MANY_ACCOUNTS: &str = "wallets-24x3";
pub(super) const DATASET_SHAPE_ACCOUNT_TRANSACTIONS_HEAVY: &str =
    "account-transactions-180-confirmed-24-pending";
pub(super) const DATASET_SHAPE_UTXO_TRANSACTIONS_HEAVY: &str =
    "bitcoin-utxo-transactions-180-confirmed-24-pending";
pub(super) const DATASET_SHAPE_SYNC_STATE_MANY_ADDRESSES: &str = "sync-state-80-addresses-mixed";
pub(super) const DATASET_SHAPE_SYNC_MOCK_HEAVY: &str =
    "sync-ledger-rebuild-account-transactions-heavy";
pub(super) const DATASET_SHAPE_SYNC_OVERLAP_HEAVY: &str =
    "sync-overlap-wallets-48x4-ledger-720-confirmed-96-pending";

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PerfScenarioDefinition {
    pub(super) id: &'static str,
    pub(super) name: &'static str,
    pub(super) endpoint_or_flow: &'static str,
    pub(super) default_dataset_id: &'static str,
    pub(super) default_dataset_shape: &'static str,
    pub(super) default_warmup_iterations: u32,
    pub(super) default_measured_iterations: u32,
    pub(super) default_concurrency: u32,
    pub(super) budget: PerfBudget,
}

pub(super) fn resolve_scenario(id: &str) -> Result<PerfScenarioDefinition, PerfError> {
    match id {
        SCENARIO_WALLETS_EMPTY_READ => Ok(PerfScenarioDefinition {
            id: SCENARIO_WALLETS_EMPTY_READ,
            name: "Wallets Empty Read",
            endpoint_or_flow: "GET /_app/user/wallets",
            default_dataset_id: "tiny-empty",
            default_dataset_shape: "tiny-empty",
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_CONCURRENCY,
            budget: PerfBudget {
                median_ms: Some(5.0),
                p95_ms: Some(10.0),
                max_ms: Some(20.0),
                max_error_count: Some(0),
                strict: false,
            },
        }),
        SCENARIO_WALLETS_LARGE_READ => Ok(PerfScenarioDefinition {
            id: SCENARIO_WALLETS_LARGE_READ,
            name: "Wallets Large Read",
            endpoint_or_flow: "GET /_app/user/wallets",
            default_dataset_id: DATASET_WALLETS_MANY_ACCOUNTS,
            default_dataset_shape: DATASET_SHAPE_WALLETS_MANY_ACCOUNTS,
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_CONCURRENCY,
            budget: PerfBudget {
                median_ms: Some(35.0),
                p95_ms: Some(40.0),
                max_ms: Some(50.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ => Ok(PerfScenarioDefinition {
            id: SCENARIO_ACCOUNT_TRANSACTIONS_LARGE_READ,
            name: "Account Transactions Large Read",
            endpoint_or_flow: "GET /_app/user/account/:account_id/transactions",
            default_dataset_id: DATASET_ACCOUNT_TRANSACTIONS_HEAVY,
            default_dataset_shape: DATASET_SHAPE_ACCOUNT_TRANSACTIONS_HEAVY,
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_CONCURRENCY,
            budget: PerfBudget {
                median_ms: Some(10.0),
                p95_ms: Some(12.0),
                max_ms: Some(18.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_UTXO_TRANSACTIONS_LARGE_READ => Ok(PerfScenarioDefinition {
            id: SCENARIO_UTXO_TRANSACTIONS_LARGE_READ,
            name: "UTXO Transactions Large Read",
            endpoint_or_flow: "GET /_app/user/account/:account_id/transactions",
            default_dataset_id: DATASET_UTXO_TRANSACTIONS_HEAVY,
            default_dataset_shape: DATASET_SHAPE_UTXO_TRANSACTIONS_HEAVY,
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_CONCURRENCY,
            budget: PerfBudget {
                median_ms: Some(13.0),
                p95_ms: Some(17.0),
                max_ms: Some(20.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_SYNC_STATE_LARGE_READ => Ok(PerfScenarioDefinition {
            id: SCENARIO_SYNC_STATE_LARGE_READ,
            name: "Sync State Large Read",
            endpoint_or_flow: "GET /_app/user/transactions/sync/state",
            default_dataset_id: DATASET_SYNC_STATE_MANY_ADDRESSES,
            default_dataset_shape: DATASET_SHAPE_SYNC_STATE_MANY_ADDRESSES,
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_CONCURRENCY,
            budget: PerfBudget {
                median_ms: Some(7.0),
                p95_ms: Some(10.0),
                max_ms: Some(15.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD => Ok(PerfScenarioDefinition {
            id: SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD,
            name: "Auth Restore During Other User DB Load",
            endpoint_or_flow: "GET /_app/auth/me while another user's synthetic sync ledger rebuild is active",
            default_dataset_id: DATASET_AUTH_RESTORE_CROSS_USER,
            default_dataset_shape: DATASET_SHAPE_AUTH_RESTORE_CROSS_USER,
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_SYNC_OVERLAP_CONCURRENCY,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(15.0),
                max_ms: Some(25.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES => Ok(PerfScenarioDefinition {
            id: SCENARIO_AUTH_RESTORE_DURING_APP_DB_WRITES,
            name: "Auth Restore During App DB Writes",
            endpoint_or_flow: "GET /_app/auth/me while repeated app DB chain-tip upserts are active",
            default_dataset_id: "tiny-empty",
            default_dataset_shape: "tiny-empty",
            default_warmup_iterations: DEFAULT_WARMUP_ITERATIONS,
            default_measured_iterations: DEFAULT_MEASURED_ITERATIONS,
            default_concurrency: DEFAULT_SYNC_OVERLAP_CONCURRENCY,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(15.0),
                max_ms: Some(25.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_READS_DURING_EXPORT => Ok(PerfScenarioDefinition {
            id: SCENARIO_READS_DURING_EXPORT,
            name: "Concurrent Reads During Export",
            endpoint_or_flow: "GET /_app/user/wallets + GET /_app/user/account/:account_id/transactions while POST /_app/user/exports/hledger/download is active",
            default_dataset_id: DATASET_ACCOUNT_TRANSACTIONS_HEAVY,
            default_dataset_shape: DATASET_SHAPE_ACCOUNT_TRANSACTIONS_HEAVY,
            default_warmup_iterations: DEFAULT_SYNC_WARMUP_ITERATIONS,
            default_measured_iterations: 4,
            default_concurrency: 4,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(35.0),
                max_ms: Some(60.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_UTXO_READS_DURING_SYNC => Ok(PerfScenarioDefinition {
            id: SCENARIO_UTXO_READS_DURING_SYNC,
            name: "UTXO Reads During Sync Rebuild",
            endpoint_or_flow: "GET /_app/user/wallets + GET /_app/user/account/:account_id/transactions while synthetic UTXO sync ledger rebuilds are active",
            default_dataset_id: DATASET_UTXO_TRANSACTIONS_HEAVY,
            default_dataset_shape: DATASET_SHAPE_UTXO_TRANSACTIONS_HEAVY,
            default_warmup_iterations: DEFAULT_SYNC_WARMUP_ITERATIONS,
            default_measured_iterations: 4,
            default_concurrency: 4,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(75.0),
                max_ms: Some(80.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_READS_DURING_SYNC => Ok(PerfScenarioDefinition {
            id: SCENARIO_READS_DURING_SYNC,
            name: "Concurrent Reads During Sync Rebuild",
            endpoint_or_flow: "GET /_app/user/wallets + GET /_app/user/account/:account_id/transactions + GET /_app/user/transactions/sync/state while synthetic sync ledger rebuilds are active",
            default_dataset_id: DATASET_SYNC_MOCK_HEAVY,
            default_dataset_shape: DATASET_SHAPE_SYNC_MOCK_HEAVY,
            default_warmup_iterations: DEFAULT_SYNC_WARMUP_ITERATIONS,
            default_measured_iterations: 3,
            default_concurrency: DEFAULT_SYNC_OVERLAP_CONCURRENCY,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(35.0),
                max_ms: Some(50.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_READS_DURING_HEAVY_SYNC => Ok(PerfScenarioDefinition {
            id: SCENARIO_READS_DURING_HEAVY_SYNC,
            name: "Concurrent Reads During Heavy Sync Rebuild",
            endpoint_or_flow: "GET /_app/user/wallets + GET /_app/user/account/:account_id/transactions + GET /_app/user/transactions/sync/state while a heavier synthetic sync ledger rebuild is active",
            default_dataset_id: DATASET_SYNC_OVERLAP_HEAVY,
            default_dataset_shape: DATASET_SHAPE_SYNC_OVERLAP_HEAVY,
            default_warmup_iterations: DEFAULT_SYNC_WARMUP_ITERATIONS,
            default_measured_iterations: 4,
            default_concurrency: HEAVY_SYNC_OVERLAP_CONCURRENCY,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(350.0),
                max_ms: Some(450.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        SCENARIO_SETTINGS_WRITES_DURING_SYNC => Ok(PerfScenarioDefinition {
            id: SCENARIO_SETTINGS_WRITES_DURING_SYNC,
            name: "Settings Writes During Sync Rebuild",
            endpoint_or_flow: "GET /_app/user/wallets + POST /_app/user/settings/currency + GET /_app/user/account/:account_id/transactions + GET /_app/user/transactions/sync/state while synthetic sync ledger rebuilds are active",
            default_dataset_id: DATASET_SYNC_MOCK_HEAVY,
            default_dataset_shape: DATASET_SHAPE_SYNC_MOCK_HEAVY,
            default_warmup_iterations: DEFAULT_SYNC_WARMUP_ITERATIONS,
            default_measured_iterations: 3,
            default_concurrency: 4,
            budget: PerfBudget {
                median_ms: None,
                p95_ms: Some(300.0),
                max_ms: Some(350.0),
                max_error_count: Some(0),
                strict: true,
            },
        }),
        other => Err(PerfError::usage(format!(
            "unsupported perf scenario '{other}'"
        ))),
    }
}

pub(super) fn sync_workload_kind_for_scenario(
    scenario: &PerfScenarioDefinition,
) -> Result<PerfSyncWorkloadKind, PerfError> {
    match scenario.id {
        SCENARIO_READS_DURING_SYNC
        | SCENARIO_READS_DURING_HEAVY_SYNC
        | SCENARIO_SETTINGS_WRITES_DURING_SYNC
        | SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD => Ok(PerfSyncWorkloadKind::AccountModel),
        SCENARIO_UTXO_READS_DURING_SYNC => Ok(PerfSyncWorkloadKind::Utxo),
        other => Err(PerfError::usage(format!(
            "unsupported sync workload scenario '{other}'"
        ))),
    }
}

pub(super) fn sync_workload_profile_for_scenario(
    scenario: &PerfScenarioDefinition,
) -> Result<PerfSyncWorkloadProfile, PerfError> {
    match scenario.id {
        SCENARIO_READS_DURING_SYNC
        | SCENARIO_SETTINGS_WRITES_DURING_SYNC
        | SCENARIO_AUTH_RESTORE_DURING_USER_DB_LOAD
        | SCENARIO_UTXO_READS_DURING_SYNC => Ok(PerfSyncWorkloadProfile {
            confirmed_count: LARGE_ACCOUNT_TRANSACTIONS_CONFIRMED_COUNT,
            pending_count: LARGE_ACCOUNT_TRANSACTIONS_PENDING_COUNT,
            rebuild_iterations: SYNC_LEDGER_REBUILD_ITERATIONS,
        }),
        SCENARIO_READS_DURING_HEAVY_SYNC => Ok(PerfSyncWorkloadProfile {
            confirmed_count: HEAVY_SYNC_OVERLAP_CONFIRMED_COUNT,
            pending_count: HEAVY_SYNC_OVERLAP_PENDING_COUNT,
            rebuild_iterations: HEAVY_SYNC_LEDGER_REBUILD_ITERATIONS,
        }),
        other => Err(PerfError::usage(format!(
            "unsupported sync workload profile for '{other}'"
        ))),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn heavy_sync_workload_profile_scales_beyond_baseline_overlap() {
        let baseline = sync_workload_profile_for_scenario(
            &resolve_scenario(SCENARIO_READS_DURING_SYNC)
                .expect("baseline scenario should resolve"),
        )
        .expect("baseline workload profile should resolve");
        let heavy = sync_workload_profile_for_scenario(
            &resolve_scenario(SCENARIO_READS_DURING_HEAVY_SYNC)
                .expect("heavy scenario should resolve"),
        )
        .expect("heavy workload profile should resolve");

        assert!(heavy.confirmed_count > baseline.confirmed_count);
        assert!(heavy.pending_count > baseline.pending_count);
        assert!(heavy.rebuild_iterations > baseline.rebuild_iterations);
    }
}

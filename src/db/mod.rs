//! Database module
//!
//! Manages multiple SQLite databases:
//! - **App database** (`{project_dir}/app/data/app.db`) - Auth, sessions, app-wide data
//! - **User database** (`{project_dir}/users/{user_id}/data/u{user_id}.db`) - Per-user settings
//!
//! Each database type has its own migrations that run at the appropriate time:
//! - App migrations run at startup (first database access)
//! - User migrations run after login (when user database is initialized)

mod account_balance_resolution;
pub(crate) mod account_limits;
mod account_transactions;
#[cfg(all(test, feature = "db-tests"))]
mod account_transfer_migration_tests;
mod amount_storage;
mod api_keys;
mod app_db;
mod app_updates;
mod app_user_preferences;
pub(crate) mod balance_reliability;
mod chain_cleanup;
mod client_capabilities;
#[cfg(feature = "server")]
pub(crate) mod encryption;
#[cfg(all(test, feature = "server", feature = "db-tests"))]
mod encryption_tests;
pub(crate) mod entitlement_snapshots;
mod error;
mod exports;
mod free_tier_entitlements;
pub(crate) mod legal_acceptances;
mod manual_asset_assertions;
#[cfg(all(test, feature = "db-tests"))]
mod manual_assets_migration_tests;
mod paired_client_names;
pub(crate) mod payments;
mod price_overrides;
pub(crate) mod prices_db;
pub(crate) mod raw_ingestion;
pub(crate) mod settings;
mod sqlite_config;
mod sync_slots;
#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod test_runtime;
mod transaction_sync;
mod transactions;
mod user_data_repairs;
mod user_db;
mod wallet_accounts;
mod wallet_data_import;
#[path = "wallets/mod.rs"]
mod wallets;

// Re-export error types
pub(crate) use error::DbError;
pub(crate) use error::DbInitError;

// Entitlement snapshots
pub(crate) use entitlement_snapshots::user_has_active_paid_entitlement;
pub(crate) use free_tier_entitlements::{
    load_free_tier_entitlement_cache, upsert_free_tier_entitlement_cache,
};

// User database functions
pub(crate) use user_data_repairs::{
    BITCOIN_HISTORY_FULL_RESYNC_REPAIR, UserDataRepairStatus, bitcoin_history_repair_owns_account,
    complete_bitcoin_history_full_resync_if_satisfied,
    load_unverified_bitcoin_history_repair_account_ids, load_user_data_repair_status,
    record_user_data_repair_failure, run_pending_user_data_repairs_conn,
};
pub(crate) use user_db::debug_assert_user_db_unlocked;
pub(crate) use user_db::{
    close_user_db, get_user_db_dek, initialize_user_db, list_open_user_db_users, with_user_db,
    with_user_db_mut,
};

// Settings
pub(crate) use api_keys::{
    clear_api_key, has_api_key, list_all_api_keys, load_api_key, save_api_key,
};
pub(crate) use app_updates::{
    AppUpdateState, load_update_state, save_successful_update_check, set_update_check_enabled,
};
#[cfg(test)]
pub(crate) use app_user_preferences::set_price_fetching_enabled;
pub(crate) use app_user_preferences::{
    get_price_fetching_enabled, set_price_fetching_enabled_with_transition,
};
pub(crate) use raw_ingestion::cleanup_raw_sync_history_with_compaction;
pub(crate) use settings::{
    load_settings, save_coingecko_api_key, save_currency, save_date_time_format,
    save_etherscan_api_key, save_etherscan_base_url, save_hledger_account_prefix, save_language,
    save_mempool_base_url, save_number_format, save_session_duration, save_timezone,
};
pub(crate) use sync_slots::{
    AccountSyncSlotRecord, active_sync_slot_account_ids, load_account_sync_slot_map,
    load_account_sync_slots, resolve_address_sync_slot_account, select_account_sync_slot,
};

pub(crate) use client_capabilities::{
    RevokeClientCapabilityResult, capability_ids_for_user, clear_expired_client_capability_wrap,
    find_capability_identity_by_verifier, insert_active_client_capability,
    list_expired_client_capabilities, load_active_client_capability,
    load_client_capabilities_for_user, load_client_capability, record_client_capability_activity,
    revoke_client_capability,
};
pub(crate) use paired_client_names::{
    delete_paired_client_name, insert_paired_client_name, list_paired_client_names,
    load_paired_client_name, remove_orphan_paired_client_names,
};

pub(crate) use price_overrides::{
    PriceOverrideRecord, delete_price_override, list_price_overrides_in_range,
    lookup_price_override, upsert_price_override,
};
pub(crate) use prices_db::{
    CURRENT_PRICE_CACHE_TTL, CoinGeckoCatalogSearchRow, CoinGeckoCatalogUpsert,
    CurrentPriceCacheRecord, CurrentPriceCacheRequest, CurrentPriceCacheUpsert,
    DailyPriceDateQuery, DailyPricePointQuery, DailyPricePointRecord, DailyPricePointUpsert,
    HistoricalPriceAttemptCooldownQuery, HistoricalPriceAttemptQuery, HistoricalPriceAttemptRecord,
    HistoricalPriceAttemptStatus, HistoricalPriceAttemptUpsert, count_active_coingecko_catalog,
    count_active_coingecko_in_set, initialize_prices_db, latest_coingecko_catalog_retrieved_at,
    latest_historical_price_attempt, latest_historical_price_cooldown_attempt,
    load_daily_price_dates, load_fresh_current_price_cache, lookup_daily_price_point,
    replace_or_upsert_coingecko_catalog_rows, search_coingecko_asset_catalog,
    select_daily_price_point, upsert_current_price_cache, upsert_daily_price_points,
    upsert_historical_price_attempt,
};

#[cfg(feature = "server")]
const _: () = {
    // Task 3 consumes this API; keep strict server builds warning-clean between commits.
    let _ = count_active_coingecko_catalog;
    let _ = count_active_coingecko_in_set;
    let _ = CURRENT_PRICE_CACHE_TTL;
    let _ = load_fresh_current_price_cache;
    let _ = load_daily_price_dates;
    let _ = lookup_daily_price_point;
    let _ = select_daily_price_point;
    let _ = upsert_current_price_cache;
    let _ = upsert_daily_price_points;
    let _ = upsert_historical_price_attempt;
    let _ = latest_historical_price_attempt;
    let _ = latest_historical_price_cooldown_attempt;
    let _ = latest_coingecko_catalog_retrieved_at;
    let _ = replace_or_upsert_coingecko_catalog_rows;
    let _ = search_coingecko_asset_catalog;
    let _ = load_free_tier_entitlement_cache;
    let _ = load_holdings_report;
    let _ = upsert_free_tier_entitlement_cache;
    // Later pairing tasks consume these APIs; keep intermediate commits warning-clean.
    let _ = find_capability_identity_by_verifier;
    let _ = clear_expired_client_capability_wrap;
    let _ = insert_active_client_capability;
    let _ = list_expired_client_capabilities;
    let _ = load_active_client_capability;
    let _ = load_client_capability;
    let _ = load_client_capabilities_for_user;
    let _ = record_client_capability_activity;
    let _ = revoke_client_capability;
    let _ = delete_paired_client_name;
    let _ = insert_paired_client_name;
    let _ = list_paired_client_names;
    let _ = load_paired_client_name;
    let _ = remove_orphan_paired_client_names;
    let _ = std::mem::size_of::<CoinGeckoCatalogSearchRow>();
    let _ = std::mem::size_of::<CoinGeckoCatalogUpsert>();
    let _ = std::mem::size_of::<CurrentPriceCacheRecord>();
    let _ = std::mem::size_of::<CurrentPriceCacheRequest>();
    let _ = std::mem::size_of::<CurrentPriceCacheUpsert>();
    let _ = std::mem::size_of::<DailyPriceDateQuery>();
    let _ = std::mem::size_of::<DailyPricePointQuery>();
    let _ = std::mem::size_of::<DailyPricePointRecord>();
    let _ = std::mem::size_of::<DailyPricePointUpsert>();
    let _ = std::mem::size_of::<HistoricalPriceAttemptCooldownQuery>();
    let _ = std::mem::size_of::<HistoricalPriceAttemptQuery>();
    let _ = std::mem::size_of::<HistoricalPriceAttemptRecord>();
    let _ = std::mem::size_of::<HistoricalPriceAttemptStatus>();
    let _ = std::mem::size_of::<HistoricalPriceAttemptUpsert>();
    let _ = std::mem::size_of::<HoldingsReportData>();
    let _ = std::mem::size_of::<HoldingsReportWalletData>();
};

pub(crate) use account_transactions::{
    AccountTransactionLedgerPage, BitcoinAccountCompletionPublication,
    BitcoinAddressProofPublication, BitcoinHdDiscoveryPublication, HoldingsReportData,
    HoldingsReportWalletData, WalletReportBalanceState, WalletReportLoadError,
    load_account_transactions_pages, load_holdings_report, load_holdings_report_range_plan,
    load_wallet_report, load_wallet_report_range_plan, publish_bitcoin_account_completion,
    rebuild_account_transaction_ledger,
    rebuild_account_transaction_ledger_with_unknown_bitcoin_basis,
};
pub(crate) use exports::{
    ExportAccountBoundaryMode, ExportAccountRow, ExportManualAssetBalanceAssertionRow,
    load_all_accounts_for_export, load_all_manual_asset_balance_assertion_rows_for_export,
};
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) use exports::{
    ExportAccountTransactionLedgerRow, ExportCommodity, ExportNativeApiBalanceAssertionRow,
    load_all_confirmed_account_transaction_ledger_rows_for_export,
    load_all_native_api_balance_assertion_rows_for_export,
};
pub(crate) use manual_asset_assertions::{
    ManualAssetAssertionDbError, ManualAssetBalanceAssertionPage, ManualAssetBalanceState,
    add_manual_asset_balance_assertion, delete_manual_asset_balance_assertion,
    load_manual_asset_account_history, load_manual_asset_current_balances,
    load_manual_asset_wallet_report_rows, update_manual_asset_balance_assertion,
};
#[cfg(all(test, feature = "db-tests"))]
pub(crate) use transaction_sync::AccountSyncStateRow;
#[cfg(any(
    all(feature = "server", feature = "dev-config", not(test)),
    all(test, feature = "db-tests")
))]
pub(crate) use transaction_sync::reconcile_address_transactions;
pub(crate) use transaction_sync::{
    AccountIntegrationSyncStart, AccountSyncBundle, AddressSyncSuccess,
    BitcoinAccountHistoryCoverage, CoverageInvalidationTargets, HdAccountChainFrontierPhase,
    HdAccountChainSyncState, HdMempoolHistoryFrontierUpdate, MempoolAddressObservationSuccess,
    MempoolHistoryPageWorkUpdate, MempoolHistoryProof, ProviderTransferKey,
    StrictMempoolScanValidation, SyncAccountTransactionRecord, SyncAccountTransferRecord,
    SyncAddress, SyncTransactionInputRecord, SyncTransactionOutputRecord, SyncTransactionRecord,
    TransactionSyncReconcileSummary, account_has_incomplete_mempool_history_with_conn,
    address_has_pending_txs, begin_mempool_history_scan, commit_mempool_history_page_work,
    complete_hd_account_discovery, delete_hd_account_chain_sync_state, get_hd_account_sync_bundles,
    get_non_hd_sync_addresses, get_sync_addresses_for_account,
    invalidate_mempool_account_history_coverage, invalidate_mempool_history_coverage,
    invalidate_mempool_history_proof, load_account_ids_with_pending_txs, load_account_labels,
    load_account_mempool_expected_tx_count, load_account_reported_tx_counts,
    load_account_sync_snapshots, load_address_ids_with_activity, load_address_ids_with_pending_txs,
    load_aggregate_sync_snapshot, load_canonical_account_transaction_count_bounded,
    load_canonical_confirmed_account_transaction_count, load_chain_tip_state,
    load_confirmed_tx_hashes_for_address, load_hd_account_chain_sync_state,
    load_known_tx_hashes_for_address, mark_account_integration_sync_started,
    mark_address_sync_completed_failure, mark_address_sync_completed_success,
    mark_address_sync_started, persist_mempool_address_observation_success,
    publish_mempool_history_proof, publish_strict_mempool_history_proof,
    reconcile_account_transactions, reconcile_address_transactions_preserving_invalidation,
    refresh_account_integration_sync_state, restart_strict_mempool_history_scan,
    update_address_etherscan_backfill_cursor, update_address_etherscan_history_status,
    update_address_mempool_backfill_cursor, update_address_mempool_expected_tx_count,
    upsert_account_sync_state, upsert_chain_tip_state, upsert_hd_account_chain_sync_state,
    validate_strict_mempool_history_scan,
};
pub(crate) use transactions::{
    load_account_transaction_counts, load_account_transaction_history, load_all_account_balances,
};
pub(crate) use wallet_accounts::{WalletAccountRecordKind, resolve_wallet_account_record_kind};
pub(crate) use wallet_data_import::{
    ImportDuplicateSkipView, ImportGlobalDuplicateSkipView, ImportNativeAccountView,
    WalletDataImportDbError, WalletDataImportResult, WalletDataImportSettings,
    extract_import_settings, import_wallet_data,
};
#[cfg(feature = "dev-config")]
pub(crate) use wallets::AddEthAddressDbResult;
pub(crate) use wallets::{
    LinkTrezorDbError, ManualAssetAccountRow, MoveAccountDbError, WalletDbConflict,
    WalletSummaryBundle, account_exists, add_bitcoin_address_with_account_label,
    add_ethereum_address_with_account_label, add_manual_asset_account,
    add_xpub_wallet_with_account_label, address_exists, classify_wallet_db_conflict,
    create_wallet_and_move_account, delete_account as delete_wallet_account, delete_wallet,
    derive_address_from_extended_pubkey, derive_next_derived_addresses_for_account,
    find_extended_pubkey_scheme_link, find_wallet_for_extended_pubkey, get_wallet_by_fingerprint,
    link_trezor_wallet, list_wallets, load_account_addresses_page, load_wallet_summary_bundle,
    move_account_to_wallet, update_account_label as update_wallet_account_label,
    update_wallet_label,
};
#[cfg(any(feature = "dev-config", feature = "db-tests"))]
pub(crate) use wallets::{add_bitcoin_address, add_ethereum_address};

// Test utilities
#[cfg(test)]
pub(crate) use app_db::{enable_test_mode, reset_test_db};
#[cfg(all(test, feature = "db-tests", feature = "dev-config"))]
pub(crate) use test_fixtures::setup_unencrypted_dev_test_user;
#[cfg(all(test, feature = "db-tests"))]
pub(crate) use test_fixtures::{
    add_eth_account_to_existing_wallet_fixture, create_eth_wallet_account_fixture,
    ensure_test_app_user, persist_sync_address_fixture, setup_test_user, unique_user_id,
    wallet_label,
};
#[cfg(test)]
pub(crate) use test_runtime::{TestRuntimeGuard, acquire_test_runtime};
#[cfg(test)]
pub(crate) use user_db::close_user_dbs_for_current_runtime;
#[cfg(test)]
pub(crate) use user_db::enable_test_mode as enable_user_test_mode;
#[cfg(all(test, feature = "db-tests"))]
pub(crate) use user_db::user_db_lock_state_for_test;
#[cfg(all(test, feature = "db-tests"))]
pub(crate) use user_db::{
    initialize_user_db_for_test, initialize_user_db_for_test_with_auto_vacuum_mode,
};

// Backwards-compatible aliases for existing code
// (with_app_db and with_app_db_mut are also available for explicit app db access)
pub(crate) use app_db::with_app_db as with_db;
pub(crate) use app_db::with_app_db_mut as with_db_mut;

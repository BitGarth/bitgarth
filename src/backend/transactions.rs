#[cfg(feature = "server")]
use crate::models::FieldErrors;
use crate::transactions::{
    AccountSyncSnapshot, AggregateSyncSnapshot, RawTransactionSyncTriggerRequest,
    TriggerSyncResponse,
};
#[cfg(feature = "server")]
use chrono::Utc;
use dioxus::prelude::*;

use super::ApiErrorEnvelope;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use super::api_error::ApiErrorCode;
#[cfg(feature = "server")]
use super::session_context::{
    InitializedSession, require_initialized_session, require_session_token,
};
#[cfg(feature = "server")]
use crate::amounts::{UnsignedAmount, format_unsigned_amount};
#[cfg(feature = "server")]
use crate::asset_capabilities::{
    asset_instance, sync_provider, synced_asset_instance, synced_asset_instance_id,
};
#[cfg(feature = "server")]
use crate::db::ManualAssetAssertionDbError;
#[cfg(feature = "server")]
use crate::db::ManualAssetBalanceState;
#[cfg(feature = "server")]
use crate::db::load_account_sync_snapshots as load_account_sync_snapshots_db;
#[cfg(feature = "server")]
use crate::db::load_account_transactions_pages as load_account_transactions_pages_db;
#[cfg(feature = "server")]
use crate::db::load_aggregate_sync_snapshot as load_aggregate_sync_snapshot_db;
#[cfg(feature = "server")]
use crate::db::load_manual_asset_account_history as load_manual_asset_account_history_db;
#[cfg(feature = "server")]
use crate::db::resolve_wallet_account_record_kind as resolve_wallet_account_record_kind_db;
#[cfg(feature = "server")]
use crate::db::update_manual_asset_balance_assertion as update_manual_asset_balance_assertion_db;
#[cfg(feature = "server")]
use crate::db::with_user_db;
#[cfg(feature = "server")]
use crate::db::{
    AccountSyncSlotRecord, WalletAccountRecordKind, account_exists as account_exists_db,
    active_sync_slot_account_ids,
    add_manual_asset_balance_assertion as add_manual_asset_balance_assertion_db,
    address_exists as address_exists_db,
    delete_manual_asset_balance_assertion as delete_manual_asset_balance_assertion_db,
    load_account_sync_slots, resolve_address_sync_slot_account,
};
#[cfg(feature = "server")]
use crate::models::SessionToken;
#[cfg(feature = "server")]
use crate::payments::types::EntitlementTier;
#[cfg(feature = "server")]
use crate::sync_control::is_sync_control_enabled;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use crate::tasks::subscribe_transaction_sync_events;
#[cfg(feature = "server")]
use crate::tasks::{
    JobId, JobKey, TriggerEnqueueResult, TriggerParams, TriggerRequest, TriggerSource,
    UserTransactionMonitorParams, enqueue_trigger, ensure_started,
};
#[cfg(feature = "server")]
use crate::transactions::NativeBalanceState;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use crate::transactions::TransactionSyncEvent;
#[cfg(feature = "server")]
use crate::transactions::TransactionSyncQueueOutcome;
#[cfg(feature = "server")]
use crate::transactions::TransactionSyncRunId;
#[cfg(feature = "server")]
use crate::transactions::{TransactionSyncScope, TransactionSyncTriggerSource};
use crate::wallets::{
    AddManualAssetBalanceAssertionRequest, AddManualAssetBalanceAssertionResponse,
    DeleteManualAssetBalanceAssertionRequest, UpdateManualAssetBalanceAssertionRequest,
    WalletAccountHistoryResponse,
};
#[cfg(feature = "server")]
use crate::wallets::{
    GetAccountTransactionsResponse, ManualAssetAccountTransactionsResponse,
    ManualAssetBalanceAssertionTableResponse, ManualAssetDisplayScale, TransactionsEmptyHint,
};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::http::StatusCode as AxumStatusCode;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::response::sse::{Event, Sse};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use dioxus::logger::tracing;
#[cfg(feature = "server")]
use rusqlite::OptionalExtension;
#[cfg(feature = "server")]
use std::collections::{HashMap, HashSet};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use std::convert::Infallible;
#[cfg(feature = "server")]
use std::str::FromStr;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use tokio::sync::mpsc;
#[cfg(feature = "server")]
use tokio_stream as _;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use tokio_stream::wrappers::ReceiverStream;

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
const USER_SYNC_SSE_CHANNEL_CAPACITY: usize = 64;

pub(crate) type TransactionsError = ApiErrorEnvelope;

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> TransactionsError {
    TransactionsError::unauthorized(message)
}

#[cfg(feature = "server")]
fn validation_error(errors: FieldErrors) -> TransactionsError {
    TransactionsError::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn single_field_conflict_error(field: &str, message: impl Into<String>) -> TransactionsError {
    let mut errors = FieldErrors::new();
    errors.add(field, message.into());
    TransactionsError::conflict("Conflict", errors)
}

#[cfg(feature = "server")]
fn resolve_decimal_precision(
    user_id: crate::models::UserId,
    account_id: crate::wallets::WalletAccountId,
) -> Result<u8, TransactionsError> {
    let raw = with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT decimal_precision
             FROM manual_asset_accounts
             WHERE id = ?1",
            rusqlite::params![account_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| {
            crate::db::DbError::new(format!(
                "Failed to load manual asset decimal_precision: {err}"
            ))
        })
    })
    .map_err(|_| TransactionsError::internal())?
    .ok_or_else(|| TransactionsError::not_found("Account not found".to_string()))?;
    let decimal_precision =
        ManualAssetDisplayScale::try_from(raw).map_err(|_| TransactionsError::internal())?;

    Ok(decimal_precision.as_u8())
}

#[cfg(feature = "server")]
fn not_found_error(message: impl Into<String>) -> TransactionsError {
    TransactionsError::not_found(message)
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> TransactionsError {
    tracing::error!(
        context,
        error = %detail,
        "transactions: internal failure"
    );
    TransactionsError::internal()
}

#[cfg(feature = "server")]
fn task_trigger_source(source: TransactionSyncTriggerSource) -> TriggerSource {
    match source {
        TransactionSyncTriggerSource::Manual => TriggerSource::ManualInternal,
        TransactionSyncTriggerSource::AutoAdd => TriggerSource::AutoAdd,
        TransactionSyncTriggerSource::AutoSessionRestore => TriggerSource::AutoSessionRestore,
        TransactionSyncTriggerSource::AutoFreshness => TriggerSource::AutoFreshness,
    }
}

#[cfg(feature = "server")]
fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, TransactionsError> {
    require_session_token("transactions", cookies, unauthorized_error)
}

#[cfg(feature = "server")]
fn initialized_session_from_cookie(
    cookies: &CookieJar,
) -> Result<InitializedSession, TransactionsError> {
    let session_token = session_token_from_cookie(cookies)?;
    require_initialized_session(
        "transactions",
        &session_token,
        unauthorized_error,
        |_message| TransactionsError::internal(),
    )
}

#[cfg(feature = "server")]
fn load_sync_slot_context(
    user_id: crate::models::UserId,
) -> Result<
    (
        Vec<AccountSyncSlotRecord>,
        HashSet<crate::wallets::DigitalAssetAccountId>,
        crate::payments::types::FeatureEntitlements,
    ),
    TransactionsError,
> {
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let records =
        load_account_sync_slots(user_id).map_err(|err| internal_error("load_sync_slots", err))?;
    let active = active_sync_slot_account_ids(&records, entitlements.sync_account_slots_limit);
    Ok((records, active, entitlements))
}

#[cfg(feature = "server")]
fn ensure_active_native_account_for_sync(
    user_id: crate::models::UserId,
    account_id: crate::wallets::DigitalAssetAccountId,
) -> Result<(), TransactionsError> {
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let eligible = crate::db::account_limits::native_account_sync_eligible_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
        account_id,
        entitlements.tier == EntitlementTier::Free,
    )
    .map_err(|err| internal_error("classify_supported_accounts_for_user", err))?;

    if eligible {
        Ok(())
    } else {
        let mut errors = FieldErrors::new();
        errors.add(
            "account_id",
            "Upgrade to activate this account.".to_string(),
        );
        Err(validation_error(errors))
    }
}

#[cfg(feature = "server")]
fn native_account_sync_slot_view(
    account_id: crate::wallets::DigitalAssetAccountId,
    records: &[AccountSyncSlotRecord],
    active: &HashSet<crate::wallets::DigitalAssetAccountId>,
    limit: u16,
    balance_sync_available_on_free: bool,
) -> crate::backend::NativeAccountSyncSlotView {
    let selected = records
        .iter()
        .find(|record| record.account_id == account_id);
    crate::backend::NativeAccountSyncSlotView {
        selected: selected.is_some(),
        active: active.contains(&account_id),
        can_select: balance_sync_available_on_free
            && selected.is_none()
            && records.len() < usize::from(limit),
        limit,
        selected_at: selected.map(|record| record.selected_at.to_rfc3339()),
        selected_under_tier: selected.map(|record| record.selected_under_tier.as_str().to_string()),
    }
}

#[cfg(feature = "server")]
fn apply_inactive_manual_sync_override(
    manual_sync: &mut crate::backend::NativeAccountManualSyncView,
    user_id: crate::models::UserId,
    account_id: crate::wallets::DigitalAssetAccountId,
    active_limit: u16,
) -> Result<(), TransactionsError> {
    let classified = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(active_limit),
    )
    .map_err(|err| internal_error("classify_supported_accounts_for_user", err))?;
    let state = crate::db::account_limits::account_state_for(
        &classified,
        &crate::wallets::WalletAccountId::from(account_id),
    );
    if state == crate::account_limits::AccountActivationState::Inactive {
        manual_sync.mode = crate::backend::ManualSyncMode::Unavailable;
        manual_sync.slot_effect = crate::backend::ManualSyncSlotEffect::NoCapacity;
        manual_sync.disabled_reason =
            Some(crate::backend::ManualSyncDisabledReason::AccountInactive);
    }
    Ok(())
}

#[cfg(feature = "server")]
fn transaction_history_coverage_notice(
    coverage: Option<crate::db::BitcoinAccountHistoryCoverage>,
    entitlements: &crate::payments::types::FeatureEntitlements,
    approximate_unsynced_count: Option<crate::transactions::TransactionCount>,
    confirmed_synced_count: crate::transactions::TransactionCount,
) -> Option<crate::wallets::requests::TransactionHistoryCoverageNoticeView> {
    if !matches!(
        coverage,
        Some(crate::db::BitcoinAccountHistoryCoverage::Limited)
    ) {
        return None;
    }

    let approximate_unsynced_count = approximate_unsynced_count.map_or(0, |count| count.value());
    if matches!(
        &entitlements.tier,
        crate::payments::types::EntitlementTier::Free
    ) {
        return Some(
            crate::wallets::requests::TransactionHistoryCoverageNoticeView::Free {
                approximate_unsynced_count,
            },
        );
    }

    Some(
        crate::wallets::requests::TransactionHistoryCoverageNoticeView::Paid {
            approximate_unsynced_count,
            confirmed_synced_count: confirmed_synced_count.value(),
            max_transactions_per_account: entitlements.historical_backfill_transactions_per_account,
        },
    )
}

#[get("/_app/user/transactions/sync/state", cookies: CookieJar)]
pub(crate) async fn get_sync_state() -> Result<AggregateSyncSnapshot, TransactionsError> {
    tracing::debug!("transactions: sync state requested");
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;

    let snapshot = load_aggregate_sync_snapshot_db(user_id)
        .map_err(|err| internal_error("load_aggregate_sync_snapshot", err))?;
    tracing::debug!(
        user_id = %user_id,
        is_running = snapshot.is_running,
        has_last_result = snapshot.last_result.is_some(),
        "transactions: sync state fetched"
    );

    Ok(snapshot)
}

#[get("/_app/user/transactions/sync/accounts", cookies: CookieJar)]
pub(crate) async fn get_account_sync_snapshots()
-> Result<Vec<AccountSyncSnapshot>, TransactionsError> {
    tracing::debug!("transactions: account sync snapshots requested");
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let snapshots = load_account_sync_snapshots_db(user_id)
        .map_err(|err| internal_error("load_account_sync_snapshots", err))?;
    let running_accounts = snapshots
        .iter()
        .filter(|snapshot| snapshot.is_running())
        .count();
    tracing::debug!(
        user_id = %user_id,
        accounts_total = snapshots.len(),
        running_accounts,
        "transactions: account sync snapshots fetched"
    );
    Ok(snapshots)
}

#[cfg(feature = "server")]
fn balance_amount_view(
    value: crate::amounts::UnsignedAmount,
    decimal_precision: u8,
) -> crate::backend::BalanceAmountView {
    crate::backend::BalanceAmountView {
        raw_value: value.raw_string(),
        formatted_value: format_unsigned_amount(value, decimal_precision),
    }
}

#[cfg(feature = "server")]
fn custom_balance_state_view(
    state: ManualAssetBalanceState,
    decimal_precision: u8,
) -> crate::backend::AccountBalanceStateView {
    match state {
        ManualAssetBalanceState::Known(amount) => crate::backend::AccountBalanceStateView::Known {
            amount: balance_amount_view(amount, decimal_precision),
        },
        ManualAssetBalanceState::Unknown => crate::backend::AccountBalanceStateView::Unknown,
    }
}

#[cfg(feature = "server")]
fn map_manual_assertion_db_error(error: ManualAssetAssertionDbError) -> TransactionsError {
    match error {
        ManualAssetAssertionDbError::AccountNotFound => {
            not_found_error("Manual asset account not found")
        }
        ManualAssetAssertionDbError::AssertionNotFound => {
            not_found_error("Balance assertion not found")
        }
        ManualAssetAssertionDbError::DuplicateAssertionDate => single_field_conflict_error(
            "asserted_on",
            "A balance assertion already exists for that date",
        ),
        ManualAssetAssertionDbError::InactiveAccountReadOnly => {
            let message = "Upgrade to modify assertions for this inactive account.";
            let mut errors = FieldErrors::new();
            errors.add("account_id", message.to_string());
            TransactionsError::validation(message, errors)
        }
        ManualAssetAssertionDbError::Database(err) => {
            internal_error("manual_asset_balance_assertions", err)
        }
    }
}

#[cfg(feature = "server")]
#[cfg(feature = "server")]
fn page_range(
    page: u32,
    page_size: u32,
    total: u32,
    row_count: usize,
) -> Result<(u32, u32), TransactionsError> {
    if total == 0 || row_count == 0 {
        return Ok((0, 0));
    }

    let start_u64 = u64::from(page.saturating_sub(1))
        .saturating_mul(u64::from(page_size))
        .saturating_add(1);
    let row_count_u64 = u64::try_from(row_count)
        .map_err(|_| internal_error("transactions_page_range", "row count overflow"))?;
    let end_u64 = start_u64
        .saturating_add(row_count_u64.saturating_sub(1))
        .min(u64::from(total));

    let start = u32::try_from(start_u64)
        .map_err(|_| internal_error("transactions_page_range", "start index overflow"))?;
    let end = u32::try_from(end_u64)
        .map_err(|_| internal_error("transactions_page_range", "end index overflow"))?;
    Ok((start, end))
}

#[cfg(feature = "server")]
fn to_table_response(
    asset_id: crate::wallets::SyncedAssetId,
    page: crate::db::AccountTransactionLedgerPage,
) -> Result<crate::wallets::AccountTransactionTableResponse, TransactionsError> {
    let decimal_precision = asset_instance(
        &synced_asset_instance(synced_asset_instance_id(asset_id)).asset_instance_id,
    )
    .ok_or_else(|| {
        internal_error(
            "asset_instance_lookup",
            "synced asset instance not found in registry",
        )
    })?
    .decimal_precision;
    let row_count = page.rows.len();
    let (start, end) = page_range(page.page, page.page_size, page.total, row_count)?;

    let rows = page
        .rows
        .into_iter()
        .map(|row| {
            let balance_reliability = row.balance_reliability;
            crate::wallets::AccountTransactionRowResponse {
                tx_hash: row.tx_hash,
                status: row.status,
                direction: row.direction,
                occurred_at: row.occurred_at.to_rfc3339(),
                from_addresses: row.from_addresses,
                to_addresses: row.to_addresses,
                value: balance_amount_view(row.value, decimal_precision),
                fee: row
                    .fee
                    .map(|value| balance_amount_view(value, decimal_precision)),
                closing_balance: row
                    .closing_balance
                    .map(|value| balance_amount_view(value, decimal_precision)),
                balance_reliability: balance_reliability.clone(),
            }
        })
        .collect();

    Ok(crate::wallets::AccountTransactionTableResponse {
        page: page.page,
        page_size: page.page_size,
        total: page.total,
        start,
        end,
        rows,
    })
}

#[cfg(feature = "server")]
fn native_balance_state_view(
    state: NativeBalanceState,
    decimal_precision: u8,
) -> crate::backend::NativeBalanceStateView {
    match state {
        NativeBalanceState::KnownAmount(amount) => crate::backend::NativeBalanceStateView::Known(
            balance_amount_view(amount, decimal_precision),
        ),
        NativeBalanceState::CanonicalZero => crate::backend::NativeBalanceStateView::CanonicalZero,
        NativeBalanceState::Unknown => crate::backend::NativeBalanceStateView::Unknown,
    }
}

#[cfg(feature = "server")]
fn confirmed_transactions_empty_hint(
    entitlements: &crate::payments::types::FeatureEntitlements,
    supports_balance_only_sync: bool,
    filters: &crate::wallets::TransactionFilters,
    confirmed_total: u32,
    closing_balance_state: NativeBalanceState,
    expected_transactions: Option<u32>,
) -> Option<TransactionsEmptyHint> {
    if confirmed_total > 0
        || !filters.status.is_empty()
        || filters.from_date.is_some()
        || filters.to_date.is_some()
    {
        return None;
    }

    if entitlements.tier == crate::payments::types::EntitlementTier::Free
        && !supports_balance_only_sync
    {
        return Some(TransactionsEmptyHint::FreePlanBalanceUnavailable);
    }

    if entitlements.historical_backfill_enabled {
        return Some(TransactionsEmptyHint::HistorySyncPending {
            expected_transactions: expected_transactions.filter(|expected| *expected > 0),
        });
    }

    let has_positive_known_balance = matches!(
        closing_balance_state,
        NativeBalanceState::KnownAmount(amount) if amount != UnsignedAmount::zero()
    );
    if !has_positive_known_balance {
        return None;
    }

    Some(TransactionsEmptyHint::FreePlanNoHistory)
}

#[cfg(feature = "server")]
fn opening_balance_state_for_history(
    has_ingested_history: bool,
    bitcoin_history_coverage: Option<crate::db::BitcoinAccountHistoryCoverage>,
    state: NativeBalanceState,
) -> NativeBalanceState {
    if !has_ingested_history
        && !matches!(
            bitcoin_history_coverage,
            Some(crate::db::BitcoinAccountHistoryCoverage::Complete { .. })
        )
    {
        NativeBalanceState::Unknown
    } else {
        state
    }
}

#[cfg(feature = "server")]
fn to_manual_assertion_table_response(
    page: crate::db::ManualAssetBalanceAssertionPage,
    decimal_precision: u8,
) -> ManualAssetBalanceAssertionTableResponse {
    ManualAssetBalanceAssertionTableResponse {
        page: page.page,
        page_size: page.page_size,
        total: page.total,
        start: page.start,
        end: page.end,
        rows: page
            .rows
            .into_iter()
            .map(
                |row| crate::wallets::ManualAssetBalanceAssertionRowResponse {
                    assertion_id: row.assertion_id,
                    asserted_on: row.asserted_on.format("%Y-%m-%d").to_string(),
                    asserted_balance: balance_amount_view(row.balance, decimal_precision),
                    note: row.note,
                },
            )
            .collect(),
    }
}

#[cfg(feature = "server")]
fn manual_asset_account_state_view(
    user_id: crate::models::UserId,
    account_id: crate::wallets::WalletAccountId,
) -> Result<crate::backend::AccountStateView, TransactionsError> {
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let classified = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
    )
    .map_err(|err| internal_error("classify_supported_accounts_for_user", err))?;
    let state = crate::db::account_limits::account_state_for(&classified, &account_id);
    Ok(match state {
        crate::account_limits::AccountActivationState::Active => {
            crate::backend::AccountStateView::Active
        }
        crate::account_limits::AccountActivationState::Inactive => {
            crate::backend::AccountStateView::Inactive
        }
    })
}

#[cfg(feature = "server")]
fn load_manual_asset_account_transactions_response(
    user_id: crate::models::UserId,
    account_id: crate::wallets::WalletAccountId,
    page: u32,
    sort: crate::wallets::TransactionSortDirection,
) -> Result<ManualAssetAccountTransactionsResponse, TransactionsError> {
    let history = load_manual_asset_account_history_db(
        user_id,
        account_id,
        page,
        crate::wallets::ACCOUNT_TRANSACTIONS_PAGE_SIZE,
        sort,
        None,
        None,
    )
    .map_err(map_manual_assertion_db_error)?;

    let decimal_precision = history.decimal_precision.as_u8();
    let account_state = manual_asset_account_state_view(user_id, account_id)?;

    Ok(ManualAssetAccountTransactionsResponse {
        account_id: history.account_id,
        wallet_id: history.wallet_id,
        wallet_label: history.wallet_label.as_str().to_string(),
        account_label: history.account_label.as_str().to_string(),
        account_state,
        sync_control_enabled: false,
        unit_code: history.unit_code.to_string(),
        decimal_precision,
        precision_status: history.precision_status,
        precision_shared_with_other_accounts: history.precision_shared_with_other_accounts,
        symbol: history.symbol,
        asset_name: history.asset_name,
        network_name: history.network_name,
        opening_balance_state: custom_balance_state_view(
            history.opening_balance_state,
            decimal_precision,
        ),
        opening_balance_date: history
            .opening_balance_date
            .map(|date| date.format("%Y-%m-%d").to_string()),
        closing_balance_state: custom_balance_state_view(
            history.closing_balance_state,
            decimal_precision,
        ),
        closing_balance_date: history
            .closing_balance_date
            .map(|date| date.format("%Y-%m-%d").to_string()),
        sort,
        active_from_date: None,
        active_to_date: None,
        assertions: to_manual_assertion_table_response(history.assertions, decimal_precision),
    })
}

#[get(
    "/_app/user/account/:account_id/transactions?pending_page&confirmed_page&sort&filters",
    cookies: CookieJar
)]
pub(crate) async fn get_account_transactions(
    account_id: crate::wallets::WalletAccountId,
    pending_page: Option<u32>,
    confirmed_page: Option<u32>,
    sort: Option<String>,
    filters: Option<String>,
) -> Result<WalletAccountHistoryResponse, TransactionsError> {
    tracing::debug!("transactions: account transactions requested");
    let validated = crate::wallets::GetAccountTransactionsRequest {
        account_id,
        pending_page,
        confirmed_page,
        sort,
        filters,
    }
    .try_into_validated()
    .map_err(validation_error)?;

    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let account_kind = resolve_wallet_account_record_kind_db(user_id, validated.account_id)
        .map_err(|err| internal_error("resolve_wallet_account_record_kind", err))?
        .ok_or_else(|| not_found_error("Account not found"))?;

    match account_kind {
        WalletAccountRecordKind::Native => {
            let native_account_id =
                crate::wallets::DigitalAssetAccountId::from_str(&validated.account_id.to_string())
                    .map_err(|err| internal_error("native_wallet_account_id_parse", err))?;
            let (sync_slot_records, active_sync_slots, entitlements) =
                load_sync_slot_context(user_id)?;
            let db_response = load_account_transactions_pages_db(
                user_id,
                native_account_id,
                (validated.pending_page, validated.confirmed_page),
                crate::wallets::ACCOUNT_TRANSACTIONS_PAGE_SIZE,
                validated.sort,
                &validated.filters,
                crate::transactions::TransactionCount::from_u32(
                    entitlements.historical_backfill_transactions_per_account,
                ),
            )
            .map_err(|err| {
                if err.to_string() == "Account not found" {
                    return not_found_error("Account not found");
                }
                internal_error("load_account_transactions_pages", err)
            })?;

            let synced = synced_asset_instance(synced_asset_instance_id(db_response.asset_id));
            let provider = sync_provider(synced.default_sync_provider);
            let instance = asset_instance(&synced.asset_instance_id).ok_or_else(|| {
                internal_error(
                    "asset_instance_lookup",
                    "synced asset instance not found in registry",
                )
            })?;
            let decimal_precision = instance.decimal_precision;
            let estimated_tx_count =
                crate::db::load_account_mempool_expected_tx_count(user_id, native_account_id)
                    .map_err(|err| internal_error("load_account_mempool_expected_tx_count", err))?;
            let (approximate_unsynced_count, confirmed_synced_count) = if matches!(
                db_response.bitcoin_history_coverage,
                Some(crate::db::BitcoinAccountHistoryCoverage::Limited)
            ) {
                let reported_address_counts =
                    crate::db::load_account_reported_tx_counts(user_id, native_account_id)
                        .map_err(|err| internal_error("load_account_reported_tx_counts", err))?;
                let known_transaction_count =
                    crate::db::load_canonical_confirmed_account_transaction_count(
                        user_id,
                        native_account_id,
                    )
                    .map_err(|err| {
                        internal_error("load_canonical_confirmed_account_transaction_count", err)
                    })?;
                (
                    Some(crate::tasks::approximate_account_unsynced_count(
                        reported_address_counts,
                        known_transaction_count,
                    )),
                    known_transaction_count,
                )
            } else {
                (None, crate::transactions::TransactionCount::zero())
            };
            let balance_sync_available_on_free = entitlements.tier
                != crate::payments::types::EntitlementTier::Free
                || provider.capabilities.supports_balance_only_sync;
            let etherscan_history_status = load_account_sync_snapshots_db(user_id)
                .map_err(|err| internal_error("load_account_sync_snapshots", err))?
                .into_iter()
                .find(|snapshot| snapshot.account_id == native_account_id)
                .and_then(|snapshot| snapshot.etherscan_history_status);

            let opening_balance_date = db_response
                .opening_balance_date
                .map(|d| d.format("%Y-%m-%d").to_string());
            let closing_balance_date = db_response
                .closing_balance_date
                .map(|d| d.format("%Y-%m-%d").to_string());

            let account_reference_kind = if db_response.account_reference.is_hd {
                crate::backend::AccountReferenceKind::ExtendedPubkey
            } else {
                crate::backend::AccountReferenceKind::SingleAddress
            };
            let confirmed_empty_hint = confirmed_transactions_empty_hint(
                &entitlements,
                provider.capabilities.supports_balance_only_sync,
                &validated.filters,
                db_response.confirmed.total,
                db_response.closing_balance_state,
                estimated_tx_count.map(|count| count.value()),
            );
            let opening_balance_state = opening_balance_state_for_history(
                db_response.has_ingested_history,
                db_response.bitcoin_history_coverage,
                db_response.opening_balance_state,
            );
            let sync_slot_map = sync_slot_records
                .iter()
                .map(|record| (record.account_id, record.clone()))
                .collect::<HashMap<_, _>>();
            let free_balance_unavailable_account_ids = if !balance_sync_available_on_free {
                std::iter::once(native_account_id).collect()
            } else {
                HashSet::new()
            };
            let mut manual_sync = crate::backend::wallets::native_account_manual_sync_view(
                native_account_id,
                db_response
                    .confirmed
                    .total
                    .saturating_add(db_response.pending.total),
                crate::backend::wallets::NativeAccountManualSyncContext {
                    sync_slots: &sync_slot_map,
                    active_sync_slot_account_ids: &active_sync_slots,
                    slot_limit: entitlements.sync_account_slots_limit,
                    tier: entitlements.tier.clone(),
                    historical_backfill_enabled: entitlements.historical_backfill_enabled,
                    historical_backfill_transactions_per_account: entitlements
                        .historical_backfill_transactions_per_account,
                    free_balance_unavailable_account_ids: &free_balance_unavailable_account_ids,
                },
            );
            apply_inactive_manual_sync_override(
                &mut manual_sync,
                user_id,
                native_account_id,
                entitlements.sync_account_slots_limit,
            )?;

            Ok(WalletAccountHistoryResponse::Native(
                GetAccountTransactionsResponse {
                    account_id: validated.account_id,
                    wallet_id: db_response.wallet_id,
                    wallet_label: db_response.wallet_label,
                    account_label: db_response.account_label,
                    sync_control_enabled: is_sync_control_enabled(),
                    native_account_id,
                    account_reference_kind,
                    account_reference: db_response.account_reference.reference_value,
                    address_scheme: db_response.account_reference.address_scheme,
                    asset: db_response.asset_id,
                    network: db_response.network,
                    unit_code: instance.unit_code.as_str().to_string(),
                    symbol: instance.symbol.as_ref().map(|s| s.to_string()),
                    bitcoin_history_coverage: db_response.bitcoin_history_coverage.map(Into::into),
                    sync_slot: Box::new(native_account_sync_slot_view(
                        native_account_id,
                        &sync_slot_records,
                        &active_sync_slots,
                        entitlements.sync_account_slots_limit,
                        balance_sync_available_on_free,
                    )),
                    manual_sync: Box::new(manual_sync),
                    etherscan_history_status,
                    is_free_tier: matches!(
                        &entitlements.tier,
                        crate::payments::types::EntitlementTier::Free
                    ),
                    current_balance_state: Box::new(native_balance_state_view(
                        db_response.current_balance_state,
                        decimal_precision,
                    )),
                    current_balance_checked_at: db_response
                        .current_balance_checked_at
                        .map(|checked_at| checked_at.to_rfc3339().into_boxed_str()),
                    transaction_history_coverage_notice: transaction_history_coverage_notice(
                        db_response.bitcoin_history_coverage,
                        &entitlements,
                        approximate_unsynced_count,
                        confirmed_synced_count,
                    ),
                    opening_balance_state: native_balance_state_view(
                        opening_balance_state,
                        decimal_precision,
                    ),
                    opening_balance_reliability: db_response.opening_balance_reliability,
                    opening_balance_date,
                    closing_balance_state: native_balance_state_view(
                        db_response.closing_balance_state,
                        decimal_precision,
                    ),
                    closing_balance_reliability: db_response.closing_balance_reliability,
                    closing_balance_date,
                    sort: validated.sort,
                    active_status_filter: validated.filters.status,
                    active_from_date: validated.filters.from_date.map(|d| d.to_rfc3339()),
                    active_to_date: validated.filters.to_date.map(|d| d.to_rfc3339()),
                    confirmed_empty_hint,
                    pending: to_table_response(db_response.asset_id, db_response.pending)?,
                    confirmed: to_table_response(db_response.asset_id, db_response.confirmed)?,
                },
            ))
        }
        WalletAccountRecordKind::Manual => Ok(WalletAccountHistoryResponse::Custom(
            load_manual_asset_account_transactions_response(
                user_id,
                validated.account_id,
                validated.confirmed_page,
                validated.sort,
            )?,
        )),
    }
}

#[post("/_app/user/manual-asset-assertions/add", cookies: CookieJar)]
pub(crate) async fn add_manual_asset_balance_assertion(
    request: AddManualAssetBalanceAssertionRequest,
) -> Result<AddManualAssetBalanceAssertionResponse, TransactionsError> {
    tracing::debug!(
        account_id = %request.account_id,
        "transactions: add manual asset balance assertion requested"
    );
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let decimal_precision = resolve_decimal_precision(user_id, request.account_id)?;

    let validated = request
        .try_into_validated(decimal_precision)
        .map_err(validation_error)?;

    let assertion_id = add_manual_asset_balance_assertion_db(user_id, validated, Utc::now())
        .map_err(map_manual_assertion_db_error)?;

    Ok(AddManualAssetBalanceAssertionResponse { assertion_id })
}

#[post("/_app/user/manual-asset-assertions/update", cookies: CookieJar)]
pub(crate) async fn update_manual_asset_balance_assertion(
    request: UpdateManualAssetBalanceAssertionRequest,
) -> Result<(), TransactionsError> {
    tracing::debug!(
        assertion_id = %request.assertion_id,
        "transactions: update manual asset balance assertion requested"
    );
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let decimal_precision = resolve_decimal_precision(user_id, request.account_id)?;

    let validated = request
        .try_into_validated(decimal_precision)
        .map_err(validation_error)?;

    update_manual_asset_balance_assertion_db(user_id, validated, Utc::now())
        .map_err(map_manual_assertion_db_error)?;
    Ok(())
}

#[post("/_app/user/manual-asset-assertions/delete", cookies: CookieJar)]
pub(crate) async fn delete_manual_asset_balance_assertion(
    request: DeleteManualAssetBalanceAssertionRequest,
) -> Result<(), TransactionsError> {
    tracing::debug!(
        assertion_id = %request.assertion_id,
        "transactions: delete manual asset balance assertion requested"
    );
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    delete_manual_asset_balance_assertion_db(user_id, request.assertion_id)
        .map_err(map_manual_assertion_db_error)?;
    Ok(())
}

#[post("/_app/user/transactions/sync", cookies: CookieJar)]
pub(crate) async fn trigger_sync(
    request: RawTransactionSyncTriggerRequest,
) -> Result<TriggerSyncResponse, TransactionsError> {
    tracing::debug!(
        request = ?request,
        "transactions: sync trigger requested"
    );
    let validated = request.try_into_validated().map_err(validation_error)?;
    let task_source = task_trigger_source(validated.source);
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let run_id = TransactionSyncRunId::new();

    match validated.scope {
        TransactionSyncScope::User => {}
        TransactionSyncScope::Account { account_id } => {
            let exists = account_exists_db(user_id, account_id)
                .map_err(|err| internal_error("account_exists", err))?;
            if !exists {
                return Err(not_found_error("Account not found"));
            }
            ensure_active_native_account_for_sync(user_id, account_id)?;
        }
        TransactionSyncScope::Address { address_id } => {
            let exists = address_exists_db(user_id, address_id)
                .map_err(|err| internal_error("address_exists", err))?;
            if !exists {
                return Err(not_found_error("Address not found"));
            }
            let Some(account_id) = resolve_address_sync_slot_account(user_id, address_id)
                .map_err(|err| internal_error("resolve_address_sync_slot_account", err))?
            else {
                return Err(not_found_error("Address not found"));
            };
            ensure_active_native_account_for_sync(user_id, account_id)?;
        }
    }

    if let Err(err) = ensure_started() {
        return Err(internal_error("ensure_started", err));
    }

    let enqueue_request = TriggerRequest {
        key: JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        },
        source: task_source,
        params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id,
            scope: validated.scope,
        }),
    };
    let enqueue_result = enqueue_trigger(enqueue_request).await;
    let (outcome, sync_run_id) = match enqueue_result {
        TriggerEnqueueResult::AcceptedStarted { run_id } => (
            TransactionSyncQueueOutcome::Started,
            run_id.ok_or_else(|| {
                internal_error(
                    "enqueue_trigger",
                    "task manager accepted sync trigger without a run id",
                )
            })?,
        ),
        TriggerEnqueueResult::AcceptedQueued { run_id } => (
            TransactionSyncQueueOutcome::Queued,
            run_id.ok_or_else(|| {
                internal_error(
                    "enqueue_trigger",
                    "task manager queued sync trigger without a run id",
                )
            })?,
        ),
        TriggerEnqueueResult::RejectedInvalidKey => {
            return Err(internal_error(
                "enqueue_trigger",
                "task manager rejected trigger key",
            ));
        }
        TriggerEnqueueResult::RejectedShuttingDown => {
            return Err(internal_error(
                "enqueue_trigger",
                "task manager is not available",
            ));
        }
    };

    tracing::debug!(
        user_id = %user_id,
        run_id = %sync_run_id,
        outcome = ?outcome,
        "transactions: sync trigger accepted"
    );

    Ok(TriggerSyncResponse {
        outcome,
        sync_run_id,
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn filters_with_no_values() -> crate::wallets::TransactionFilters {
        crate::wallets::TransactionFilters {
            status: Vec::new(),
            from_date: None,
            to_date: None,
        }
    }

    #[test]
    fn balance_only_without_ingested_history_has_unknown_opening_balance() {
        assert_eq!(
            opening_balance_state_for_history(false, None, NativeBalanceState::CanonicalZero,),
            NativeBalanceState::Unknown,
        );
    }

    #[test]
    fn filtered_empty_period_keeps_resolved_opening_when_account_has_history() {
        assert_eq!(
            opening_balance_state_for_history(true, None, NativeBalanceState::CanonicalZero,),
            NativeBalanceState::CanonicalZero,
        );
    }

    #[test]
    fn complete_empty_bitcoin_response_preserves_canonical_zero_opening() {
        assert_eq!(
            opening_balance_state_for_history(
                false,
                Some(crate::db::BitcoinAccountHistoryCoverage::Complete {
                    coverage_height: crate::transactions::ChainTipHeight::try_new(900_000)
                        .expect("height should parse"),
                }),
                NativeBalanceState::CanonicalZero,
            ),
            NativeBalanceState::CanonicalZero,
        );
    }

    #[test]
    fn older_transaction_response_without_bitcoin_coverage_deserializes() {
        let empty_table = crate::wallets::AccountTransactionTableResponse {
            page: 1,
            page_size: 50,
            total: 0,
            start: 0,
            end: 0,
            rows: Vec::new(),
        };
        let response = GetAccountTransactionsResponse {
            account_id: crate::wallets::WalletAccountId::new(),
            wallet_id: crate::wallets::WalletId::new(),
            wallet_label: "Wallet".to_string(),
            account_label: None,
            sync_control_enabled: true,
            native_account_id: crate::wallets::DigitalAssetAccountId::new(),
            account_reference_kind: crate::backend::AccountReferenceKind::SingleAddress,
            account_reference: "bc1qtest".to_string(),
            address_scheme: crate::wallets::AddressScheme::NativeSegwit,
            asset: crate::wallets::SyncedAssetId::Bitcoin,
            network: crate::wallets::Network::Mainnet,
            unit_code: "BTC".to_string(),
            symbol: Some("₿".to_string()),
            bitcoin_history_coverage: Some(
                crate::balance_reliability::BitcoinHistoryCoverageView::Complete,
            ),
            sync_slot: Box::new(crate::backend::NativeAccountSyncSlotView {
                selected: true,
                active: true,
                can_select: false,
                limit: 1,
                selected_at: None,
                selected_under_tier: None,
            }),
            manual_sync: Box::new(crate::backend::NativeAccountManualSyncView {
                mode: crate::backend::ManualSyncMode::TransactionHistory,
                slot_effect: crate::backend::ManualSyncSlotEffect::AlreadySelected,
                disabled_reason: None,
                used_slots: 1,
                slot_limit: 1,
                next_tier_display_name: None,
            }),
            etherscan_history_status: None,
            is_free_tier: true,
            current_balance_state: Box::new(crate::backend::NativeBalanceStateView::CanonicalZero),
            current_balance_checked_at: None,
            transaction_history_coverage_notice: None,
            opening_balance_state: crate::backend::NativeBalanceStateView::CanonicalZero,
            opening_balance_reliability: crate::balance_reliability::BalanceReliability::Final,
            opening_balance_date: None,
            closing_balance_state: crate::backend::NativeBalanceStateView::CanonicalZero,
            closing_balance_reliability: crate::balance_reliability::BalanceReliability::Final,
            closing_balance_date: None,
            sort: crate::wallets::TransactionSortDirection::Descending,
            active_status_filter: Vec::new(),
            active_from_date: None,
            active_to_date: None,
            confirmed_empty_hint: None,
            pending: empty_table.clone(),
            confirmed: empty_table,
        };
        let mut older_payload = serde_json::to_value(response).expect("response should serialize");
        older_payload
            .as_object_mut()
            .expect("response should serialize as an object")
            .remove("bitcoin_history_coverage");

        let decoded: GetAccountTransactionsResponse =
            serde_json::from_value(older_payload).expect("older response should deserialize");

        assert_eq!(decoded.bitcoin_history_coverage, None);
    }

    #[test]
    fn limited_coverage_notice_for_free_tier_omits_synced_count() {
        assert_eq!(
            transaction_history_coverage_notice(
                Some(crate::db::BitcoinAccountHistoryCoverage::Limited),
                &crate::payments::types::FeatureEntitlements::free(),
                Some(crate::transactions::TransactionCount::from_u32(28)),
                crate::transactions::TransactionCount::from_u32(0),
            ),
            Some(
                crate::wallets::requests::TransactionHistoryCoverageNoticeView::Free {
                    approximate_unsynced_count: 28,
                }
            ),
        );
    }

    #[test]
    fn limited_coverage_notice_for_paid_tier_includes_synced_count_and_cap() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tier = crate::payments::types::EntitlementTier::Basic;
        entitlements.historical_backfill_transactions_per_account = 1;
        assert_eq!(
            transaction_history_coverage_notice(
                Some(crate::db::BitcoinAccountHistoryCoverage::Limited),
                &entitlements,
                Some(crate::transactions::TransactionCount::from_u32(28)),
                crate::transactions::TransactionCount::from_u32(2),
            ),
            Some(
                crate::wallets::requests::TransactionHistoryCoverageNoticeView::Paid {
                    approximate_unsynced_count: 28,
                    confirmed_synced_count: 2,
                    max_transactions_per_account: 1,
                }
            ),
        );
    }

    #[test]
    fn complete_or_syncing_coverage_omits_typed_notice() {
        for coverage in [
            Some(crate::db::BitcoinAccountHistoryCoverage::Complete {
                coverage_height: crate::transactions::ChainTipHeight::try_new(900_000)
                    .expect("height should parse"),
            }),
            Some(crate::db::BitcoinAccountHistoryCoverage::Syncing),
        ] {
            assert_eq!(
                transaction_history_coverage_notice(
                    coverage,
                    &crate::payments::types::FeatureEntitlements::free(),
                    Some(crate::transactions::TransactionCount::from_u32(28)),
                    crate::transactions::TransactionCount::from_u32(2),
                ),
                None,
            );
        }
    }

    #[test]
    fn empty_hint_free_plan_known_balance_prompts_upgrade() {
        let entitlements = crate::payments::types::FeatureEntitlements::free();
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            true,
            &filters_with_no_values(),
            0,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
            None,
        );

        assert_eq!(hint, Some(TransactionsEmptyHint::FreePlanNoHistory));
    }

    #[test]
    fn empty_hint_filters_keep_generic_empty_state() {
        let entitlements = crate::payments::types::FeatureEntitlements::free();
        let mut filters = filters_with_no_values();
        filters.status = vec![crate::transactions::ChainTransactionStatus::Confirmed];
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            true,
            &filters,
            0,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
            None,
        );

        assert_eq!(hint, None);
    }

    #[test]
    fn empty_hint_paid_plan_known_balance_reports_pending_history() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tier = crate::payments::types::EntitlementTier::Basic;
        entitlements.historical_backfill_enabled = true;
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            true,
            &filters_with_no_values(),
            0,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
            None,
        );

        assert_eq!(
            hint,
            Some(TransactionsEmptyHint::HistorySyncPending {
                expected_transactions: None,
            })
        );
    }

    #[test]
    fn empty_hint_is_history_sync_pending_when_entitled_with_expected_count() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tier = crate::payments::types::EntitlementTier::Basic;
        entitlements.historical_backfill_enabled = true;
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            true,
            &filters_with_no_values(),
            0,
            NativeBalanceState::Unknown,
            Some(44),
        );
        assert_eq!(
            hint,
            Some(TransactionsEmptyHint::HistorySyncPending {
                expected_transactions: Some(44),
            })
        );
    }

    #[test]
    fn empty_hint_is_generic_history_sync_pending_when_expected_count_absent() {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.tier = crate::payments::types::EntitlementTier::Basic;
        entitlements.historical_backfill_enabled = true;
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            true,
            &filters_with_no_values(),
            0,
            NativeBalanceState::Unknown,
            None,
        );
        assert_eq!(
            hint,
            Some(TransactionsEmptyHint::HistorySyncPending {
                expected_transactions: None,
            })
        );
    }

    #[test]
    fn empty_hint_free_balance_unavailable_reports_provider_limit() {
        let entitlements = crate::payments::types::FeatureEntitlements::free();
        let hint = confirmed_transactions_empty_hint(
            &entitlements,
            false,
            &filters_with_no_values(),
            0,
            NativeBalanceState::Unknown,
            None,
        );

        assert_eq!(
            hint,
            Some(TransactionsEmptyHint::FreePlanBalanceUnavailable)
        );
    }

    #[test]
    fn manual_asset_assertion_inactive_error_maps_to_validation_upgrade_message() {
        let error =
            map_manual_assertion_db_error(ManualAssetAssertionDbError::InactiveAccountReadOnly);

        assert_eq!(
            error.code,
            crate::backend::api_error::ApiErrorCode::Validation
        );
        assert_eq!(
            error.message,
            "Upgrade to modify assertions for this inactive account."
        );
        assert_eq!(
            error.first_field_error("account_id").map(String::as_str),
            Some("Upgrade to modify assertions for this inactive account.")
        );
    }
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
fn transactions_error_to_status(error: TransactionsError) -> AxumStatusCode {
    match error.code {
        ApiErrorCode::Unauthorized => AxumStatusCode::UNAUTHORIZED,
        ApiErrorCode::Forbidden => AxumStatusCode::FORBIDDEN,
        ApiErrorCode::BadRequest => AxumStatusCode::BAD_REQUEST,
        ApiErrorCode::Validation => AxumStatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Conflict => AxumStatusCode::CONFLICT,
        ApiErrorCode::NotFound => AxumStatusCode::NOT_FOUND,
        ApiErrorCode::TooManyRequests => AxumStatusCode::TOO_MANY_REQUESTS,
        ApiErrorCode::Internal => AxumStatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) async fn transactions_sync_events_sse(
    cookies: CookieJar,
) -> Result<Sse<ReceiverStream<Result<Event, Infallible>>>, AxumStatusCode> {
    let initialized_session =
        initialized_session_from_cookie(&cookies).map_err(transactions_error_to_status)?;
    let user_id = initialized_session.session.user_id;
    let snapshot = load_aggregate_sync_snapshot_db(user_id)
        .map_err(|_| AxumStatusCode::INTERNAL_SERVER_ERROR)?;
    let snapshot_event = TransactionSyncEvent::sync_snapshot(snapshot, chrono::Utc::now());
    let mut broadcast_receiver = subscribe_transaction_sync_events(user_id)
        .map_err(|_| AxumStatusCode::INTERNAL_SERVER_ERROR)?;

    let (stream_tx, stream_rx) =
        mpsc::channel::<Result<Event, Infallible>>(USER_SYNC_SSE_CHANNEL_CAPACITY);
    let snapshot_payload = serde_json::to_string(&snapshot_event)
        .map_err(|_| AxumStatusCode::INTERNAL_SERVER_ERROR)?;
    let initial_event = Event::default()
        .event(snapshot_event.event_name())
        .data(snapshot_payload);

    stream_tx
        .send(Ok(initial_event))
        .await
        .map_err(|_| AxumStatusCode::INTERNAL_SERVER_ERROR)?;

    tokio::spawn(async move {
        while let Ok(sync_event) = broadcast_receiver.recv().await {
            let serialized = match serde_json::to_string(&sync_event) {
                Ok(payload) => payload,
                Err(err) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %err,
                        "transactions sse: failed to serialize sync event"
                    );
                    continue;
                }
            };
            let sse_event = Event::default()
                .event(sync_event.event_name())
                .data(serialized);
            if stream_tx.send(Ok(sse_event)).await.is_err() {
                tracing::debug!(
                    user_id = %user_id,
                    "transactions sse: client stream closed"
                );
                break;
            }
        }
    });

    tracing::debug!(
        user_id = %user_id,
        "transactions sse: client subscribed"
    );

    Ok(Sse::new(ReceiverStream::new(stream_rx)))
}

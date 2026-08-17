use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::session_context::{
    InitializedSession, require_initialized_session, require_session_token,
};
#[cfg(feature = "server")]
use crate::models::FieldErrors;
#[cfg(feature = "server")]
use crate::models::UserId;
#[cfg(feature = "server")]
use crate::payments::types::EntitlementTier;
#[cfg(feature = "server")]
use crate::sync_control::is_sync_control_enabled;

use crate::transactions::{
    AccountSyncControlStateResponse, RunAccountSyncControlRequest, SyncControlInvocationResponse,
};
#[cfg(feature = "server")]
use crate::transactions::{RawSyncIterationCount, SyncControlAddressState};
use crate::wallets::WalletAccountId;
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;

pub(crate) type SyncControlError = ApiErrorEnvelope;

#[cfg(feature = "server")]
fn forbidden_error(message: impl Into<String>) -> SyncControlError {
    SyncControlError::forbidden(message)
}

#[cfg(feature = "server")]
fn not_found_error(message: impl Into<String>) -> SyncControlError {
    SyncControlError::not_found(message)
}

#[cfg(feature = "server")]
fn validation_error(errors: FieldErrors) -> SyncControlError {
    SyncControlError::validation("Validation error", errors)
}

#[cfg(feature = "server")]
fn conflict_error(field: &str, message: impl Into<String>) -> SyncControlError {
    let mut errors = FieldErrors::new();
    errors.add(field, message.into());
    SyncControlError::conflict("Conflict", errors)
}

#[cfg(feature = "server")]
fn ensure_sync_control_repair_allowed(repair_owns_account: bool) -> Result<(), SyncControlError> {
    if repair_owns_account {
        return Err(conflict_error(
            "sync_control",
            "Bitcoin history correctness repair is in progress",
        ));
    }
    Ok(())
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> SyncControlError {
    tracing::error!(
        context,
        error = %detail,
        "sync control: internal failure"
    );
    SyncControlError::internal()
}

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> SyncControlError {
    SyncControlError::unauthorized(message)
}

#[cfg(feature = "server")]
async fn run_manual_sync_control_blocking(
    user_id: UserId,
    native_account_id: crate::wallets::DigitalAssetAccountId,
    iteration_budget: u32,
    guard: crate::sync_execution_lease::UserSyncExecutionLease,
) -> Result<SyncControlInvocationResponse, SyncControlError> {
    #[cfg(test)]
    let runtime_context = crate::runtime_context::current_runtime_context();

    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        #[cfg(test)]
        let _runtime_context_guard =
            runtime_context.map(crate::runtime_context::push_default_runtime_context);

        crate::tasks::run_manual_sync_control(user_id, native_account_id, iteration_budget)
    })
    .await
    .map_err(|err| internal_error("run_manual_sync_control_join", err))?
    .map_err(|err| internal_error("run_manual_sync_control", err))
}

#[cfg(feature = "server")]
fn initialized_session_from_cookie(
    cookies: &CookieJar,
) -> Result<InitializedSession, SyncControlError> {
    let session_token = require_session_token("sync_control", cookies, unauthorized_error)?;
    require_initialized_session(
        "sync_control",
        &session_token,
        unauthorized_error,
        |_message| SyncControlError::internal(),
    )
}

#[cfg(feature = "server")]
fn require_sync_control_enabled() -> Result<(), SyncControlError> {
    if !is_sync_control_enabled() {
        return Err(forbidden_error(
            "Sync control is disabled in this environment",
        ));
    }
    Ok(())
}

#[cfg(feature = "server")]
fn require_native_account(
    user_id: UserId,
    account_id: WalletAccountId,
) -> Result<crate::wallets::DigitalAssetAccountId, SyncControlError> {
    use crate::db::{WalletAccountRecordKind, resolve_wallet_account_record_kind};
    use std::str::FromStr;

    let account_kind = resolve_wallet_account_record_kind(user_id, account_id)
        .map_err(|err| internal_error("resolve_wallet_account_record_kind", err))?
        .ok_or_else(|| not_found_error("Account not found"))?;

    match account_kind {
        WalletAccountRecordKind::Native => {
            crate::wallets::DigitalAssetAccountId::from_str(&account_id.to_string())
                .map_err(|err| internal_error("native_wallet_account_id_parse", err))
        }
        WalletAccountRecordKind::Manual => Err(forbidden_error(
            "Sync control is not available for manual accounts",
        )),
    }
}

#[cfg(feature = "server")]
fn require_active_native_account_for_sync(
    user_id: UserId,
    native_account_id: crate::wallets::DigitalAssetAccountId,
) -> Result<(), SyncControlError> {
    let now = chrono::Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|err| internal_error("load_feature_entitlements", err))?;
    let eligible = crate::db::account_limits::native_account_sync_eligible_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
        native_account_id,
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
fn validate_iterations(raw: RawSyncIterationCount) -> Result<u32, SyncControlError> {
    raw.validate()
        .map(|validated| validated.value())
        .map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add("iterations", err.to_string());
            validation_error(errors)
        })
}

#[cfg(feature = "server")]
fn map_sync_control_address(address: &crate::db::SyncAddress) -> SyncControlAddressState {
    let backfill_state = crate::tasks::unfinished_backfill_state(address);
    let backfill_active = backfill_state.is_some();
    let backfill_cursor_display = backfill_state
        .as_ref()
        .map(|state| state.cursor.display_string());

    SyncControlAddressState {
        address_id: address.address_id,
        asset_id: address.asset_id,
        network: address.network,
        full_address: address.address.as_str().to_string(),
        truncated_address: truncate_address(address.address.as_str()),
        last_sync_at: address.last_completed_at.map(|dt| dt.to_rfc3339()),
        last_result: address.last_result.map(|r| r.as_db_value().to_string()),
        backfill_active,
        backfill_cursor_display,
        estimated_remaining_pages: None,
    }
}

#[cfg(feature = "server")]
fn truncate_address(address: &str) -> String {
    if address.starts_with("0x") {
        if address.len() > 14 {
            format!("{}\u{2026}{}", &address[..8], &address[address.len() - 4..])
        } else {
            address.to_string()
        }
    } else if address.len() > 16 {
        format!(
            "{}\u{2026}{}",
            &address[..10],
            &address[address.len() - 4..]
        )
    } else {
        address.to_string()
    }
}

#[cfg(feature = "server")]
fn acquire_sync_control_lease(
    user_id: UserId,
) -> Result<crate::sync_execution_lease::UserSyncExecutionLease, SyncControlError> {
    crate::sync_execution_lease::UserSyncExecutionLease::try_acquire(user_id).ok_or_else(|| {
        conflict_error(
            "sync_control",
            "A manual sync or Bitcoin history repair is already running for this user",
        )
    })
}

#[get("/_app/user/account/:account_id/sync-control/state", cookies: CookieJar)]
pub(crate) async fn get_account_sync_control_state(
    account_id: WalletAccountId,
) -> Result<AccountSyncControlStateResponse, SyncControlError> {
    tracing::debug!("sync control: state requested");
    require_sync_control_enabled()?;
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let native_account_id = require_native_account(user_id, account_id)?;
    require_active_native_account_for_sync(user_id, native_account_id)?;

    let addresses = crate::db::get_sync_addresses_for_account(user_id, native_account_id)
        .map_err(|err| internal_error("get_sync_addresses_for_account", err))?;

    let integration = addresses.first().map(|addr| {
        match crate::asset_capabilities::default_sync_provider(addr.asset_id) {
            crate::asset_capabilities::SyncProviderId::MempoolSpace => "mempool".to_string(),
            crate::asset_capabilities::SyncProviderId::Etherscan => "etherscan".to_string(),
        }
    });

    let address_states: Vec<SyncControlAddressState> =
        addresses.iter().map(map_sync_control_address).collect();

    Ok(AccountSyncControlStateResponse {
        account_id,
        addresses_total: u32::try_from(address_states.len()).unwrap_or(u32::MAX),
        integration,
        addresses: address_states,
    })
}

#[post(
    "/_app/user/account/:account_id/sync-control/run",
    cookies: CookieJar
)]
pub(crate) async fn run_account_sync_control(
    account_id: WalletAccountId,
    request: RunAccountSyncControlRequest,
) -> Result<SyncControlInvocationResponse, SyncControlError> {
    tracing::debug!(
        account_id = %account_id,
        iterations = request.iterations.0,
        "sync control: run requested"
    );
    require_sync_control_enabled()?;
    let initialized_session = initialized_session_from_cookie(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let native_account_id = require_native_account(user_id, account_id)?;
    let guard = acquire_sync_control_lease(user_id)?;
    ensure_sync_control_repair_allowed(
        crate::db::bitcoin_history_repair_owns_account(user_id, native_account_id)
            .map_err(|err| internal_error("bitcoin_history_repair_owns_account", err))?,
    )?;
    require_active_native_account_for_sync(user_id, native_account_id)?;
    let iteration_budget = validate_iterations(request.iterations)?;

    run_manual_sync_control_blocking(user_id, native_account_id, iteration_budget, guard).await
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    #[test]
    fn repair_in_progress_rejects_sync_control_before_lock() {
        let error = ensure_sync_control_repair_allowed(true)
            .expect_err("repair-owned account must reject sync control");
        assert!(error.is_conflict());
        assert_eq!(
            error.first_field_error("sync_control").map(String::as_str),
            Some("Bitcoin history correctness repair is in progress")
        );
        assert!(ensure_sync_control_repair_allowed(false).is_ok());
    }

    #[test]
    fn shared_sync_lease_survives_cancelled_waiter() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        let user_id = UserId::new();
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();

        runtime.block_on(async {
            let guard =
                acquire_sync_control_lease(user_id).expect("first lock acquisition should succeed");
            let worker_started = Arc::clone(&started);
            let worker_release = Arc::clone(&release);
            let waiter = tokio::task::spawn_blocking(move || {
                let _guard = guard;
                worker_started.wait();
                worker_release.wait();
                drop(_guard);
                done_tx.send(()).expect("completion signal should send");
            });

            started.wait();
            drop(waiter);
            assert!(
                acquire_sync_control_lease(user_id).is_err(),
                "dropping the waiter must not release a running blocking task's lock"
            );

            release.wait();
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("blocking task should finish within the bound");
            let reacquired = acquire_sync_control_lease(user_id)
                .expect("lock should release when blocking work finishes");
            drop(reacquired);
        });
    }
}

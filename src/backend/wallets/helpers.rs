#![cfg(feature = "server")]

use crate::backend::session_context::require_session_token;
use crate::db::{
    LinkTrezorDbError, MoveAccountDbError, WalletDbConflict, classify_wallet_db_conflict,
};
use crate::models::{FieldErrors, SessionToken};
use crate::sync_control::sync_control_mode;
use crate::tasks::automatic_sync::should_enqueue_automatic_add_sync;
use crate::tasks::automatic_sync::{AutomaticSyncAddTarget, automatic_add_sync_scope};
use crate::tasks::{
    JobId, JobKey, TriggerEnqueueResult, TriggerParams, TriggerRequest, TriggerSource,
    UserTransactionMonitorParams, enqueue_trigger, ensure_started,
};
use crate::transactions::TransactionSyncRunId;
use crate::transactions::TransactionSyncScope;
use axum_extra::extract::cookie::CookieJar;
use dioxus::logger::tracing;

use super::types::WalletError;

pub(super) fn unauthorized_error(message: String) -> WalletError {
    WalletError::unauthorized(message)
}

pub(super) fn not_found_error(message: impl Into<String>) -> WalletError {
    WalletError::not_found(message)
}

pub(super) fn validation_error(errors: FieldErrors) -> WalletError {
    WalletError::validation("Validation error", errors)
}

pub(super) fn conflict_error(errors: FieldErrors) -> WalletError {
    WalletError::conflict("Conflict", errors)
}

pub(super) fn internal_error(context: &str, detail: impl std::fmt::Display) -> WalletError {
    tracing::error!(
        context,
        error = %detail,
        "wallets: internal failure"
    );
    WalletError::internal()
}

fn is_supported_account_hard_cap_error(message: &str) -> bool {
    message.contains("Supported account hard cap exceeded")
}

pub(super) async fn run_wallet_db_blocking<F>(
    work: F,
    join_context: &'static str,
) -> Result<(), WalletError>
where
    F: FnOnce() -> Result<(), crate::db::DbError> + Send + 'static,
{
    #[cfg(test)]
    let runtime_context = crate::runtime_context::current_runtime_context();

    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        let _runtime_context_guard =
            runtime_context.map(crate::runtime_context::push_default_runtime_context);

        work()
    })
    .await
    .map_err(|e| internal_error("wallets", format!("{join_context}: {e}")))?
    .map_err(|e| internal_error("wallets", e))
}

pub(super) fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, WalletError> {
    require_session_token("wallets", cookies, unauthorized_error)
}

pub(super) fn automatic_add_trigger_request(
    user_id: crate::models::UserId,
    target: AutomaticSyncAddTarget,
) -> TriggerRequest {
    TriggerRequest {
        key: JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        },
        source: TriggerSource::AutoAdd,
        params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id: TransactionSyncRunId::new(),
            scope: automatic_add_sync_scope(target),
        }),
    }
}

pub(super) async fn enqueue_automatic_add_sync(
    user_id: crate::models::UserId,
    target: AutomaticSyncAddTarget,
) {
    if !should_enqueue_automatic_add_sync(sync_control_mode()) {
        tracing::info!(
            user_id = %user_id,
            target = ?target,
            "wallets: automatic add sync suppressed because sync control is enabled"
        );
        return;
    }

    if let Err(err) = ensure_started() {
        tracing::warn!(
            user_id = %user_id,
            error = %err,
            "wallets: failed to start task manager for automatic add sync"
        );
        return;
    }

    let request = automatic_add_trigger_request(user_id, target);
    let requested_scope = match request.params {
        TriggerParams::UserTransactionMonitor(params) => params.scope,
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => TransactionSyncScope::User,
    };
    match enqueue_trigger(request).await {
        TriggerEnqueueResult::AcceptedStarted { run_id } => {
            tracing::info!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                run_id = ?run_id,
                "wallets: automatic add sync started"
            );
        }
        TriggerEnqueueResult::AcceptedQueued { run_id } => {
            tracing::info!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                run_id = ?run_id,
                "wallets: automatic add sync queued"
            );
        }
        TriggerEnqueueResult::RejectedInvalidKey => {
            tracing::warn!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                "wallets: automatic add sync rejected because the task key was invalid"
            );
        }
        TriggerEnqueueResult::RejectedShuttingDown => {
            tracing::warn!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                "wallets: automatic add sync rejected because the task manager is unavailable"
            );
        }
    }
}

pub(super) fn map_move_account_db_error(error: MoveAccountDbError) -> WalletError {
    match error {
        MoveAccountDbError::AccountNotFound => not_found_error("Account not found"),
        MoveAccountDbError::TargetWalletNotFound => not_found_error("Target wallet not found"),
        MoveAccountDbError::AlreadyInTargetWallet => {
            let mut errors = FieldErrors::new();
            errors.add(
                "destination",
                "Account is already in the selected wallet".to_string(),
            );
            validation_error(errors)
        }
        MoveAccountDbError::Conflict(WalletDbConflict::WalletLabel) => {
            single_field_conflict_error("destination.label", "Wallet label already exists")
        }
        MoveAccountDbError::Conflict(WalletDbConflict::AccountLabelInWallet) => {
            single_field_conflict_error("label", "Account label already exists in this wallet")
        }
        MoveAccountDbError::Conflict(WalletDbConflict::ExtendedPubkey) => {
            single_field_conflict_error(
                "extended_pubkey",
                "This extended public key is already linked to a wallet",
            )
        }
        MoveAccountDbError::Conflict(WalletDbConflict::AddressAlreadyLinked) => {
            single_field_conflict_error("address", "This address is already linked to a wallet")
        }
        MoveAccountDbError::Internal(message) => {
            tracing::error!(
                error = %message,
                "wallets: move account db error"
            );
            WalletError::internal()
        }
    }
}

pub(super) fn map_link_trezor_db_error(error: LinkTrezorDbError) -> WalletError {
    match error {
        LinkTrezorDbError::MultiWalletAffinityConflict => single_field_conflict_error(
            "accounts",
            "Selected accounts are already linked across multiple wallets. Link accounts from one wallet at a time.",
        ),
        LinkTrezorDbError::MasterFingerprintConflict => single_field_conflict_error(
            "master_fingerprint",
            "Selected accounts are bound to a wallet with a different master fingerprint.",
        ),
        LinkTrezorDbError::Conflict(WalletDbConflict::WalletLabel) => {
            single_field_conflict_error("wallet_label", "Wallet label already exists")
        }
        LinkTrezorDbError::Conflict(WalletDbConflict::AccountLabelInWallet) => {
            single_field_conflict_error("label", "Account label already exists in this wallet")
        }
        LinkTrezorDbError::Conflict(WalletDbConflict::ExtendedPubkey) => {
            single_field_conflict_error(
                "extended_pubkey",
                "This extended public key is already linked to a wallet",
            )
        }
        LinkTrezorDbError::Conflict(WalletDbConflict::AddressAlreadyLinked) => {
            single_field_conflict_error("address", "This address is already linked to a wallet")
        }
        LinkTrezorDbError::Internal(message) if is_supported_account_hard_cap_error(&message) => {
            single_field_validation_error("accounts", "Supported account hard cap exceeded")
        }
        LinkTrezorDbError::Internal(message) => {
            tracing::error!(
                error = %message,
                "wallets: link trezor db error"
            );
            WalletError::internal()
        }
    }
}

pub(super) fn single_field_validation_error(
    field: &str,
    message: impl Into<String>,
) -> WalletError {
    let mut errors = FieldErrors::new();
    errors.add(field, message.into());
    validation_error(errors)
}

pub(super) fn single_field_conflict_error(field: &str, message: impl Into<String>) -> WalletError {
    let mut errors = FieldErrors::new();
    errors.add(field, message.into());
    conflict_error(errors)
}

pub(super) fn map_wallet_db_error(
    error: crate::db::DbError,
    wallet_label_field: &str,
) -> WalletError {
    let message = error.to_string();
    if message.contains("wallet_label is required when creating a wallet") {
        return single_field_validation_error(wallet_label_field, "Wallet label is required");
    }
    if is_supported_account_hard_cap_error(&message) {
        return single_field_validation_error(
            "account_limit",
            "Supported account hard cap exceeded",
        );
    }

    match classify_wallet_db_conflict(&error) {
        Some(WalletDbConflict::WalletLabel) => {
            single_field_conflict_error(wallet_label_field, "Wallet label already exists")
        }
        Some(WalletDbConflict::AccountLabelInWallet) => {
            single_field_conflict_error("label", "Account label already exists in this wallet")
        }
        Some(WalletDbConflict::ExtendedPubkey) => single_field_conflict_error(
            "extended_pubkey",
            "This extended public key is already linked to a wallet",
        ),
        Some(WalletDbConflict::AddressAlreadyLinked) => {
            single_field_conflict_error("address", "This address is already linked to a wallet")
        }
        None => {
            tracing::error!(
                error = %message,
                "wallets: unclassified wallet db error"
            );
            WalletError::internal()
        }
    }
}

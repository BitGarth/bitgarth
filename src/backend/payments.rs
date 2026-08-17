use super::ApiErrorEnvelope;
#[cfg(feature = "server")]
use super::session_context::{
    InitializedSession, require_initialized_session, require_session_token,
};
#[cfg(feature = "server")]
use crate::db::entitlement_snapshots::AppEntitlementSnapshotSource;
#[cfg(feature = "server")]
use crate::payments::client::{
    CentralOrderPayment, CentralOrderStatusOutcome, CentralProductOptions, CentralTierCapabilities,
};
#[cfg(feature = "server")]
use crate::payments::keys::VerifiedEntitlementToken;
#[cfg(feature = "server")]
use crate::payments::types::{
    CentralOrderStatus, CentralOrderVerificationState, EntitlementCapabilities,
    FeatureEntitlements, PaymentOrderId, PaymentOrderStatus, ProductOptionId, ProductTier,
    payment_state_status_from_order,
};
#[cfg(feature = "server")]
use crate::payments::views::{
    AdditionalPaymentView, PaymentOrderHistoryView, PaymentOrderStatusView, PaymentStateStatus,
    PaymentSummaryView, PaymentSupportReferenceView,
};
#[cfg(feature = "server")]
use crate::payments::views::{
    AppCompatibilityStatusView, AppCompatibilityView, PaymentOptionView, PaymentTierView,
};
use crate::payments::views::{
    PaymentCatalogView, PaymentStateView, PremiumOrderLaunchView, PremiumTopUpLaunchView,
};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::{DateTime, Duration, Utc};
#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use std::str::FromStr;

pub(crate) type PaymentError = ApiErrorEnvelope;

#[cfg(feature = "server")]
const ENTITLEMENT_REFRESH_STALE_AFTER: Duration = Duration::hours(24);

#[cfg(feature = "server")]
fn should_enqueue_upgrade_backfill(
    previous_backfill_enabled: bool,
    new_backfill_enabled: bool,
) -> bool {
    !previous_backfill_enabled && new_backfill_enabled
}

#[cfg(feature = "server")]
fn upgrade_backfill_trigger_request(
    user_id: crate::models::UserId,
) -> crate::tasks::TriggerRequest {
    crate::tasks::TriggerRequest {
        key: crate::tasks::JobKey::User {
            job_id: crate::tasks::JobId::UserTransactionMonitor,
            user_id,
        },
        source: crate::tasks::TriggerSource::AutoUpgrade,
        params: crate::tasks::TriggerParams::UserTransactionMonitor(
            crate::tasks::UserTransactionMonitorParams {
                run_id: crate::transactions::TransactionSyncRunId::new(),
                scope: crate::transactions::TransactionSyncScope::User,
            },
        ),
    }
}

#[cfg(feature = "server")]
async fn enqueue_upgrade_backfill_sync(user_id: crate::models::UserId) {
    if !crate::tasks::automatic_sync::should_enqueue_automatic_add_sync(
        crate::sync_control::sync_control_mode(),
    ) {
        tracing::info!(
            user_id = %user_id,
            "payments: upgrade backfill sync suppressed because sync control is enabled"
        );
        return;
    }
    if let Err(err) = crate::tasks::ensure_started() {
        tracing::warn!(
            user_id = %user_id,
            error = %err,
            "payments: failed to start task manager for upgrade backfill sync"
        );
        return;
    }
    match crate::tasks::enqueue_trigger(upgrade_backfill_trigger_request(user_id)).await {
        crate::tasks::TriggerEnqueueResult::AcceptedStarted { run_id }
        | crate::tasks::TriggerEnqueueResult::AcceptedQueued { run_id } => {
            tracing::info!(
                user_id = %user_id,
                run_id = ?run_id,
                "payments: upgrade backfill sync enqueued"
            );
        }
        outcome => {
            tracing::warn!(
                user_id = %user_id,
                outcome = ?outcome,
                "payments: upgrade backfill sync rejected"
            );
        }
    }
}

#[cfg(feature = "server")]
async fn store_premium_token_and_maybe_enqueue_backfill(
    user_id: crate::models::UserId,
    order_id: Option<PaymentOrderId>,
    verified: &VerifiedEntitlementToken,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), PaymentError> {
    let activated = crate::db::payments::store_verified_premium_token_with_activation_transition(
        user_id, order_id, verified, paid_at, now,
    )
    .map_err(|err| internal_error("store_verified_premium_token", err))?;
    if should_enqueue_upgrade_backfill(
        !activated,
        verified.entitlements.historical_backfill_enabled,
    ) {
        enqueue_upgrade_backfill_sync(user_id).await;
    }
    Ok(())
}

#[cfg(feature = "server")]
fn unauthorized_error(message: String) -> PaymentError {
    PaymentError::unauthorized(message)
}

#[cfg(feature = "server")]
fn bad_request(message: impl Into<String>) -> PaymentError {
    PaymentError::bad_request(message)
}

#[cfg(feature = "server")]
fn not_found(message: impl Into<String>) -> PaymentError {
    PaymentError::not_found(message)
}

#[cfg(feature = "server")]
fn internal_error(context: &str, detail: impl std::fmt::Display) -> PaymentError {
    tracing::error!(
        context,
        error = %detail,
        "payments: internal failure"
    );
    PaymentError::internal()
}

#[cfg(feature = "server")]
fn central_error(
    context: &'static str,
    user_id: crate::models::UserId,
    entitlement_holder_id: String,
    token_id: String,
    error: crate::payments::client::CentralClientError,
) -> PaymentError {
    const UNKNOWN_ID: &str = "unknown";
    let entitlement_holder_id = if entitlement_holder_id.is_empty() {
        UNKNOWN_ID
    } else {
        entitlement_holder_id.as_str()
    };
    let token_id = if token_id.is_empty() {
        UNKNOWN_ID
    } else {
        token_id.as_str()
    };

    if error.is_upgrade_required() {
        tracing::warn!(
            context,
            user_id = %user_id,
            entitlement_holder_id = %entitlement_holder_id,
            token_id = %token_id,
            error = %error,
            "payments: Central requires app upgrade"
        );
        return bad_request("BitGarth needs an update before paid plans can be managed.");
    }

    tracing::warn!(
        context,
        user_id = %user_id,
        entitlement_holder_id = %entitlement_holder_id,
        token_id = %token_id,
        error = %error,
        "payments: Central request failed"
    );
    bad_request("Could not reach BitGarth payment service.")
}

#[cfg(feature = "server")]
fn record_verified_entitlement_snapshot(
    user_id: crate::models::UserId,
    source: AppEntitlementSnapshotSource,
    verified: &VerifiedEntitlementToken,
    now: DateTime<Utc>,
) {
    if let Err(err) = crate::db::entitlement_snapshots::record_verified_app_entitlement_snapshot(
        user_id, source, verified, now,
    ) {
        tracing::error!(
            user_id = %user_id,
            token_id = %verified.claims.token_id.to_storage_value(),
            entitlement_tier = verified.claims.tier.as_str(),
            source = source.as_str(),
            error = %err,
            "payments: failed to record app entitlement snapshot"
        );
    }
}

#[cfg(feature = "server")]
fn require_payment_session(cookies: &CookieJar) -> Result<InitializedSession, PaymentError> {
    let session_token = require_session_token("payments", cookies, unauthorized_error)?;
    require_initialized_session("payments", &session_token, unauthorized_error, |message| {
        internal_error("require_initialized_session", message)
    })
}

#[cfg(feature = "server")]
fn default_payment_state(
    status: PaymentStateStatus,
    entitlements: FeatureEntitlements,
    message: Option<String>,
) -> PaymentStateView {
    payment_state_for_entitlements(status, entitlements, message)
}

#[cfg(feature = "server")]
fn payment_state_for_entitlements(
    status: PaymentStateStatus,
    entitlements: FeatureEntitlements,
    message: Option<String>,
) -> PaymentStateView {
    PaymentStateView {
        status,
        tier: entitlements.tier.as_str().to_string(),
        tier_display_name: entitlements.tier.display_name(),
        sync_account_slots_limit: entitlements.sync_account_slots_limit,
        historical_backfill_enabled: entitlements.historical_backfill_enabled,
        historical_backfill_transactions_per_account: entitlements
            .historical_backfill_transactions_per_account,
        order_id: None,
        paid_through: entitlements
            .subscription_valid_until
            .map(|value| value.to_rfc3339()),
        display_amount: None,
        currency: None,
        message,
        payment_summary: None,
        additional_payment: None,
        support_reference: None,
    }
}

#[cfg(feature = "server")]
fn support_reference_for_verified(
    verified: &VerifiedEntitlementToken,
    order_id: Option<PaymentOrderId>,
) -> PaymentSupportReferenceView {
    PaymentSupportReferenceView {
        token_id: Some(verified.claims.token_id.to_storage_value()),
        subscription_subject_id: Some(verified.claims.subscription_subject_id.to_storage_value()),
        entitlement_holder_id: verified.claims.entitlement_holder_id.to_storage_value(),
        order_id: order_id.map(|value| value.to_storage_value()),
    }
}

#[cfg(feature = "server")]
fn support_reference_for_order(
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    order_id: PaymentOrderId,
) -> PaymentSupportReferenceView {
    PaymentSupportReferenceView {
        token_id: None,
        subscription_subject_id: None,
        entitlement_holder_id: entitlement_holder_id.to_storage_value(),
        order_id: Some(order_id.to_storage_value()),
    }
}

#[cfg(feature = "server")]
fn active_payment_state(
    verified: &VerifiedEntitlementToken,
    order_id: Option<PaymentOrderId>,
) -> PaymentStateView {
    let mut state = payment_state_for_entitlements(
        PaymentStateStatus::Active,
        verified.entitlements.clone(),
        None,
    );
    state.support_reference = Some(support_reference_for_verified(verified, order_id));
    state
}

#[cfg(feature = "server")]
fn active_payment_state_with_sync_warning(
    verified: &VerifiedEntitlementToken,
    order_id: Option<PaymentOrderId>,
) -> PaymentStateView {
    let mut state = payment_state_for_entitlements(
        PaymentStateStatus::ActiveWithSyncWarning,
        verified.entitlements.clone(),
        Some("Central sync issue. Your local subscription is valid, but BitGarth could not verify the latest token state with Central.".to_string()),
    );
    state.support_reference = Some(support_reference_for_verified(verified, order_id));
    state
}

#[cfg(feature = "server")]
fn has_stale_wiped_token_state(subject: &crate::db::payments::PaymentSubjectRecord) -> bool {
    subject.management_secret.is_some()
        && subject.active_token_history_id.is_none()
        && subject.last_successful_capability_refresh_at.is_some()
}

#[cfg(feature = "server")]
fn recently_failed_recovery(
    subject: &crate::db::payments::PaymentSubjectRecord,
    now: DateTime<Utc>,
) -> bool {
    subject.last_refresh_status.as_deref() == Some("recovery_failed")
        && subject
            .last_refresh_at
            .is_some_and(|last| now - last < ENTITLEMENT_REFRESH_STALE_AFTER)
}

#[cfg(feature = "server")]
fn support_reference_for_entitlement_holder(
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
) -> PaymentSupportReferenceView {
    PaymentSupportReferenceView {
        token_id: None,
        subscription_subject_id: None,
        entitlement_holder_id: entitlement_holder_id.to_storage_value(),
        order_id: None,
    }
}

#[cfg(feature = "server")]
fn recovery_failed_payment_state(
    subject: &crate::db::payments::PaymentSubjectRecord,
    now: DateTime<Utc>,
) -> PaymentStateView {
    let mut state = default_payment_state(
        PaymentStateStatus::RecoveryFailed,
        crate::payments::free_tier::resolve_free_entitlements(now),
        Some(
            "BitGarth found a previous paid subscription on this device, but could not restore it from Central right now.".to_string(),
        ),
    );
    state.support_reference = Some(support_reference_for_entitlement_holder(
        subject.entitlement_holder_id,
    ));
    state
}

#[cfg(feature = "server")]
enum RecoveryOutcome {
    NotNeeded,
    Throttled,
    Recovered(Box<VerifiedEntitlementToken>),
    NoActiveToken,
}

#[cfg(feature = "server")]
async fn attempt_wiped_token_recovery(
    user_id: crate::models::UserId,
    subject: &crate::db::payments::PaymentSubjectRecord,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    let Some(management_secret) = subject.management_secret.as_ref() else {
        return Ok(default_payment_state(
            PaymentStateStatus::NotActive,
            crate::payments::free_tier::resolve_free_entitlements(now),
            None,
        ));
    };
    match recover_wiped_token_if_needed(
        user_id,
        subject,
        management_secret,
        now,
        AppEntitlementSnapshotSource::PaymentReconcile,
    )
    .await
    {
        Ok(RecoveryOutcome::Recovered(verified)) => {
            let verified = *verified;
            let latest_order = crate::db::payments::load_latest_payment_order(user_id)
                .map_err(|err| internal_error("load_latest_payment_order", err))?;
            Ok(active_payment_state(
                &verified,
                latest_order.map(|order| order.order_id),
            ))
        }
        Ok(RecoveryOutcome::Throttled) => Ok(recovery_failed_payment_state(subject, now)),
        Ok(RecoveryOutcome::NotNeeded | RecoveryOutcome::NoActiveToken) => {
            crate::db::payments::record_payment_refresh_status(user_id, "recovery_failed", now)
                .map_err(|err| internal_error("record_payment_refresh_status", err))?;
            Ok(recovery_failed_payment_state(subject, now))
        }
        Err(err) => {
            tracing::debug!(error = %err, "payments: automatic recovery failed");
            crate::db::payments::record_payment_refresh_status(user_id, "error", now)
                .map_err(|err| internal_error("record_payment_refresh_status", err))?;
            Ok(recovery_failed_payment_state(subject, now))
        }
    }
}

#[cfg(feature = "server")]
async fn recover_wiped_token_if_needed(
    user_id: crate::models::UserId,
    subject: &crate::db::payments::PaymentSubjectRecord,
    management_secret: &crate::payments::types::PaymentSecret,
    now: DateTime<Utc>,
    snapshot_source: AppEntitlementSnapshotSource,
) -> Result<RecoveryOutcome, PaymentError> {
    if !has_stale_wiped_token_state(subject) {
        return Ok(RecoveryOutcome::NotNeeded);
    }
    if recently_failed_recovery(subject, now) {
        return Ok(RecoveryOutcome::Throttled);
    }

    match reconcile_payment_history_from_central(
        user_id,
        subject.entitlement_holder_id,
        management_secret,
        now,
        snapshot_source,
    )
    .await
    {
        Ok(Some(verified)) => Ok(RecoveryOutcome::Recovered(Box::new(verified))),
        Ok(None) => Ok(RecoveryOutcome::NoActiveToken),
        Err(err) => Err(err),
    }
}

#[cfg(feature = "server")]
fn should_refresh_entitlements(
    subject: &crate::db::payments::PaymentSubjectRecord,
    active_history: Option<&crate::db::payments::PaymentTokenHistoryRecord>,
    now: DateTime<Utc>,
) -> bool {
    if subject.management_secret.is_none() || subject.active_token_history_id.is_none() {
        return false;
    }

    let Some(active_history) = active_history else {
        return false;
    };

    if active_history.token_expires_at <= now {
        return true;
    }

    subject
        .last_capability_refresh_at
        .is_some_and(|last_refresh_at| now - last_refresh_at >= ENTITLEMENT_REFRESH_STALE_AFTER)
}

#[cfg(feature = "server")]
#[derive(serde::Deserialize)]
struct StoredEntitlementCapabilities {
    capabilities: EntitlementCapabilities,
}

#[cfg(feature = "server")]
fn token_capabilities_from_history(
    active_history: &crate::db::payments::PaymentTokenHistoryRecord,
) -> Option<EntitlementCapabilities> {
    serde_json::from_str::<StoredEntitlementCapabilities>(
        active_history.capabilities_json.as_ref()?,
    )
    .ok()
    .map(|stored| stored.capabilities)
}

#[cfg(feature = "server")]
fn catalog_capabilities_are_greater(
    catalog: &CentralTierCapabilities,
    active_history: &crate::db::payments::PaymentTokenHistoryRecord,
) -> bool {
    let Some(token_capabilities) = token_capabilities_from_history(active_history) else {
        return false;
    };
    catalog.sync_account_slots
        > token_capabilities.account_limit_for_schema(active_history.capability_schema_version)
        || catalog.historical_backfill_transactions_per_account
            > token_capabilities
                .transaction_limit_for_schema(active_history.capability_schema_version)
        || (catalog.transaction_history_sync
            && !token_capabilities
                .transaction_history_enabled_for_schema(active_history.capability_schema_version))
        || (catalog.balance_sync && !token_capabilities.features.balance_sync)
        || (catalog.exchange_rates_current && !token_capabilities.features.exchange_rates_current)
        || (catalog.exchange_rates_history && !token_capabilities.features.exchange_rates_history)
        || (catalog.price_overrides && !token_capabilities.features.price_overrides)
        || (catalog.balance_assertions && !token_capabilities.features.balance_assertions)
        || (catalog.hledger_export && !token_capabilities.features.hledger_export)
        || (catalog.tax_reports && !token_capabilities.features.tax_reports)
}

#[cfg(feature = "server")]
fn catalog_supersedes_active_token(
    product_options: &CentralProductOptions,
    active_history: &crate::db::payments::PaymentTokenHistoryRecord,
) -> bool {
    let Some(catalog_tier) = product_options
        .tiers
        .iter()
        .find(|tier| tier.tier == active_history.entitlement_tier.as_str())
    else {
        return false;
    };
    let catalog = &catalog_tier.capabilities;
    if catalog.capability_schema_version > active_history.capability_schema_version {
        return true;
    }
    if catalog.capability_set_id.is_some()
        && catalog.capability_set_id != active_history.capability_set_id
    {
        return true;
    }
    catalog_capabilities_are_greater(catalog, active_history)
}

#[cfg(feature = "server")]
fn should_refresh_entitlements_with_catalog(
    subject: &crate::db::payments::PaymentSubjectRecord,
    active_history: Option<&crate::db::payments::PaymentTokenHistoryRecord>,
    product_options: Option<&CentralProductOptions>,
    now: DateTime<Utc>,
) -> bool {
    if should_refresh_entitlements(subject, active_history, now) {
        return true;
    }
    if subject.management_secret.is_none() || subject.active_token_history_id.is_none() {
        return false;
    }
    let Some(active_history) = active_history else {
        return false;
    };
    product_options.is_some_and(|options| catalog_supersedes_active_token(options, active_history))
}

#[cfg(feature = "server")]
async fn refresh_entitlement_state_from_central(
    user_id: crate::models::UserId,
    subject: &crate::db::payments::PaymentSubjectRecord,
    active_history: Option<&crate::db::payments::PaymentTokenHistoryRecord>,
    now: DateTime<Utc>,
    snapshot_source: AppEntitlementSnapshotSource,
) -> Result<PaymentStateView, PaymentError> {
    let management_secret = subject
        .management_secret
        .as_ref()
        .ok_or_else(|| bad_request("Plan status cannot be refreshed yet."))?;
    let active_token_history_id = subject
        .active_token_history_id
        .ok_or_else(|| bad_request("Plan status cannot be refreshed yet."))?;
    let Some(history) = active_history else {
        tracing::warn!(
            token_id = %active_token_history_id.to_storage_value(),
            "payments: active token pointer had no matching history row; refusing Central refresh"
        );
        return Ok(default_payment_state(
            PaymentStateStatus::Unavailable,
            crate::payments::free_tier::resolve_free_entitlements(now),
            Some("Could not verify plan status.".to_string()),
        ));
    };
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let last_known_token = if history.active_token.trim().is_empty() {
        None
    } else {
        Some(history.active_token.clone())
    };
    let outcome = client
        .refresh_subscription(
            subject.entitlement_holder_id,
            active_token_history_id,
            management_secret,
            last_known_token,
        )
        .await
        .map_err(|err| {
            central_error(
                "refresh_subscription",
                user_id,
                subject.entitlement_holder_id.to_storage_value(),
                active_token_history_id.to_storage_value(),
                err,
            )
        })?;

    match outcome {
        crate::payments::client::CentralRefreshOutcome::Active {
            premium_access_token,
            token_id,
            subscription_valid_until,
            token_expires_at,
        } => {
            let verified = crate::payments::keys::verify_premium_token(
                &premium_access_token,
                subject.entitlement_holder_id,
                now,
            )
            .map_err(|err| internal_error("verify_premium_token", err))?;
            if verified.claims.token_id != token_id
                || verified.claims.subscription_valid_until != subscription_valid_until
                || verified.claims.token_expires_at != token_expires_at
            {
                return Err(internal_error(
                    "verify_premium_token",
                    "Central token metadata did not match signed claims",
                ));
            }

            store_premium_token_and_maybe_enqueue_backfill(user_id, None, &verified, None, now)
                .await?;
            record_verified_entitlement_snapshot(user_id, snapshot_source, &verified, now);
            crate::db::payments::record_payment_refresh_status(user_id, "active", now)
                .map_err(|err| internal_error("record_payment_refresh_status", err))?;
            let latest_order = crate::db::payments::load_latest_payment_order(user_id)
                .map_err(|err| internal_error("load_latest_payment_order", err))?;
            Ok(active_payment_state(
                &verified,
                latest_order.map(|order| order.order_id),
            ))
        }
        crate::payments::client::CentralRefreshOutcome::Revoked {
            reason: crate::payments::types::RefreshRevokedReason::TokenSuperseded,
        } => handle_token_superseded(user_id, subject, active_history, now).await,
        crate::payments::client::CentralRefreshOutcome::Revoked { reason } => {
            tracing::debug!(reason = ?reason, "payments: Central revoked entitlement refresh");
            crate::db::payments::clear_verified_premium_token(
                user_id,
                crate::db::payments::TokenHistoryStatus::Revoked,
                None,
                "revoked",
                now,
            )
            .map_err(|err| internal_error("clear_verified_premium_token", err))?;
            Ok(default_payment_state(
                PaymentStateStatus::NotActive,
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
            ))
        }
    }
}

#[cfg(feature = "server")]
async fn handle_token_superseded(
    user_id: crate::models::UserId,
    subject: &crate::db::payments::PaymentSubjectRecord,
    active_history: Option<&crate::db::payments::PaymentTokenHistoryRecord>,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    tracing::debug!("payments: Central reported token_superseded; checking local token");
    let Some(history) = active_history else {
        crate::db::payments::clear_verified_premium_token(
            user_id,
            crate::db::payments::TokenHistoryStatus::Revoked,
            None,
            "revoked",
            now,
        )
        .map_err(|err| internal_error("clear_verified_premium_token", err))?;
        return Ok(default_payment_state(
            PaymentStateStatus::NotActive,
            crate::payments::free_tier::resolve_free_entitlements(now),
            None,
        ));
    };
    let verified = match crate::payments::keys::verify_premium_token(
        &history.active_token,
        subject.entitlement_holder_id,
        now,
    ) {
        Ok(verified) => verified,
        Err(error) => {
            tracing::debug!(error = %error, "payments: local token not usable after token_superseded");
            crate::db::payments::clear_verified_premium_token(
                user_id,
                crate::db::payments::TokenHistoryStatus::Revoked,
                None,
                "revoked",
                now,
            )
            .map_err(|err| internal_error("clear_verified_premium_token", err))?;
            return Ok(default_payment_state(
                PaymentStateStatus::NotActive,
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
            ));
        }
    };

    let Some(management_secret) = subject.management_secret.as_ref() else {
        crate::db::payments::record_payment_refresh_status(user_id, "sync_warning", now)
            .map_err(|err| internal_error("record_payment_refresh_status", err))?;
        let latest_order = crate::db::payments::load_latest_payment_order(user_id)
            .map_err(|err| internal_error("load_latest_payment_order", err))?;
        return Ok(active_payment_state_with_sync_warning(
            &verified,
            latest_order.map(|order| order.order_id),
        ));
    };

    let reconcile_outcome = reconcile_payment_history_from_central(
        user_id,
        subject.entitlement_holder_id,
        management_secret,
        now,
        AppEntitlementSnapshotSource::Refresh,
    )
    .await;

    match reconcile_outcome {
        Ok(Some(reconciled)) => {
            let latest_order = crate::db::payments::load_latest_payment_order(user_id)
                .map_err(|err| internal_error("load_latest_payment_order", err))?;
            Ok(active_payment_state(
                &reconciled,
                latest_order.map(|order| order.order_id),
            ))
        }
        Ok(None) | Err(_) => {
            crate::db::payments::record_payment_refresh_status(user_id, "sync_warning", now)
                .map_err(|err| internal_error("record_payment_refresh_status", err))?;
            let latest_order = crate::db::payments::load_latest_payment_order(user_id)
                .map_err(|err| internal_error("load_latest_payment_order", err))?;
            Ok(active_payment_state_with_sync_warning(
                &verified,
                latest_order.map(|order| order.order_id),
            ))
        }
    }
}

#[cfg(feature = "server")]
async fn refresh_stale_entitlement_state(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
    snapshot_source: AppEntitlementSnapshotSource,
    product_options: Option<&CentralProductOptions>,
) -> Result<Option<PaymentStateView>, PaymentError> {
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err))?;
    let Some(subject) = subject else {
        return Ok(None);
    };
    let active_history = crate::db::payments::load_active_token_history(user_id)
        .map_err(|err| internal_error("load_active_token_history", err))?;
    if !should_refresh_entitlements_with_catalog(
        &subject,
        active_history.as_ref(),
        product_options,
        now,
    ) {
        return Ok(None);
    }

    match refresh_entitlement_state_from_central(
        user_id,
        &subject,
        active_history.as_ref(),
        now,
        snapshot_source,
    )
    .await
    {
        Ok(state) => Ok(Some(state)),
        Err(err) => {
            if let Err(record_err) =
                crate::db::payments::record_payment_refresh_status(user_id, "error", now)
            {
                tracing::warn!(
                    error = %record_err,
                    "payments: failed to record entitlement refresh failure"
                );
            }
            Err(err)
        }
    }
}

#[cfg(feature = "server")]
async fn refresh_entitlements_after_login(user_id: crate::models::UserId) {
    let now = Utc::now();
    let client = match crate::payments::client::BitGarthCentralClient::new(user_id) {
        Ok(client) => Some(client),
        Err(err) => {
            tracing::debug!(
                error = %err,
                "payments: free tier login refresh client build failed"
            );
            None
        }
    };
    let product_options = if let Some(client) = client {
        match client.payment_product_options().await {
            Ok(options) => {
                crate::payments::free_tier::record_free_tier_from_product_options(
                    &options,
                    Utc::now(),
                );
                Some(options)
            }
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "payments: free tier login refresh unavailable"
                );
                None
            }
        }
    } else {
        None
    };

    if let Err(err) = refresh_stale_entitlement_state(
        user_id,
        now,
        AppEntitlementSnapshotSource::LoginRefresh,
        product_options.as_ref(),
    )
    .await
    {
        tracing::debug!(
            user_id = %user_id,
            error = %err,
            "payments: login entitlement refresh failed"
        );
    }

    let recovery_subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err));
    let Ok(Some(recovery_subject)) = recovery_subject else {
        return;
    };
    let Some(management_secret) = recovery_subject.management_secret.as_ref() else {
        return;
    };
    match recover_wiped_token_if_needed(
        user_id,
        &recovery_subject,
        management_secret,
        now,
        AppEntitlementSnapshotSource::LoginRefresh,
    )
    .await
    {
        Ok(RecoveryOutcome::Recovered(_))
        | Ok(RecoveryOutcome::NotNeeded)
        | Ok(RecoveryOutcome::Throttled) => {}
        Ok(RecoveryOutcome::NoActiveToken) => {
            if let Err(record_err) =
                crate::db::payments::record_payment_refresh_status(user_id, "recovery_failed", now)
            {
                tracing::warn!(
                    error = %record_err,
                    "payments: failed to record wiped-token recovery no-active-token"
                );
            }
        }
        Err(err) => {
            tracing::debug!(
                user_id = %user_id,
                error = %err,
                "payments: login wiped-token recovery failed"
            );
            if let Err(record_err) =
                crate::db::payments::record_payment_refresh_status(user_id, "error", now)
            {
                tracing::warn!(
                    error = %record_err,
                    "payments: failed to record wiped-token recovery failure"
                );
            }
        }
    }
}

#[cfg(feature = "server")]
pub(crate) fn refresh_entitlements_after_login_in_background(user_id: crate::models::UserId) {
    crate::runtime_context::spawn_with_current_runtime_context(async move {
        refresh_entitlements_after_login(user_id).await;
    });
}

#[cfg(feature = "server")]
fn order_payment_state_with(
    order: &crate::db::payments::PaymentOrderRecord,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    status: PaymentStateStatus,
    entitlements: FeatureEntitlements,
    message: Option<String>,
    payment_summary: Option<PaymentSummaryView>,
    additional_payment: Option<AdditionalPaymentView>,
) -> PaymentStateView {
    let mut state = payment_state_for_entitlements(status, entitlements, message);
    state.order_id = Some(order.order_id.to_storage_value());
    state.paid_through = None;
    state.display_amount = Some(order.amount.atlos_decimal_amount());
    state.currency = Some(order.amount.currency.clone());
    state.payment_summary = payment_summary;
    state.additional_payment = additional_payment;
    state.support_reference = Some(support_reference_for_order(
        entitlement_holder_id,
        order.order_id,
    ));
    state
}

#[cfg(feature = "server")]
fn free_order_payment_state_with(
    order: &crate::db::payments::PaymentOrderRecord,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    status: PaymentStateStatus,
    entitlements: FeatureEntitlements,
    message: Option<String>,
    payment_summary: Option<PaymentSummaryView>,
    additional_payment: Option<AdditionalPaymentView>,
) -> PaymentStateView {
    order_payment_state_with(
        order,
        entitlement_holder_id,
        status,
        entitlements,
        message,
        payment_summary,
        additional_payment,
    )
}

#[cfg(feature = "server")]
fn payment_summary(payments: &[CentralOrderPayment]) -> Option<PaymentSummaryView> {
    let latest = payments.first()?;
    Some(PaymentSummaryView {
        paid_order_amount: latest.paid_order_amount.atlos_decimal_amount(),
        paid_order_currency: latest.paid_order_amount.currency.clone(),
        paid_asset_amount: latest
            .paid_asset_amount
            .as_ref()
            .map(|asset| asset.amount.clone()),
        paid_asset_code: latest
            .paid_asset_amount
            .as_ref()
            .and_then(|asset| asset.asset_code.clone()),
        blockchain_hash: latest.blockchain_hash.clone(),
        confirmed_at: latest.confirmed_at.map(|ts| ts.to_rfc3339()),
    })
}

#[cfg(feature = "server")]
fn manual_review_message(reason: &str) -> String {
    match reason {
        "amount_mismatch" => {
            "The payment was received, but it did not match the expected order amount exactly."
                .to_string()
        }
        _ => {
            "The payment was received, but BitGarth could not verify it automatically.".to_string()
        }
    }
}

#[cfg(feature = "server")]
fn non_paid_central_order_state(
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    order: &crate::db::payments::PaymentOrderRecord,
    outcome: &CentralOrderStatusOutcome,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    let payment_summary = payment_summary(&outcome.payments);
    let free_entitlements = crate::payments::free_tier::resolve_free_entitlements(now);
    match outcome.verification_state {
        CentralOrderVerificationState::AwaitingPayment => Ok(free_order_payment_state_with(
            order,
            entitlement_holder_id,
            payment_state_status_from_order(order.status),
            free_entitlements,
            None,
            None,
            None,
        )),
        CentralOrderVerificationState::PaymentConfirmedUnverified => {
            Ok(free_order_payment_state_with(
                order,
                entitlement_holder_id,
                PaymentStateStatus::Verifying,
                free_entitlements,
                None,
                payment_summary,
                None,
            ))
        }
        CentralOrderVerificationState::AdditionalPaymentRequired => {
            let remaining_amount = outcome.remaining_amount.as_ref().ok_or_else(|| {
                internal_error(
                    "order_status",
                    "additional payment outcome missing remaining_amount",
                )
            })?;
            let paid_amount = outcome
                .paid_amount_minor_units
                .map(|minor_units| {
                    crate::payments::types::format_minor_units(
                        minor_units,
                        remaining_amount.decimal_precision,
                    )
                })
                .unwrap_or_else(|| "0".to_string());
            Ok(free_order_payment_state_with(
                order,
                entitlement_holder_id,
                PaymentStateStatus::AdditionalPaymentRequired,
                free_entitlements,
                Some(
                    "The received payment was short. Pay the remaining amount to unlock the selected plan."
                        .to_string(),
                ),
                payment_summary,
                Some(AdditionalPaymentView {
                    paid_amount,
                    paid_currency: remaining_amount.currency.clone(),
                    remaining_amount: remaining_amount.atlos_decimal_amount(),
                    remaining_currency: remaining_amount.currency.clone(),
                }),
            ))
        }
        CentralOrderVerificationState::UnderManualReview => {
            let manual_review = outcome.manual_review.as_ref().ok_or_else(|| {
                internal_error(
                    "order_status",
                    "manual review outcome missing manual_review summary",
                )
            })?;
            Ok(free_order_payment_state_with(
                order,
                entitlement_holder_id,
                PaymentStateStatus::ManualReview,
                free_entitlements,
                Some(manual_review_message(&manual_review.reason)),
                payment_summary,
                None,
            ))
        }
        CentralOrderVerificationState::PremiumGranted => Err(internal_error(
            "order_status",
            "non-paid Central order unexpectedly reported premium_granted",
        )),
    }
}

#[cfg(feature = "server")]
async fn refresh_order_state_from_central(
    user_id: crate::models::UserId,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    order: &crate::db::payments::PaymentOrderRecord,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let outcome = client
        .order_status(order.order_id, &order.order_secret)
        .await
        .map_err(|err| {
            central_error(
                "order_status",
                user_id,
                entitlement_holder_id.to_storage_value(),
                "".to_string(),
                err,
            )
        })?;

    apply_central_order_outcome(user_id, entitlement_holder_id, order, now, outcome).await
}

#[cfg(feature = "server")]
async fn apply_central_order_outcome(
    user_id: crate::models::UserId,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    order: &crate::db::payments::PaymentOrderRecord,
    now: DateTime<Utc>,
    outcome: CentralOrderStatusOutcome,
) -> Result<PaymentStateView, PaymentError> {
    match outcome.status {
        CentralOrderStatus::Pending => {
            if order.status != PaymentOrderStatus::Pending {
                crate::db::payments::mark_payment_order_status(
                    user_id,
                    order.order_id,
                    PaymentOrderStatus::Pending,
                    None,
                    now,
                )
                .map_err(|err| internal_error("mark_payment_order_status", err))?;
            }
            let updated = crate::db::payments::load_payment_order(user_id, order.order_id)
                .map_err(|err| internal_error("load_payment_order", err))?
                .ok_or_else(|| not_found("Payment order not found"))?;
            non_paid_central_order_state(entitlement_holder_id, &updated, &outcome, now)
        }
        CentralOrderStatus::Expired => order_status_state(
            user_id,
            entitlement_holder_id,
            order.order_id,
            PaymentOrderStatus::Expired,
            now,
            Some(&outcome),
        ),
        CentralOrderStatus::Failed => order_status_state(
            user_id,
            entitlement_holder_id,
            order.order_id,
            PaymentOrderStatus::Failed,
            now,
            Some(&outcome),
        ),
        CentralOrderStatus::Paid => {
            let paid_details = outcome.paid_details.ok_or_else(|| {
                internal_error("order_status", "paid Central order missing paid_details")
            })?;
            let verified = crate::payments::keys::verify_premium_token(
                &paid_details.premium_access_token,
                entitlement_holder_id,
                now,
            )
            .map_err(|err| internal_error("verify_premium_token", err))?;
            if verified.claims.token_id != paid_details.token_id
                || verified.claims.subscription_valid_until != paid_details.subscription_valid_until
                || verified.claims.token_expires_at != paid_details.token_expires_at
            {
                return Err(internal_error(
                    "verify_premium_token",
                    "Central token metadata did not match signed claims",
                ));
            }

            store_premium_token_and_maybe_enqueue_backfill(
                user_id,
                Some(order.order_id),
                &verified,
                Some(paid_details.paid_at),
                now,
            )
            .await?;
            record_verified_entitlement_snapshot(
                user_id,
                AppEntitlementSnapshotSource::PaymentPoll,
                &verified,
                now,
            );
            Ok(active_payment_state(&verified, Some(order.order_id)))
        }
    }
}

/// Outcome of resolving payment state from local data alone.
///
/// The `Resolved` variant is fully derived from the local database. The
/// other two variants mark the points where the Central-allowed
/// [`build_payment_state`] would contact Central; the local-only
/// [`build_payment_state_local`] substitutes a last-known status instead.
#[cfg(feature = "server")]
enum LocalPaymentStateOutcome {
    Resolved(Box<PaymentStateView>),
    WipedTokenRecovery {
        subject: crate::db::payments::PaymentSubjectRecord,
        latest_order: Option<crate::db::payments::PaymentOrderRecord>,
    },
    InFlightOrderRefresh {
        subject: crate::db::payments::PaymentSubjectRecord,
        order: crate::db::payments::PaymentOrderRecord,
    },
}

#[cfg(feature = "server")]
impl LocalPaymentStateOutcome {
    fn resolved(state: PaymentStateView) -> Self {
        Self::Resolved(Box::new(state))
    }
}

#[cfg(feature = "server")]
fn resolve_local_payment_state(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
) -> Result<LocalPaymentStateOutcome, PaymentError> {
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err))?;
    let Some(subject) = subject else {
        return Ok(LocalPaymentStateOutcome::resolved(default_payment_state(
            PaymentStateStatus::NotActive,
            crate::payments::free_tier::resolve_free_entitlements(now),
            None,
        )));
    };

    // TODO: refactor to load history once (Task 9)
    let active_history = crate::db::payments::load_active_token_history(user_id)
        .map_err(|err| internal_error("load_active_token_history", err))?;
    if let Some(history) = active_history {
        match crate::payments::keys::verify_premium_token(
            &history.active_token,
            subject.entitlement_holder_id,
            now,
        ) {
            Ok(verified) => {
                let latest_order = crate::db::payments::load_latest_payment_order(user_id)
                    .map_err(|err| internal_error("load_latest_payment_order", err))?;
                return Ok(LocalPaymentStateOutcome::resolved(active_payment_state(
                    &verified,
                    latest_order.map(|order| order.order_id),
                )));
            }
            Err(crate::payments::keys::PremiumTokenError::SubscriptionExpired) => {
                return Ok(LocalPaymentStateOutcome::resolved(default_payment_state(
                    PaymentStateStatus::NotActive,
                    crate::payments::free_tier::resolve_free_entitlements(now),
                    None,
                )));
            }
            Err(crate::payments::keys::PremiumTokenError::TokenExpired)
                if subject.management_secret.is_some()
                    && subject.active_token_history_id.is_some() =>
            {
                return Ok(LocalPaymentStateOutcome::resolved(default_payment_state(
                    PaymentStateStatus::Unavailable,
                    crate::payments::free_tier::resolve_free_entitlements(now),
                    Some("Could not refresh plan status.".to_string()),
                )));
            }
            Err(error) => {
                tracing::warn!(error = %error, "payments: stored premium token is not usable");
                return Ok(LocalPaymentStateOutcome::resolved(default_payment_state(
                    PaymentStateStatus::Unavailable,
                    crate::payments::free_tier::resolve_free_entitlements(now),
                    Some("Could not verify plan status.".to_string()),
                )));
            }
        }
    }

    let latest_order = crate::db::payments::load_latest_payment_order(user_id)
        .map_err(|err| internal_error("load_latest_payment_order", err))?;

    if has_stale_wiped_token_state(&subject) {
        return Ok(LocalPaymentStateOutcome::WipedTokenRecovery {
            subject,
            latest_order,
        });
    }

    if let Some(order) = &latest_order
        && order.status != PaymentOrderStatus::Paid
        && order.status != PaymentOrderStatus::Canceled
    {
        let order = order.clone();
        return Ok(LocalPaymentStateOutcome::InFlightOrderRefresh { subject, order });
    }

    if let Some(order) = latest_order
        && order.status == PaymentOrderStatus::Canceled
    {
        return Ok(LocalPaymentStateOutcome::resolved(
            free_order_payment_state_with(
                &order,
                subject.entitlement_holder_id,
                payment_state_status_from_order(order.status),
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
                None,
                None,
            ),
        ));
    }

    Ok(LocalPaymentStateOutcome::resolved(default_payment_state(
        PaymentStateStatus::NotActive,
        crate::payments::free_tier::resolve_free_entitlements(now),
        None,
    )))
}

#[cfg(feature = "server")]
async fn build_payment_state(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    match resolve_local_payment_state(user_id, now)? {
        LocalPaymentStateOutcome::Resolved(state) => Ok(*state),
        LocalPaymentStateOutcome::WipedTokenRecovery {
            subject,
            latest_order,
        } => {
            let recovery = attempt_wiped_token_recovery(user_id, &subject, now).await;
            if let Ok(state) = &recovery
                && state.status != PaymentStateStatus::RecoveryFailed
            {
                return recovery;
            }
            // Recovery failed. If a canceled order exists, show it instead.
            if let Some(order) = latest_order.as_ref()
                && order.status == PaymentOrderStatus::Canceled
            {
                return Ok(free_order_payment_state_with(
                    order,
                    subject.entitlement_holder_id,
                    payment_state_status_from_order(order.status),
                    crate::payments::free_tier::resolve_free_entitlements(now),
                    None,
                    None,
                    None,
                ));
            }
            recovery
        }
        LocalPaymentStateOutcome::InFlightOrderRefresh { subject, order } => {
            refresh_order_state_from_central(user_id, subject.entitlement_holder_id, &order, now)
                .await
        }
    }
}

/// Plan state derived only from local data — never contacts Central.
///
/// Used by the SSR path so the payments page renders the user's last-known
/// status instantly. For the wiped-token and in-flight-order cases (which the
/// Central-allowed [`build_payment_state`] refreshes against Central), this
/// returns the last persisted local status; a post-paint client refresh
/// reconciles it with Central afterwards.
#[cfg(feature = "server")]
fn build_payment_state_local(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    match resolve_local_payment_state(user_id, now)? {
        LocalPaymentStateOutcome::Resolved(state) => Ok(*state),
        LocalPaymentStateOutcome::WipedTokenRecovery { subject, .. } => {
            Ok(recovery_failed_payment_state(&subject, now))
        }
        LocalPaymentStateOutcome::InFlightOrderRefresh { subject, order } => {
            Ok(free_order_payment_state_with(
                &order,
                subject.entitlement_holder_id,
                payment_state_status_from_order(order.status),
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
                None,
                None,
            ))
        }
    }
}

#[cfg(feature = "server")]
fn paid_order_cancel_state(
    user_id: crate::models::UserId,
    subject: &crate::db::payments::PaymentSubjectRecord,
    order: &crate::db::payments::PaymentOrderRecord,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    let active_history = crate::db::payments::load_active_token_history(user_id)
        .map_err(|err| internal_error("load_active_token_history", err))?;
    let Some(history) = active_history else {
        return Ok(free_order_payment_state_with(
            order,
            subject.entitlement_holder_id,
            PaymentStateStatus::Unavailable,
            crate::payments::free_tier::resolve_free_entitlements(now),
            Some("Could not verify plan status.".to_string()),
            None,
            None,
        ));
    };
    match crate::payments::keys::verify_premium_token(
        &history.active_token,
        subject.entitlement_holder_id,
        now,
    ) {
        Ok(verified) => Ok(active_payment_state(&verified, Some(order.order_id))),
        Err(crate::payments::keys::PremiumTokenError::SubscriptionExpired) => {
            Ok(free_order_payment_state_with(
                order,
                subject.entitlement_holder_id,
                PaymentStateStatus::NotActive,
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
                None,
                None,
            ))
        }
        Err(error) => {
            tracing::warn!(error = %error, "payments: stored premium token is not usable");
            Ok(free_order_payment_state_with(
                order,
                subject.entitlement_holder_id,
                PaymentStateStatus::Unavailable,
                crate::payments::free_tier::resolve_free_entitlements(now),
                Some("Could not verify plan status.".to_string()),
                None,
                None,
            ))
        }
    }
}

#[cfg(feature = "server")]
fn map_product_options(
    product_options: crate::payments::client::CentralProductOptions,
) -> Result<PaymentPageViewParts, PaymentError> {
    let pricing_summary = product_options
        .pricing_summary
        .as_deref()
        .map(crate::payments::views::parse_bullet);
    let tiers = product_options
        .tiers
        .into_iter()
        .map(|tier| PaymentTierView {
            tier: tier.tier,
            display_name: tier.display_name,
            summary: tier.presentation.summary,
            bullets: tier
                .presentation
                .bullets
                .iter()
                .map(|raw| crate::payments::views::parse_bullet(raw))
                .collect(),
            is_featured: tier.presentation.is_featured,
            ribbon_label: tier.presentation.ribbon_label,
        })
        .collect();
    let options = product_options
        .options
        .into_iter()
        .filter_map(|option| {
            if option.tier != ProductTier::Basic.as_str()
                && option.tier != ProductTier::Premium.as_str()
            {
                tracing::info!(
                    product_option_id = %option.id,
                    product_tier = option.tier,
                    "payments: ignoring unsupported Central product option tier"
                );
                return None;
            }
            Some(option)
        })
        .map(|option| {
            let currency_symbol = option.price.currency_symbol.clone().ok_or_else(|| {
                internal_error(
                    "map_product_options",
                    format!("product option {} missing currency symbol", option.id),
                )
            })?;
            Ok(PaymentOptionView {
                id: option.id.to_string(),
                tier: option.tier,
                tier_display_name: option.tier_display_name,
                term_quantity: Some(option.term_quantity),
                term_unit: Some(option.term_unit),
                term_label: option.term_label,
                minor_units: option.price.minor_units,
                decimal_precision: option.price.decimal_precision,
                display_amount: option.price.atlos_decimal_amount(),
                currency: option.price.currency,
                currency_symbol,
                is_default: option.is_default,
            })
        })
        .collect::<Result<Vec<_>, PaymentError>>()?;
    let app_compatibility =
        product_options
            .app_compatibility
            .map(|compatibility| AppCompatibilityView {
                status: match compatibility.status {
                    crate::payments::client::CentralAppCompatibilityStatus::UpgradeRequired => {
                        AppCompatibilityStatusView::UpgradeRequired
                    }
                },
                detail: compatibility.detail,
                minimum_app_version: compatibility.minimum_app_version,
            });
    let options_message = if options.is_empty() {
        Some("Price unavailable. Could not load current payment options.".to_string())
    } else {
        None
    };

    Ok(PaymentPageViewParts {
        tiers,
        options,
        app_compatibility,
        options_message,
        pricing_summary,
    })
}

#[cfg(feature = "server")]
fn payment_order_status_view(status: PaymentOrderStatus) -> PaymentOrderStatusView {
    match status {
        PaymentOrderStatus::Pending => PaymentOrderStatusView::Pending,
        PaymentOrderStatus::Paid => PaymentOrderStatusView::Paid,
        PaymentOrderStatus::Expired => PaymentOrderStatusView::Expired,
        PaymentOrderStatus::Failed => PaymentOrderStatusView::Failed,
        PaymentOrderStatus::Canceled => PaymentOrderStatusView::Canceled,
    }
}

#[cfg(feature = "server")]
fn payment_order_history_view(
    order: crate::db::payments::PaymentOrderHistoryRecord,
) -> PaymentOrderHistoryView {
    PaymentOrderHistoryView {
        order_id: order.order_id.to_storage_value(),
        product_tier: order.product_tier.as_str().to_string(),
        display_amount: order.amount.atlos_decimal_amount(),
        currency: order.amount.currency,
        status: payment_order_status_view(order.status),
        paid_at: order.paid_at.map(|value| value.to_rfc3339()),
    }
}

#[cfg(feature = "server")]
struct PaymentPageViewParts {
    tiers: Vec<PaymentTierView>,
    options: Vec<PaymentOptionView>,
    app_compatibility: Option<AppCompatibilityView>,
    options_message: Option<String>,
    pricing_summary: Option<crate::payments::views::TierBulletView>,
}

#[cfg(feature = "server")]
async fn load_payment_page_options(
    user_id: crate::models::UserId,
) -> Result<PaymentPageViewParts, PaymentError> {
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    match client.payment_product_options().await {
        Ok(options) => {
            crate::payments::free_tier::record_free_tier_from_product_options(&options, Utc::now());
            map_product_options(options)
        }
        Err(error) => {
            if error.is_upgrade_required() {
                tracing::warn!(error = %error, "payments: product options require app upgrade");
                return Ok(PaymentPageViewParts {
                    tiers: Vec::new(),
                    options: Vec::new(),
                    app_compatibility: Some(AppCompatibilityView {
                        status: AppCompatibilityStatusView::UpgradeRequired,
                        detail:
                            "BitGarth needs an update before paid plans can be purchased or refreshed."
                                .to_string(),
                        minimum_app_version: None,
                    }),
                    options_message: Some(
                        "BitGarth needs an update before paid plans can be purchased or refreshed."
                            .to_string(),
                    ),
                    pricing_summary: None,
                });
            }

            tracing::warn!(error = %error, "payments: product options unavailable");
            Ok(PaymentPageViewParts {
                tiers: Vec::new(),
                options: Vec::new(),
                app_compatibility: None,
                options_message: Some(
                    "Price unavailable. Could not reach BitGarth payment service.".to_string(),
                ),
                pricing_summary: None,
            })
        }
    }
}

/// Resolves payment state with a Central refresh: refreshes a stale
/// entitlement snapshot, then falls back to [`build_payment_state`] (which
/// itself contacts Central for in-flight orders and wiped-token recovery).
///
/// Called after first paint by the payments page so the SSR-rendered
/// last-known status is reconciled with Central without blocking render.
#[cfg(feature = "server")]
async fn build_refreshed_payment_state(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
) -> Result<PaymentStateView, PaymentError> {
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let product_options = match client.payment_product_options().await {
        Ok(options) => {
            crate::payments::free_tier::record_free_tier_from_product_options(&options, Utc::now());
            Some(options)
        }
        Err(error) => {
            tracing::debug!(error = %error, "payments: product options unavailable during refresh");
            None
        }
    };
    let refreshed_state = match refresh_stale_entitlement_state(
        user_id,
        now,
        AppEntitlementSnapshotSource::PaymentsRefresh,
        product_options.as_ref(),
    )
    .await
    {
        Ok(state) => state,
        Err(err) => {
            tracing::debug!(error = %err, "payments: stale entitlement refresh failed");
            None
        }
    };
    match refreshed_state {
        Some(state) => Ok(state),
        None => build_payment_state(user_id, now).await,
    }
}

#[get("/_app/user/payments/state", cookies: CookieJar)]
pub(crate) async fn get_payment_state() -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    build_payment_state(user_id, Utc::now()).await
}

#[get("/_app/user/payments/state-refresh", cookies: CookieJar)]
pub(crate) async fn refresh_payment_state() -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    build_refreshed_payment_state(user_id, Utc::now()).await
}

#[cfg(feature = "server")]
async fn build_payment_catalog_view(
    user_id: crate::models::UserId,
) -> Result<PaymentCatalogView, PaymentError> {
    let options = load_payment_page_options(user_id).await?;
    let order_history = crate::db::payments::load_all_payment_order_history(user_id)
        .map_err(|err| internal_error("load_all_payment_order_history", err))?
        .into_iter()
        .map(payment_order_history_view)
        .collect();
    Ok(PaymentCatalogView {
        tiers: options.tiers,
        options: options.options,
        app_compatibility: options.app_compatibility,
        options_message: options.options_message,
        order_history,
        pricing_summary: options.pricing_summary,
    })
}

#[get("/_app/user/payments/state-local", cookies: CookieJar)]
pub(crate) async fn get_payment_state_local() -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    build_payment_state_local(user_id, Utc::now())
}

#[get("/_app/user/payments/catalog", cookies: CookieJar)]
pub(crate) async fn get_payment_catalog() -> Result<PaymentCatalogView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    build_payment_catalog_view(user_id).await
}

#[post("/_app/user/payments/premium/start", cookies: CookieJar)]
pub(crate) async fn start_premium_order(
    product_option_id: String,
) -> Result<PremiumOrderLaunchView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let product_option_id = ProductOptionId::from_str(&product_option_id)
        .map_err(|err| bad_request(format!("Invalid product option id: {err}")))?;
    match build_payment_state(user_id, now).await? {
        PaymentStateView {
            status: PaymentStateStatus::ManualReview,
            ..
        } => {
            return Err(bad_request(
                "This payment is under manual review. Wait before starting another checkout.",
            ));
        }
        PaymentStateView {
            status: PaymentStateStatus::Verifying,
            ..
        }
        | PaymentStateView {
            status: PaymentStateStatus::Pending,
            ..
        } => {
            return Err(bad_request(
                "BitGarth is still checking the current payment.",
            ));
        }
        PaymentStateView {
            status: PaymentStateStatus::AdditionalPaymentRequired,
            ..
        } => {
            return Err(bad_request(
                "Pay the remaining amount before starting another checkout.",
            ));
        }
        _ => {}
    }
    let subject = crate::db::payments::load_or_create_payment_subject(user_id, now)
        .map_err(|err| internal_error("load_or_create_payment_subject", err))?;
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let product_options = client.payment_product_options().await.map_err(|err| {
        central_error(
            "payment_product_options",
            user_id,
            subject.entitlement_holder_id.to_storage_value(),
            "".to_string(),
            err,
        )
    })?;
    crate::payments::free_tier::record_free_tier_from_product_options(&product_options, Utc::now());
    if let Some(app_compatibility) = product_options.app_compatibility
        && app_compatibility.status
            == crate::payments::client::CentralAppCompatibilityStatus::UpgradeRequired
    {
        return Err(bad_request(app_compatibility.detail));
    }
    let selected_option = product_options
        .options
        .into_iter()
        .filter(|option| {
            option.tier == ProductTier::Basic.as_str()
                || option.tier == ProductTier::Premium.as_str()
        })
        .find(|option| option.id == product_option_id)
        .ok_or_else(|| {
            bad_request(
                "The selected paid option is no longer available. Refresh the page and try again.",
            )
        })?;
    let selected_tier = ProductTier::from_str(&selected_option.tier)
        .map_err(|err| internal_error("selected_product_tier", err))?;
    let session = client
        .create_order_session(
            subject.entitlement_holder_id,
            selected_option.id,
            subject.management_secret.as_ref(),
        )
        .await
        .map_err(|err| {
            central_error(
                "create_order_session",
                user_id,
                subject.entitlement_holder_id.to_storage_value(),
                "".to_string(),
                err,
            )
        })?;

    if let Some(management_secret) = &session.management_secret {
        crate::db::payments::update_payment_management_secret(user_id, management_secret, now)
            .map_err(|err| internal_error("update_payment_management_secret", err))?;
    }

    crate::db::payments::insert_payment_order(
        user_id,
        &crate::db::payments::NewPaymentOrder {
            order_id: session.order_id,
            order_secret: session.order_secret.clone(),
            product_tier: selected_tier,
            amount: session.order_amount.clone(),
        },
        now,
    )
    .map_err(|err| internal_error("insert_payment_order", err))?;

    let mut state = default_payment_state(
        PaymentStateStatus::Pending,
        crate::payments::free_tier::resolve_free_entitlements(now),
        None,
    );
    state.order_id = Some(session.order_id.to_storage_value());
    state.display_amount = Some(session.order_amount.atlos_decimal_amount());
    state.currency = Some(session.order_amount.currency.clone());
    state.support_reference = Some(support_reference_for_order(
        subject.entitlement_holder_id,
        session.order_id,
    ));

    let payment_attempt = session.payment_attempt;
    let payment_attempt_amount = payment_attempt.amount;

    Ok(PremiumOrderLaunchView {
        state,
        merchant_id: session.merchant_id,
        central_order_id: session.order_id.to_storage_value(),
        atlos_order_id: payment_attempt.atlos_order_id,
        order_amount: payment_attempt_amount.atlos_decimal_amount(),
        order_currency: payment_attempt_amount.currency,
    })
}

#[post("/_app/user/payments/premium/top-up", cookies: CookieJar)]
pub(crate) async fn start_premium_top_up(
    central_order_id: String,
) -> Result<PremiumTopUpLaunchView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let order_id = PaymentOrderId::from_str(&central_order_id)
        .map_err(|err| bad_request(format!("Invalid payment order id: {err}")))?;
    let subject = crate::db::payments::load_or_create_payment_subject(user_id, now)
        .map_err(|err| internal_error("load_or_create_payment_subject", err))?;
    let order = crate::db::payments::load_payment_order(user_id, order_id)
        .map_err(|err| internal_error("load_payment_order", err))?
        .ok_or_else(|| not_found("Payment order not found"))?;
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let outcome = client
        .order_status(order.order_id, &order.order_secret)
        .await
        .map_err(|err| {
            central_error(
                "order_status",
                user_id,
                subject.entitlement_holder_id.to_storage_value(),
                "".to_string(),
                err,
            )
        })?;
    let next_action = outcome.next_action;
    let additional_payment_request = outcome.additional_payment_request.clone();
    let state =
        apply_central_order_outcome(user_id, subject.entitlement_holder_id, &order, now, outcome)
            .await?;

    if next_action != crate::payments::types::CentralOrderNextAction::RequestAdditionalPayment {
        return Ok(PremiumTopUpLaunchView {
            state,
            launch: None,
        });
    }

    let Some(additional_payment_request) = additional_payment_request else {
        return Err(internal_error(
            "order_status",
            "request_additional_payment missing launch request",
        ));
    };
    let amount = additional_payment_request.amount;
    Ok(PremiumTopUpLaunchView {
        state: state.clone(),
        launch: Some(PremiumOrderLaunchView {
            state,
            merchant_id: additional_payment_request.merchant_id,
            central_order_id: order.order_id.to_storage_value(),
            atlos_order_id: additional_payment_request.atlos_order_id,
            order_amount: amount.atlos_decimal_amount(),
            order_currency: amount.currency,
        }),
    })
}

#[post("/_app/user/payments/premium/cancel", cookies: CookieJar)]
pub(crate) async fn cancel_premium_order(
    order_id: String,
) -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let order_id = PaymentOrderId::from_str(&order_id)
        .map_err(|err| bad_request(format!("Invalid payment order id: {err}")))?;
    let order = crate::db::payments::load_payment_order(user_id, order_id)
        .map_err(|err| internal_error("load_payment_order", err))?
        .ok_or_else(|| not_found("Payment order not found"))?;
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err))?
        .ok_or_else(|| not_found("Payment subject not found"))?;

    match order.status {
        PaymentOrderStatus::Pending => {
            crate::db::payments::cancel_payment_order(user_id, order_id, now)
                .map_err(|err| internal_error("cancel_payment_order", err))?;
            let updated = crate::db::payments::load_payment_order(user_id, order_id)
                .map_err(|err| internal_error("load_payment_order", err))?
                .ok_or_else(|| not_found("Payment order not found"))?;
            Ok(free_order_payment_state_with(
                &updated,
                subject.entitlement_holder_id,
                payment_state_status_from_order(updated.status),
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
                None,
                None,
            ))
        }
        PaymentOrderStatus::Canceled => Ok(free_order_payment_state_with(
            &order,
            subject.entitlement_holder_id,
            payment_state_status_from_order(order.status),
            crate::payments::free_tier::resolve_free_entitlements(now),
            None,
            None,
            None,
        )),
        PaymentOrderStatus::Paid => paid_order_cancel_state(user_id, &subject, &order, now),
        PaymentOrderStatus::Failed | PaymentOrderStatus::Expired => {
            Ok(free_order_payment_state_with(
                &order,
                subject.entitlement_holder_id,
                payment_state_status_from_order(order.status),
                crate::payments::free_tier::resolve_free_entitlements(now),
                None,
                None,
                None,
            ))
        }
    }
}

#[post("/_app/user/payments/premium/poll", cookies: CookieJar)]
pub(crate) async fn poll_premium_order(order_id: String) -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let order_id = PaymentOrderId::from_str(&order_id)
        .map_err(|err| bad_request(format!("Invalid payment order id: {err}")))?;
    let subject = crate::db::payments::load_or_create_payment_subject(user_id, now)
        .map_err(|err| internal_error("load_or_create_payment_subject", err))?;
    let order = crate::db::payments::load_payment_order(user_id, order_id)
        .map_err(|err| internal_error("load_payment_order", err))?
        .ok_or_else(|| not_found("Payment order not found"))?;
    refresh_order_state_from_central(user_id, subject.entitlement_holder_id, &order, now).await
}

#[cfg(feature = "server")]
fn order_status_state(
    user_id: crate::models::UserId,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    order_id: PaymentOrderId,
    status: PaymentOrderStatus,
    now: DateTime<Utc>,
    outcome: Option<&CentralOrderStatusOutcome>,
) -> Result<PaymentStateView, PaymentError> {
    crate::db::payments::mark_payment_order_status(user_id, order_id, status, None, now)
        .map_err(|err| internal_error("mark_payment_order_status", err))?;
    let order = crate::db::payments::load_payment_order(user_id, order_id)
        .map_err(|err| internal_error("load_payment_order", err))?
        .ok_or_else(|| not_found("Payment order not found"))?;
    if let Some(outcome) = outcome {
        return non_paid_central_order_state(entitlement_holder_id, &order, outcome, now);
    }
    Ok(free_order_payment_state_with(
        &order,
        entitlement_holder_id,
        payment_state_status_from_order(order.status),
        crate::payments::free_tier::resolve_free_entitlements(now),
        None,
        None,
        None,
    ))
}

#[post("/_app/user/payments/premium/reconcile", cookies: CookieJar)]
pub(crate) async fn reconcile_payment_history() -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err))?
        .ok_or_else(|| not_found("Payment subject not found"))?;
    let management_secret = subject
        .management_secret
        .as_ref()
        .ok_or_else(|| bad_request("Payment history cannot be reconciled yet."))?;
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let outcome = client
        .subscription_history(management_secret)
        .await
        .map_err(|err| {
            central_error(
                "subscription_history",
                user_id,
                subject.entitlement_holder_id.to_storage_value(),
                "".to_string(),
                err,
            )
        })?;

    let crate::payments::client::CentralHistoryOutcome::History {
        orders,
        premium_access_token,
        token_id,
        subscription_valid_until,
        token_expires_at,
    } = outcome;

    // If history includes paid entitlement data, verify and store the token.
    if let (Some(token), Some(token_id), Some(valid_until), Some(expires_at)) = (
        &premium_access_token,
        token_id,
        subscription_valid_until,
        token_expires_at,
    ) {
        let verified =
            crate::payments::keys::verify_premium_token(token, subject.entitlement_holder_id, now)
                .map_err(|err| internal_error("verify_premium_token", err))?;
        if verified.claims.token_id != token_id
            || verified.claims.subscription_valid_until != valid_until
            || verified.claims.token_expires_at != expires_at
        {
            return Err(internal_error(
                "verify_premium_token",
                "History token metadata did not match signed claims",
            ));
        }
        store_premium_token_and_maybe_enqueue_backfill(user_id, None, &verified, None, now).await?;
        record_verified_entitlement_snapshot(
            user_id,
            AppEntitlementSnapshotSource::PaymentReconcile,
            &verified,
            now,
        );
    }

    // Reconcile known local/imported orders from history.
    let known_orders = crate::db::payments::load_all_payment_order_history(user_id)
        .map_err(|err| internal_error("load_all_payment_order_history", err))?;
    let known_order_ids: std::collections::HashSet<PaymentOrderId> =
        known_orders.iter().map(|o| o.order_id).collect();

    for history_order in &orders {
        if !known_order_ids.contains(&history_order.order_id) {
            continue;
        }
        let reconciled_status = match history_order.status {
            CentralOrderStatus::Paid => PaymentOrderStatus::Paid,
            CentralOrderStatus::Failed => PaymentOrderStatus::Failed,
            CentralOrderStatus::Expired => PaymentOrderStatus::Expired,
            CentralOrderStatus::Pending => continue,
        };
        crate::db::payments::reconcile_payment_order_status(
            user_id,
            history_order.order_id,
            reconciled_status,
            history_order.paid_at,
            now,
        )
        .map_err(|err| internal_error("reconcile_payment_order_status", err))?;
        crate::db::payments::reconcile_imported_payment_order_history_status(
            user_id,
            history_order.order_id,
            reconciled_status,
            history_order.paid_at,
            now,
        )
        .map_err(|err| internal_error("reconcile_imported_payment_order_history_status", err))?;
    }

    build_payment_state(user_id, now).await
}

#[cfg(feature = "server")]
async fn reconcile_payment_history_from_central(
    user_id: crate::models::UserId,
    entitlement_holder_id: crate::payments::types::EntitlementHolderId,
    management_secret: &crate::payments::types::PaymentSecret,
    now: DateTime<Utc>,
    snapshot_source: AppEntitlementSnapshotSource,
) -> Result<Option<VerifiedEntitlementToken>, PaymentError> {
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| internal_error("central_client", err))?;
    let outcome = client
        .subscription_history(management_secret)
        .await
        .map_err(|err| {
            central_error(
                "subscription_history",
                user_id,
                entitlement_holder_id.to_storage_value(),
                "".to_string(),
                err,
            )
        })?;

    let crate::payments::client::CentralHistoryOutcome::History {
        premium_access_token,
        token_id,
        subscription_valid_until,
        token_expires_at,
        ..
    } = outcome;

    let (Some(token), Some(token_id), Some(valid_until), Some(expires_at)) = (
        &premium_access_token,
        token_id,
        subscription_valid_until,
        token_expires_at,
    ) else {
        return Ok(None);
    };

    let verified = crate::payments::keys::verify_premium_token(token, entitlement_holder_id, now)
        .map_err(|err| internal_error("verify_premium_token", err))?;
    if verified.claims.token_id != token_id
        || verified.claims.subscription_valid_until != valid_until
        || verified.claims.token_expires_at != expires_at
    {
        return Err(internal_error(
            "verify_premium_token",
            "History token metadata did not match signed claims",
        ));
    }
    store_premium_token_and_maybe_enqueue_backfill(user_id, None, &verified, None, now).await?;
    record_verified_entitlement_snapshot(user_id, snapshot_source, &verified, now);
    crate::db::payments::record_payment_refresh_status(user_id, "active", now)
        .map_err(|err| internal_error("record_payment_refresh_status", err))?;
    Ok(Some(verified))
}

#[post("/_app/user/payments/premium/refresh", cookies: CookieJar)]
pub(crate) async fn refresh_premium_status() -> Result<PaymentStateView, PaymentError> {
    let initialized_session = require_payment_session(&cookies)?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| internal_error("load_payment_subject", err))?
        .ok_or_else(|| not_found("Payment subject not found"))?;
    let active_history = crate::db::payments::load_active_token_history(user_id)
        .map_err(|err| internal_error("load_active_token_history", err))?;
    refresh_entitlement_state_from_central(
        user_id,
        &subject,
        active_history.as_ref(),
        now,
        AppEntitlementSnapshotSource::Refresh,
    )
    .await
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::db::payments::{
        PaymentSubjectRecord, PaymentTokenHistoryRecord, TokenHistoryStatus,
    };
    use crate::payments::client::{
        CentralProductOptions, CentralProductTier, CentralTierCapabilities,
    };
    use crate::payments::types::{
        AccountLimits, EntitlementCapabilityLimits, EntitlementFeatureFlags, EntitlementHolderId,
        EntitlementTier, HistoryLimits, ProductTier, SubscriptionSubjectId, TokenId,
        entitlement_capabilities_storage_json,
    };

    #[test]
    fn upgrade_backfill_trigger_request_targets_user_scope() {
        let user_id = crate::models::UserId::new();
        let request = upgrade_backfill_trigger_request(user_id);
        assert_eq!(
            request.key,
            crate::tasks::JobKey::User {
                job_id: crate::tasks::JobId::UserTransactionMonitor,
                user_id,
            }
        );
        assert_eq!(request.source, crate::tasks::TriggerSource::AutoUpgrade);
        match request.params {
            crate::tasks::TriggerParams::UserTransactionMonitor(params) => {
                assert_eq!(
                    params.scope,
                    crate::transactions::TransactionSyncScope::User
                );
            }
            crate::tasks::TriggerParams::SessionCleanup(_)
            | crate::tasks::TriggerParams::TraceCleanup(_)
            | crate::tasks::TriggerParams::InactiveUserCleanup(_)
            | crate::tasks::TriggerParams::PriceHistoryReconciliation(_) => {
                panic!("expected sync params");
            }
        }
    }

    #[test]
    fn upgrade_backfill_enqueues_only_on_transition_into_backfill() {
        assert!(should_enqueue_upgrade_backfill(false, true));
        assert!(!should_enqueue_upgrade_backfill(true, true));
        assert!(!should_enqueue_upgrade_backfill(false, false));
        assert!(!should_enqueue_upgrade_backfill(true, false));
    }

    fn dt(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("test timestamp should parse")
            .with_timezone(&Utc)
    }

    fn subject() -> PaymentSubjectRecord {
        PaymentSubjectRecord {
            entitlement_holder_id: EntitlementHolderId::from_str("01JQABCDEF000000000000000A")
                .expect("holder id should parse"),
            management_secret: Some(
                crate::payments::types::PaymentSecret::from_raw(
                    "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI",
                )
                .expect("management secret should parse"),
            ),
            active_token_history_id: Some(
                TokenId::from_str("01JQABCDEF000000000000000B").expect("token id should parse"),
            ),
            last_refresh_at: Some(dt("2026-07-03T21:28:17Z")),
            last_refresh_status: Some("active".to_string()),
            last_capability_refresh_at: Some(dt("2026-07-03T21:28:17Z")),
            last_successful_capability_refresh_at: Some(dt("2026-07-03T21:28:17Z")),
        }
    }

    fn active_history(
        capability_set_id: &str,
        capability_schema_version: u16,
        sync_account_slots: u16,
        transaction_history_sync: bool,
    ) -> PaymentTokenHistoryRecord {
        let capabilities =
            if capability_schema_version == crate::payments::types::CAPABILITY_SCHEMA_VERSION_V3 {
                EntitlementCapabilities {
                    limits: EntitlementCapabilityLimits {
                        accounts: Some(AccountLimits {
                            total: sync_account_slots,
                        }),
                        synced_accounts: sync_account_slots,
                        history: HistoryLimits {
                            max_transactions_per_account: 10_000,
                        },
                    },
                    features: EntitlementFeatureFlags {
                        historical_sync: false,
                        transaction_history_sync,
                        balance_sync: true,
                        exchange_rates_current: false,
                        exchange_rates_history: false,
                        price_overrides: false,
                        balance_assertions: false,
                        hledger_export: false,
                        tax_reports: false,
                    },
                }
            } else {
                EntitlementCapabilities::legacy_from_parts(
                    sync_account_slots,
                    10_000,
                    transaction_history_sync,
                )
            };
        PaymentTokenHistoryRecord {
            token_id: TokenId::from_str("01JQABCDEF000000000000000B")
                .expect("token id should parse"),
            subscription_subject_id: SubscriptionSubjectId::from_str("01JQABCDEF000000000000000C")
                .expect("subject id should parse"),
            active_token: "token".to_string(),
            entitlement_tier: EntitlementTier::from_str(ProductTier::Basic.as_str())
                .expect("tier should parse"),
            subscription_valid_until: dt("2026-07-11T11:59:31Z"),
            token_expires_at: dt("2026-07-10T21:28:17Z"),
            token_issued_at: dt("2026-07-03T21:28:17Z"),
            capability_set_id: Some(capability_set_id.to_string()),
            capability_schema_version,
            capabilities_json: Some(
                entitlement_capabilities_storage_json(capability_schema_version, &capabilities)
                    .expect("capabilities should serialize"),
            ),
            status: TokenHistoryStatus::Active,
            status_reason: None,
            first_seen_at: dt("2026-07-03T21:28:17Z"),
            last_seen_at: dt("2026-07-03T21:28:17Z"),
            deactivated_at: None,
        }
    }

    fn product_options_for_basic(capabilities: CentralTierCapabilities) -> CentralProductOptions {
        CentralProductOptions {
            tiers: vec![CentralProductTier {
                tier: ProductTier::Basic.as_str().to_string(),
                display_name: "Basic".to_string(),
                capabilities,
                presentation: crate::payments::client::CentralTierPresentation {
                    summary: "Basic".to_string(),
                    bullets: Vec::new(),
                    is_featured: false,
                    ribbon_label: None,
                },
            }],
            options: Vec::new(),
            app_compatibility: None,
            pricing_summary: None,
        }
    }

    fn basic_catalog(
        capability_set_id: &str,
        capability_schema_version: u16,
        sync_account_slots: u16,
        transaction_history_sync: bool,
    ) -> CentralProductOptions {
        product_options_for_basic(CentralTierCapabilities {
            capability_set_id: Some(capability_set_id.to_string()),
            capability_schema_version,
            sync_account_slots,
            historical_backfill_transactions_per_account: 10_000,
            historical_sync: false,
            transaction_history_sync,
            balance_sync: true,
            exchange_rates_current: false,
            exchange_rates_history: false,
            price_overrides: false,
            balance_assertions: false,
            hledger_export: false,
            tax_reports: false,
        })
    }

    #[test]
    fn catalog_newer_schema_refreshes_before_twenty_four_hour_stale_window() {
        let now = dt("2026-07-03T22:28:17Z");
        let subject = subject();
        let history = active_history("basic.v2.grandfathered", 2, 200, false);
        let catalog = basic_catalog("basic.v3", 3, 200, true);

        assert!(should_refresh_entitlements_with_catalog(
            &subject,
            Some(&history),
            Some(&catalog),
            now,
        ));
    }

    #[test]
    fn matching_catalog_keeps_twenty_four_hour_refresh_gate() {
        let now = dt("2026-07-03T22:28:17Z");
        let subject = subject();
        let history = active_history("basic.v3", 3, 200, true);
        let catalog = basic_catalog("basic.v3", 3, 200, true);

        assert!(!should_refresh_entitlements_with_catalog(
            &subject,
            Some(&history),
            Some(&catalog),
            now,
        ));
    }

    #[test]
    fn catalog_with_higher_account_limit_refreshes_before_stale_window() {
        let now = dt("2026-07-03T22:28:17Z");
        let subject = subject();
        let history = active_history("basic.v3", 3, 30, true);
        let catalog = basic_catalog("basic.v3", 3, 200, true);

        assert!(should_refresh_entitlements_with_catalog(
            &subject,
            Some(&history),
            Some(&catalog),
            now,
        ));
    }
}

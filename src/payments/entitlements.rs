#![cfg(feature = "server")]

use super::free_tier::resolve_free_entitlements;
use super::keys::verify_premium_token;
use super::types::FeatureEntitlements;
use crate::db::DbError;
use crate::models::UserId;
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;

pub(crate) fn load_feature_entitlements(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<FeatureEntitlements, DbError> {
    let Some(subject) = crate::db::payments::load_payment_subject(user_id)? else {
        return Ok(resolve_free_entitlements(now));
    };
    // TODO: refactor to load history once (Task 8)
    let Some(history) = crate::db::payments::load_active_token_history(user_id)? else {
        return Ok(resolve_free_entitlements(now));
    };

    Ok(
        match verify_premium_token(&history.active_token, subject.entitlement_holder_id, now) {
            Ok(verified) => verified.entitlements,
            Err(error) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %error,
                    "payments: premium token verification failed; using free entitlements"
                );
                resolve_free_entitlements(now)
            }
        },
    )
}

use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HostedRetentionStatus {
    pub(crate) is_hosted: bool,
    pub(crate) active_paid: bool,
}

#[cfg(feature = "server")]
fn build_status(is_hosted: bool, active_paid: bool) -> HostedRetentionStatus {
    HostedRetentionStatus {
        is_hosted,
        // active_paid is only meaningful for hosted instances; collapse it to
        // false elsewhere so non-hosted clients never branch on stale data.
        active_paid: is_hosted && active_paid,
    }
}

#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::Utc;
#[cfg(feature = "server")]
use dioxus::logger::tracing;

pub(crate) type RetentionError = super::ApiErrorEnvelope;

#[cfg(feature = "server")]
fn current_user_id(cookies: &CookieJar) -> Option<crate::models::UserId> {
    let token = super::session_token::lookup_session_token("retention", cookies)?;
    match crate::auth::session::get_session_by_token(&token) {
        Ok(Some(session)) => Some(session.user_id),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(error = %err, "retention: session lookup failed; treating as anonymous");
            None
        }
    }
}

#[get("/_app/hosted/retention-status", cookies: CookieJar)]
pub(crate) async fn hosted_retention_status() -> Result<HostedRetentionStatus, RetentionError> {
    let is_hosted = crate::channel::channel() == crate::channel::Channel::Hosted;

    // Anonymous (registration) callers have no session → active_paid is false.
    let active_paid = match current_user_id(&cookies) {
        Some(user_id) if is_hosted => {
            crate::db::user_has_active_paid_entitlement(user_id, Utc::now()).unwrap_or(false)
        }
        _ => false,
    };

    Ok(build_status(is_hosted, active_paid))
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn build_status_zeroes_active_paid_when_not_hosted() {
        assert_eq!(
            build_status(false, true),
            HostedRetentionStatus {
                is_hosted: false,
                active_paid: false
            }
        );
    }

    #[test]
    fn build_status_preserves_active_paid_when_hosted() {
        assert_eq!(
            build_status(true, true),
            HostedRetentionStatus {
                is_hosted: true,
                active_paid: true
            }
        );
        assert_eq!(
            build_status(true, false),
            HostedRetentionStatus {
                is_hosted: true,
                active_paid: false
            }
        );
    }
}

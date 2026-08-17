use super::{
    ETHERSCAN_RATE_LIMIT_BACKOFF, LABEL_ETHERSCAN, MEMPOOL_RATE_LIMIT_BACKOFF_BASE,
    MEMPOOL_RATE_LIMIT_BACKOFF_MAX,
};
use crate::models::UserId;
use crate::transactions::SyncIntegrationId;
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub(super) struct RateLimitState {
    blocked_until: Instant,
    consecutive_hits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct RateLimitScopeKey {
    user_id: UserId,
    integration: String,
}

impl RateLimitScopeKey {
    fn new(user_id: UserId, integration: &str) -> Self {
        Self {
            user_id,
            integration: integration.to_string(),
        }
    }
}

pub(super) fn global_rate_limiter() -> &'static Mutex<HashMap<RateLimitScopeKey, RateLimitState>> {
    static GLOBAL_RATE_LIMITER: OnceLock<Mutex<HashMap<RateLimitScopeKey, RateLimitState>>> =
        OnceLock::new();
    GLOBAL_RATE_LIMITER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn scale_duration(duration: Duration, factor: u32) -> Duration {
    if factor == 0 {
        return Duration::ZERO;
    }
    let millis = duration.as_millis();
    let scaled = millis.saturating_mul(u128::from(factor));
    Duration::from_millis(u64::try_from(scaled).unwrap_or(u64::MAX))
}

fn capped_mempool_rate_limit_backoff(consecutive_hits: u32) -> Duration {
    let exponent = consecutive_hits.saturating_sub(1).min(6);
    let factor = 1_u32 << exponent;
    let scaled = scale_duration(MEMPOOL_RATE_LIMIT_BACKOFF_BASE, factor);
    std::cmp::min(scaled, MEMPOOL_RATE_LIMIT_BACKOFF_MAX)
}

fn rate_limit_jitter(scope: &RateLimitScopeKey, backoff: Duration) -> Duration {
    let max_jitter_ms_u128 = (backoff.as_millis() / 5).min(5_000);
    let max_jitter_ms = u64::try_from(max_jitter_ms_u128).unwrap_or(0);
    if max_jitter_ms == 0 {
        return Duration::ZERO;
    }
    let mut hasher = DefaultHasher::new();
    scope.hash(&mut hasher);
    let value = hasher.finish();
    Duration::from_millis(value % max_jitter_ms.saturating_add(1))
}

pub(super) fn rate_limit_backoff_for_scope(
    scope: &RateLimitScopeKey,
    consecutive_hits: u32,
    retry_after: Option<Duration>,
) -> Duration {
    let base = if scope.integration == LABEL_ETHERSCAN {
        let retry_after = retry_after.unwrap_or(Duration::ZERO);
        return std::cmp::max(ETHERSCAN_RATE_LIMIT_BACKOFF, retry_after);
    } else {
        capped_mempool_rate_limit_backoff(consecutive_hits)
    };

    let jitter = rate_limit_jitter(scope, base);
    let with_jitter = std::cmp::min(
        base.checked_add(jitter)
            .unwrap_or(MEMPOOL_RATE_LIMIT_BACKOFF_MAX),
        MEMPOOL_RATE_LIMIT_BACKOFF_MAX,
    );
    match retry_after {
        Some(retry_after) => std::cmp::max(with_jitter, retry_after),
        None => with_jitter,
    }
}

pub(super) fn record_rate_limit(
    user_id: UserId,
    integration: &str,
    now: Instant,
    retry_after: Option<Duration>,
) {
    let scope = RateLimitScopeKey::new(user_id, integration);
    match global_rate_limiter().lock() {
        Ok(mut guard) => {
            let consecutive_hits = match guard.get(&scope).copied() {
                Some(previous) if now < previous.blocked_until => {
                    previous.consecutive_hits.saturating_add(1)
                }
                _ => 1,
            };
            let backoff = rate_limit_backoff_for_scope(&scope, consecutive_hits, retry_after);
            guard.insert(
                scope,
                RateLimitState {
                    blocked_until: now + backoff,
                    consecutive_hits,
                },
            );
        }
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while recording limit"
            );
        }
    }
}

pub(super) fn is_rate_limited(user_id: UserId, integration: &str, now: Instant) -> bool {
    let scope = RateLimitScopeKey::new(user_id, integration);
    match global_rate_limiter().lock() {
        Ok(mut guard) => {
            let Some(state) = guard.get(&scope).copied() else {
                return false;
            };
            if now < state.blocked_until {
                true
            } else {
                guard.remove(&scope);
                false
            }
        }
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while reading limit"
            );
            false
        }
    }
}

pub(crate) fn blocked_integrations_for_user(
    user_id: UserId,
    now: Instant,
    integrations: &HashSet<SyncIntegrationId>,
) -> HashSet<SyncIntegrationId> {
    match global_rate_limiter().lock() {
        Ok(mut guard) => {
            guard.retain(|_, state| now < state.blocked_until);
            guard
                .iter()
                .filter_map(|(scope, state)| {
                    if scope.user_id != user_id || now >= state.blocked_until {
                        return None;
                    }

                    let integration_id = SyncIntegrationId::from_db_value(&scope.integration)?;
                    integrations
                        .contains(&integration_id)
                        .then_some(integration_id)
                })
                .collect()
        }
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while reading blocked integrations"
            );
            HashSet::new()
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) fn earliest_rate_limit_unblock_for_user(
    user_id: UserId,
    now: Instant,
) -> Option<Instant> {
    match global_rate_limiter().lock() {
        Ok(mut guard) => {
            guard.retain(|_, state| now < state.blocked_until);
            guard
                .iter()
                .filter_map(|(scope, state)| {
                    (scope.user_id == user_id && now < state.blocked_until)
                        .then_some(state.blocked_until)
                })
                .min()
        }
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while reading earliest unblock"
            );
            None
        }
    }
}

/// Returns the UTC datetime after which the given integration will be unblocked, or `None` if
/// the integration is not currently rate-limited. Used to populate the `retry_after` field on
/// `account_integration_sync_failed` SSE events.
pub(super) fn retry_after_utc_for_integration(
    user_id: UserId,
    integration_db_value: &str,
    now_instant: Instant,
    now_utc: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let scope = RateLimitScopeKey::new(user_id, integration_db_value);
    match global_rate_limiter().lock() {
        Ok(guard) => guard.get(&scope).and_then(|state| {
            if now_instant < state.blocked_until {
                let duration = state.blocked_until.saturating_duration_since(now_instant);
                chrono::Duration::from_std(duration)
                    .ok()
                    .and_then(|d| now_utc.checked_add_signed(d))
            } else {
                None
            }
        }),
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while reading retry_after"
            );
            None
        }
    }
}

pub(crate) fn earliest_rate_limit_unblock_for_integrations(
    user_id: UserId,
    now: Instant,
    integrations: &HashSet<SyncIntegrationId>,
) -> Option<Instant> {
    if integrations.is_empty() {
        return None;
    }

    match global_rate_limiter().lock() {
        Ok(mut guard) => {
            guard.retain(|_, state| now < state.blocked_until);
            guard
                .iter()
                .filter_map(|(scope, state)| {
                    (scope.user_id == user_id
                        && now < state.blocked_until
                        && integrations
                            .iter()
                            .any(|integration| scope.integration == integration.as_db_value()))
                    .then_some(state.blocked_until)
                })
                .min()
        }
        Err(_) => {
            tracing::error!(
                "transactions sync: global rate limiter lock poisoned while reading scoped unblock"
            );
            None
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::models::UserId;
    use crate::tasks::jobs::sync::{
        ETHERSCAN_RATE_LIMIT_BACKOFF, LABEL_ETHERSCAN, LABEL_MEMPOOL,
        MEMPOOL_RATE_LIMIT_BACKOFF_BASE, MEMPOOL_RATE_LIMIT_BACKOFF_MAX,
    };

    #[test]
    fn rate_limit_backoff_for_scope_uses_exponential_strategy() {
        let user_id = UserId::new();
        let etherscan_scope = RateLimitScopeKey::new(user_id, LABEL_ETHERSCAN);
        let mempool_scope = RateLimitScopeKey::new(user_id, LABEL_MEMPOOL);

        let etherscan_backoff = rate_limit_backoff_for_scope(&etherscan_scope, 1, None);
        let mempool_backoff_first = rate_limit_backoff_for_scope(&mempool_scope, 1, None);
        let mempool_backoff_second = rate_limit_backoff_for_scope(&mempool_scope, 2, None);

        assert_eq!(etherscan_backoff, ETHERSCAN_RATE_LIMIT_BACKOFF);
        assert!(mempool_backoff_first >= MEMPOOL_RATE_LIMIT_BACKOFF_BASE);
        assert!(mempool_backoff_second >= mempool_backoff_first);
        assert!(mempool_backoff_second <= MEMPOOL_RATE_LIMIT_BACKOFF_MAX);
    }

    #[test]
    fn mempool_rate_limit_backoff_uses_short_bounded_reliability_window() {
        let user_id = UserId::new();
        let scope = RateLimitScopeKey::new(user_id, LABEL_MEMPOOL);

        let first = rate_limit_backoff_for_scope(&scope, 1, None);
        let second = rate_limit_backoff_for_scope(&scope, 2, None);
        let third = rate_limit_backoff_for_scope(&scope, 3, None);
        let fourth = rate_limit_backoff_for_scope(&scope, 4, None);
        let fifth = rate_limit_backoff_for_scope(&scope, 5, None);

        assert!(first >= Duration::from_secs(60));
        assert!(first <= Duration::from_secs(65));
        assert!(second >= Duration::from_secs(120));
        assert!(second <= Duration::from_secs(125));
        assert!(third >= Duration::from_secs(240));
        assert!(third <= Duration::from_secs(245));
        assert_eq!(fourth, Duration::from_secs(300));
        assert_eq!(fifth, Duration::from_secs(300));
    }

    #[test]
    fn mempool_rate_limit_backoff_honors_longer_retry_after() {
        let user_id = UserId::new();
        let scope = RateLimitScopeKey::new(user_id, LABEL_MEMPOOL);

        let backoff = rate_limit_backoff_for_scope(&scope, 1, Some(Duration::from_secs(600)));

        assert_eq!(backoff, Duration::from_secs(600));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod db_tests {
    use super::*;
    use crate::db::unique_user_id;
    use crate::tasks::jobs::sync::{
        LABEL_ETHERSCAN, LABEL_MEMPOOL, test_support::with_rate_limiter_isolated,
    };

    #[test]
    fn aggregate_rate_limit_readers_share_scope_and_isolate_user_provider() {
        with_rate_limiter_isolated(|| {
            let user_id = unique_user_id();
            let other_user_id = unique_user_id();
            let now = Instant::now();
            let integrations =
                HashSet::from([SyncIntegrationId::Mempool, SyncIntegrationId::Etherscan]);

            record_rate_limit(user_id, LABEL_MEMPOOL, now, None);

            assert_eq!(
                blocked_integrations_for_user(user_id, now, &integrations),
                HashSet::from([SyncIntegrationId::Mempool])
            );
            assert!(
                earliest_rate_limit_unblock_for_integrations(user_id, now, &integrations).is_some()
            );
            assert!(blocked_integrations_for_user(other_user_id, now, &integrations).is_empty());
            assert!(!is_rate_limited(user_id, LABEL_ETHERSCAN, now));

            let expired_at = now + MEMPOOL_RATE_LIMIT_BACKOFF_MAX + Duration::from_secs(1);
            assert!(blocked_integrations_for_user(user_id, expired_at, &integrations).is_empty());
            assert_eq!(
                earliest_rate_limit_unblock_for_integrations(user_id, expired_at, &integrations),
                None
            );
        });
    }
}

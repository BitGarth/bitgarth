use tokio::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct EligibilityRefresh {
    failure: Option<FailureEpisode>,
}

struct FailureEpisode {
    next_retry: Instant,
    last_warning: Instant,
    attempts: u64,
}

impl EligibilityRefresh {
    pub(super) fn ready(&self, now: Instant) -> bool {
        self.failure
            .as_ref()
            .is_none_or(|failure| now >= failure.next_retry)
    }

    pub(super) fn failed(&mut self, now: Instant) -> Option<u64> {
        let next_retry = now + Duration::from_secs(30);
        let Some(failure) = self.failure.as_mut() else {
            self.failure = Some(FailureEpisode {
                next_retry,
                last_warning: now,
                attempts: 1,
            });
            return Some(1);
        };

        failure.next_retry = next_retry;
        failure.attempts = failure.attempts.saturating_add(1);
        if now.duration_since(failure.last_warning) >= Duration::from_secs(300) {
            failure.last_warning = now;
            Some(failure.attempts)
        } else {
            None
        }
    }

    pub(super) fn recovered(&mut self) -> bool {
        self.failure.take().is_some()
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn cooldown_warning_and_recovery_have_fixed_boundaries() {
        let now = Instant::now();
        let mut state = EligibilityRefresh::default();
        assert!(state.ready(now));
        assert_eq!(state.failed(now), Some(1));
        assert!(!state.ready(now + Duration::from_secs(29)));
        assert!(state.ready(now + Duration::from_secs(30)));
        assert_eq!(state.failed(now + Duration::from_secs(30)), None);
        assert!(!state.ready(now + Duration::from_secs(59)));
        assert!(state.ready(now + Duration::from_secs(60)));
        assert_eq!(state.failed(now + Duration::from_secs(299)), None);
        assert_eq!(state.failed(now + Duration::from_secs(300)), Some(4));
        assert_eq!(state.failed(now + Duration::from_secs(599)), None);
        assert_eq!(state.failed(now + Duration::from_secs(600)), Some(6));
        assert!(state.recovered());
        assert!(!state.recovered());
        assert!(state.ready(now + Duration::from_secs(601)));
        assert_eq!(state.failed(now + Duration::from_secs(601)), Some(1));
    }
}

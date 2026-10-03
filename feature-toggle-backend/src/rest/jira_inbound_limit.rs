//! In-process rate limit for the public inbound Jira events route
//! (design §3.4, JI-44). The backend runs as one instance, so the state is
//! in memory and a restart resets it.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter};
use uuid::Uuid;

/// Burst of the shared bucket for ids that match no integration.
const UNKNOWN_BURST: u32 = 10;
/// At most one drop warning per integration in this window.
const WARN_INTERVAL: Duration = Duration::from_secs(60);
/// Log key for the shared unknown-id bucket.
const UNKNOWN_KEY: &str = "unknown";

/// Result of one admission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Allowed,
    /// Over the limit: the caller answers 429 with `Retry-After`.
    Limited {
        retry_after_secs: u64,
    },
}

/// Drops since the last warning for one key.
struct DropCounter {
    last_warned: Option<Instant>,
    dropped: u64,
}

pub struct JiraInboundLimiter {
    known: DefaultKeyedRateLimiter<Uuid>,
    unknown: DefaultDirectRateLimiter,
    clock: DefaultClock,
    drops: Mutex<HashMap<String, DropCounter>>,
}

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value.max(1)).expect("value is at least 1")
}

impl JiraInboundLimiter {
    /// `per_minute` and `burst` apply per integration. Unknown ids share one
    /// bucket of `unknown_per_minute` with a burst of 10.
    pub fn new(per_minute: u32, burst: u32, unknown_per_minute: u32) -> Self {
        let known = Quota::per_minute(non_zero(per_minute)).allow_burst(non_zero(burst));
        let unknown =
            Quota::per_minute(non_zero(unknown_per_minute)).allow_burst(non_zero(UNKNOWN_BURST));
        Self {
            known: RateLimiter::keyed(known),
            unknown: RateLimiter::direct(unknown),
            clock: DefaultClock::default(),
            drops: Mutex::new(HashMap::new()),
        }
    }

    /// Admission for an existing, enabled integration.
    pub fn check_known(&self, integration_id: Uuid) -> Admission {
        match self.known.check_key(&integration_id) {
            Ok(()) => Admission::Allowed,
            Err(not_until) => self.limited(
                &integration_id.to_string(),
                not_until.wait_time_from(self.clock.now()),
            ),
        }
    }

    /// Admission for a bad id, or an id of no enabled integration.
    pub fn check_unknown(&self) -> Admission {
        match self.unknown.check() {
            Ok(()) => Admission::Allowed,
            Err(not_until) => self.limited(UNKNOWN_KEY, not_until.wait_time_from(self.clock.now())),
        }
    }

    fn limited(&self, key: &str, wait: Duration) -> Admission {
        self.record_drop(key, Instant::now());
        Admission::Limited {
            retry_after_secs: wait.as_secs_f64().ceil().max(1.0) as u64,
        }
    }

    /// Counts one drop. Logs the count at most once per [`WARN_INTERVAL`].
    fn record_drop(&self, key: &str, now: Instant) {
        let Ok(mut drops) = self.drops.lock() else {
            return;
        };
        let counter = drops.entry(key.to_string()).or_insert(DropCounter {
            last_warned: None,
            dropped: 0,
        });
        counter.dropped += 1;
        let due = counter
            .last_warned
            .is_none_or(|at| now.duration_since(at) >= WARN_INTERVAL);
        if due {
            log::warn!(
                "Jira integration {key}: {} events rate limited in the last minute",
                counter.dropped
            );
            counter.last_warned = Some(now);
            counter.dropped = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_limited_with_retry_after() {
        let l = JiraInboundLimiter::new(60, 3, 30);
        let id = Uuid::new_v4();
        for _ in 0..3 {
            assert!(matches!(l.check_known(id), Admission::Allowed));
        }
        match l.check_known(id) {
            Admission::Limited { retry_after_secs } => assert!(retry_after_secs >= 1),
            _ => panic!("expected Limited"),
        }
    }

    #[test]
    fn integrations_do_not_share_a_bucket() {
        let l = JiraInboundLimiter::new(60, 2, 30);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(l.check_known(a), Admission::Allowed);
        assert_eq!(l.check_known(a), Admission::Allowed);
        assert!(matches!(l.check_known(a), Admission::Limited { .. }));
        assert_eq!(l.check_known(b), Admission::Allowed);
    }

    #[test]
    fn unknown_ids_share_one_bucket() {
        let l = JiraInboundLimiter::new(60, 3, 30);
        for _ in 0..10 {
            assert_eq!(l.check_unknown(), Admission::Allowed);
        }
        assert!(matches!(l.check_unknown(), Admission::Limited { .. }));
        // known buckets are unaffected
        assert_eq!(l.check_known(Uuid::new_v4()), Admission::Allowed);
    }
}

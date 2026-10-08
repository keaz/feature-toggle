//! Runtime stall detection.
//!
//! A task sleeps for [`TICK`] in a loop and measures how late each wake-up
//! is. A late wake-up means the runtime thread could not poll the task: a
//! long synchronous poll held it, or the process was not scheduled (for
//! example CFS throttling under a container CPU limit). Consecutive late
//! ticks form one stall, logged once when it ends.

use std::time::{Duration, Instant, SystemTime};
use tracing::warn;

/// Interval of the probe timer.
pub const TICK: Duration = Duration::from_millis(100);
/// A tick this late or later counts as stalled.
pub const LATE: Duration = Duration::from_millis(50);

/// A finished stall: consecutive ticks that each fired at least [`LATE`] late.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stall {
    /// Wall-clock time at which the first late tick started sleeping.
    pub started_at: SystemTime,
    /// From the first late tick's sleep start to the end of the last one.
    pub duration: Duration,
    /// Largest single delay.
    pub worst: Duration,
    /// Number of late ticks.
    pub ticks: u32,
}

#[derive(Debug)]
struct OpenStall {
    started_at: SystemTime,
    since: Instant,
    worst: Duration,
    ticks: u32,
}

/// Groups late ticks into stalls.
#[derive(Debug, Default)]
pub struct LagTracker {
    open: Option<OpenStall>,
}

impl LagTracker {
    /// Record a tick that started sleeping at `slept_at` (`slept_at_wall` on
    /// the wall clock) and woke `lag` later than requested. Returns the stall
    /// that an on-time tick ends, if any.
    pub fn record(
        &mut self,
        slept_at: Instant,
        slept_at_wall: SystemTime,
        lag: Duration,
    ) -> Option<Stall> {
        if lag >= LATE {
            match self.open.as_mut() {
                Some(open) => {
                    open.worst = open.worst.max(lag);
                    open.ticks += 1;
                }
                None => {
                    self.open = Some(OpenStall {
                        started_at: slept_at_wall,
                        since: slept_at,
                        worst: lag,
                        ticks: 1,
                    })
                }
            }
            return None;
        }
        let open = self.open.take()?;
        Some(Stall {
            started_at: open.started_at,
            // The last late tick ended where this on-time tick started.
            duration: slept_at.saturating_duration_since(open.since),
            worst: open.worst,
            ticks: open.ticks,
        })
    }
}

/// Watch the runtime this task runs on and log each stall as it ends.
pub async fn watch_runtime_lag(runtime: &'static str) {
    let mut tracker = LagTracker::default();
    loop {
        let slept_at = Instant::now();
        let slept_at_wall = SystemTime::now();
        tokio::time::sleep(TICK).await;
        let lag = slept_at.elapsed().saturating_sub(TICK);
        if let Some(stall) = tracker.record(slept_at, slept_at_wall, lag) {
            let started_unix_ms = stall
                .started_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            warn!(
                "{} runtime stalled for {} ms from unix_ms={} ({} late ticks, worst {} ms late)",
                runtime,
                stall.duration.as_millis(),
                started_unix_ms,
                stall.ticks,
                stall.worst.as_millis()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(tracker: &mut LagTracker, base: Instant, at_ms: u64, lag_ms: u64) -> Option<Stall> {
        tracker.record(
            base + Duration::from_millis(at_ms),
            SystemTime::UNIX_EPOCH + Duration::from_millis(at_ms),
            Duration::from_millis(lag_ms),
        )
    }

    #[test]
    fn on_time_ticks_report_nothing() {
        let mut tracker = LagTracker::default();
        let base = Instant::now();
        for i in 0..10 {
            assert_eq!(tick(&mut tracker, base, i * 100, 49), None);
        }
    }

    #[test]
    fn consecutive_late_ticks_form_one_stall() {
        let mut tracker = LagTracker::default();
        let base = Instant::now();
        assert_eq!(tick(&mut tracker, base, 0, 1), None);
        assert_eq!(tick(&mut tracker, base, 101, 80), None);
        assert_eq!(tick(&mut tracker, base, 281, 300), None);
        assert_eq!(tick(&mut tracker, base, 681, 60), None);

        let stall = tick(&mut tracker, base, 841, 2).expect("an on-time tick ends the stall");
        assert_eq!(
            stall,
            Stall {
                started_at: SystemTime::UNIX_EPOCH + Duration::from_millis(101),
                duration: Duration::from_millis(740),
                worst: Duration::from_millis(300),
                ticks: 3,
            }
        );

        // The next on-time tick starts clean.
        assert_eq!(tick(&mut tracker, base, 943, 0), None);
    }
}

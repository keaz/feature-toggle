use crate::logic::metrics::{MetricLogic, MetricLogicError};
use chrono::{Duration as ChronoDuration, Utc};
use log::{info, warn};
use std::time::Duration;
use tokio::time::interval;

pub struct MetricsAggregator {
    metric_logic: Box<dyn MetricLogic>,
    interval: Duration,
}

impl MetricsAggregator {
    pub fn new(metric_logic: Box<dyn MetricLogic>, interval: Duration) -> Self {
        Self {
            metric_logic,
            interval,
        }
    }

    /// Run aggregation in a loop on the configured interval
    pub async fn start(self) {
        info!(
            "Starting metrics aggregator with interval {:?}",
            self.interval
        );
        let mut ticker = interval(self.interval);

        loop {
            ticker.tick().await;
            if let Err(e) = self.run_once().await {
                warn!("Metric aggregation run failed: {}", e);
            }
        }
    }

    /// Aggregate the past 24h into hourly buckets
    pub async fn run_once(&self) -> Result<u64, MetricLogicError> {
        let now = Utc::now();
        let from = now - ChronoDuration::hours(24);
        self.metric_logic.aggregate_metrics(from, now, "hour").await
    }
}

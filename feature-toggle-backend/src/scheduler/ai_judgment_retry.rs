use std::sync::Arc;
use std::time::Duration;

use log::info;
use tokio::time;

use crate::judgment::service::JudgmentService;

/// Re-runs pending judgments lost on restart and failed ones under the attempt limit.
pub struct AiJudgmentRetryScheduler {
    service: Arc<JudgmentService>,
    interval: Duration,
}

impl AiJudgmentRetryScheduler {
    pub fn new(service: Arc<JudgmentService>, interval: Duration) -> Self {
        Self { service, interval }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            let retried = self.service.retry_tick().await;
            if retried > 0 {
                info!("AI judgment retry sweep re-ran {retried} judgment(s)");
            }
        }
    }
}

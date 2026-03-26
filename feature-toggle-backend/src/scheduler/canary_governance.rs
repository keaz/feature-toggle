use crate::logic::canary::CanaryLogic;
use log::{info, warn};
use std::time::Duration;
use tokio::time;

pub struct CanaryGovernanceScheduler {
    canary_logic: Box<dyn CanaryLogic>,
    interval: Duration,
}

impl CanaryGovernanceScheduler {
    pub fn new(canary_logic: Box<dyn CanaryLogic>, interval: Duration) -> Self {
        Self {
            canary_logic,
            interval,
        }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            match self.run_once().await {
                Ok(processed) => {
                    if processed > 0 {
                        info!("Canary governance processed {} enabled gate(s)", processed);
                    }
                }
                Err(err) => {
                    warn!("Canary governance scheduler encountered an error: {}", err);
                }
            }
        }
    }

    pub async fn run_once(&self) -> Result<usize, crate::logic::canary::CanaryLogicError> {
        self.canary_logic.analyze_enabled_gates().await
    }
}

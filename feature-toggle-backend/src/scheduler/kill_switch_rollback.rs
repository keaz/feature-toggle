use std::time::Duration;
use tokio::time::interval;
use log::{info, warn, error};
use crate::logic::feature::FeatureLogic;

pub struct KillSwitchRollbackScheduler {
    feature_logic: Box<dyn FeatureLogic>,
}

impl KillSwitchRollbackScheduler {
    pub fn new(feature_logic: Box<dyn FeatureLogic>) -> Self {
        Self {
            feature_logic,
        }
    }

    pub async fn start_scheduler(&self) {
        info!("Starting Kill Switch rollback scheduler");
        
        let mut interval = interval(Duration::from_secs(60)); // Check every minute
        
        loop {
            interval.tick().await;
            
            match self.check_and_process_rollbacks().await {
                Ok(count) => {
                    if count > 0 {
                        info!("Processed {} feature rollbacks", count);
                    }
                },
                Err(e) => {
                    error!("Error processing rollbacks: {}", e);
                }
            }
        }
    }

    async fn check_and_process_rollbacks(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let features_to_rollback = self.feature_logic.get_features_pending_rollback().await?;
        let mut processed_count = 0;

        for feature in features_to_rollback {
            match self.feature_logic.emergency_enable_feature(feature.id.clone()).await {
                Ok(_) => {
                    info!("Auto-rolled back feature: {} ({:?})", feature.key, feature.id);
                    processed_count += 1;
                },
                Err(e) => {
                    warn!("Failed to auto-rollback feature {} ({:?}): {}", feature.key, feature.id, e);
                }
            }
        }

        Ok(processed_count)
    }
}
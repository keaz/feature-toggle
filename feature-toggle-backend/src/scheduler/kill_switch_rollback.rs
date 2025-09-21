use crate::database::feature::FeatureRepository;
use crate::logic::feature::FeatureLogic;
use log::{error, info, warn};
use std::time::Duration;
use tokio::time::interval;

pub struct KillSwitchRollbackScheduler {
    feature_logic: Box<dyn FeatureLogic>,
    feature_repo: Box<dyn FeatureRepository>,
    pool: sqlx::PgPool,
    updates_tx: tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
}

impl KillSwitchRollbackScheduler {
    pub fn new(
        feature_logic: Box<dyn FeatureLogic>,
        feature_repo: Box<dyn FeatureRepository>,
        pool: sqlx::PgPool,
        updates_tx: tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    ) -> Self {
        Self {
            feature_logic,
            feature_repo,
            pool,
            updates_tx,
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
                }
                Err(e) => {
                    error!("Error processing rollbacks: {}", e);
                }
            }
        }
    }

    async fn check_and_process_rollbacks(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let features_to_rollback = self.feature_logic.get_features_pending_rollback().await?;
        let mut processed_count = 0;

        for feature in features_to_rollback {
            match self
                .feature_logic
                .emergency_enable_feature(feature.id.clone())
                .await
            {
                Ok(_) => {
                    info!(
                        "Auto-rolled back feature: {} ({:?})",
                        feature.key, feature.id
                    );
                    processed_count += 1;

                    // Broadcast feature update for gRPC clients (edge servers)
                    if let Ok(feature_uuid) = uuid::Uuid::try_from(feature.id.clone())
                        && let Ok(db_feature) =
                            self.feature_repo.get_feature_by_id(feature_uuid).await
                    {
                        // Map db_feature -> pb::FeatureFull and broadcast
                        if let Ok(full) = Self::map_db_feature_to_full_for_broadcast(
                            self.pool.clone(),
                            db_feature,
                        )
                        .await
                        {
                            let _ = self.updates_tx.send(crate::grpc::pb::FeatureUpdate {
                                message_id: uuid::Uuid::new_v4().to_string(),
                                action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                                feature: Some(full),
                                feature_key: String::new(),
                                error: String::new(),
                            });
                            info!(
                                "Broadcast feature update for auto-rollback: {} ({:?})",
                                feature.key, feature.id
                            );
                        } else {
                            warn!(
                                "Failed to map feature for broadcast: {} ({:?})",
                                feature.key, feature.id
                            );
                        }
                    } else {
                        warn!(
                            "Failed to reload feature from database for broadcast: {} ({:?})",
                            feature.key, feature.id
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to auto-rollback feature {} ({:?}): {}",
                        feature.key, feature.id, e
                    );
                }
            }
        }

        Ok(processed_count)
    }

    // Helper function to map database feature to gRPC FeatureFull for broadcasting
    async fn map_db_feature_to_full_for_broadcast(
        pool: sqlx::PgPool,
        f: crate::database::entity::Feature,
    ) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
        use crate::grpc::pb;
        let repo = crate::database::feature::feature_repository(pool.clone());

        // stages with criterias
        let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(f.stages.len());
        for s in f.stages.iter() {
            let crits = repo.get_stage_criteria(s.id).await?;
            let criterias = crits
                .into_iter()
                .map(|c| pb::StageCriterionFull {
                    id: c.id.to_string(),
                    context_key: c.context_key,
                    context: Some(pb::CriterionContext {
                        key: c.context.key,
                        entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                    }),
                    rollout_percentage: c.rollout_percentage,
                })
                .collect::<Vec<_>>();

            stage_msgs.push(pb::FeatureStageFull {
                id: s.id.to_string(),
                environment_id: s.environment_id.to_string(),
                order_index: s.order_index,
                position: s.position.clone(),
                enabled: s.enabled,
                bucketing_key: s.bucketing_key.clone().unwrap_or_default(),
                criterias,
            });
        }

        let deps = f
            .dependencies
            .iter()
            .map(|d| pb::FeatureDependencyFull {
                feature_id: d.feature_id.to_string(),
                depends_on_id: d.depends_on_id.to_string(),
            })
            .collect::<Vec<_>>();

        Ok(pb::FeatureFull {
            id: f.id.to_string(),
            key: f.key,
            description: f.description.unwrap_or_default(),
            feature_type: format!("{:?}", f.feature_type),
            team_id: f.team_id.to_string(),
            created_at: f.created_at.to_rfc3339(),
            kill_switch_enabled: f.kill_switch_enabled,
            kill_switch_activated_at: f
                .kill_switch_activated_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            rollback_scheduled_at: f
                .rollback_scheduled_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            stages: stage_msgs,
            dependencies: deps,
        })
    }
}

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
                        info!("Processed {} kill switch scheduler item(s)", count);
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
                .execute_scheduled_disable(feature.id.clone(), None) // Automated disable - no user actor
                .await
            {
                Ok(_) => {
                    info!(
                        "Auto-disabled feature via scheduled rollback: {} ({:?})",
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
                                "Broadcast feature update for auto-disable: {} ({:?})",
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
                        "Failed to auto-disable feature {} ({:?}): {}",
                        feature.key, feature.id, e
                    );
                }
            }
        }

        let features_with_expired_overrides = self
            .feature_logic
            .get_features_pending_emergency_expiry()
            .await?;

        for feature in features_with_expired_overrides {
            match self
                .feature_logic
                .expire_emergency_override(feature.id.clone())
                .await
            {
                Ok(_) => {
                    info!(
                        "Expired emergency override and restored feature: {} ({:?})",
                        feature.key, feature.id
                    );
                    processed_count += 1;

                    if let Ok(feature_uuid) = uuid::Uuid::try_from(feature.id.clone())
                        && let Ok(db_feature) =
                            self.feature_repo.get_feature_by_id(feature_uuid).await
                        && let Ok(full) = Self::map_db_feature_to_full_for_broadcast(
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
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to expire emergency override for feature {} ({:?}): {}",
                        feature.key, feature.id, e
                    );
                }
            }
        }

        Ok(processed_count)
    }

    // Maps a database feature to the gRPC FeatureFull of a broadcast, with the
    // same mapping as the stream snapshot.
    async fn map_db_feature_to_full_for_broadcast(
        pool: sqlx::PgPool,
        f: crate::database::entity::Feature,
    ) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
        let feature_repository = crate::database::feature::feature_repository(pool);
        let mut mapped = crate::grpc::map_features_to_full(&*feature_repository, vec![f]).await?;
        Ok(mapped.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::feature::MockFeatureRepository;
    use crate::logic::feature::MockFeatureLogic;
    use crate::model::Feature as ModelFeature;
    use crate::model::FeatureType as ModelFeatureType;
    use crate::model::ID;
    use crate::model::LifecycleStage;
    use chrono::Utc;

    const FEATURE_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn sample_feature_pending_rollback() -> ModelFeature {
        ModelFeature {
            id: ID::from(FEATURE_ID),
            key: "scheduled-kill".to_string(),
            description: None,
            feature_type: ModelFeatureType::Simple,
            enabled: true, // Feature is still enabled
            created_at: Utc::now(),
            kill_switch_enabled: true, // Kill switch is enabled (not activated yet)
            kill_switch_activated_at: None, // Not activated yet
            rollback_scheduled_at: Some(Utc::now() - chrono::Duration::minutes(5)), // Scheduled in the past
            emergency_override_reason: Some("Incident mitigation".to_string()),
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: Some(Utc::now() - chrono::Duration::minutes(10)),
            lifecycle_stage: LifecycleStage::Active,
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: Some(Utc::now() - chrono::Duration::minutes(6)),
            evaluation_count_7d: 2,
            evaluation_count_30d: 5,
            evaluation_count_90d: 10,
            is_stale: false,
            stale_reasons: vec![],
            dependencies: vec![],
            team_id: ID::from("22222222-2222-2222-2222-222222222222"),
            pending_approval_request_id: None,
            flag_kind: None,
            flag_kind_confidence: None,
            flag_kind_source: None,
        }
    }

    fn sample_feature_after_disable() -> ModelFeature {
        ModelFeature {
            id: ID::from(FEATURE_ID),
            key: "scheduled-kill".to_string(),
            description: None,
            feature_type: ModelFeatureType::Simple,
            enabled: false, // Feature is now disabled (active = false)
            created_at: Utc::now(),
            kill_switch_enabled: false, // Kill switch is now activated (disabled)
            kill_switch_activated_at: Some(Utc::now()), // Activation timestamp set
            rollback_scheduled_at: None, // Cleared after execution
            emergency_override_reason: Some("Incident mitigation".to_string()),
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: Some(Utc::now()),
            lifecycle_stage: LifecycleStage::Active,
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: Some(Utc::now()),
            evaluation_count_7d: 3,
            evaluation_count_30d: 7,
            evaluation_count_90d: 15,
            is_stale: false,
            stale_reasons: vec![],
            dependencies: vec![],
            team_id: ID::from("22222222-2222-2222-2222-222222222222"),
            pending_approval_request_id: None,
            flag_kind: None,
            flag_kind_confidence: None,
            flag_kind_source: None,
        }
    }

    fn sample_feature_expired_override() -> ModelFeature {
        ModelFeature {
            emergency_override_reason: Some("Incident mitigation".to_string()),
            emergency_override_expires_at: Some(Utc::now() - chrono::Duration::minutes(1)),
            emergency_override_actor_id: None,
            emergency_override_applied_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            ..sample_feature_after_disable()
        }
    }

    fn sample_feature_after_expiry() -> ModelFeature {
        ModelFeature {
            enabled: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            ..sample_feature_after_disable()
        }
    }

    #[tokio::test]
    async fn scheduler_disables_due_features() {
        let mut logic = MockFeatureLogic::new();
        logic
            .expect_get_features_pending_rollback()
            .times(1)
            .returning(|| Ok(vec![sample_feature_pending_rollback()]));
        logic
            .expect_execute_scheduled_disable()
            .times(1)
            .withf(|id, actor| id == &ID::from(FEATURE_ID) && actor.is_none())
            .returning(|_, _| Ok(sample_feature_after_disable()));
        logic
            .expect_get_features_pending_emergency_expiry()
            .times(1)
            .returning(|| Ok(vec![]));

        let mut repo = MockFeatureRepository::new();
        repo.expect_get_feature_by_id()
            .returning(|_| Err(crate::Error::NotFound(uuid::Uuid::new_v4())));

        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let (tx, _rx) = tokio::sync::broadcast::channel(4);

        let scheduler = KillSwitchRollbackScheduler::new(Box::new(logic), Box::new(repo), pool, tx);

        let processed = scheduler
            .check_and_process_rollbacks()
            .await
            .expect("scheduler should succeed");
        assert_eq!(processed, 1);
    }

    #[tokio::test]
    async fn scheduler_handles_no_pending_rollbacks() {
        let mut logic = MockFeatureLogic::new();
        logic
            .expect_get_features_pending_rollback()
            .times(1)
            .returning(|| Ok(vec![]));
        logic
            .expect_get_features_pending_emergency_expiry()
            .times(1)
            .returning(|| Ok(vec![]));

        let repo = MockFeatureRepository::new();
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let (tx, _rx) = tokio::sync::broadcast::channel(4);

        let scheduler = KillSwitchRollbackScheduler::new(Box::new(logic), Box::new(repo), pool, tx);

        let processed = scheduler
            .check_and_process_rollbacks()
            .await
            .expect("scheduler should succeed");
        assert_eq!(
            processed, 0,
            "No features should be processed when none are pending"
        );
    }

    #[tokio::test]
    async fn scheduler_expires_due_emergency_overrides() {
        let mut logic = MockFeatureLogic::new();
        logic
            .expect_get_features_pending_rollback()
            .times(1)
            .returning(|| Ok(vec![]));
        logic
            .expect_get_features_pending_emergency_expiry()
            .times(1)
            .returning(|| Ok(vec![sample_feature_expired_override()]));
        logic
            .expect_expire_emergency_override()
            .times(1)
            .withf(|id| id == &ID::from(FEATURE_ID))
            .returning(|_| Ok(sample_feature_after_expiry()));

        let mut repo = MockFeatureRepository::new();
        repo.expect_get_feature_by_id()
            .returning(|_| Err(crate::Error::NotFound(uuid::Uuid::new_v4())));

        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let (tx, _rx) = tokio::sync::broadcast::channel(4);

        let scheduler = KillSwitchRollbackScheduler::new(Box::new(logic), Box::new(repo), pool, tx);

        let processed = scheduler
            .check_and_process_rollbacks()
            .await
            .expect("scheduler should succeed");
        assert_eq!(processed, 1);
    }

    #[tokio::test]
    async fn scheduler_handles_execute_disable_error() {
        let mut logic = MockFeatureLogic::new();
        logic
            .expect_get_features_pending_rollback()
            .times(1)
            .returning(|| Ok(vec![sample_feature_pending_rollback()]));
        logic
            .expect_execute_scheduled_disable()
            .times(1)
            .withf(|id, actor| id == &ID::from(FEATURE_ID) && actor.is_none())
            .returning(|_, _| Err(crate::Error::NotFound(uuid::Uuid::new_v4())));
        logic
            .expect_get_features_pending_emergency_expiry()
            .times(1)
            .returning(|| Ok(vec![]));

        let repo = MockFeatureRepository::new();
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let (tx, _rx) = tokio::sync::broadcast::channel(4);

        let scheduler = KillSwitchRollbackScheduler::new(Box::new(logic), Box::new(repo), pool, tx);

        let processed = scheduler
            .check_and_process_rollbacks()
            .await
            .expect("scheduler should not fail on individual feature errors");
        assert_eq!(
            processed, 0,
            "Failed disable should not be counted as processed"
        );
    }
}

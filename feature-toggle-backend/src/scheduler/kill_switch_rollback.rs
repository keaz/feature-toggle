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

        Ok(processed_count)
    }

    // Helper function to map database feature to gRPC FeatureFull for broadcasting
    async fn map_db_feature_to_full_for_broadcast(
        pool: sqlx::PgPool,
        f: crate::database::entity::Feature,
    ) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
        use crate::grpc::pb;
        let feature_repository = crate::database::feature::feature_repository(pool.clone());
        let stages = feature_repository.get_feature_stages(f.id).await?;
        // stages with criterias
        let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(stages.len());
        for s in stages.iter() {
            let crits = feature_repository.get_stage_criteria(s.id).await?;
            let criterias = crits
                .into_iter()
                .map(|c| {
                    // Map rule groups
                    let rule_groups = c
                        .rule_groups
                        .into_iter()
                        .map(|group| pb::RuleGroup {
                            id: group.id.to_string(),
                            logic_operator: match group.logic_operator {
                                crate::database::entity::LogicOperator::And => "AND".to_string(),
                                crate::database::entity::LogicOperator::Or => "OR".to_string(),
                            },
                            conditions: group
                                .conditions
                                .into_iter()
                                .map(|cond| pb::RuleCondition {
                                    id: cond.id.to_string(),
                                    context_key: cond.context_key,
                                    operator: cond.operator,
                                    value: cond.value.to_string(),
                                    order_index: cond.order_index,
                                })
                                .collect(),
                        })
                        .collect();

                    // Map variant allocations
                    let variant_allocations = c
                        .variant_allocations
                        .into_iter()
                        .map(|alloc| pb::VariantAllocation {
                            variant_control: alloc.variant_control,
                            weight: alloc.weight,
                        })
                        .collect();

                    pb::StageCriterionFull {
                        id: c.id.to_string(),
                        stage_id: c.stage_id.to_string(),
                        priority: c.priority,
                        rule_groups,
                        variant_allocations,
                    }
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

        // Load variants from database only for Contextual features
        use crate::database::entity::FeatureType as EntityFeatureType;
        let variant_msgs = if matches!(f.feature_type, EntityFeatureType::Contextual) {
            let db_variants = feature_repository.get_feature_variants(f.id).await?;

            db_variants
                .into_iter()
                .map(|v| pb::FeatureVariant {
                    control: v.control,
                    value: serde_json::to_string(&v.value).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        Ok(pb::FeatureFull {
            id: f.id.to_string(),
            key: f.key,
            description: f.description.unwrap_or_default(),
            feature_type: format!("{:?}", f.feature_type),
            team_id: f.team_id.to_string(),
            active: f.active,
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
            variants: variant_msgs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::feature::MockFeatureRepository;
    use crate::graphql::schema::Feature as GraphQLFeature;
    use crate::graphql::schema::FeatureType as GraphQLFeatureType;
    use crate::graphql::schema::LifecycleStage;
    use crate::logic::feature::MockFeatureLogic;
    use async_graphql::ID;
    use chrono::Utc;

    const FEATURE_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn sample_feature_pending_rollback() -> GraphQLFeature {
        GraphQLFeature {
            id: ID::from(FEATURE_ID),
            key: "scheduled-kill".to_string(),
            description: None,
            feature_type: GraphQLFeatureType::Simple,
            enabled: true,                  // Feature is still enabled
            kill_switch_enabled: true,      // Kill switch is enabled (not activated yet)
            kill_switch_activated_at: None, // Not activated yet
            rollback_scheduled_at: Some(Utc::now() - chrono::Duration::minutes(5)), // Scheduled in the past
            lifecycle_stage: LifecycleStage::Active,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: Some(Utc::now() - chrono::Duration::minutes(6)),
            evaluation_count_7d: 2,
            evaluation_count_30d: 5,
            evaluation_count_90d: 10,
            dependencies: vec![],
            team_id: ID::from("22222222-2222-2222-2222-222222222222"),
            pending_approval_request_id: None,
        }
    }

    fn sample_feature_after_disable() -> GraphQLFeature {
        GraphQLFeature {
            id: ID::from(FEATURE_ID),
            key: "scheduled-kill".to_string(),
            description: None,
            feature_type: GraphQLFeatureType::Simple,
            enabled: false,             // Feature is now disabled (active = false)
            kill_switch_enabled: false, // Kill switch is now activated (disabled)
            kill_switch_activated_at: Some(Utc::now()), // Activation timestamp set
            rollback_scheduled_at: None, // Cleared after execution
            lifecycle_stage: LifecycleStage::Active,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: Some(Utc::now()),
            evaluation_count_7d: 3,
            evaluation_count_30d: 7,
            evaluation_count_90d: 15,
            dependencies: vec![],
            team_id: ID::from("22222222-2222-2222-2222-222222222222"),
            pending_approval_request_id: None,
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

        let mut repo = MockFeatureRepository::new();
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

        let mut repo = MockFeatureRepository::new();
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

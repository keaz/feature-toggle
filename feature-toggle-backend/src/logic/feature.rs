use crate::database::entity::FeatureType as EntityFeatureType;
use crate::database::feature::{
    CreateFeature, CreateFeatureStage, FeatureRepository, UpdateFeature,
};
use crate::model::{
    CreateFeatureInput, CreateFeatureStageInput, CreateRelationshipInput, Feature,
    FeatureType as ModelFeatureType, LifecycleStage, UpdateFeatureInput,
};
use crate::logic::approval::ApprovalLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::stage_builder::{build_stage_relationships, id_to_uuid};
use crate::Error;
use crate::model::ID;
use feature_toggle_shared::constants::StageStatus;
use uuid::Uuid;

use mockall::automock;

/// Rollout metrics for dashboard
#[derive(Debug, Clone)]
pub struct RolloutMetrics {
    pub average_time_in_pipeline: f64,
    pub approval_rate: f64,
    pub features_deployed_this_week: i32,
    pub features_deployed_last_week: i32,
    pub deployment_change: f64,
    pub bottleneck_stage: String,
    pub bottleneck_duration: f64,
    pub total_pending_approvals: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageChangeRequestType {
    DeploymentRequested,
    DeploymentRejected,
    Deployed,
    RollbackRequested,
    RollbackRejected,
    Rollbacked,
}

#[automock]
#[async_trait::async_trait]
/// Core CRUD operations for features
#[async_trait::async_trait]
pub trait FeatureCrudLogic: Send + Sync {
    async fn get_feature_by_id(&self, id: ID) -> Result<Feature, Error>;
    async fn get_features(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
    ) -> Result<Vec<Feature>, Error>;
    async fn get_features_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Feature>, i64), Error>;
    async fn get_features_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error>;
    async fn create_feature(
        &self,
        team_id: ID,
        input: CreateFeatureInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ID, Error>;
    async fn update_feature(
        &self,
        id: ID,
        input: UpdateFeatureInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error>;
    async fn delete_feature(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error>;

    // Count features
    async fn count_features(&self, team_id: Option<ID>) -> Result<i64, Error>;

    // Rollout metrics (for dashboard)
    async fn get_rollout_metrics(&self, team_id: Option<ID>) -> Result<RolloutMetrics, Error>;

    // Get features with pending approvals (DEPLOYMENT_REQUESTED or ROLLBACK_REQUESTED)
    async fn get_features_with_pending_approvals(
        &self,
        team_id: Option<ID>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error>;
    async fn get_features_with_pending_approvals_with_offset(
        &self,
        team_id: Option<ID>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error>;

    // Get features with active kill switches
    async fn get_features_with_kill_switches(
        &self,
        team_id: Option<ID>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error>;
    async fn get_features_with_kill_switches_with_offset(
        &self,
        team_id: Option<ID>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error>;

    // Kill switch functionality
    async fn emergency_disable_feature(
        &self,
        id: ID,
        rollback_in_minutes: Option<i32>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error>;
    async fn emergency_enable_feature(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error>;
    async fn get_features_pending_rollback(&self) -> Result<Vec<Feature>, Error>;
    async fn execute_scheduled_disable(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error>;
}

/// Stage context and criteria management operations
#[automock]
#[async_trait::async_trait]
pub trait StageLogic: Send + Sync {
    // Stage-contexts
    async fn get_stage_contexts(
        &self,
        stage_id: ID,
    ) -> Result<Vec<crate::model::Context>, Error>;
    async fn set_stage_contexts(
        &self,
        stage_id: ID,
        context_ids: Vec<ID>,
    ) -> Result<Vec<crate::model::Context>, Error>;

    // Stage-criteria
    async fn get_stage_criteria(
        &self,
        stage_id: ID,
    ) -> Result<Vec<crate::model::StageCriterion>, Error>;
    async fn set_stage_criteria(
        &self,
        stage_id: ID,
        criteria: Vec<crate::model::CreateStageCriterionInput>,
    ) -> Result<Vec<crate::model::StageCriterion>, Error>;
}

/// Deployment workflow and stage change operations
#[automock]
#[async_trait::async_trait]
pub trait DeploymentLogic: Send + Sync {
    async fn request_stage_change(
        &self,
        stage_id: ID,
        request: StageChangeRequestType,
        user_id: Uuid,
    ) -> Result<Feature, Error>;

    // Helper for broadcasting: get owning feature id by stage id
    async fn get_feature_id_by_stage_id(&self, stage_id: ID) -> Result<Option<Uuid>, Error>;
}

/// Combined interface for backward compatibility and convenience
pub trait FeatureLogic: FeatureCrudLogic + StageLogic + DeploymentLogic + Send + Sync {
    fn clone_box(&self) -> Box<dyn FeatureLogic>;
}

#[cfg(test)]
mockall::mock! {
    pub FeatureLogic {}

    #[async_trait::async_trait]
    impl FeatureCrudLogic for FeatureLogic {
        async fn get_feature_by_id(&self, id: ID) -> Result<Feature, Error>;
        async fn get_features(
            &self,
            team_id: ID,
            name: Option<String>,
            feature_type: Option<ModelFeatureType>,
        ) -> Result<Vec<Feature>, Error>;
        async fn get_features_paginated(
            &self,
            team_id: ID,
            name: Option<String>,
            feature_type: Option<ModelFeatureType>,
            page_number: i32,
            page_size: i32,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn get_features_with_offset(
            &self,
            team_id: ID,
            name: Option<String>,
            feature_type: Option<ModelFeatureType>,
            offset: i64,
            limit: i64,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn create_feature(&self, team_id: ID, input: CreateFeatureInput, actor: Option<crate::logic::ActorContext>) -> Result<ID, Error>;
        async fn update_feature(&self, id: ID, input: UpdateFeatureInput, actor: Option<crate::logic::ActorContext>) -> Result<Feature, Error>;
        async fn delete_feature(&self, id: ID, actor: Option<crate::logic::ActorContext>) -> Result<(), Error>;
        async fn count_features(&self, team_id: Option<ID>) -> Result<i64, Error>;
        async fn get_rollout_metrics(&self, team_id: Option<ID>) -> Result<RolloutMetrics, Error>;
        async fn get_features_with_pending_approvals(
            &self,
            team_id: Option<ID>,
            page_number: Option<i32>,
            page_size: Option<i32>,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn get_features_with_pending_approvals_with_offset(
            &self,
            team_id: Option<ID>,
            offset: i64,
            limit: i64,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn get_features_with_kill_switches(
            &self,
            team_id: Option<ID>,
            page_number: Option<i32>,
            page_size: Option<i32>,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn get_features_with_kill_switches_with_offset(
            &self,
            team_id: Option<ID>,
            offset: i64,
            limit: i64,
        ) -> Result<(Vec<Feature>, i64), Error>;
        async fn emergency_disable_feature(
            &self,
            id: ID,
            rollback_in_minutes: Option<i32>,
            actor: Option<crate::logic::ActorContext>,
        ) -> Result<Feature, Error>;
        async fn emergency_enable_feature(&self, id: ID, actor: Option<crate::logic::ActorContext>) -> Result<Feature, Error>;
        async fn get_features_pending_rollback(&self) -> Result<Vec<Feature>, Error>;
        async fn execute_scheduled_disable(&self, id: ID, actor: Option<crate::logic::ActorContext>) -> Result<Feature, Error>;
    }

    #[async_trait::async_trait]
    impl StageLogic for FeatureLogic {
        async fn get_stage_contexts(
            &self,
            stage_id: ID,
        ) -> Result<Vec<crate::model::Context>, Error>;
        async fn set_stage_contexts(
            &self,
            stage_id: ID,
            context_ids: Vec<ID>,
        ) -> Result<Vec<crate::model::Context>, Error>;
        async fn get_stage_criteria(
            &self,
            stage_id: ID,
        ) -> Result<Vec<crate::model::StageCriterion>, Error>;
        async fn set_stage_criteria(
            &self,
            stage_id: ID,
            criteria: Vec<crate::model::CreateStageCriterionInput>,
        ) -> Result<Vec<crate::model::StageCriterion>, Error>;
    }

    #[async_trait::async_trait]
    impl DeploymentLogic for FeatureLogic {
        async fn request_stage_change(
            &self,
            stage_id: ID,
            request: StageChangeRequestType,
            user_id: Uuid,
        ) -> Result<Feature, Error>;
        async fn get_feature_id_by_stage_id(&self, stage_id: ID) -> Result<Option<Uuid>, Error>;
    }

    impl crate::logic::feature::FeatureLogic for FeatureLogic {
        fn clone_box(&self) -> Box<dyn crate::logic::feature::FeatureLogic>;
    }
}

impl Clone for Box<dyn FeatureLogic> {
    fn clone(&self) -> Box<dyn FeatureLogic> {
        self.clone_box()
    }
}

pub fn feature_logic(
    repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
    user_repository: Box<dyn crate::database::user::UserRepository>,
) -> Box<dyn FeatureLogic> {
    feature_logic_with_approval(
        repository,
        environment_logic,
        activity_log_repository,
        user_repository,
        None,
    )
}

pub fn feature_logic_with_approval(
    repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
    user_repository: Box<dyn crate::database::user::UserRepository>,
    approval_logic: Option<Box<dyn ApprovalLogic>>,
) -> Box<dyn FeatureLogic> {
    Box::new(FeatureLogicImpl {
        repository,
        environment_logic,
        activity_log_repository,
        user_repository,
        approval_logic,
    })
}

struct FeatureLogicImpl {
    repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
    user_repository: Box<dyn crate::database::user::UserRepository>,
    approval_logic: Option<Box<dyn ApprovalLogic>>,
}

impl Clone for FeatureLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
            environment_logic: self.environment_logic.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
            user_repository: self.user_repository.clone_box(),
            approval_logic: self.approval_logic.as_ref().map(|a| a.clone_box()),
        }
    }
}

impl FeatureLogicImpl {
    fn map_api_to_entity_feature_type(feature_type: ModelFeatureType) -> EntityFeatureType {
        match feature_type {
            ModelFeatureType::Simple => EntityFeatureType::Simple,
            ModelFeatureType::Contextual => EntityFeatureType::Contextual,
        }
    }

    fn map_entity_to_api_feature_type(feature_type: EntityFeatureType) -> ModelFeatureType {
        match feature_type {
            EntityFeatureType::Simple => ModelFeatureType::Simple,
            EntityFeatureType::Contextual => ModelFeatureType::Contextual,
        }
    }

    fn map_to_create_feature(
        team_id: Uuid,
        input: CreateFeatureInput,
    ) -> Result<CreateFeature, Error> {
        let feature_type = Self::map_api_to_entity_feature_type(input.feature_type);
        let stages = Self::get_create_stages_to_create(input.stages, input.relationships)?;

        let dependencies = input
            .dependencies
            .into_iter()
            .map(|id| id_to_uuid(id))
            .collect::<Result<Vec<_>, _>>()?;

        // Map variants from API model to database format
        let variants = input.variants.map(|v| {
            v.into_iter()
                .map(|variant| {
                    let value_type = match variant.value_type {
                        crate::model::VariantValueType::String => {
                            crate::database::entity::VariantValueType::String
                        }
                        crate::model::VariantValueType::Number => {
                            crate::database::entity::VariantValueType::Number
                        }
                        crate::model::VariantValueType::Boolean => {
                            crate::database::entity::VariantValueType::Boolean
                        }
                        crate::model::VariantValueType::Json => {
                            crate::database::entity::VariantValueType::Json
                        }
                    };
                    (
                        variant.control,
                        variant.value,
                        value_type,
                        variant.description,
                    )
                })
                .collect::<Vec<_>>()
        });

        Ok(CreateFeature {
            team_id,
            key: input.key,
            description: input.description,
            feature_type,
            stages,
            dependencies,
            variants,
        })
    }

    fn get_create_stages_to_create(
        stages: Vec<CreateFeatureStageInput>,
        relationships: Vec<CreateRelationshipInput>,
    ) -> Result<Vec<CreateFeatureStage>, Error> {
        let stages = stages
            .into_iter()
            .map(|stage| -> Result<CreateFeatureStage, Error> {
                Ok(CreateFeatureStage {
                    id: match stage.id {
                        Some(id) => id_to_uuid(id)?,
                        None => Uuid::new_v4(),
                    },
                    environment_id: id_to_uuid(stage.environment_id)?,
                    order_index: stage.order_index,
                    position: stage.position,
                    enabled: false,
                    parent_stage: None,
                })
            })
            .collect::<Result<Vec<CreateFeatureStage>, Error>>()?;

        // Use shared relationship building logic
        Ok(build_stage_relationships(stages, relationships))
    }

    fn map_to_update_feature(id: ID, input: UpdateFeatureInput) -> Result<UpdateFeature, Error> {
        let id = id_to_uuid(id)?;
        let feature_type = Some(Self::map_api_to_entity_feature_type(input.feature_type));

        let stages = Self::get_create_stages_to_create(input.stages, input.relationships)?;
        let dependencies = input
            .dependencies
            .into_iter()
            .map(|id| id_to_uuid(id))
            .collect::<Result<Vec<_>, _>>()?;

        // Map variants from API model to database format
        let variants = input.variants.map(|v| {
            v.into_iter()
                .map(|variant| {
                    let value_type = match variant.value_type {
                        crate::model::VariantValueType::String => {
                            crate::database::entity::VariantValueType::String
                        }
                        crate::model::VariantValueType::Number => {
                            crate::database::entity::VariantValueType::Number
                        }
                        crate::model::VariantValueType::Boolean => {
                            crate::database::entity::VariantValueType::Boolean
                        }
                        crate::model::VariantValueType::Json => {
                            crate::database::entity::VariantValueType::Json
                        }
                    };
                    (
                        variant.control,
                        variant.value,
                        value_type,
                        variant.description,
                    )
                })
                .collect::<Vec<_>>()
        });

        Ok(UpdateFeature {
            id,
            key: Some(input.key),
            description: input.description,
            feature_type,
            stages,
            dependencies,
            variants,
        })
    }

    fn map_entity_to_api_feature(feature: crate::database::entity::Feature) -> Feature {
        Feature {
            id: feature.id.into(),
            key: feature.key,
            description: feature.description,
            feature_type: Self::map_entity_to_api_feature_type(feature.feature_type),
            enabled: feature.active,
            kill_switch_enabled: feature.kill_switch_enabled,
            kill_switch_activated_at: feature.kill_switch_activated_at,
            rollback_scheduled_at: feature.rollback_scheduled_at,
            lifecycle_stage: Self::map_db_lifecycle_stage(&feature.lifecycle_stage),
            deprecated_at: feature.deprecated_at,
            deprecation_notice: feature.deprecation_notice.clone(),
            last_evaluated_at: feature.last_evaluated_at,
            evaluation_count_7d: feature.evaluation_count_7d,
            evaluation_count_30d: feature.evaluation_count_30d,
            evaluation_count_90d: feature.evaluation_count_90d,
            team_id: feature.team_id.into(),
            dependencies: feature
                .dependencies
                .into_iter()
                .map(|d| d.depends_on_id.into())
                .collect(),
            pending_approval_request_id: None,
        }
    }

    fn map_db_lifecycle_stage(stage: &str) -> LifecycleStage {
        match stage.to_lowercase().as_str() {
            "deprecated" => LifecycleStage::Deprecated,
            "archived" => LifecycleStage::Archived,
            "permanent" => LifecycleStage::Permanent,
            _ => LifecycleStage::Active,
        }
    }
}

#[async_trait::async_trait]
impl FeatureCrudLogic for FeatureLogicImpl {
    async fn get_feature_by_id(&self, id: ID) -> Result<Feature, Error> {
        let id = Uuid::try_from(id).map_err(|_| Error::InvalidInput("Invalid ID".to_string()))?;
        let feature = self.repository.get_feature_by_id(id).await?;
        let feature = Self::map_entity_to_api_feature(feature);
        Ok(feature)
    }

    async fn get_features(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
    ) -> Result<Vec<Feature>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let entity_feature_type = feature_type.map(Self::map_api_to_entity_feature_type);
        let features = self
            .repository
            .get_features(team_id, name, entity_feature_type)
            .await?;

        Ok(features
            .into_iter()
            .map(Self::map_entity_to_api_feature)
            .collect())
    }

    async fn get_features_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_id = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let entity_feature_type = feature_type.map(Self::map_api_to_entity_feature_type);
        let (features, total) = self
            .repository
            .get_features_paginated(team_id, name, entity_feature_type, page_number, page_size)
            .await?;

        let mapped_features = features
            .into_iter()
            .map(Self::map_entity_to_api_feature)
            .collect();

        Ok((mapped_features, total))
    }

    async fn get_features_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<ModelFeatureType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_id = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let entity_feature_type = feature_type.map(Self::map_api_to_entity_feature_type);
        let (features, total) = self
            .repository
            .get_features_with_offset(team_id, name, entity_feature_type, offset, limit)
            .await?;

        let mapped_features = features
            .into_iter()
            .map(Self::map_entity_to_api_feature)
            .collect();

        Ok((mapped_features, total))
    }

    async fn create_feature(
        &self,
        team_id: ID,
        input: CreateFeatureInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ID, Error> {
        let team_id = id_to_uuid(team_id)?;
        let feature_key = input.key.clone();
        let input = Self::map_to_create_feature(team_id, input)?;
        let feature_id = self.repository.create_feature(input).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::FEATURE_CREATED,
            &feature_id.to_string(),
            actor_id,
            actor_name,
            format!("Created feature '{}'", feature_key),
            Some(serde_json::json!({
                "feature_id": feature_id.to_string(),
                "feature_key": feature_key,
                "team_id": team_id.to_string(),
            })),
        )
        .await;

        Ok(ID::from(feature_id.to_string()))
    }

    async fn update_feature(
        &self,
        id: ID,
        input: UpdateFeatureInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error> {
        let input = Self::map_to_update_feature(id.clone(), input)?;
        let feature = self.repository.update_feature(input).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::FEATURE_UPDATED,
            &feature.id.to_string(),
            actor_id,
            actor_name,
            format!("Updated feature '{}'", feature.key),
            Some(serde_json::json!({
                "feature_id": feature.id.to_string(),
                "feature_key": feature.key.clone(),
            })),
        )
        .await;

        Ok(Self::map_entity_to_api_feature(feature))
    }

    async fn delete_feature(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error> {
        let feature_uuid = id_to_uuid(id.clone())?;
        self.repository.delete_feature(feature_uuid).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::FEATURE_DELETED,
            &id.to_string(),
            actor_id,
            actor_name,
            format!("Deleted feature with ID '{}'", id.to_string()),
            Some(serde_json::json!({
                "feature_id": id.to_string(),
            })),
        )
        .await;

        Ok(())
    }

    async fn count_features(&self, team_id: Option<ID>) -> Result<i64, Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        self.repository.count_features(team_uuid).await
    }

    async fn get_rollout_metrics(&self, team_id: Option<ID>) -> Result<RolloutMetrics, Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        let data = self.repository.get_rollout_metrics_data(team_uuid).await?;

        // Calculate approval rate (avoid division by zero)
        let total_decisions = data.total_deployed + data.total_rejected;
        let approval_rate = if total_decisions > 0 {
            (data.total_deployed as f64 / total_decisions as f64) * 100.0
        } else {
            0.0
        };

        // Calculate deployment change percentage
        let deployment_change = if data.deployed_last_week > 0 {
            ((data.deployed_this_week - data.deployed_last_week) as f64
                / data.deployed_last_week as f64)
                * 100.0
        } else if data.deployed_this_week > 0 {
            100.0 // If we had 0 last week but have deployments this week, that's 100% growth
        } else {
            0.0
        };

        // Average time in pipeline - for now returning 0 since we don't have approved_time in all cases
        // This would need to be calculated from approved_time - created_at when that data is available
        let average_time_in_pipeline = 0.0;

        Ok(RolloutMetrics {
            average_time_in_pipeline,
            approval_rate,
            features_deployed_this_week: data.deployed_this_week as i32,
            features_deployed_last_week: data.deployed_last_week as i32,
            deployment_change,
            bottleneck_stage: data.bottleneck_stage.unwrap_or_else(|| "None".to_string()),
            bottleneck_duration: data.bottleneck_avg_wait_hours.unwrap_or(0.0),
            total_pending_approvals: data.pending_approvals as i32,
        })
    }

    async fn get_features_with_pending_approvals(
        &self,
        team_id: Option<ID>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        let (features, total) = self
            .repository
            .get_features_with_pending_approvals(team_uuid, page_number, page_size)
            .await?;

        // Map each feature and load its environments
        let mut mapped_features = Vec::new();
        for feature in features {
            let mapped_feature = Self::map_entity_to_api_feature(feature);
            mapped_features.push(mapped_feature);
        }

        Ok((mapped_features, total))
    }

    async fn get_features_with_pending_approvals_with_offset(
        &self,
        team_id: Option<ID>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        let (features, total) = self
            .repository
            .get_features_with_pending_approvals_with_offset(team_uuid, offset, limit)
            .await?;

        let mut mapped_features = Vec::new();
        for feature in features {
            let mapped_feature = Self::map_entity_to_api_feature(feature);
            mapped_features.push(mapped_feature);
        }

        Ok((mapped_features, total))
    }

    async fn get_features_with_kill_switches(
        &self,
        team_id: Option<ID>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        let (features, total) = self
            .repository
            .get_features_with_kill_switches(team_uuid, page_number, page_size)
            .await?;

        // Map each feature and load its environments
        let mut mapped_features = Vec::new();
        for feature in features {
            // Create feature with properly mapped stages
            let mapped_feature = Self::map_entity_to_api_feature(feature);
            mapped_features.push(mapped_feature);
        }

        Ok((mapped_features, total))
    }

    async fn get_features_with_kill_switches_with_offset(
        &self,
        team_id: Option<ID>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Feature>, i64), Error> {
        let team_uuid = team_id.map(|id| id_to_uuid(id)).transpose()?;
        let (features, total) = self
            .repository
            .get_features_with_kill_switches_with_offset(team_uuid, offset, limit)
            .await?;

        let mut mapped_features = Vec::new();
        for feature in features {
            let mapped_feature = Self::map_entity_to_api_feature(feature);
            mapped_features.push(mapped_feature);
        }

        Ok((mapped_features, total))
    }

    async fn emergency_disable_feature(
        &self,
        id: ID,
        rollback_in_minutes: Option<i32>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error> {
        let feature_id = id_to_uuid(id)?;
        let feature = self
            .repository
            .emergency_disable_feature(feature_id, rollback_in_minutes)
            .await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        let log_message = match rollback_in_minutes {
            Some(minutes) if minutes > 0 => {
                let scheduled = feature
                    .rollback_scheduled_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "unknown".to_string());
                format!(
                    "Kill switch scheduled for feature '{}' at {} (in {} minutes)",
                    feature.key, scheduled, minutes
                )
            }
            _ => format!("Kill switch activated for feature '{}'", feature.key),
        };

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::KILL_SWITCH_ACTIVATED,
            &feature.id.to_string(),
            actor_id,
            actor_name,
            log_message,
            Some(serde_json::json!({
                "feature_id": feature.id.to_string(),
                "feature_key": feature.key.clone(),
                "rollback_in_minutes": rollback_in_minutes,
            })),
        )
        .await;

        Ok(Self::map_entity_to_api_feature(feature))
    }

    async fn emergency_enable_feature(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error> {
        let feature_id = id_to_uuid(id)?;
        let feature = self.repository.emergency_enable_feature(feature_id).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::KILL_SWITCH_DEACTIVATED,
            &feature.id.to_string(),
            actor_id,
            actor_name,
            format!(
                "Feature is enabled and kill switch deactivated for '{}'",
                feature.key
            ),
            Some(serde_json::json!({
                "feature_id": feature.id.to_string(),
                "feature_key": feature.key.clone(),
            })),
        )
        .await;

        Ok(Self::map_entity_to_api_feature(feature))
    }

    async fn get_features_pending_rollback(&self) -> Result<Vec<Feature>, Error> {
        let features = self.repository.get_features_pending_rollback().await?;
        Ok(features
            .into_iter()
            .map(Self::map_entity_to_api_feature)
            .collect())
    }

    async fn execute_scheduled_disable(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Feature, Error> {
        let feature_id = id_to_uuid(id)?;
        let feature = self
            .repository
            .execute_scheduled_disable(feature_id)
            .await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log the scheduled disable execution
        let _ = crate::utils::activity_logger::log_feature_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::KILL_SWITCH_ACTIVATED,
            &feature.id.to_string(),
            actor_id,
            actor_name,
            format!(
                "Scheduled kill switch executed for feature '{}' (auto-disabled)",
                feature.key
            ),
            Some(serde_json::json!({
                "feature_id": feature.id.to_string(),
                "feature_key": feature.key.clone(),
                "scheduled_execution": true,
            })),
        )
        .await;

        Ok(Self::map_entity_to_api_feature(feature))
    }
}

#[async_trait::async_trait]
impl StageLogic for FeatureLogicImpl {
    async fn get_stage_contexts(
        &self,
        stage_id: ID,
    ) -> Result<Vec<crate::model::Context>, Error> {
        let stage_id = id_to_uuid(stage_id)?;
        let list = self.repository.get_stage_contexts(stage_id).await?;
        Ok(list.into_iter().map(map_db_ctx_to_model).collect())
    }

    async fn set_stage_contexts(
        &self,
        stage_id: ID,
        context_ids: Vec<ID>,
    ) -> Result<Vec<crate::model::Context>, Error> {
        let stage_id = id_to_uuid(stage_id)?;
        let context_ids: Vec<Uuid> = context_ids
            .into_iter()
            .map(|id| id_to_uuid(id))
            .collect::<Result<Vec<_>, _>>()?;
        let list = self
            .repository
            .set_stage_contexts(stage_id, context_ids)
            .await?;
        Ok(list.into_iter().map(map_db_ctx_to_model).collect())
    }

    async fn get_stage_criteria(
        &self,
        stage_id: ID,
    ) -> Result<Vec<crate::model::StageCriterion>, Error> {
        let stage_id = id_to_uuid(stage_id)?;
        let list = self.repository.get_stage_criteria(stage_id).await?;
        Ok(list.into_iter().map(map_db_criterion_to_model).collect())
    }

    async fn set_stage_criteria(
        &self,
        stage_id: ID,
        criteria: Vec<crate::model::CreateStageCriterionInput>,
    ) -> Result<Vec<crate::model::StageCriterion>, Error> {
        let stage_id = id_to_uuid(stage_id)?;
        let create: Result<Vec<crate::database::feature::CreateStageCriterion>, Error> = criteria
            .into_iter()
            .map(
                |c| -> Result<crate::database::feature::CreateStageCriterion, Error> {
                    Ok(crate::database::feature::CreateStageCriterion {
                        priority: c.priority,
                        variant_selection_mode: match c.variant_selection_mode.unwrap_or_default() {
                            crate::model::VariantSelectionMode::WeightedSplit => {
                                crate::database::entity::VariantSelectionMode::WeightedSplit
                            }
                            crate::model::VariantSelectionMode::SpecificVariant => {
                                crate::database::entity::VariantSelectionMode::SpecificVariant
                            }
                        },
                        selected_variant_control: c.selected_variant_control,
                    })
                },
            )
            .collect();
        let list = self
            .repository
            .set_stage_criteria(stage_id, create?)
            .await?;
        Ok(list.into_iter().map(map_db_criterion_to_model).collect())
    }
}

#[async_trait::async_trait]
impl DeploymentLogic for FeatureLogicImpl {
    async fn request_stage_change(
        &self,
        stage_id: ID,
        request: StageChangeRequestType,
        user_id: Uuid,
    ) -> Result<Feature, Error> {
        let stage_uuid = id_to_uuid(stage_id.clone())?;
        let next_status = match request {
            StageChangeRequestType::DeploymentRequested => {
                StageStatus::DeploymentRequested.as_str()
            }
            StageChangeRequestType::DeploymentRejected => StageStatus::DeploymentRejected.as_str(),
            StageChangeRequestType::Deployed => StageStatus::Deployed.as_str(),
            StageChangeRequestType::RollbackRequested => StageStatus::RollbackRequested.as_str(),
            StageChangeRequestType::RollbackRejected => StageStatus::RollbackRejected.as_str(),
            StageChangeRequestType::Rollbacked => StageStatus::Rollbacked.as_str(),
        };

        let stage = self
            .repository
            .get_stage_by_id(stage_uuid)
            .await?
            .ok_or(Error::NotFound(stage_uuid))?;

        // If no approval gating, validate transition immediately to fail fast before any DB side effects.
        if self.approval_logic.is_none() {
            if let Err(e) = crate::validation::validate_stage_transition(
                &stage.status,
                next_status,
            ) {
                return Err(Error::InvalidInput(e));
            }
        }

        let mut feature_id_for_stage: Option<Uuid> = None;

        // If an approval policy applies, create a pending approval request instead of executing immediately.
        if let Some(approval_logic) = &self.approval_logic {
            if crate::logic::approval::status_requires_interception(next_status) {
                // Set a pending status to indicate a gated action while approvals are collected.
                let pending_status = match next_status {
                    "DEPLOYED" | "DEPLOYMENT_REJECTED" => StageStatus::DeploymentRequested.as_str(),
                    "ROLLBACKED" | "ROLLBACK_REJECTED" => StageStatus::RollbackRequested.as_str(),
                    other => other,
                };

                // Validate the transition to the pending state before further DB work.
                if let Err(e) = crate::validation::validate_stage_transition(
                    &stage.status,
                    pending_status,
                ) {
                    return Err(Error::InvalidInput(e));
                }

                feature_id_for_stage = Some(
                    self.repository
                        .get_feature_id_by_stage_id(stage_uuid)
                        .await?
                        .ok_or(Error::NotFound(stage_uuid))?,
                );

                let db_feature = self
                    .repository
                    .get_feature_by_id(feature_id_for_stage.unwrap())
                    .await?;

                if let Some(request) = approval_logic
                    .maybe_create_stage_change_request(&db_feature, &stage, next_status, user_id)
                    .await?
                {
                    if pending_status == "DEPLOYMENT_REQUESTED"
                        || pending_status == "ROLLBACK_REQUESTED"
                    {
                        let now = chrono::Utc::now();
                        let updated = self
                            .repository
                            .request_stage_change(stage_uuid, pending_status, user_id, now)
                            .await?;
                        if !updated {
                            return Err(Error::NotFound(stage_uuid));
                        }
                    }
                    let mut api_feature =
                        FeatureLogicImpl::map_entity_to_api_feature(db_feature);
                    api_feature.pending_approval_request_id = Some(ID::from(request.id));
                    return Ok(api_feature);
                }
            }
        }

        // No approval gating: validate and apply directly.
        if let Err(e) = crate::validation::validate_stage_transition(
            &stage.status,
            next_status,
        ) {
            return Err(Error::InvalidInput(e));
        }

        let feature_id_for_stage = match feature_id_for_stage {
            Some(id) => id,
            None => self
                .repository
                .get_feature_id_by_stage_id(stage_uuid)
                .await?
                .ok_or(Error::NotFound(stage_uuid))?,
        };

        let ok = match request {
            StageChangeRequestType::DeploymentRequested
            | StageChangeRequestType::RollbackRequested => {
                let now = chrono::Utc::now();
                self.repository
                    .request_stage_change(stage_uuid, next_status, user_id, now)
                    .await?
            }
            StageChangeRequestType::DeploymentRejected
            | StageChangeRequestType::Deployed
            | StageChangeRequestType::RollbackRejected
            | StageChangeRequestType::Rollbacked => {
                self.repository
                    .approve_or_reject_stage_change(stage_uuid, next_status, user_id)
                    .await?
            }
        };
        if !ok {
            return Err(Error::NotFound(stage_uuid));
        }

        // Load the owning feature of this stage and return it, mapped to the API model
        let db_feature = self
            .repository
            .get_feature_by_id(feature_id_for_stage)
            .await?;

        // Find the stage to get environment information
        let stages = self
            .repository
            .get_feature_stages(db_feature.id.clone())
            .await?;
        let stage = stages.iter().find(|s| s.id == stage_uuid);

        // Get environment name if stage is found
        let environment_name = if let Some(stage) = stage {
            match self
                .environment_logic
                .get_environment_by_id(ID::from(stage.environment_id))
                .await
            {
                Ok(env) => Some(env.name),
                Err(_) => None,
            }
        } else {
            None
        };

        // Log activity based on request type (ignore errors to not fail the operation)
        let (activity_type, description) = match request {
            StageChangeRequestType::Deployed => {
                let desc = if let Some(ref env_name) = environment_name {
                    format!(
                        "Deployed feature '{}' to environment '{}'",
                        db_feature.key, env_name
                    )
                } else {
                    format!(
                        "Deployed feature '{}' to stage '{}'",
                        db_feature.key,
                        stage_id.to_string()
                    )
                };
                (
                    crate::utils::activity_logger::activity_types::STAGE_DEPLOYED,
                    desc,
                )
            }
            StageChangeRequestType::DeploymentRejected
            | StageChangeRequestType::RollbackRejected => {
                let desc = if let Some(ref env_name) = environment_name {
                    format!(
                        "Rejected change request for feature '{}' environment '{}'",
                        db_feature.key, env_name
                    )
                } else {
                    format!(
                        "Rejected change request for feature '{}' stage '{}'",
                        db_feature.key,
                        stage_id.to_string()
                    )
                };
                (
                    crate::utils::activity_logger::activity_types::STAGE_REJECTED,
                    desc,
                )
            }
            StageChangeRequestType::Rollbacked => {
                let desc = if let Some(ref env_name) = environment_name {
                    format!(
                        "Rolled back feature '{}' from environment '{}'",
                        db_feature.key, env_name
                    )
                } else {
                    format!(
                        "Rolled back feature '{}' from stage '{}'",
                        db_feature.key,
                        stage_id.to_string()
                    )
                };
                (
                    crate::utils::activity_logger::activity_types::STAGE_ROLLBACKED,
                    desc,
                )
            }
            _ => {
                let desc = if let Some(ref env_name) = environment_name {
                    format!(
                        "Requested {} for feature '{}' environment '{}'",
                        next_status, db_feature.key, env_name
                    )
                } else {
                    format!(
                        "Requested {} for feature '{}' stage '{}'",
                        next_status,
                        db_feature.key,
                        stage_id.to_string()
                    )
                };
                ("stage_change_requested", desc)
            }
        };

        let mut metadata = serde_json::json!({
            "feature_id": db_feature.id.to_string(),
            "feature_key": db_feature.key.clone(),
            "stage_id": stage_id.to_string(),
            "status": next_status,
            "team_id": db_feature.team_id.to_string(),
            "teamId": db_feature.team_id.to_string(),
        });

        // Add environment name to metadata if available
        if let Some(env_name) = environment_name {
            metadata["environment_name"] = serde_json::json!(env_name);
        }
        // Capture environment identifier to support team scoping of activity feeds
        if let Some(stage) = stage {
            metadata["environment_id"] = serde_json::json!(stage.environment_id.to_string());
        }

        let _ = crate::utils::activity_logger::log_activity(
            &self.activity_log_repository,
            activity_type,
            crate::utils::activity_logger::entity_types::STAGE,
            &stage_id.to_string(),
            Some(user_id),
            None,
            description,
            Some(metadata),
        )
        .await;

        Ok(FeatureLogicImpl::map_entity_to_api_feature(db_feature))
    }

    async fn get_feature_id_by_stage_id(&self, stage_id: ID) -> Result<Option<Uuid>, Error> {
        let stage_uuid = id_to_uuid(stage_id)?;
        self.repository.get_feature_id_by_stage_id(stage_uuid).await
    }
}

impl FeatureLogic for FeatureLogicImpl {
    fn clone_box(&self) -> Box<dyn FeatureLogic> {
        Box::new(self.clone())
    }
}

fn map_db_ctx_to_model(c: crate::database::entity::Context) -> crate::model::Context {
    crate::model::Context {
        id: ID::from(c.id),
        team_id: ID::from(c.team_id),
        key: c.key,
        entries: c
            .entries
            .into_iter()
            .map(|e| crate::model::ContextEntry {
                id: ID::from(e.id),
                value: e.value,
            })
            .collect(),
    }
}

fn map_db_criterion_to_model(
    sc: crate::database::entity::StageCriterion,
) -> crate::model::StageCriterion {
    use crate::model::RuleOperator;

    // Map compound rule groups
    let rule_groups = sc
        .rule_groups
        .into_iter()
        .map(|group| crate::model::CompoundRuleGroup {
            id: ID::from(group.id),
            logic_operator: match group.logic_operator {
                crate::database::entity::LogicOperator::And => {
                    crate::model::LogicOperator::And
                }
                crate::database::entity::LogicOperator::Or => {
                    crate::model::LogicOperator::Or
                }
            },
            conditions: group
                .conditions
                .into_iter()
                .map(|cond| {
                    let cond_operator = match cond.operator.to_uppercase().as_str() {
                        "EQUALS" => RuleOperator::Equals,
                        "NOTEQUALS" | "NOT_EQUALS" => RuleOperator::NotEquals,
                        "GREATERTHAN" | "GREATER_THAN" => RuleOperator::GreaterThan,
                        "LESSTHAN" | "LESS_THAN" => RuleOperator::LessThan,
                        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => {
                            RuleOperator::GreaterThanOrEqual
                        }
                        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => RuleOperator::LessThanOrEqual,
                        "CONTAINS" => RuleOperator::Contains,
                        "STARTSWITH" | "STARTS_WITH" => RuleOperator::StartsWith,
                        "ENDSWITH" | "ENDS_WITH" => RuleOperator::EndsWith,
                        "REGEX" => RuleOperator::Regex,
                        "IN" => RuleOperator::In,
                        "NOTIN" | "NOT_IN" => RuleOperator::NotIn,
                        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => {
                            RuleOperator::SemverGreaterThan
                        }
                        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => RuleOperator::SemverLessThan,
                        _ => RuleOperator::In,
                    };
                    crate::model::CompoundRuleCondition {
                        id: ID::from(cond.id),
                        context_key: cond.context_key,
                        operator: cond_operator,
                        value: cond.value,
                        order_index: cond.order_index,
                    }
                })
                .collect(),
        })
        .collect();

    // Map variant allocations
    let variant_allocations = sc
        .variant_allocations
        .into_iter()
        .map(|alloc| {
            crate::model::VariantAllocation {
                id: ID::from(uuid::Uuid::new_v4()), // Generate an API ID (not stored in simple version)
                criteria_id: ID::from(sc.id),
                variant_control: alloc.variant_control,
                weight: alloc.weight,
            }
        })
        .collect();

    crate::model::StageCriterion {
        id: ID::from(sc.id),
        stage_id: ID::from(sc.stage_id),
        priority: sc.priority,
        rule_groups,
        variant_allocations,
        variant_selection_mode: match sc.variant_selection_mode {
            crate::database::entity::VariantSelectionMode::WeightedSplit => {
                crate::model::VariantSelectionMode::WeightedSplit
            }
            crate::database::entity::VariantSelectionMode::SpecificVariant => {
                crate::model::VariantSelectionMode::SpecificVariant
            }
        },
        selected_variant_control: sc.selected_variant_control,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::database::activity_log::MockActivityLogRepository;
    use crate::database::entity::{Feature as EntityFeature, FeaturePipelineStage};
    use crate::database::feature::MockFeatureRepository;
    use crate::model::FeatureType;
    use crate::logic::environment::MockEnvironmentLogic;

    fn create_mock_activity_log() -> Box<dyn crate::database::activity_log::ActivityLogRepository> {
        let mut mock = MockActivityLogRepository::new();
        // Allow any number of activity log calls in tests
        mock.expect_create_activity().returning(|_| {
            Ok(crate::database::activity_log::ActivityLogRow {
                id: uuid::Uuid::new_v4(),
                activity_type: "test".to_string(),
                entity_type: "test".to_string(),
                entity_id: "test".to_string(),
                actor_id: None,
                actor_name: None,
                description: "test".to_string(),
                metadata: None,
                created_at: chrono::Utc::now(),
            })
        });
        mock.expect_clone_box()
            .returning(|| create_mock_activity_log());
        Box::new(mock)
    }

    fn create_mock_user_repository() -> Box<dyn crate::database::user::UserRepository> {
        let mut mock = crate::database::user::MockUserRepository::new();
        // Allow any number of user repository calls in tests
        mock.expect_get_user_by_id().returning(|_| {
            Ok(crate::database::user::User {
                id: uuid::Uuid::new_v4(),
                username: "test_user".to_string(),
                password_hash: "hash".to_string(),
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email: "test@example.com".to_string(),
                is_admin: false,
                enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_login: None,
                is_temporary_password: false,
            })
        });
        mock.expect_clone_box()
            .returning(|| create_mock_user_repository());
        Box::new(mock)
    }

    #[test]
    fn test_get_create_stages_to_create() {
        let stages = create_dummy_stages();

        let relationships = vec![];

        let result = FeatureLogicImpl::get_create_stages_to_create(stages, relationships).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_index, 0);
        assert_eq!(result[1].order_index, 1);
    }

    #[test]
    fn test_get_create_stages_to_create_with_relationships() {
        let stages = create_dummy_stages();

        let relationships = vec![CreateRelationshipInput {
            source_id: 0,
            target_id: 1,
        }];

        let result = FeatureLogicImpl::get_create_stages_to_create(stages, relationships).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_index, 0);
        assert_eq!(result[1].order_index, 1);
        assert!(result[1].parent_stage.is_some());
    }

    fn create_dummy_stages() -> Vec<CreateFeatureStageInput> {
        let stages = vec![
            CreateFeatureStageInput {
                id: None,
                environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                order_index: 0,
                position: "top".to_string(),
                bucketing_key: None,
            },
            CreateFeatureStageInput {
                id: Some(ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96")),
                environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                order_index: 1,
                position: "bottom".to_string(),
                bucketing_key: None,
            },
        ];
        stages
    }

    #[tokio::test]
    async fn test_get_feature_by_id() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_get_feature_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(EntityFeature {
                    id: Uuid::parse_str(ID).unwrap(),
                    key: "Test Feature".to_string(),
                    description: Some("Test description".to_string()),
                    feature_type: EntityFeatureType::Simple,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    active: true,
                    created_at: chrono::Utc::now(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: None,
                    lifecycle_stage: "active".to_string(),
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic.get_feature_by_id(ID::from(ID)).await;

        assert!(result.is_ok());
        let feature = result.unwrap();
        assert_eq!(feature.id.to_string(), ID);
        assert_eq!(feature.key, "Test Feature");
        assert_eq!(feature.description, Some("Test description".to_string()));
        assert!(matches!(feature.feature_type, ModelFeatureType::Simple));
    }

    #[tokio::test]
    async fn test_get_non_existing_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_get_feature_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic.get_feature_by_id(ID::from(ID)).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_create_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let input = CreateFeatureInput {
            key: "New Feature".to_string(),
            description: Some("New feature description".to_string()),
            feature_type: ModelFeatureType::Simple,
            enabled: Some(true),
            dependencies: vec![],
            relationships: vec![],
            stages: vec![],
            variants: Some(vec![]),
        };

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        let id = Uuid::parse_str(ID).unwrap();
        repository
            .expect_create_feature()
            .withf(|input| input.key == "New Feature")
            .times(1)
            .returning(move |_| Ok(id));

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .create_feature(
                ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                input,
                None,
            )
            .await;

        assert!(result.is_ok());
        let feature_id = result.unwrap();
        assert_eq!(feature_id, ID::from(ID));
    }

    #[tokio::test]
    async fn test_update_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        const NAME: &str = "Updated Feature";

        let input = UpdateFeatureInput {
            key: NAME.to_string(),
            description: Some("Updated description".to_string()),
            feature_type: ModelFeatureType::Contextual,
            enabled: Some(true),
            dependencies: vec![],
            relationships: vec![],
            stages: vec![],
            variants: Some(vec![]),
        };

        repository
            .expect_update_feature()
            .withf(|input| {
                input.id == Uuid::parse_str(ID).unwrap() && input.key == Some(NAME.to_string())
            })
            .times(1)
            .returning(move |_| {
                Ok(EntityFeature {
                    id: Uuid::parse_str(ID).unwrap(),
                    key: NAME.to_string(),
                    description: Some("Updated description".to_string()),
                    feature_type: EntityFeatureType::Contextual,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    active: true,
                    created_at: chrono::Utc::now(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: None,
                    lifecycle_stage: "active".to_string(),
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic.update_feature(ID::from(ID), input, None).await;

        assert!(result.is_ok());
        let feature = result.unwrap();
        assert_eq!(feature.key, NAME);
        assert_eq!(feature.description, Some("Updated description".to_string()));
        assert!(matches!(
            feature.feature_type,
            ModelFeatureType::Contextual
        ));
    }

    #[tokio::test]
    async fn test_delete_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_delete_feature()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic.delete_feature(ID::from(ID), None).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_features() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        repository
            .expect_get_features()
            .withf(|_, name, feature_type| name.is_none() && feature_type.is_none())
            .times(1)
            .returning(move |_, _, _| {
                Ok(vec![
                    EntityFeature {
                        id: Uuid::new_v4(),
                        key: "Test Feature".to_string(),
                        description: Some("Test description".to_string()),
                        feature_type: EntityFeatureType::Simple,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        active: true,
                        created_at: chrono::Utc::now(),
                        kill_switch_enabled: true,
                        kill_switch_activated_at: None,
                        rollback_scheduled_at: None,
                        lifecycle_stage: "active".to_string(),
                        deprecated_at: None,
                        deprecation_notice: None,
                        last_evaluated_at: None,
                        evaluation_count_7d: 0,
                        evaluation_count_30d: 0,
                        evaluation_count_90d: 0,
                        dependencies: vec![],
                    },
                    EntityFeature {
                        id: Uuid::new_v4(),
                        key: "Another Feature".to_string(),
                        description: Some("Another description".to_string()),
                        feature_type: EntityFeatureType::Contextual,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        active: true,
                        created_at: chrono::Utc::now(),
                        kill_switch_enabled: true,
                        kill_switch_activated_at: None,
                        rollback_scheduled_at: None,
                        lifecycle_stage: "active".to_string(),
                        deprecated_at: None,
                        deprecation_notice: None,
                        last_evaluated_at: None,
                        evaluation_count_7d: 0,
                        evaluation_count_30d: 0,
                        evaluation_count_90d: 0,
                        dependencies: vec![],
                    },
                ])
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .get_features(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), None, None)
            .await;

        assert!(result.is_ok());
        let features = result.unwrap();
        assert_eq!(features.len(), 2);
    }

    #[tokio::test]
    async fn test_request_stage_change_deployment_requested() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "NOT_DEPLOYED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        // Mock the feature lookup for stage validation
        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1) // Called once after the stage change
            .returning(move |_| Ok(Some(feature_id)));

        // Mock the feature retrieval for validation
        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1) // Called once after the stage change
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id,
                    stage_id,
                    "NOT_DEPLOYED",
                ))
            });

        // Mock the stage change request
        repository
            .expect_request_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("DEPLOYMENT_REQUESTED"),
                mockall::predicate::eq(user_id),
                mockall::predicate::function(|_: &chrono::DateTime<chrono::Utc>| true),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::DeploymentRequested,
                user_id,
            )
            .await;

        assert!(result.is_ok());
        let feature = result.unwrap();
        assert_eq!(feature.id, ID::from(feature_id));
    }

    #[tokio::test]
    async fn test_request_stage_change_deployment_rejected() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "DEPLOYMENT_REQUESTED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        // Mock the feature lookup for stage validation
        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        // Mock the feature retrieval for validation
        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id,
                    stage_id,
                    "DEPLOYMENT_APPROVED",
                ))
            });

        // Mock the stage change approval/rejection
        repository
            .expect_approve_or_reject_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("DEPLOYMENT_REJECTED"),
                mockall::predicate::eq(user_id),
            )
            .times(1)
            .returning(|_, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::DeploymentRejected,
                user_id,
            )
            .await;

        assert!(result.is_ok(), "{}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_request_stage_change_deployed() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "DEPLOYMENT_APPROVED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id,
                    stage_id,
                    "DEPLOYMENT_APPROVED",
                ))
            });

        repository
            .expect_approve_or_reject_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("DEPLOYED"),
                mockall::predicate::eq(user_id),
            )
            .times(1)
            .returning(|_, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::Deployed,
                user_id,
            )
            .await;

        assert!(result.is_ok(), "{}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollback_requested() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "DEPLOYED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id, stage_id, "DEPLOYED",
                ))
            });

        repository
            .expect_request_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("ROLLBACK_REQUESTED"),
                mockall::predicate::eq(user_id),
                mockall::predicate::function(|_: &chrono::DateTime<chrono::Utc>| true),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::RollbackRequested,
                user_id,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollback_rejected() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "ROLLBACK_REQUESTED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id,
                    stage_id,
                    "ROLLBACK_REQUESTED",
                ))
            });

        repository
            .expect_approve_or_reject_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("ROLLBACK_REJECTED"),
                mockall::predicate::eq(user_id),
            )
            .times(1)
            .returning(|_, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::RollbackRejected,
                user_id,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollbacked() {
        let mut repository = MockFeatureRepository::new();
        let mut environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "ROLLBACK_APPROVED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        let stage_clone_for_list = stage.clone();
        repository
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| Ok(vec![stage_clone_for_list.clone()]));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(create_entity_feature_with_stage_status(
                    feature_id,
                    stage_id,
                    "ROLLBACK_APPROVED",
                ))
            });

        repository
            .expect_approve_or_reject_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("ROLLBACKED"),
                mockall::predicate::eq(user_id),
            )
            .times(1)
            .returning(|_, _, _| Ok(true));

        // Mock get_environment_by_id (called for activity logging)
        environment_logic
            .expect_get_environment_by_id()
            .times(1)
            .returning(|_| {
                Ok(crate::model::Environment {
                    id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                    name: "Test Environment".to_string(),
                    active: true,
                    team_id: ID::from(Uuid::new_v4()),
                    environment_type: "Development".to_string(),
                })
            });

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::Rollbacked,
                user_id,
            )
            .await;

        assert!(result.is_ok(), "{}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_request_stage_change_invalid_transition() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "NOT_DEPLOYED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .never();

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .never();

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::Deployed, // Invalid: can't go from NOT_DEPLOYED to DEPLOYED
                user_id,
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            Error::InvalidInput(msg) => {
                assert!(msg.contains("Invalid transition"));
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_request_stage_change_nonexistent_stage() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(|_| Ok(None));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .never(); // Stage not found should short-circuit before feature lookup

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::DeploymentRequested,
                user_id,
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            Error::NotFound(_) => {
                // Expected error type
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_request_stage_change_repository_failure() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let stage = create_pipeline_stage_with_status(stage_id, feature_id, "NOT_DEPLOYED");

        let stage_clone_for_lookup = stage.clone();
        repository
            .expect_get_stage_by_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(stage_clone_for_lookup.clone())));

        repository
            .expect_get_feature_id_by_stage_id()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(move |_| Ok(Some(feature_id)));

        repository
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .never();

        repository
            .expect_request_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("DEPLOYMENT_REQUESTED"),
                mockall::predicate::eq(user_id),
                mockall::predicate::function(|_: &chrono::DateTime<chrono::Utc>| true),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(false)); // Repository operation failed

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::DeploymentRequested,
                user_id,
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            Error::NotFound(_) => {
                // Expected error type when repository operation fails
            }
            _ => panic!("Expected NotFound error when repository operation fails"),
        }
    }

    #[tokio::test]
    async fn test_request_stage_change_enum_to_string_mapping() {
        // Test that the enum variants map to the correct string values
        let mappings = vec![
            (
                StageChangeRequestType::DeploymentRequested,
                "DEPLOYMENT_REQUESTED",
            ),
            (
                StageChangeRequestType::DeploymentRejected,
                "DEPLOYMENT_REJECTED",
            ),
            (StageChangeRequestType::Deployed, "DEPLOYED"),
            (
                StageChangeRequestType::RollbackRequested,
                "ROLLBACK_REQUESTED",
            ),
            (
                StageChangeRequestType::RollbackRejected,
                "ROLLBACK_REJECTED",
            ),
            (StageChangeRequestType::Rollbacked, "ROLLBACKED"),
        ];

        for (enum_val, expected_string) in mappings {
            let string_val = match enum_val {
                StageChangeRequestType::DeploymentRequested => "DEPLOYMENT_REQUESTED",
                StageChangeRequestType::DeploymentRejected => "DEPLOYMENT_REJECTED",
                StageChangeRequestType::Deployed => "DEPLOYED",
                StageChangeRequestType::RollbackRequested => "ROLLBACK_REQUESTED",
                StageChangeRequestType::RollbackRejected => "ROLLBACK_REJECTED",
                StageChangeRequestType::Rollbacked => "ROLLBACKED",
            };

            assert_eq!(
                string_val, expected_string,
                "Enum {:?} should map to string '{}'",
                enum_val, expected_string
            );
        }
    }

    // Helper function to create entity feature with stage status for testing
    fn create_entity_feature_with_stage_status(
        feature_id: Uuid,
        _stage_id: Uuid,
        status: &str,
    ) -> crate::database::entity::Feature {
        crate::database::entity::Feature {
            id: feature_id,
            key: "Test Feature".to_string(),
            description: Some("Test description".to_string()),
            feature_type: crate::database::entity::FeatureType::Simple,
            team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
            created_at: chrono::Utc::now(),
            active: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            rollback_scheduled_at: Some(chrono::Utc::now() + chrono::Duration::minutes(30)),
            lifecycle_stage: "active".to_string(),
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
        }
    }

    fn create_pipeline_stage_with_status(
        stage_id: Uuid,
        feature_id: Uuid,
        status: &str,
    ) -> FeaturePipelineStage {
        FeaturePipelineStage {
            id: stage_id,
            feature_id,
            environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
            order_index: 0,
            parent_stage_id: None,
            position: "{ \"x\": 0, \"y\": 0 }".to_string(),
            enabled: true,
            status: status.to_string(),
        }
    }

    #[tokio::test]
    async fn test_get_features_paginated_success() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let feature1_id = Uuid::new_v4();
        let feature2_id = Uuid::new_v4();

        let expected_features = vec![
            crate::database::entity::Feature {
                id: feature1_id,
                key: "feature-1".to_string(),
                description: Some("First feature".to_string()),
                feature_type: crate::database::entity::FeatureType::Simple,
                team_id,
                active: true,
                created_at: chrono::Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: None,
                lifecycle_stage: "active".to_string(),
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            },
            crate::database::entity::Feature {
                id: feature2_id,
                key: "feature-2".to_string(),
                description: Some("Second feature".to_string()),
                feature_type: crate::database::entity::FeatureType::Contextual,
                team_id,
                active: true,
                created_at: chrono::Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: None,
                lifecycle_stage: "active".to_string(),
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            },
        ];

        repository
            .expect_get_features_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<crate::database::entity::FeatureType>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(move |_, _, _, _, _| Ok((expected_features.clone(), 50)));

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(ID::from(team_id), None, None, 1, 10)
            .await
            .unwrap();

        assert_eq!(features.len(), 2);
        assert_eq!(total, 50);
        assert_eq!(features[0].key, "feature-1");
        assert_eq!(features[0].feature_type, FeatureType::Simple);
        assert_eq!(features[1].key, "feature-2");
        assert_eq!(features[1].feature_type, FeatureType::Contextual);
    }

    #[tokio::test]
    async fn test_get_features_paginated_with_filters() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

        repository
            .expect_get_features_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(Some("test".to_string())),
                mockall::predicate::eq(Some(crate::database::entity::FeatureType::Simple)),
                mockall::predicate::eq(2),
                mockall::predicate::eq(5),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(
                ID::from(team_id),
                Some("test".to_string()),
                Some(crate::model::FeatureType::Simple),
                2,
                5,
            )
            .await
            .unwrap();

        assert_eq!(features.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_features_paginated_edge_cases() {
        let mut repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();
        let environment_id = Uuid::new_v4();

        // Test with page_number = 0 (passed through as-is)
        repo.expect_get_features_paginated()
            .with(
                mockall::predicate::eq(environment_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<crate::database::entity::FeatureType>),
                mockall::predicate::eq(0), // Passed through as-is
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = super::feature_logic(
            Box::new(repo),
            Box::new(env_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(
                ID::from(environment_id),
                None,
                None,
                0, // Edge case: page 0
                10,
            )
            .await
            .unwrap();

        assert_eq!(features.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_features_paginated_negative_page() {
        let mut repo = MockFeatureRepository::new();
        let env_logic = MockEnvironmentLogic::new();
        let environment_id = Uuid::new_v4();

        // Test with negative page_number (passed through as-is)
        repo.expect_get_features_paginated()
            .with(
                mockall::predicate::eq(environment_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<crate::database::entity::FeatureType>),
                mockall::predicate::eq(-5), // Passed through as-is
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = super::feature_logic(
            Box::new(repo),
            Box::new(env_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(
                ID::from(environment_id),
                None,
                None,
                -5, // Edge case: negative page
                10,
            )
            .await
            .unwrap();

        assert_eq!(features.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_features_paginated_zero_page_size() {
        let mut repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();
        let environment_id = Uuid::new_v4();

        // Test with zero page_size
        repo.expect_get_features_paginated()
            .with(
                mockall::predicate::eq(environment_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<crate::database::entity::FeatureType>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(0), // Zero page size
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = super::feature_logic(
            Box::new(repo),
            Box::new(env_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(
                ID::from(environment_id),
                None,
                None,
                1,
                0, // Edge case: zero page size
            )
            .await
            .unwrap();

        assert_eq!(features.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_features_paginated_extreme_values() {
        let mut repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();
        let environment_id = Uuid::new_v4();

        // Test with extreme values
        repo.expect_get_features_paginated()
            .with(
                mockall::predicate::eq(environment_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<crate::database::entity::FeatureType>),
                mockall::predicate::eq(999999),
                mockall::predicate::eq(1),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 50))); // Has data but not on this page

        let logic = super::feature_logic(
            Box::new(repo),
            Box::new(env_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let (features, total) = logic
            .get_features_paginated(
                ID::from(environment_id),
                None,
                None,
                999999, // Very large page number
                1,      // Very small page size
            )
            .await
            .unwrap();

        assert_eq!(features.len(), 0);
        assert_eq!(total, 50);
    }

    #[tokio::test]
    async fn test_get_features_paginated_invalid_team_id() {
        let repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let logic = feature_logic(
            Box::new(repository),
            Box::new(environment_logic),
            create_mock_activity_log(),
            create_mock_user_repository(),
        );
        let result = logic
            .get_features_paginated(ID::from("invalid-uuid"), None, None, 1, 10)
            .await;

        assert!(matches!(result, Err(Error::InvalidInput(_))));
    }
}

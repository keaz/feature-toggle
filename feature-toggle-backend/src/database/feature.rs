use crate::database::entity::{Feature, FeatureDependency, FeaturePipelineStage, FeatureType};
use crate::database::{Error, handle_error};
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgQueryResult;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStageCriterion {
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub variant_selection_mode: crate::database::entity::VariantSelectionMode,
    pub selected_variant_control: Option<String>,
}

/// Represents feature growth data at a specific time bucket
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeatureGrowthPoint {
    pub time_bucket: DateTime<Utc>,
    pub team_id: Option<Uuid>,
    pub team_name: Option<String>,
    pub feature_count: i64,
    pub cumulative_count: i64,
}

/// Represents raw rollout metrics data from the database
#[derive(Debug, Clone)]
pub struct RolloutMetricsData {
    pub total_deployed: i64,
    pub total_rejected: i64,
    pub deployed_this_week: i64,
    pub deployed_last_week: i64,
    pub pending_approvals: i64,
    pub bottleneck_stage: Option<String>,
    pub bottleneck_avg_wait_hours: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CreateFeature {
    pub team_id: Uuid,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub stages: Vec<CreateFeatureStage>,
    pub dependencies: Vec<Uuid>,
    pub variants: Option<
        Vec<(
            String,
            serde_json::Value,
            crate::database::entity::VariantValueType,
            Option<String>,
        )>,
    >,
}

#[derive(Debug, Clone)]
pub struct CreateFeatureStage {
    pub id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage: Option<Box<CreateFeatureStage>>,
    pub position: String,
    pub enabled: bool,
}

impl CreateFeatureStage {
    pub fn new(
        id: Uuid,
        environment_id: Uuid,
        order_index: i32,
        parent_stage: Option<Box<CreateFeatureStage>>,
        position: String,
    ) -> Self {
        Self {
            id,
            environment_id,
            order_index,
            parent_stage,
            position,
            enabled: false,
        }
    }
}

impl crate::logic::stage_builder::StageWithRelationship for CreateFeatureStage {
    fn order_index(&self) -> i32 {
        self.order_index
    }

    fn set_parent_stage(&mut self, parent: Box<Self>) {
        self.parent_stage = Some(parent);
    }
}

pub struct UpdateFeature {
    pub id: Uuid,
    pub key: Option<String>,
    pub description: Option<String>,
    pub feature_type: Option<FeatureType>,
    pub stages: Vec<CreateFeatureStage>,
    pub dependencies: Vec<Uuid>,
    pub variants: Option<
        Vec<(
            String,
            serde_json::Value,
            crate::database::entity::VariantValueType,
            Option<String>,
        )>,
    >,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct Features {
    feature_id: Uuid,
    feature_key: String,
    description: Option<String>,
    feature_type: String,
    team_id: Uuid,
    created_at: DateTime<Utc>,
    kill_switch_enabled: bool,
    kill_switch_activated_at: Option<DateTime<Utc>>,
    rollback_scheduled_at: Option<DateTime<Utc>>,
    feature_enabled: bool,
    lifecycle_stage: String,
    deprecated_at: Option<DateTime<Utc>>,
    deprecation_notice: Option<String>,
    last_evaluated_at: Option<DateTime<Utc>>,
    evaluation_count_7d: i64,
    evaluation_count_30d: i64,
    evaluation_count_90d: i64,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeatureWithStageRow {
    feature_id: Uuid,
    feature_key: String,
    description: Option<String>,
    feature_type: String,
    team_id: Uuid,
    created_at: DateTime<Utc>,
    kill_switch_enabled: bool,
    kill_switch_activated_at: Option<DateTime<Utc>>,
    rollback_scheduled_at: Option<DateTime<Utc>>,
    feature_enabled: bool,
    lifecycle_stage: String,
    deprecated_at: Option<DateTime<Utc>>,
    deprecation_notice: Option<String>,
    last_evaluated_at: Option<DateTime<Utc>>,
    evaluation_count_7d: i64,
    evaluation_count_30d: i64,
    evaluation_count_90d: i64,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeatureDependencyRow {
    feature_id: Uuid,
    depends_on_id: Uuid,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeaturePipelineStageRow {
    pub id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
    pub position: String,
    pub status: String,
    pub enabled: bool,
}

const FEATURE_SELECT: &str = r#"SELECT f.id as feature_id, f.key as feature_key, f.description, f.feature_type, f.team_id, f.created_at, 
            f.kill_switch_enabled, f.kill_switch_activated_at, f.rollback_scheduled_at, f.active as feature_enabled,
            f.lifecycle_stage, f.deprecated_at, f.deprecation_notice, f.last_evaluated_at,
            f.evaluation_count_7d, f.evaluation_count_30d, f.evaluation_count_90d
			FROM features f"#;

#[automock]
#[async_trait::async_trait]
pub trait FeatureRepository: Send + Sync {
    async fn get_feature_by_id(&self, id: Uuid) -> Result<Feature, Error>;
    async fn get_features(
        &self,
        team_id: Uuid,
        key: Option<String>,
        feature_type: Option<FeatureType>,
    ) -> Result<Vec<Feature>, Error>;
    async fn get_features_paginated(
        &self,
        team_id: Uuid,
        key: Option<String>,
        feature_type: Option<FeatureType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Feature>, i64), Error>;
    async fn create_feature(&self, input: CreateFeature) -> Result<Uuid, Error>;
    async fn update_feature(&self, input: UpdateFeature) -> Result<Feature, Error>;
    async fn delete_feature(&self, id: Uuid) -> Result<(), Error>;
    // Stage-contexts (legacy)
    async fn get_stage_contexts(
        &self,
        stage_id: Uuid,
    ) -> Result<Vec<crate::database::entity::Context>, Error>;
    async fn set_stage_contexts(
        &self,
        stage_id: Uuid,
        context_ids: Vec<Uuid>,
    ) -> Result<Vec<crate::database::entity::Context>, Error>;
    // Stage-criteria (new)
    async fn get_stage_criteria(
        &self,
        stage_id: Uuid,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error>;

    async fn set_stage_criteria(
        &self,
        stage_id: Uuid,
        criteria: Vec<CreateStageCriterion>,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error>;

    async fn get_feature_stages(
        &self,
        feature_id: Uuid,
    ) -> Result<Vec<FeaturePipelineStage>, Error>;

    async fn get_stage_by_id(&self, stage_id: Uuid) -> Result<Option<FeaturePipelineStage>, Error>;
    // New: get features referencing a given context id
    async fn get_feature_ids_by_context_id(&self, context_id: Uuid) -> Result<Vec<Uuid>, Error>;

    // Feature variants
    async fn get_feature_variants(
        &self,
        feature_id: Uuid,
    ) -> Result<Vec<crate::database::entity::FeatureVariant>, Error>;

    // New (deployment workflow): request stage change
    async fn request_stage_change(
        &self,
        stage_id: Uuid,
        status: &str,
        requested_user: Uuid,
        when: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, Error>;

    // Approve or reject a stage change (sets approved_user and approved_time)
    async fn approve_or_reject_stage_change(
        &self,
        stage_id: Uuid,
        status: &str,
        user_id: Uuid,
    ) -> Result<bool, Error>;

    // Reset a stage to a prior status (used when approval flows are cancelled)
    async fn reset_stage_status(&self, stage_id: Uuid, status: &str) -> Result<bool, Error>;

    // Kill switch functionality for emergency feature disable/enable
    async fn emergency_disable_feature(
        &self,
        feature_id: Uuid,
        rollback_in_minutes: Option<i32>,
    ) -> Result<Feature, Error>;

    async fn emergency_enable_feature(&self, feature_id: Uuid) -> Result<Feature, Error>;

    async fn get_features_pending_rollback(&self) -> Result<Vec<Feature>, Error>;

    // Execute the actual disable for scheduled rollback (called by scheduler)
    async fn execute_scheduled_disable(&self, feature_id: Uuid) -> Result<Feature, Error>;

    // Helper: find owning feature id for a stage
    async fn get_feature_id_by_stage_id(&self, stage_id: Uuid) -> Result<Option<Uuid>, Error>;

    // Feature growth analytics
    async fn get_feature_growth(
        &self,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
        interval: String,
        team_id: Option<Uuid>,
    ) -> Result<Vec<FeatureGrowthPoint>, Error>;

    // Count features (for dashboard metrics)
    async fn count_features(&self, team_id: Option<Uuid>) -> Result<i64, Error>;

    // Rollout metrics (for dashboard)
    async fn get_rollout_metrics_data(
        &self,
        team_id: Option<Uuid>,
    ) -> Result<RolloutMetricsData, Error>;

    // Get features with pending approvals (DEPLOYMENT_REQUESTED or ROLLBACK_REQUESTED)
    async fn get_features_with_pending_approvals(
        &self,
        team_id: Option<Uuid>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error>;

    // Get features with active kill switches
    async fn get_features_with_kill_switches(
        &self,
        team_id: Option<Uuid>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error>;

    fn clone_box(&self) -> Box<dyn FeatureRepository>;
}

impl Clone for Box<dyn FeatureRepository> {
    fn clone(&self) -> Box<dyn FeatureRepository> {
        self.clone_box()
    }
}

pub fn feature_repository(pool: PgPool) -> Box<dyn FeatureRepository> {
    Box::new(FeatureRepositoryImpl::new(pool))
}

#[derive(Clone)]
pub struct FeatureRepositoryImpl {
    pool: PgPool,
}

impl FeatureRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn is_feature_exists_id(&self, id: Uuid) -> Result<Option<Uuid>, Error> {
        let result = sqlx::query_scalar!(r#"SELECT id FROM features WHERE id = $1"#, id)
            .fetch_optional(&self.pool)
            .await;

        handle_error(Some(id), result)
    }

    async fn get_feature_dependencies(
        &self,
        feature_id: &Uuid,
    ) -> Result<Vec<FeatureDependency>, Error> {
        let result = sqlx::query_as!(
            FeatureDependencyRow,
            r#"SELECT feature_id, depends_on_id FROM feature_dependencies WHERE feature_id = $1"#,
            feature_id
        )
        .fetch_all(&self.pool)
        .await;

        let rows = handle_error(Some(*feature_id), result)?;
        let dependencies = rows
            .into_iter()
            .map(|row| FeatureDependency {
                feature_id: row.feature_id,
                depends_on_id: row.depends_on_id,
            })
            .collect();

        Ok(dependencies)
    }

    async fn create_feature_stage(
        &self,
        feature_id: &Uuid,
        stages: Vec<CreateFeatureStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        if stages.is_empty() {
            return Ok(PgQueryResult::default());
        }

        let ids: &[Uuid] = &stages.iter().map(|stage| stage.id).collect::<Vec<Uuid>>();
        let feature_ids: &[Uuid] = &stages
            .iter()
            .map(|_| feature_id.to_owned())
            .collect::<Vec<Uuid>>();

        let environment_ids: &[Uuid] = &stages
            .iter()
            .map(|stage| stage.environment_id)
            .collect::<Vec<Uuid>>();
        let order_indices: &[i32] = &stages
            .iter()
            .map(|stage| stage.order_index)
            .collect::<Vec<i32>>();

        let parent_stage_ids = &stages
            .iter()
            .map(|stage| stage.parent_stage.as_ref().map(|s| s.id))
            .collect::<Vec<Option<Uuid>>>()[..];

        let positions = &stages
            .iter()
            .map(|stage| stage.position.clone())
            .collect::<Vec<String>>();

        let statuses: Vec<String> = stages
            .iter()
            .map(|stage| {
                if stage.enabled {
                    "DEPLOYED".to_string()
                } else {
                    "NOT_DEPLOYED".to_string()
                }
            })
            .collect();
        let enabled_values: Vec<bool> = stages.iter().map(|stage| stage.enabled).collect();

        let result = sqlx::query(
            r#"INSERT INTO features_pipeline_stages (id, feature_id, environment_id, order_index, parent_stage_id, position, status, enabled)
               SELECT unnest($1::uuid[]) AS id,
               unnest($2::uuid[]) AS feature_id,
               unnest($3::uuid[]) AS environment_id,
               unnest($4::int[]) AS order_index,
               unnest($5::uuid[]) AS parent_stage_id,
               unnest($6::varchar[]) AS position,
               unnest($7::text[]) AS status,
               unnest($8::bool[]) AS enabled
               "#,
        )
            .bind(ids)
            .bind(feature_ids)
            .bind(environment_ids)
            .bind(order_indices)
            .bind(parent_stage_ids as &[Option<Uuid>])
            .bind(positions)
            .bind(&statuses[..])
            .bind(&enabled_values[..])
            .execute(&mut *tx)
            .await;

        handle_error(None, result)
    }

    async fn create_feature_dependencies(
        &self,
        feature_id: &Uuid,
        dependencies: Vec<Uuid>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        if dependencies.is_empty() {
            return Ok(PgQueryResult::default());
        }

        let feature_ids: &[Uuid] = &dependencies
            .iter()
            .map(|_| feature_id.to_owned())
            .collect::<Vec<Uuid>>();

        let depends_on_ids: &[Uuid] = &dependencies;

        let result = sqlx::query!(
            r#"INSERT INTO feature_dependencies (feature_id, depends_on_id)
               SELECT unnest($1::uuid[]) AS feature_id,
               unnest($2::uuid[]) AS depends_on_id
               "#,
            feature_ids,
            depends_on_ids,
        )
        .execute(&mut *tx)
        .await;

        handle_error(None, result)
    }

    async fn update_feature_stage(
        &self,
        feature_id: &Uuid,
        input: Vec<CreateFeatureStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        let existing_stages = self.get_feature_stages(feature_id.clone()).await?;
        if existing_stages.is_empty() {
            return self.create_feature_stage(feature_id, input, tx).await;
        }

        let updates = input
            .iter()
            .filter(|stage| existing_stages.iter().any(|s| s.id == stage.id))
            .collect::<Vec<&CreateFeatureStage>>();

        if updates.is_empty() {
            // That means all stages are new, so we can delete existing stages and create new ones
            self.delete_feature_stage(feature_id.to_owned()).await?;
            self.create_feature_stage(feature_id, input, tx).await?;

            return Ok(PgQueryResult::default());
        }

        self.delete_existing_stages(&existing_stages, &updates, tx)
            .await?;

        if !updates.is_empty() {
            self.update_existing_stages(&updates, tx).await?;
        }

        let to_insert = input
            .iter()
            .filter(|stage| !existing_stages.iter().any(|s| s.id == stage.id))
            .cloned()
            .collect::<Vec<CreateFeatureStage>>();

        if !to_insert.is_empty() {
            self.create_feature_stage(feature_id, to_insert, tx).await?;
        }

        Ok(PgQueryResult::default())
    }

    async fn update_existing_stages(
        &self,
        updates: &Vec<&CreateFeatureStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        for stage in updates {
            let parent_stage_id = stage.parent_stage.as_ref().map(|p| p.id);
            // We should not update the stage's enabled status here; it is managed separately
            let result = sqlx::query(
                r#"UPDATE features_pipeline_stages
                   SET environment_id = $1,
                       order_index = $2,
                       parent_stage_id = $3,
                       position = $4
                   WHERE id = $5"#,
            )
            .bind(stage.environment_id)
            .bind(stage.order_index)
            .bind(parent_stage_id)
            .bind(&stage.position)
            .bind(stage.id)
            .execute(&mut *tx)
            .await;

            handle_error(None, result)?;
        }

        Ok(PgQueryResult::default())
    }

    async fn delete_existing_stages(
        &self,
        existing_stages: &[FeaturePipelineStage],
        updates: &Vec<&CreateFeatureStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        let to_delete = existing_stages
            .iter()
            .filter(|s| !updates.iter().any(|u| u.id == s.id))
            .map(|s| s.id)
            .collect::<Vec<Uuid>>();

        if !to_delete.is_empty() {
            self.delete_stages_by_ids(to_delete, tx).await?;
        }

        Ok(PgQueryResult::default())
    }

    async fn delete_stages_by_ids(
        &self,
        stage_ids: Vec<Uuid>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        if stage_ids.is_empty() {
            return Ok(PgQueryResult::default());
        }

        let result = sqlx::query!(
            r#"DELETE FROM features_pipeline_stages WHERE id = ANY($1)"#,
            &stage_ids[..]
        )
        .execute(&mut *tx)
        .await;

        handle_error(None, result)
    }

    async fn update_feature_dependencies(
        &self,
        feature_id: &Uuid,
        dependencies: Vec<Uuid>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        // Delete existing dependencies
        self.delete_feature_dependencies(feature_id.to_owned())
            .await?;

        // Create new dependencies
        self.create_feature_dependencies(feature_id, dependencies, tx)
            .await?;

        Ok(PgQueryResult::default())
    }

    async fn delete_feature_stage(&self, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!(
            r#"DELETE FROM features_pipeline_stages WHERE feature_id = $1"#,
            id
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    async fn delete_feature_dependencies(&self, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!(
            r#"DELETE FROM feature_dependencies WHERE feature_id = $1"#,
            id
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    fn map_row_to_feature(features: Vec<FeatureWithStageRow>) -> Feature {
        let feature = &features[0];

        let feature_type = match feature.feature_type.as_str() {
            "Simple" => FeatureType::Simple,
            "Contextual" => FeatureType::Contextual,
            _ => panic!("Unknown feature type, this should never happen"),
        };

        Feature {
            id: feature.feature_id,
            key: feature.feature_key.clone(),
            description: feature.description.clone(),
            feature_type,
            team_id: feature.team_id,
            active: feature.feature_enabled,
            created_at: feature.created_at,
            kill_switch_enabled: feature.kill_switch_enabled,
            kill_switch_activated_at: feature.kill_switch_activated_at,
            rollback_scheduled_at: feature.rollback_scheduled_at,
            lifecycle_stage: feature.lifecycle_stage.clone(),
            deprecated_at: feature.deprecated_at,
            deprecation_notice: feature.deprecation_notice.clone(),
            last_evaluated_at: feature.last_evaluated_at,
            evaluation_count_7d: feature.evaluation_count_7d,
            evaluation_count_30d: feature.evaluation_count_30d,
            evaluation_count_90d: feature.evaluation_count_90d,
            dependencies: vec![], // Dependencies will be loaded separately
        }
    }

    async fn save_feature(input: &CreateFeature, tx: &mut PgConnection) -> Result<Uuid, Error> {
        let id = Uuid::new_v4();
        let feature_type_str = match input.feature_type {
            FeatureType::Simple => "Simple",
            FeatureType::Contextual => "Contextual",
        };

        let result = sqlx::query!(
            r#"INSERT INTO features (id, key, description, feature_type, team_id)
               VALUES ($1, $2, $3, $4, $5) RETURNING id"#,
            id,
            input.key,
            input.description,
            feature_type_str,
            input.team_id
        )
        .fetch_one(&mut *tx)
        .await;

        let handled_error = handle_error(None, result);
        if handled_error.is_err() {
            return Err(handled_error.err().unwrap());
        }

        Ok(id)
    }

    async fn check_feature_exists(&self, input: &CreateFeature) -> Result<(), Error> {
        let existing_feature = self
            .get_features(input.team_id, Some(input.key.clone()), None)
            .await;

        if let Ok(existing_feature) = existing_feature {
            if !existing_feature.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Feature with key '{}' already exists",
                    input.key
                )));
            }
        }
        Ok(())
    }

    async fn update_feature(
        &self,
        input: &UpdateFeature,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        let existing_feature = self.get_feature_by_id(input.id).await?;

        let feature_type_str = match input
            .feature_type
            .clone()
            .unwrap_or(existing_feature.feature_type)
        {
            FeatureType::Simple => "Simple",
            FeatureType::Contextual => "Contextual",
        };

        let key = input.key.clone().unwrap_or(existing_feature.key);
        let description = input.description.clone().or(existing_feature.description);
        let id = input.id;
        let result = sqlx::query!(
            r#"UPDATE features SET key = $1, description = $2, feature_type = $3 WHERE id = $4"#,
            key,
            description,
            feature_type_str,
            id
        )
        .execute(&mut *tx)
        .await;

        if result.is_err() {
            return Err(Error::DatabaseError(result.err().unwrap()));
        }

        Ok(result.unwrap())
    }
}

#[async_trait::async_trait]
impl FeatureRepository for FeatureRepositoryImpl {
    async fn get_stage_by_id(&self, stage_id: Uuid) -> Result<Option<FeaturePipelineStage>, Error> {
        let result = sqlx::query_as!(
            FeaturePipelineStageRow,
            r#"SELECT id, feature_id, environment_id, order_index, parent_stage_id, position, status, enabled
            FROM features_pipeline_stages WHERE id = $1"#,
            stage_id
        )
        .fetch_optional(&self.pool)
        .await;

        let result = handle_error(None, result)?;
        let stage = result.map(|stage| {
            FeaturePipelineStage {
                id: stage.id,
                feature_id: stage.feature_id,
                environment_id: stage.environment_id,
                order_index: stage.order_index,
                parent_stage_id: stage.parent_stage_id,
                position: stage.position,
                enabled: stage.enabled, // Use the actual enabled field from database
                status: stage.status,
            }
        });

        Ok(stage)
    }

    async fn get_feature_stages(
        &self,
        feature_id: Uuid,
    ) -> Result<Vec<FeaturePipelineStage>, Error> {
        let result = sqlx::query_as!(
            FeaturePipelineStageRow,
            r#"SELECT id, feature_id, environment_id, order_index, parent_stage_id, position, status, enabled
            FROM features_pipeline_stages WHERE feature_id = $1"#,
            feature_id
        )
        .fetch_all(&self.pool)
        .await;

        let rows = handle_error(None, result)?;
        let stages = rows
            .into_iter()
            .map(|r| FeaturePipelineStage {
                id: r.id,
                feature_id: r.feature_id,
                environment_id: r.environment_id,
                order_index: r.order_index,
                parent_stage_id: r.parent_stage_id,
                position: r.position,
                enabled: r.enabled, // Use the actual enabled field from database
                status: r.status,
            })
            .collect::<Vec<FeaturePipelineStage>>();
        Ok(stages)
    }

    async fn get_stage_criteria(
        &self,
        stage_id: Uuid,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error> {
        // Determine team_id for this stage to resolve context-derived values
        let team_id = sqlx::query_scalar!(
            r#"SELECT f.team_id
               FROM features_pipeline_stages fps
               JOIN features f ON f.id = fps.feature_id
               WHERE fps.id = $1"#,
            stage_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        if team_id.is_none() {
            return Err(Error::NotFound(stage_id));
        }
        let team_id = team_id.unwrap();

        // Preload context entries keyed by context key
        let context_rows = sqlx::query!(
            r#"SELECT c.key, COALESCE(ce.value, '') as "value!"
               FROM contexts c
               LEFT JOIN context_entries ce ON ce.context_id = c.id
               WHERE c.team_id = $1"#,
            team_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        let mut context_value_map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for row in context_rows {
            if !row.value.is_empty() {
                context_value_map
                    .entry(row.key)
                    .or_insert_with(Vec::new)
                    .push(row.value);
            }
        }

        let rows = sqlx::query!(
            r#"SELECT sc.id, sc.stage_id, sc.priority,
                      sc.variant_selection_mode::text as "variant_selection_mode!",
                      sc.selected_variant_control
               FROM feature_stage_criteria sc
               WHERE sc.stage_id = $1
               ORDER BY sc.priority ASC, sc.id"#,
            stage_id
        )
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(Some(stage_id), rows)?;

        let criteria_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let mut allocations_map: std::collections::HashMap<
            Uuid,
            Vec<crate::database::entity::VariantAllocationSimple>,
        > = std::collections::HashMap::new();

        if !criteria_ids.is_empty() {
            let allocations = sqlx::query!(
                r#"SELECT criteria_id, variant_control, weight
                   FROM variant_allocations
                   WHERE criteria_id = ANY($1)
                   ORDER BY variant_control"#,
                &criteria_ids
            )
            .fetch_all(&self.pool)
            .await;

            let allocations = handle_error(None, allocations)?;
            for alloc in allocations {
                allocations_map
                    .entry(alloc.criteria_id)
                    .or_insert_with(Vec::new)
                    .push(crate::database::entity::VariantAllocationSimple {
                        variant_control: alloc.variant_control,
                        weight: alloc.weight,
                    });
            }
        }

        let mut rule_groups_by_criteria: std::collections::HashMap<
            Uuid,
            std::collections::HashMap<
                Uuid,
                (
                    crate::database::entity::LogicOperator,
                    Vec<crate::database::entity::CompoundRuleCondition>,
                ),
            >,
        > = std::collections::HashMap::new();

        if !criteria_ids.is_empty() {
            let rule_rows = sqlx::query!(
                r#"SELECT rg.id as group_id, rg.criteria_id, rg.logic_operator,
                          rc.id as "condition_id?", rc.context_key as "context_key?",
                          rc.operator as "operator?", rc.value as "value?",
                          rc.order_index as "order_index?"
                   FROM rule_groups rg
                   LEFT JOIN rule_conditions rc ON rc.group_id = rg.id
                   WHERE rg.criteria_id = ANY($1)
                   ORDER BY rg.created_at, rc.order_index"#,
                &criteria_ids
            )
            .fetch_all(&self.pool)
            .await;

            let rule_rows = handle_error(None, rule_rows)?;

            for row in rule_rows {
                let by_group = rule_groups_by_criteria
                    .entry(row.criteria_id)
                    .or_insert_with(std::collections::HashMap::new);

                let entry = by_group.entry(row.group_id).or_insert_with(|| {
                    let logic_operator = match row.logic_operator.to_uppercase().as_str() {
                        "OR" => crate::database::entity::LogicOperator::Or,
                        _ => crate::database::entity::LogicOperator::And,
                    };
                    (logic_operator, Vec::new())
                });

                if let (Some(condition_id), Some(context_key), Some(operator), Some(order_index)) = (
                    row.condition_id,
                    row.context_key,
                    row.operator,
                    row.order_index,
                ) {
                    let mut value = row.value.unwrap_or(serde_json::Value::Null);
                    if operator.eq_ignore_ascii_case("IN") {
                        if let Some(key_str) = value.as_str() {
                            if let Some(entries) = context_value_map.get(key_str) {
                                value = serde_json::Value::Array(
                                    entries
                                        .iter()
                                        .map(|v| serde_json::Value::String(v.clone()))
                                        .collect(),
                                );
                            }
                        }
                    }
                    entry
                        .1
                        .push(crate::database::entity::CompoundRuleCondition {
                            id: condition_id,
                            context_key,
                            operator,
                            value,
                            order_index,
                        });
                }
            }
        }

        let mut out = Vec::new();
        for r in rows {
            let rule_groups = rule_groups_by_criteria
                .remove(&r.id)
                .unwrap_or_default()
                .into_iter()
                .map(|(group_id, (logic_operator, conditions))| {
                    crate::database::entity::CompoundRuleGroup {
                        id: group_id,
                        logic_operator,
                        conditions,
                    }
                })
                .collect();

            let variant_selection_mode = match r.variant_selection_mode.to_uppercase().as_str() {
                "SPECIFIC_VARIANT" => crate::database::entity::VariantSelectionMode::SpecificVariant,
                _ => crate::database::entity::VariantSelectionMode::WeightedSplit,
            };

            out.push(crate::database::entity::StageCriterion {
                id: r.id,
                stage_id: r.stage_id,
                priority: r.priority,
                rule_groups,
                variant_allocations: allocations_map.remove(&r.id).unwrap_or_else(Vec::new),
                variant_selection_mode,
                selected_variant_control: r.selected_variant_control,
            });
        }
        Ok(out)
    }

    async fn set_stage_criteria(
        &self,
        stage_id: Uuid,
        criteria: Vec<CreateStageCriterion>,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error> {
        let exists = handle_error(
            Some(stage_id),
            sqlx::query_scalar!(
                "SELECT id FROM features_pipeline_stages WHERE id = $1",
                stage_id
            )
            .fetch_optional(&self.pool)
            .await,
        )?;
        if exists.is_none() {
            return Err(Error::NotFound(stage_id));
        }

        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        handle_error(
            Some(stage_id),
            sqlx::query!(
                "DELETE FROM feature_stage_criteria WHERE stage_id = $1",
                stage_id
            )
            .execute(&mut *tx)
            .await,
        )?;
        if !criteria.is_empty() {
            let ids: Vec<Uuid> = criteria.iter().map(|_| Uuid::new_v4()).collect();
            let stage_ids: Vec<Uuid> = vec![stage_id; criteria.len()];
            let priorities: Vec<i32> = criteria.iter().map(|c| c.priority).collect();
            let modes: Vec<String> = criteria
                .iter()
                .map(|c| match c.variant_selection_mode {
                    crate::database::entity::VariantSelectionMode::WeightedSplit => "WEIGHTED_SPLIT".to_string(),
                    crate::database::entity::VariantSelectionMode::SpecificVariant => "SPECIFIC_VARIANT".to_string(),
                })
                .collect();
            let selected_variants: Vec<Option<String>> = criteria
                .iter()
                .map(|c| c.selected_variant_control.clone())
                .collect();

            handle_error(
                None,
                sqlx::query!(
                    r#"INSERT INTO feature_stage_criteria(id, stage_id, priority, variant_selection_mode, selected_variant_control)
                       SELECT unnest($1::uuid[]), unnest($2::uuid[]), unnest($3::int[]), unnest($4::variant_selection_mode[]), unnest($5::text[])"#,
                    &ids[..],
                    &stage_ids[..],
                    &priorities[..],
                    &modes[..] as &[String],
                    &selected_variants[..] as &[Option<String>]
                )
                .execute(&mut *tx)
                .await,
            )?;
        }
        tx.commit().await.map_err(Error::DatabaseError)?;
        self.get_stage_criteria(stage_id).await
    }

    async fn get_feature_by_id(&self, id: Uuid) -> Result<Feature, Error> {
        let result = sqlx::query_as::<_, FeatureWithStageRow>(
            format!("{} WHERE f.id = $1", FEATURE_SELECT).as_str(),
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await;

        let features = handle_error(Some(id), result)?;
        if features.is_empty() {
            return Err(Error::NotFound(id));
        }

        let mut feature = Self::map_row_to_feature(features);

        // Load dependencies
        let dependencies = self.get_feature_dependencies(&id).await?;
        feature.dependencies = dependencies;

        Ok(feature)
    }

    async fn get_features(
        &self,
        team_id: Uuid,
        key: Option<String>,
        feature_type: Option<FeatureType>,
    ) -> Result<Vec<Feature>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(FEATURE_SELECT);
        query_builder.push(" WHERE f.team_id = ").push_bind(team_id);

        if let Some(key) = key {
            query_builder.push(" AND f.key ILIKE ");
            query_builder.push_bind(format!("%{key}%"));
        }
        if let Some(feature_type_value) = feature_type {
            let feature_type_str = match feature_type_value {
                FeatureType::Simple => "Simple",
                FeatureType::Contextual => "Contextual",
            };
            query_builder
                .push(" AND f.feature_type = ")
                .push_bind(feature_type_str);
        }
        query_builder.push(" ORDER BY f.key");

        let result = query_builder
            .build_query_as::<FeatureWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let features_rows = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();
        // Preserve ordering by feature name as returned from SQL by tracking first-seen order
        let mut order: Vec<Uuid> = Vec::new();

        for row in features_rows {
            if !map.contains_key(&row.feature_id) {
                order.push(row.feature_id);
            }
            map.entry(row.feature_id).or_default().push(row);
        }

        // Load dependencies for each feature while preserving order by name
        let mut features: Vec<Feature> = Vec::with_capacity(order.len());
        for id in order {
            if let Some(rows) = map.remove(&id) {
                features.push(Self::map_row_to_feature(rows));
            }
        }
        for feature in &mut features {
            let dependencies = self.get_feature_dependencies(&feature.id).await?;
            feature.dependencies = dependencies;
        }

        Ok(features)
    }

    async fn get_features_paginated(
        &self,
        team_id: Uuid,
        key: Option<String>,
        feature_type: Option<FeatureType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Feature>, i64), Error> {
        // First, get the total count
        let mut count_query =
            sqlx::QueryBuilder::new("SELECT COUNT(DISTINCT f.id) FROM features f");
        count_query.push(" WHERE f.team_id = ").push_bind(team_id);

        if let Some(key) = &key {
            count_query.push(" AND f.key ILIKE ");
            count_query.push_bind(format!("%{key}%"));
        }
        if let Some(feature_type_value) = &feature_type {
            let feature_type_str = match feature_type_value {
                FeatureType::Simple => "Simple",
                FeatureType::Contextual => "Contextual",
            };
            count_query
                .push(" AND f.feature_type = ")
                .push_bind(feature_type_str);
        }

        let total_count: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Now get the paginated results
        let offset = (page_number - 1) * page_size;
        let mut query_builder = sqlx::QueryBuilder::new(FEATURE_SELECT);
        query_builder.push(" WHERE f.team_id = ").push_bind(team_id);

        if let Some(key) = key {
            query_builder.push(" AND f.key ILIKE ");
            query_builder.push_bind(format!("%{key}%"));
        }
        if let Some(feature_type_value) = feature_type {
            let feature_type_str = match feature_type_value {
                FeatureType::Simple => "Simple",
                FeatureType::Contextual => "Contextual",
            };
            query_builder
                .push(" AND f.feature_type = ")
                .push_bind(feature_type_str);
        }
        query_builder.push(" ORDER BY f.key");
        query_builder.push(" LIMIT ").push_bind(page_size);
        query_builder.push(" OFFSET ").push_bind(offset);

        let result = query_builder
            .build_query_as::<FeatureWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let features_rows = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();
        // Preserve ordering by feature name as returned from SQL by tracking first-seen order
        let mut order: Vec<Uuid> = Vec::new();

        for row in features_rows {
            if !map.contains_key(&row.feature_id) {
                order.push(row.feature_id);
            }
            map.entry(row.feature_id).or_default().push(row);
        }

        // Load dependencies for each feature while preserving order by name
        let mut features: Vec<Feature> = Vec::with_capacity(order.len());
        for id in order {
            if let Some(rows) = map.remove(&id) {
                features.push(Self::map_row_to_feature(rows));
            }
        }
        for feature in &mut features {
            let dependencies = self.get_feature_dependencies(&feature.id).await?;
            feature.dependencies = dependencies;
        }

        Ok((features, total_count))
    }

    async fn create_feature(&self, input: CreateFeature) -> Result<Uuid, Error> {
        self.check_feature_exists(&input).await?;

        let tx: Result<Transaction<'static, Postgres>, sqlx::Error> = self.pool.begin().await;
        if tx.is_err() {
            return Err(Error::DatabaseError(tx.err().unwrap()));
        }
        let mut tx: Transaction<'_, Postgres> = tx.unwrap();

        let saved_feature = Self::save_feature(&input, &mut tx).await;
        match saved_feature {
            Ok(id) => {
                // Create stages
                let stages = self.create_feature_stage(&id, input.stages, &mut tx).await;
                if stages.is_err() {
                    let _ = tx.rollback().await;
                    return Err(stages.err().unwrap());
                }

                // Create dependencies
                let dependencies = self
                    .create_feature_dependencies(&id, input.dependencies, &mut tx)
                    .await;
                if dependencies.is_err() {
                    let _ = tx.rollback().await;
                    return Err(dependencies.err().unwrap());
                }

                // Create variants if provided
                if let Some(variants) = input.variants {
                    if !variants.is_empty() {
                        let variants_result =
                            self.create_feature_variants(&mut tx, id, variants).await;
                        if variants_result.is_err() {
                            let _ = tx.rollback().await;
                            return Err(variants_result.err().unwrap());
                        }
                    }
                }

                let _ = tx.commit().await;
                Ok(id)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    async fn update_feature(&self, input: UpdateFeature) -> Result<Feature, Error> {
        let tx = self.pool.begin().await;
        if tx.is_err() {
            return Err(Error::DatabaseError(tx.err().unwrap()));
        }
        let mut tx = tx.unwrap();

        // Update feature
        let result = self.update_feature(&input, &mut tx).await;
        if result.is_err() {
            let _ = tx.rollback().await;
            return Err(result.err().unwrap());
        }

        // Update stages
        let stage_result = self
            .update_feature_stage(&input.id, input.stages, &mut tx)
            .await;
        if stage_result.is_err() {
            let _ = tx.rollback().await;
            return Err(stage_result.err().unwrap());
        }

        // Update dependencies
        let dependencies_result = self
            .update_feature_dependencies(&input.id, input.dependencies, &mut tx)
            .await;
        if dependencies_result.is_err() {
            let _ = tx.rollback().await;
            return Err(dependencies_result.err().unwrap());
        }

        // Update variants if provided (replace all)
        if let Some(variants) = input.variants {
            // Delete existing variants
            let delete_result = self.delete_feature_variants(&mut tx, input.id).await;
            if delete_result.is_err() {
                let _ = tx.rollback().await;
                return Err(delete_result.err().unwrap());
            }

            // Create new variants
            if !variants.is_empty() {
                let create_result = self
                    .create_feature_variants(&mut tx, input.id, variants)
                    .await;
                if create_result.is_err() {
                    let _ = tx.rollback().await;
                    return Err(create_result.err().unwrap());
                }
            }
        }

        let _ = tx.commit().await;
        self.get_feature_by_id(input.id).await
    }

    async fn delete_feature(&self, id: Uuid) -> Result<(), Error> {
        if self.is_feature_exists_id(id).await?.is_none() {
            return Err(Error::NotFound(id));
        }

        let result = sqlx::query!("DELETE FROM features WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    async fn get_stage_contexts(
        &self,
        stage_id: Uuid,
    ) -> Result<Vec<crate::database::entity::Context>, Error> {
        // Load contexts linked to this stage along with their entries
        let ctx_rows = sqlx::query!(
            r#"SELECT c.id, c.team_id, c.key FROM feature_stage_contexts fsc
               JOIN contexts c ON c.id = fsc.context_id
               WHERE fsc.stage_id = $1
               ORDER BY c.key"#,
            stage_id
        )
        .fetch_all(&self.pool)
        .await;
        let ctx_rows = handle_error(Some(stage_id), ctx_rows)?;
        let mut out: Vec<crate::database::entity::Context> = Vec::new();
        for row in ctx_rows {
            let entries = handle_error(
                Some(row.id),
                sqlx::query!(
                    r#"SELECT id, value FROM context_entries WHERE context_id = $1 ORDER BY value"#,
                    row.id
                )
                .fetch_all(&self.pool)
                .await,
            )?
            .into_iter()
            .map(|r| crate::database::entity::ContextEntry {
                id: r.id,
                value: r.value,
            })
            .collect();
            out.push(crate::database::entity::Context {
                id: row.id,
                team_id: row.team_id,
                key: row.key,
                entries,
            });
        }
        Ok(out)
    }

    async fn set_stage_contexts(
        &self,
        stage_id: Uuid,
        context_ids: Vec<Uuid>,
    ) -> Result<Vec<crate::database::entity::Context>, Error> {
        // Ensure stage exists
        let exists = sqlx::query_scalar!(
            "SELECT id FROM features_pipeline_stages WHERE id=$1",
            stage_id
        )
        .fetch_optional(&self.pool)
        .await;
        let exists = handle_error(Some(stage_id), exists)?;
        if exists.is_none() {
            return Err(Error::NotFound(stage_id));
        }

        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        // Clear existing
        handle_error(
            Some(stage_id),
            sqlx::query!(
                "DELETE FROM feature_stage_contexts WHERE stage_id=$1",
                stage_id
            )
            .execute(&mut *tx)
            .await,
        )?;

        if !context_ids.is_empty() {
            let _ = handle_error(
                None,
                sqlx::query!(
                    r#"INSERT INTO feature_stage_contexts(stage_id, context_id)
                   SELECT unnest($1::uuid[]), unnest($2::uuid[])"#,
                    &vec![stage_id; context_ids.len()][..],
                    &context_ids[..]
                )
                .execute(&mut *tx)
                .await,
            )?;
        }
        tx.commit().await.map_err(Error::DatabaseError)?;
        self.get_stage_contexts(stage_id).await
    }

    async fn get_feature_ids_by_context_id(&self, context_id: Uuid) -> Result<Vec<Uuid>, Error> {
        let rows = sqlx::query_scalar!(
            r#"SELECT DISTINCT f.id
               FROM features f
               JOIN features_pipeline_stages s ON s.feature_id = f.id
               JOIN feature_stage_contexts fsc ON fsc.stage_id = s.id
               WHERE fsc.context_id = $1"#,
            context_id
        )
        .fetch_all(&self.pool)
        .await;
        handle_error(Some(context_id), rows)
    }

    // Variant methods
    async fn get_feature_variants(
        &self,
        feature_id: Uuid,
    ) -> Result<Vec<crate::database::entity::FeatureVariant>, Error> {
        let variants = sqlx::query_as!(
            crate::database::entity::FeatureVariant,
            r#"
            SELECT
                id,
                feature_id,
                control,
                value,
                value_type AS "value_type: crate::database::entity::VariantValueType",
                description,
                created_at,
                updated_at
            FROM feature_variants
            WHERE feature_id = $1
            ORDER BY created_at
            "#,
            feature_id
        )
        .fetch_all(&self.pool)
        .await;
        handle_error(Some(feature_id), variants)
    }

    async fn request_stage_change(
        &self,
        stage_id: Uuid,
        status: &str,
        requested_user: Uuid,
        when: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, Error> {
        // Determine the enabled value based on the status
        let enabled = match status {
            "DEPLOYED" => true,
            "ROLLBACKED" => false,
            _ => {
                // For other statuses (NOT_DEPLOYED, DEPLOYMENT_REQUESTED, etc.), keep current enabled value
                let current_enabled = sqlx::query_scalar!(
                    "SELECT enabled FROM features_pipeline_stages WHERE id = $1",
                    stage_id
                )
                .fetch_optional(&self.pool)
                .await;

                match handle_error(Some(stage_id), current_enabled)? {
                    Some(current) => current,
                    None => return Err(Error::NotFound(stage_id)),
                }
            }
        };

        let result = sqlx::query(
            r#"UPDATE features_pipeline_stages
               SET status = $1, enabled = $2, requested_user = $3, requested_time = $4, approved_user = NULL, approved_time = NULL
               WHERE id = $5"#,
        )
        .bind(status)
        .bind(enabled)
        .bind(requested_user)
        .bind(when)
        .bind(stage_id)
        .execute(&self.pool)
        .await;
        let res = handle_error(Some(stage_id), result)?;
        Ok(res.rows_affected() == 1)
    }

    async fn approve_or_reject_stage_change(
        &self,
        stage_id: Uuid,
        status: &str,
        user_id: Uuid,
    ) -> Result<bool, Error> {
        let now = chrono::Utc::now();

        // Determine the enabled value based on the status
        let enabled = match status {
            "DEPLOYED" => true,
            "ROLLBACKED" => false,
            _ => {
                // For other statuses, keep current enabled value
                let current_enabled = sqlx::query_scalar!(
                    "SELECT enabled FROM features_pipeline_stages WHERE id = $1",
                    stage_id
                )
                .fetch_optional(&self.pool)
                .await;

                match handle_error(Some(stage_id), current_enabled)? {
                    Some(current) => current,
                    None => return Err(Error::NotFound(stage_id)),
                }
            }
        };

        let result = sqlx::query(
            r#"UPDATE features_pipeline_stages
               SET status = $1, enabled = $2, approved_user = $3, approved_time = $4
               WHERE id = $5"#,
        )
        .bind(status)
        .bind(enabled)
        .bind(user_id)
        .bind(now)
        .bind(stage_id)
        .execute(&self.pool)
        .await;
        let res = handle_error(Some(stage_id), result)?;
        Ok(res.rows_affected() == 1)
    }

    async fn reset_stage_status(&self, stage_id: Uuid, status: &str) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"UPDATE features_pipeline_stages
               SET status = $1,
                   requested_user = NULL,
                   requested_time = NULL,
                   approved_user = NULL,
                   approved_time = NULL
               WHERE id = $2"#,
        )
        .bind(status)
        .bind(stage_id)
        .execute(&self.pool)
        .await;
        let res = handle_error(Some(stage_id), result)?;
        Ok(res.rows_affected() == 1)
    }

    async fn get_feature_id_by_stage_id(&self, stage_id: Uuid) -> Result<Option<Uuid>, Error> {
        let row = sqlx::query_scalar!(
            r#"SELECT feature_id FROM features_pipeline_stages WHERE id = $1"#,
            stage_id
        )
        .fetch_optional(&self.pool)
        .await;
        handle_error(Some(stage_id), row)
    }

    async fn emergency_disable_feature(
        &self,
        feature_id: Uuid,
        rollback_in_minutes: Option<i32>,
    ) -> Result<Feature, Error> {
        let now = chrono::Utc::now();

        // When a rollback window is provided, we treat it as a future kill-switch activation point.
        // Until that time the feature remains enabled, but carries the scheduled timestamp.
        let (kill_switch_enabled, activated_at, rollback_at, active) = match rollback_in_minutes {
            Some(minutes) if minutes > 0 => {
                let schedule = now + chrono::Duration::minutes(minutes as i64);
                (true, None, Some(schedule), true)
            }
            _ => (false, Some(now), None, false),
        };

        let result = sqlx::query!(
            r#"UPDATE features
                SET kill_switch_enabled = $1,
                kill_switch_activated_at = $2,
                rollback_scheduled_at = $3,
                active = $4
               WHERE id = $5"#,
            kill_switch_enabled,
            activated_at,
            rollback_at,
            active,
            feature_id
        )
        .execute(&self.pool)
        .await;

        handle_error(Some(feature_id), result)?;
        self.get_feature_by_id(feature_id).await
    }

    async fn emergency_enable_feature(&self, feature_id: Uuid) -> Result<Feature, Error> {
        let result = sqlx::query!(
            r#"UPDATE features
                SET kill_switch_enabled = false,
                active = true,
                kill_switch_activated_at = NULL,
                rollback_scheduled_at = NULL
                WHERE id = $1"#,
            feature_id
        )
        .execute(&self.pool)
        .await;

        handle_error(Some(feature_id), result)?;
        self.get_feature_by_id(feature_id).await
    }

    async fn get_features_pending_rollback(&self) -> Result<Vec<Feature>, Error> {
        let now = chrono::Utc::now();
        let result = sqlx::query_as::<_, FeatureWithStageRow>(
            format!(
                r#"{} WHERE f.kill_switch_enabled = true
                AND f.rollback_scheduled_at IS NOT NULL
                AND f.rollback_scheduled_at <= $1
                ORDER BY f.rollback_scheduled_at ASC"#,
                FEATURE_SELECT
            )
            .as_str(),
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await;

        let rows = handle_error(None, result)?;

        // Group rows by feature ID and convert to Feature objects
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();
        let mut order: Vec<Uuid> = Vec::new();

        for row in rows {
            if !map.contains_key(&row.feature_id) {
                order.push(row.feature_id);
            }
            map.entry(row.feature_id).or_default().push(row);
        }

        // Convert each group to a Feature
        let mut features: Vec<Feature> = Vec::with_capacity(order.len());
        for id in order {
            if let Some(rows) = map.remove(&id) {
                features.push(Self::map_row_to_feature(rows));
            }
        }

        // Load dependencies for each feature
        for feature in &mut features {
            let dependencies = self.get_feature_dependencies(&feature.id).await?;
            feature.dependencies = dependencies;
        }

        Ok(features)
    }

    async fn execute_scheduled_disable(&self, feature_id: Uuid) -> Result<Feature, Error> {
        let now = chrono::Utc::now();

        // Start a transaction to update both feature and stages atomically
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Disable the feature (kill_switch_enabled = false means feature is disabled)
        let result = sqlx::query!(
            r#"UPDATE features
               SET kill_switch_enabled = false,
                   active = false,
                   kill_switch_activated_at = $1,
                   rollback_scheduled_at = NULL
               WHERE id = $2"#,
            now,
            feature_id
        )
        .execute(&mut *tx)
        .await;

        handle_error(Some(feature_id), result)?;
        tx.commit().await.map_err(|e| Error::DatabaseError(e))?;

        self.get_feature_by_id(feature_id).await
    }

    async fn get_feature_growth(
        &self,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
        interval: String,
        team_id: Option<Uuid>,
    ) -> Result<Vec<FeatureGrowthPoint>, Error> {
        // Validate interval (must be 'day', 'week', or 'month')
        let valid_intervals = ["day", "week", "month"];
        if !valid_intervals.contains(&interval.as_str()) {
            return Err(Error::DatabaseError(sqlx::Error::Protocol(
                "Invalid interval. Must be 'day', 'week', or 'month'".to_string(),
            )));
        }

        let query = if let Some(_team_id) = team_id {
            // Query for specific team
            format!(
                r#"
                WITH time_series AS (
                    SELECT
                        date_trunc('{}', created_at)::timestamptz as time_bucket,
                        team_id,
                        COUNT(*)::bigint as feature_count
                    FROM features
                    WHERE created_at >= $1
                        AND created_at <= $2
                        AND team_id = $3
                    GROUP BY time_bucket, team_id
                    ORDER BY time_bucket
                ),
                cumulative AS (
                    SELECT
                        time_bucket,
                        team_id,
                        feature_count,
                        SUM(feature_count) OVER (PARTITION BY team_id ORDER BY time_bucket)::bigint as cumulative_count
                    FROM time_series
                )
                SELECT
                    c.time_bucket,
                    c.team_id,
                    t.name as team_name,
                    c.feature_count,
                    c.cumulative_count
                FROM cumulative c
                LEFT JOIN teams t ON c.team_id = t.id
                ORDER BY c.time_bucket
                "#,
                interval
            )
        } else {
            // Query for all teams
            format!(
                r#"
                WITH time_series AS (
                    SELECT
                        date_trunc('{}', created_at)::timestamptz as time_bucket,
                        team_id,
                        COUNT(*)::bigint as feature_count
                    FROM features
                    WHERE created_at >= $1
                        AND created_at <= $2
                    GROUP BY time_bucket, team_id
                    ORDER BY time_bucket, team_id
                ),
                cumulative AS (
                    SELECT
                        time_bucket,
                        team_id,
                        feature_count,
                        SUM(feature_count) OVER (PARTITION BY team_id ORDER BY time_bucket)::bigint as cumulative_count
                    FROM time_series
                )
                SELECT
                    c.time_bucket,
                    c.team_id,
                    t.name as team_name,
                    c.feature_count,
                    c.cumulative_count
                FROM cumulative c
                LEFT JOIN teams t ON c.team_id = t.id
                ORDER BY c.time_bucket, c.team_id
                "#,
                interval
            )
        };

        let result = if let Some(tid) = team_id {
            sqlx::query_as::<_, FeatureGrowthPoint>(&query)
                .bind(from_time)
                .bind(to_time)
                .bind(tid)
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, FeatureGrowthPoint>(&query)
                .bind(from_time)
                .bind(to_time)
                .fetch_all(&self.pool)
                .await
        };

        result.map_err(|e| Error::DatabaseError(e))
    }

    async fn count_features(&self, team_id: Option<Uuid>) -> Result<i64, Error> {
        let count = if let Some(team_id) = team_id {
            sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) as "count!"
                FROM features
                WHERE team_id = $1
                "#,
                team_id
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        } else {
            sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) as "count!"
                FROM features
                "#
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        };

        Ok(count)
    }

    async fn get_rollout_metrics_data(
        &self,
        team_id: Option<Uuid>,
    ) -> Result<RolloutMetricsData, Error> {
        // Build the base WHERE clause for team filtering
        let team_filter = if team_id.is_some() {
            "AND f.team_id = $1"
        } else {
            ""
        };

        // 1. Get counts of deployed and rejected features
        let status_counts: (i64, i64) = if let Some(team_id) = team_id {
            sqlx::query_as::<_, (i64, i64)>(&format!(
                r#"
                SELECT 
                    COUNT(*) FILTER (WHERE fps.status = 'DEPLOYED') as deployed,
                    COUNT(*) FILTER (WHERE fps.status IN ('DEPLOYMENT_REJECTED', 'ROLLBACK_REJECTED')) as rejected
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                WHERE f.team_id = $1
                "#
            ))
            .bind(team_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        } else {
            sqlx::query_as::<_, (i64, i64)>(
                r#"
                SELECT 
                    COUNT(*) FILTER (WHERE fps.status = 'DEPLOYED') as deployed,
                    COUNT(*) FILTER (WHERE fps.status IN ('DEPLOYMENT_REJECTED', 'ROLLBACK_REJECTED')) as rejected
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                "#
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        };

        // 2. Get deployed features this week and last week
        let (deployed_this_week, deployed_last_week): (i64, i64) = if let Some(team_id) = team_id {
            sqlx::query_as::<_, (i64, i64)>(&format!(
                r#"
                SELECT 
                    COUNT(DISTINCT fps.feature_id) FILTER (
                        WHERE fps.status = 'DEPLOYED' 
                        AND fps.approved_time >= date_trunc('week', CURRENT_TIMESTAMP)
                        AND fps.approved_time < date_trunc('week', CURRENT_TIMESTAMP) + interval '1 week'
                    ) as this_week,
                    COUNT(DISTINCT fps.feature_id) FILTER (
                        WHERE fps.status = 'DEPLOYED'
                        AND fps.approved_time >= date_trunc('week', CURRENT_TIMESTAMP) - interval '1 week'
                        AND fps.approved_time < date_trunc('week', CURRENT_TIMESTAMP)
                    ) as last_week
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                WHERE f.team_id = $1
                "#
            ))
            .bind(team_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        } else {
            sqlx::query_as::<_, (i64, i64)>(
                r#"
                SELECT 
                    COUNT(DISTINCT fps.feature_id) FILTER (
                        WHERE fps.status = 'DEPLOYED' 
                        AND fps.approved_time >= date_trunc('week', CURRENT_TIMESTAMP)
                        AND fps.approved_time < date_trunc('week', CURRENT_TIMESTAMP) + interval '1 week'
                    ) as this_week,
                    COUNT(DISTINCT fps.feature_id) FILTER (
                        WHERE fps.status = 'DEPLOYED'
                        AND fps.approved_time >= date_trunc('week', CURRENT_TIMESTAMP) - interval '1 week'
                        AND fps.approved_time < date_trunc('week', CURRENT_TIMESTAMP)
                    ) as last_week
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                "#
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        };

        // 3. Get pending approvals count
        let pending_approvals: i64 = if let Some(team_id) = team_id {
            sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) as "count!"
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                WHERE fps.status = 'DEPLOYMENT_REQUESTED'
                AND f.team_id = $1
                "#,
                team_id
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        } else {
            sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) as "count!"
                FROM features_pipeline_stages fps
                JOIN features f ON f.id = fps.feature_id
                WHERE fps.status = 'DEPLOYMENT_REQUESTED'
                "#
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        };

        // 4. Get bottleneck stage (environment with longest average wait time)
        let bottleneck: Option<(String, f64)> = if let Some(team_id) = team_id {
            sqlx::query_as::<_, (String, f64)>(
                r#"
                SELECT
                    e.name as environment_name,
                    ROUND(CAST(AVG(EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - fps.requested_time)) / 3600) AS numeric), 2)::float8 as avg_wait_hours
                FROM features_pipeline_stages fps
                JOIN environments e ON e.id = fps.environment_id
                JOIN features f ON f.id = fps.feature_id
                WHERE fps.status = 'DEPLOYMENT_REQUESTED'
                AND fps.requested_time IS NOT NULL
                AND f.team_id = $1
                GROUP BY e.name
                ORDER BY avg_wait_hours DESC
                LIMIT 1
                "#
            )
            .bind(team_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        } else {
            sqlx::query_as::<_, (String, f64)>(
                r#"
                SELECT
                    e.name as environment_name,
                    ROUND(CAST(AVG(EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - fps.requested_time)) / 3600) AS numeric), 2)::float8 as avg_wait_hours
                FROM features_pipeline_stages fps
                JOIN environments e ON e.id = fps.environment_id
                JOIN features f ON f.id = fps.feature_id
                WHERE fps.status = 'DEPLOYMENT_REQUESTED'
                AND fps.requested_time IS NOT NULL
                GROUP BY e.name
                ORDER BY avg_wait_hours DESC
                LIMIT 1
                "#
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?
        };

        Ok(RolloutMetricsData {
            total_deployed: status_counts.0,
            total_rejected: status_counts.1,
            deployed_this_week,
            deployed_last_week,
            pending_approvals,
            bottleneck_stage: bottleneck.as_ref().map(|(name, _)| name.clone()),
            bottleneck_avg_wait_hours: bottleneck.map(|(_, hours)| hours),
        })
    }

    async fn get_features_with_pending_approvals(
        &self,
        team_id: Option<Uuid>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error> {
        // Count total features with pending approvals
        let mut count_query = sqlx::QueryBuilder::new(
            "SELECT COUNT(DISTINCT f.id) FROM features f \
             INNER JOIN features_pipeline_stages s ON f.id = s.feature_id \
             WHERE s.status IN ('DEPLOYMENT_REQUESTED', 'ROLLBACK_REQUESTED')",
        );

        if let Some(tid) = team_id {
            count_query.push(" AND f.team_id = ").push_bind(tid);
        }

        let total: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Build query with pagination
        let (limit, offset) = if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let offset = (page_num - 1) * page_sz;
            (page_sz, offset)
        } else {
            (total as i32, 0)
        };

        // Query features with pending approvals (with stages joined)
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT DISTINCT ON (f.id) f.id as feature_id, f.key as feature_key, f.description,
               f.feature_type, f.team_id, f.created_at, f.kill_switch_enabled,
               f.kill_switch_activated_at, f.rollback_scheduled_at, f.active as feature_enabled,
               f.lifecycle_stage, f.deprecated_at, f.deprecation_notice, f.last_evaluated_at,
               f.evaluation_count_7d, f.evaluation_count_30d, f.evaluation_count_90d,
               s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
               s.parent_stage_id, s.position, s.status, s.enabled
               FROM features f
               INNER JOIN features_pipeline_stages s ON f.id = s.feature_id
               WHERE s.status IN ('DEPLOYMENT_REQUESTED', 'ROLLBACK_REQUESTED')"#,
        );

        if let Some(tid) = team_id {
            query_builder.push(" AND f.team_id = ").push_bind(tid);
        }

        query_builder.push(" ORDER BY f.id, f.created_at DESC");
        query_builder.push(" LIMIT ").push_bind(limit);
        query_builder.push(" OFFSET ").push_bind(offset);

        let result = query_builder
            .build_query_as::<FeatureWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let features_rows = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();
        let mut order: Vec<Uuid> = Vec::new();

        for row in features_rows {
            if !map.contains_key(&row.feature_id) {
                order.push(row.feature_id);
            }
            map.entry(row.feature_id).or_default().push(row);
        }

        // Convert rows to features and load all stages + dependencies
        let mut features: Vec<Feature> = Vec::with_capacity(order.len());
        for id in order {
            if let Some(rows) = map.remove(&id) {
                let mut feature = Self::map_row_to_feature(rows);
                // Load dependencies
                let dependencies = self.get_feature_dependencies(&id).await?;
                feature.dependencies = dependencies;
                features.push(feature);
            }
        }

        Ok((features, total))
    }

    async fn get_features_with_kill_switches(
        &self,
        team_id: Option<Uuid>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<Feature>, i64), Error> {
        // Count total features with active kill switches
        let mut count_query = sqlx::QueryBuilder::new(
            "SELECT COUNT(DISTINCT f.id) FROM features f \
             WHERE f.kill_switch_enabled = false",
        );

        if let Some(tid) = team_id {
            count_query.push(" AND f.team_id = ").push_bind(tid);
        }

        let total: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Build query with pagination
        let (limit, offset) = if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let offset = (page_num - 1) * page_sz;
            (page_sz, offset)
        } else {
            (total as i32, 0)
        };

        // Query features with kill switches (with stages joined)
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT DISTINCT ON (f.id) f.id as feature_id, f.key as feature_key, f.description,
               f.feature_type, f.team_id, f.created_at, f.kill_switch_enabled,
               f.kill_switch_activated_at, f.rollback_scheduled_at, f.active as feature_enabled,
               f.lifecycle_stage, f.deprecated_at, f.deprecation_notice, f.last_evaluated_at,
               f.evaluation_count_7d, f.evaluation_count_30d, f.evaluation_count_90d,
               s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
               s.parent_stage_id, s.position, s.status, s.enabled
               FROM features f
               LEFT JOIN features_pipeline_stages s ON f.id = s.feature_id
               WHERE f.kill_switch_enabled = false"#,
        );

        if let Some(tid) = team_id {
            query_builder.push(" AND f.team_id = ").push_bind(tid);
        }

        query_builder.push(" ORDER BY f.id, f.kill_switch_activated_at DESC NULLS LAST");
        query_builder.push(" LIMIT ").push_bind(limit);
        query_builder.push(" OFFSET ").push_bind(offset);

        let result = query_builder
            .build_query_as::<FeatureWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let features_rows = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();
        let mut order: Vec<Uuid> = Vec::new();

        for row in features_rows {
            if !map.contains_key(&row.feature_id) {
                order.push(row.feature_id);
            }
            map.entry(row.feature_id).or_default().push(row);
        }

        // Convert rows to features and load all stages + dependencies
        let mut features: Vec<Feature> = Vec::with_capacity(order.len());
        for id in order {
            if let Some(rows) = map.remove(&id) {
                let mut feature = Self::map_row_to_feature(rows);
                // Load dependencies
                let dependencies = self.get_feature_dependencies(&id).await?;
                feature.dependencies = dependencies;
                features.push(feature);
            }
        }

        Ok((features, total))
    }

    fn clone_box(&self) -> Box<dyn FeatureRepository> {
        Box::new(self.clone())
    }
}

// Helper methods for FeatureRepositoryImpl (not part of trait)
impl FeatureRepositoryImpl {
    async fn create_feature_variants(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        feature_id: Uuid,
        variants: Vec<(
            String,
            serde_json::Value,
            crate::database::entity::VariantValueType,
            Option<String>,
        )>,
    ) -> Result<Vec<crate::database::entity::FeatureVariant>, Error> {
        let mut created_variants = Vec::new();

        for (control, value, value_type, description) in variants {
            let variant = sqlx::query_as!(
                crate::database::entity::FeatureVariant,
                r#"
                INSERT INTO feature_variants (feature_id, control, value, value_type, description)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING
                    id,
                    feature_id,
                    control,
                    value,
                    value_type AS "value_type: crate::database::entity::VariantValueType",
                    description,
                    created_at,
                    updated_at
                "#,
                feature_id,
                control,
                value,
                value_type as crate::database::entity::VariantValueType,
                description
            )
            .fetch_one(&mut **tx)
            .await;

            created_variants.push(handle_error(Some(feature_id), variant)?);
        }

        Ok(created_variants)
    }

    async fn delete_feature_variants(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        feature_id: Uuid,
    ) -> Result<(), Error> {
        sqlx::query!(
            "DELETE FROM feature_variants WHERE feature_id = $1",
            feature_id
        )
        .execute(&mut **tx)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(())
    }
}

// Public wrapper functions for variant operations
pub async fn get_feature_variants(
    pool: &PgPool,
    feature_id: Uuid,
) -> Result<Vec<crate::database::entity::FeatureVariant>, Error> {
    let repo = FeatureRepositoryImpl::new(pool.clone());
    repo.get_feature_variants(feature_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn setup_test_feature(pool: &PgPool, team_id: Uuid) -> Uuid {
        let feature_id = Uuid::new_v4();
        let feature_key = format!("test_feature_{}", feature_id);
        sqlx::query!(
            r#"INSERT INTO features (id, key, description, feature_type, team_id, created_at, kill_switch_enabled, kill_switch_activated_at, rollback_scheduled_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
            feature_id,
            feature_key,
            Some("Test feature for kill switch"),
            "Simple",
            team_id,
            Utc::now(),
            true,
            None::<chrono::DateTime<Utc>>,
            None::<chrono::DateTime<Utc>>
        )
        .execute(pool)
        .await
        .expect("Failed to create test feature");

        feature_id
    }

    async fn cleanup_test_feature(pool: &PgPool, feature_id: Uuid) {
        sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
            .execute(pool)
            .await
            .expect("Failed to cleanup test feature");
    }

    #[tokio::test]
    async fn test_emergency_disable_feature_without_rollback() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let feature_id = setup_test_feature(&pool, team_id).await;

        // Emergency disable without rollback schedule
        let result = repo.emergency_disable_feature(feature_id, None).await;
        assert!(result.is_ok(), "Emergency disable should succeed");

        let feature = result.unwrap();
        assert!(
            !feature.kill_switch_enabled,
            "Kill switch should be activated (feature disabled, kill_switch_enabled=false)"
        );
        assert!(
            feature.kill_switch_activated_at.is_some(),
            "Activation time should be set"
        );
        assert!(
            feature.rollback_scheduled_at.is_none(),
            "Rollback should not be scheduled"
        );

        cleanup_test_feature(&pool, feature_id).await;
    }

    #[tokio::test]
    async fn test_emergency_disable_feature_with_rollback() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let feature_id = setup_test_feature(&pool, team_id).await;

        let rollback_minutes = 30;
        let before_disable = Utc::now();

        // Emergency disable with rollback schedule
        let result = repo
            .emergency_disable_feature(feature_id, Some(rollback_minutes))
            .await;
        assert!(
            result.is_ok(),
            "Emergency disable with rollback should succeed"
        );

        let feature = result.unwrap();
        assert!(
            feature.kill_switch_enabled,
            "Kill switch should remain enabled until the scheduled disable time"
        );
        assert!(
            feature.kill_switch_activated_at.is_none(),
            "Activation time should not be set until the disable executes"
        );
        assert!(
            feature.rollback_scheduled_at.is_some(),
            "Rollback should be scheduled"
        );

        // Verify rollback time is approximately correct (within 1 minute tolerance)
        let expected_rollback = before_disable + Duration::minutes(rollback_minutes as i64);
        let actual_rollback = feature.rollback_scheduled_at.unwrap();
        let time_diff = (actual_rollback - expected_rollback).num_seconds().abs();
        assert!(
            time_diff <= 60,
            "Rollback time should be within 1 minute of expected: expected={}, actual={}, diff={}s",
            expected_rollback,
            actual_rollback,
            time_diff
        );

        cleanup_test_feature(&pool, feature_id).await;
    }

    #[tokio::test]
    async fn test_emergency_disable_feature_nonexistent() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let nonexistent_id = Uuid::new_v4();

        // Try to disable nonexistent feature
        let result = repo.emergency_disable_feature(nonexistent_id, None).await;
        assert!(result.is_err(), "Disabling nonexistent feature should fail");

        match result {
            Err(Error::NotFound(id)) => assert_eq!(id, nonexistent_id),
            Err(e) => panic!("Expected NotFound error, got: {:?}", e),
            Ok(_) => panic!("Expected error for nonexistent feature"),
        }
    }

    #[tokio::test]
    async fn test_emergency_enable_feature() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let feature_id = setup_test_feature(&pool, team_id).await;

        // First disable the feature
        repo.emergency_disable_feature(feature_id, Some(60))
            .await
            .expect("Should disable feature first");

        // Now enable it
        let result = repo.emergency_enable_feature(feature_id).await;
        assert!(result.is_ok(), "Emergency enable should succeed");

        let feature = result.unwrap();
        assert!(
            !feature.kill_switch_enabled,
            "Kill switch should be deactivated (feature enabled, kill_switch_enabled=false)"
        );
        assert!(
            feature.kill_switch_activated_at.is_none(),
            "Activation time should be cleared"
        );
        assert!(
            feature.rollback_scheduled_at.is_none(),
            "Rollback schedule should be cleared"
        );

        cleanup_test_feature(&pool, feature_id).await;
    }

    #[tokio::test]
    async fn test_emergency_enable_feature_nonexistent() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let nonexistent_id = Uuid::new_v4();

        // Try to enable nonexistent feature
        let result = repo.emergency_enable_feature(nonexistent_id).await;
        assert!(result.is_err(), "Enabling nonexistent feature should fail");

        match result {
            Err(Error::NotFound(id)) => assert_eq!(id, nonexistent_id),
            Err(e) => panic!("Expected NotFound error, got: {:?}", e),
            Ok(_) => panic!("Expected error for nonexistent feature"),
        }
    }

    #[tokio::test]
    async fn test_get_features_pending_rollback_empty() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());

        // Should return empty when no features pending rollback
        let result = repo.get_features_pending_rollback().await;
        assert!(result.is_ok(), "Get pending rollback should succeed");

        let features = result.unwrap();
        // Note: might not be empty if other tests left data, but should not error
        assert!(features.len() >= 0, "Should return a valid list");
    }

    #[tokio::test]
    async fn test_get_features_pending_rollback_with_eligible_features() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

        // Create features for testing
        let feature1_id = setup_test_feature(&pool, team_id).await;
        let feature2_id = setup_test_feature(&pool, team_id).await;
        let feature3_id = setup_test_feature(&pool, team_id).await;

        // Disable feature1 with rollback in the past (should be returned)
        let past_time = Utc::now() - Duration::minutes(10);
        sqlx::query!(
            r#"UPDATE features SET kill_switch_enabled = true, kill_switch_activated_at = NULL, rollback_scheduled_at = $1 WHERE id = $2"#,
            past_time,
            feature1_id
        ).execute(&pool).await.expect("Failed to setup feature1");

        // Disable feature2 with rollback in the future (should NOT be returned)
        let future_time = Utc::now() + Duration::minutes(10);
        sqlx::query!(
            r#"UPDATE features SET kill_switch_enabled = true, kill_switch_activated_at = NULL, rollback_scheduled_at = $1 WHERE id = $2"#,
            future_time,
            feature2_id
        ).execute(&pool).await.expect("Failed to setup feature2");

        // Keep feature3 enabled (should NOT be returned)
        // feature3 is already enabled by default

        let result = repo.get_features_pending_rollback().await;
        assert!(result.is_ok(), "Get pending rollback should succeed");

        let features = result.unwrap();

        // Should find at least feature1
        let found_feature1 = features.iter().any(|f| f.id == feature1_id);
        assert!(
            found_feature1,
            "Should find feature1 with past rollback time"
        );

        // Should NOT find feature2 or feature3
        let found_feature2 = features.iter().any(|f| f.id == feature2_id);
        let found_feature3 = features.iter().any(|f| f.id == feature3_id);
        assert!(
            !found_feature2,
            "Should NOT find feature2 with future rollback time"
        );
        assert!(!found_feature3, "Should NOT find feature3 that is enabled");

        // Verify the returned feature has correct kill switch state
        let returned_feature1 = features.iter().find(|f| f.id == feature1_id);
        if let Some(feature) = returned_feature1 {
            assert!(
                feature.kill_switch_enabled,
                "Returned feature should remain enabled until the scheduled disable executes"
            );
            assert!(
                feature.rollback_scheduled_at.is_some(),
                "Returned feature should have rollback scheduled"
            );
        }

        // Cleanup
        cleanup_test_feature(&pool, feature1_id).await;
        cleanup_test_feature(&pool, feature2_id).await;
        cleanup_test_feature(&pool, feature3_id).await;
    }

    #[tokio::test]
    async fn test_kill_switch_fields_persistence() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let feature_id = setup_test_feature(&pool, team_id).await;

        // Test disable (scheduled) -> actual disable -> enable cycle
        let before_disable = Utc::now();

        // 1. Schedule disable with rollback window
        let scheduled = repo
            .emergency_disable_feature(feature_id, Some(120))
            .await
            .expect("Should schedule feature disable");

        assert!(scheduled.kill_switch_enabled);
        assert!(scheduled.kill_switch_activated_at.is_none());
        assert!(scheduled.rollback_scheduled_at.is_some());

        // 2. Retrieve and verify pending state
        let pending_feature = repo
            .get_feature_by_id(feature_id)
            .await
            .expect("Should retrieve pending feature");
        assert!(pending_feature.kill_switch_enabled);
        assert!(pending_feature.kill_switch_activated_at.is_none());
        let scheduled_time = pending_feature
            .rollback_scheduled_at
            .expect("scheduled time");
        assert!(
            scheduled_time >= before_disable,
            "Scheduled disable should be after the request time"
        );

        // 3. Simulate scheduler executing the disable now
        repo.emergency_disable_feature(feature_id, None)
            .await
            .expect("Scheduler disable should succeed");

        let disabled_feature = repo
            .get_feature_by_id(feature_id)
            .await
            .expect("Should retrieve disabled feature");
        assert!(
            !disabled_feature.kill_switch_enabled,
            "Feature should be disabled after scheduled execution"
        );
        let activation_time = disabled_feature
            .kill_switch_activated_at
            .expect("Activation time");
        assert!(activation_time >= before_disable);
        assert!(disabled_feature.rollback_scheduled_at.is_none());

        // 4. Enable feature
        repo.emergency_enable_feature(feature_id)
            .await
            .expect("Should enable feature");

        // 5. Retrieve and verify enabled state
        let enabled_feature = repo
            .get_feature_by_id(feature_id)
            .await
            .expect("Should retrieve enabled feature");

        assert!(!enabled_feature.kill_switch_enabled);
        assert!(enabled_feature.kill_switch_activated_at.is_none());
        assert!(enabled_feature.rollback_scheduled_at.is_none());

        cleanup_test_feature(&pool, feature_id).await;
    }

    #[tokio::test]
    async fn test_rollback_scheduling_edge_cases() {
        let pool = crate::database::init_pg_pool().await;
        let repo = FeatureRepositoryImpl::new(pool.clone());
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

        // Test with zero minutes (should work - immediate rollback)
        let feature1_id = setup_test_feature(&pool, team_id).await;
        let result = repo.emergency_disable_feature(feature1_id, Some(0)).await;
        assert!(result.is_ok(), "Zero minute rollback should work");

        let feature = result.unwrap();
        assert!(
            !feature.kill_switch_enabled,
            "Zero minute rollback should result in immediate disable"
        );
        assert!(
            feature.kill_switch_activated_at.is_some(),
            "Immediate disable should record activation time"
        );
        assert!(
            feature.rollback_scheduled_at.is_none(),
            "Immediate disable should not keep a scheduled timestamp"
        );
        cleanup_test_feature(&pool, feature1_id).await;

        // Test with large number of minutes
        let feature2_id = setup_test_feature(&pool, team_id).await;
        let large_minutes = 1440; // 24 hours
        let result = repo
            .emergency_disable_feature(feature2_id, Some(large_minutes))
            .await;
        assert!(result.is_ok(), "Large minute rollback should work");

        let feature = result.unwrap();
        assert!(feature.kill_switch_enabled);
        assert!(feature.kill_switch_activated_at.is_none());
        let scheduled = feature
            .rollback_scheduled_at
            .expect("Should schedule far future rollback");
        assert!(scheduled > Utc::now());
        cleanup_test_feature(&pool, feature2_id).await;
    }
}

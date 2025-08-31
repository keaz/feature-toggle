use crate::database::entity::{Feature, FeatureDependency, FeaturePipelineStage, FeatureType};
use crate::database::{handle_error, Error};
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgQueryResult;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStageCriterion {
    pub context_key: String,
    pub context_id: Uuid,
    pub rollout_percentage: i32,
}

#[derive(Debug, Clone)]
pub struct CreateFeature {
    pub team_id: Uuid,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub stages: Vec<CreateFeatureStage>,
    pub dependencies: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateFeatureStage {
    pub id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage: Option<Box<CreateFeatureStage>>,
    pub position: String,
    pub bucketing_key: Option<String>,
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
            bucketing_key: None,
        }
    }
}

pub struct UpdateFeature {
    pub id: Uuid,
    pub key: Option<String>,
    pub description: Option<String>,
    pub feature_type: Option<FeatureType>,
    pub stages: Vec<CreateFeatureStage>,
    pub dependencies: Vec<Uuid>,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeatureWithStageRow {
    feature_id: Uuid,
    feature_key: String,
    description: Option<String>,
    feature_type: String,
    team_id: Uuid,
    created_at: DateTime<Utc>,

    stage_id: Option<Uuid>,
    feature_id_stage: Option<Uuid>,
    environment_id: Option<Uuid>,
    order_index: Option<i32>,
    parent_stage_id: Option<Uuid>,
    position: Option<String>,
    bucketing_key: Option<String>,
    status: Option<String>,
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
    pub bucketing_key: Option<String>,
    pub status: String,
}

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

    // New: get features referencing a given context id
    async fn get_feature_ids_by_context_id(&self, context_id: Uuid) -> Result<Vec<Uuid>, Error>;

    // New (deployment workflow): request stage change
    async fn request_stage_change(&self, stage_id: Uuid, status: &str, requested_user: Uuid, when: chrono::DateTime<chrono::Utc>) -> Result<bool, Error>;

    // Approve or reject a stage change (sets approved_user and approved_time)
    async fn approve_or_reject_stage_change(&self, stage_id: Uuid, status: &str, user_id: Uuid) -> Result<bool, Error>;

    // Helper: find owning feature id for a stage
    async fn get_feature_id_by_stage_id(&self, stage_id: Uuid) -> Result<Option<Uuid>, Error>;

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

    async fn get_feature_stages(
        &self,
        feature_id: Option<&Uuid>,
        parent_stage_id: Option<&Uuid>,
    ) -> Result<Vec<FeaturePipelineStage>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT id, feature_id, environment_id, order_index, parent_stage_id, position, bucketing_key, status FROM features_pipeline_stages"#,
        );

        let mut has_where = false;
        if feature_id.is_some() || parent_stage_id.is_some() {
            query_builder.push(" WHERE ");
        }

        if let Some(feature_id) = feature_id {
            query_builder.push(" feature_id = ");
            query_builder.push_bind(feature_id);
            has_where = true;
        }
        if let Some(parent_stage_id) = parent_stage_id {
            if has_where {
                query_builder.push(" AND ");
            }
            query_builder
                .push("parent_stage_id = ")
                .push_bind(parent_stage_id);
        }

        let result = query_builder
            .build_query_as::<FeaturePipelineStageRow>()
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
                enabled: r.status == "DEPLOYED",
                bucketing_key: r.bucketing_key,
                status: r.status,
            })
            .collect::<Vec<FeaturePipelineStage>>();
        Ok(stages)
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

        let bucketing_keys: Vec<Option<String>> = stages
            .iter()
            .map(|stage| stage.bucketing_key.clone())
            .collect();
        let statuses: Vec<String> = stages
            .iter()
            .map(|_| "NOT_DEPLOYED".to_string())
            .collect();

        let result = sqlx::query(
            r#"INSERT INTO features_pipeline_stages (id, feature_id, environment_id, order_index, parent_stage_id, position, bucketing_key, status)
               SELECT unnest($1::uuid[]) AS id,
               unnest($2::uuid[]) AS feature_id,
               unnest($3::uuid[]) AS environment_id,
               unnest($4::int[]) AS order_index,
               unnest($5::uuid[]) AS parent_stage_id,
               unnest($6::varchar[]) AS position,
               unnest($7::varchar[]) AS bucketing_key,
               unnest($8::text[]) AS status
               "#,
        )
            .bind(ids)
            .bind(feature_ids)
            .bind(environment_ids)
            .bind(order_indices)
            .bind(parent_stage_ids as &[Option<Uuid>])
            .bind(positions)
            .bind(&bucketing_keys[..])
            .bind(&statuses[..])
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
        let existing_stages = self.get_feature_stages(Some(feature_id), None).await?;
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

            let result = sqlx::query(
                r#"UPDATE features_pipeline_stages
                   SET environment_id = $1,
                       order_index = $2,
                       parent_stage_id = $3,
                       position = $4,
                       bucketing_key = $5
                   WHERE id = $6"#, 
            )
            .bind(stage.environment_id)
            .bind(stage.order_index)
            .bind(parent_stage_id)
            .bind(&stage.position)
            .bind(stage.bucketing_key.clone())
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

        let mut seen = HashSet::new();
        let stages = &features
            .clone()
            .split_off(0)
            .into_iter()
            .filter(|r| {
                if let Some(stage_id) = r.stage_id {
                    seen.insert(stage_id)
                } else {
                    true // Keep rows without a stage_id
                }
            })
            .filter_map(|r| {
                r.stage_id.map(|id| FeaturePipelineStage {
                    id,
                    feature_id: r.feature_id_stage.unwrap(),
                    environment_id: r.environment_id.unwrap(),
                    order_index: r.order_index.unwrap(),
                    parent_stage_id: r.parent_stage_id,
                    position: r.position.unwrap(),
                    enabled: matches!(r.status.as_deref(), Some("DEPLOYED")),
                    bucketing_key: r.bucketing_key,
                    status: r.status.unwrap_or_else(|| "NOT_DEPLOYED".to_string()),
                })
            })
            .collect::<Vec<FeaturePipelineStage>>();

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
            created_at: feature.created_at,
            stages: stages.clone(),
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

        let result = sqlx::query!(
            r#"UPDATE features SET key = $1, description = $2, feature_type = $3 WHERE id = $4"#,
            input.key.clone().unwrap_or(existing_feature.key),
            input.description.clone().or(existing_feature.description),
            feature_type_str,
            input.id
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
    async fn get_stage_criteria(
        &self,
        stage_id: Uuid,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error> {
        // load criteria join contexts and entries
        let rows = sqlx::query!(
            r#"SELECT sc.id, sc.stage_id, sc.context_key, sc.context_id, sc.rollout_percentage,
                      c.team_id, c.key
               FROM feature_stage_criteria sc
               JOIN contexts c ON c.id = sc.context_id
               WHERE sc.stage_id = $1
               ORDER BY sc.context_key, c.key"#,
            stage_id
        )
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(Some(stage_id), rows)?;
        let mut out = Vec::new();
        for r in rows {
            // entries for context
            let entries = handle_error(
                Some(r.context_id),
                sqlx::query!(
                    r#"SELECT id, value FROM context_entries WHERE context_id = $1 ORDER BY value"#,
                    r.context_id
                )
                .fetch_all(&self.pool)
                .await,
            )?
            .into_iter()
            .map(|e| crate::database::entity::ContextEntry {
                id: e.id,
                value: e.value,
            })
            .collect();
            let context = crate::database::entity::Context {
                id: r.context_id,
                team_id: r.team_id,
                key: r.key,
                entries,
            };
            out.push(crate::database::entity::StageCriterion {
                id: r.id,
                stage_id: r.stage_id,
                context_key: r.context_key,
                context,
                rollout_percentage: r.rollout_percentage,
            });
        }
        Ok(out)
    }

    async fn set_stage_criteria(
        &self,
        stage_id: Uuid,
        criteria: Vec<CreateStageCriterion>,
    ) -> Result<Vec<crate::database::entity::StageCriterion>, Error> {
        // ensure stage exists
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

        // validate rollout range and contexts/key match
        for c in &criteria {
            if c.rollout_percentage < 0 || c.rollout_percentage > 100 {
                return Err(Error::InvalidInput(format!(
                    "rollout_percentage for context {} must be between 0 and 100",
                    c.context_id
                )));
            }
        }

        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        // clear
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
            let context_keys: Vec<String> =
                criteria.iter().map(|c| c.context_key.clone()).collect();
            let context_ids: Vec<Uuid> = criteria.iter().map(|c| c.context_id).collect();
            let rollouts: Vec<i32> = criteria.iter().map(|c| c.rollout_percentage).collect();
            handle_error(None, sqlx::query!(
                r#"INSERT INTO feature_stage_criteria(id, stage_id, context_key, context_id, rollout_percentage)
                   SELECT unnest($1::uuid[]), unnest($2::uuid[]), unnest($3::varchar[]), unnest($4::uuid[]), unnest($5::int[])"#,
                &ids[..], &stage_ids[..], &context_keys[..], &context_ids[..], &rollouts[..]
            ).execute(&mut *tx).await)?;
        }
        tx.commit().await.map_err(Error::DatabaseError)?;
        self.get_stage_criteria(stage_id).await
    }
    async fn get_feature_by_id(&self, id: Uuid) -> Result<Feature, Error> {
        let result = sqlx::query_as::<_, FeatureWithStageRow>(
            r#"SELECT f.id as feature_id, f.key as feature_key, f.description, f.feature_type, f.team_id, f.created_at, 
            s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position, s.bucketing_key, s.status
			FROM features f LEFT JOIN features_pipeline_stages s ON f.id = s.feature_id
			WHERE f.id = $1"#,
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
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT f.id as feature_id, f.key as feature_key, f.description, f.feature_type, f.team_id, f.created_at, 
            s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position, s.bucketing_key, s.status
			FROM features f LEFT JOIN features_pipeline_stages s ON f.id = s.feature_id"#, 
        );
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
               JOIN feature_stage_criteria sc ON sc.stage_id = s.id
               WHERE sc.context_id = $1"#,
            context_id
        )
        .fetch_all(&self.pool)
        .await;
        handle_error(Some(context_id), rows)
    }

    async fn request_stage_change(&self, stage_id: Uuid, status: &str, requested_user: Uuid, when: chrono::DateTime<chrono::Utc>) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"UPDATE features_pipeline_stages
               SET status = $1, requested_user = $2, requested_time = $3, approved_user = NULL, approved_time = NULL
               WHERE id = $4"#,
        )
        .bind(status)
        .bind(requested_user)
        .bind(when)
        .bind(stage_id)
        .execute(&self.pool)
        .await;
        let res = handle_error(Some(stage_id), result)?;
        Ok(res.rows_affected() == 1)
    }

    async fn approve_or_reject_stage_change(&self, stage_id: Uuid, status: &str, user_id: Uuid) -> Result<bool, Error> {
        let now = chrono::Utc::now();
        let result = sqlx::query(
            r#"UPDATE features_pipeline_stages
               SET status = $1, approved_user = $2, approved_time = $3
               WHERE id = $4"#,
        )
        .bind(status)
        .bind(user_id)
        .bind(now)
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

    fn clone_box(&self) -> Box<dyn FeatureRepository> {
        Box::new(self.clone())
    }
}

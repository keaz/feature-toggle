use crate::database::entity::{ContextualEntry, ContextualType, Feature, FeatureDependency, FeaturePipelineStage, FeatureType};
use crate::database::{handle_error, Error};
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreateFeature {
    pub team_id: Uuid,
    pub name: String,
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
    pub enabled: bool,
}

impl CreateFeatureStage {
    pub fn new(
        id: Uuid,
        environment_id: Uuid,
        order_index: i32,
        parent_stage: Option<Box<CreateFeatureStage>>,
        position: String,
        enabled: bool,
    ) -> Self {
        Self {
            id,
            environment_id,
            order_index,
            parent_stage,
            position,
            enabled,
        }
    }
}

pub struct UpdateFeature {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub feature_type: Option<FeatureType>,
    pub stages: Vec<CreateFeatureStage>,
    pub dependencies: Vec<Uuid>,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeatureWithStageRow {
    feature_id: Uuid,
    feature_name: String,
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
    enabled: Option<bool>,

    context_id: Option<Uuid>,
    context_name: Option<String>,
    context_description: Option<String>,
    entry_id: Option<Uuid>,
    entry_value: Option<String>,

}

#[derive(Debug, sqlx::FromRow, Clone)]
struct FeatureDependencyRow {
    feature_id: Uuid,
    depends_on_id: Uuid,
}

#[automock]
#[async_trait::async_trait]
pub trait FeatureRepository: Send + Sync {
    async fn get_feature_by_id(&self, id: Uuid) -> Result<Feature, Error>;
    async fn get_features(
        &self,
        team_id: Uuid,
        name: Option<String>,
        feature_type: Option<FeatureType>,
    ) -> Result<Vec<Feature>, Error>;
    async fn create_feature(&self, input: CreateFeature) -> Result<Uuid, Error>;
    async fn update_feature(&self, input: UpdateFeature) -> Result<Feature, Error>;
    async fn delete_feature(&self, id: Uuid) -> Result<(), Error>;

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
struct FeatureRepositoryImpl {
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
            r#"SELECT id, feature_id, environment_id, order_index, parent_stage_id, position, enabled FROM features_pipeline_stages"#,
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
            .build_query_as::<FeaturePipelineStage>()
            .fetch_all(&self.pool)
            .await;

        handle_error(None, result)
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

        let enabled_values = &stages
            .iter()
            .map(|stage| stage.enabled)
            .collect::<Vec<bool>>();

        let result = sqlx::query!(
            r#"INSERT INTO features_pipeline_stages (id, feature_id, environment_id, order_index, parent_stage_id, position, enabled)
               SELECT unnest($1::uuid[]) AS id,
               unnest($2::uuid[]) AS feature_id,
               unnest($3::uuid[]) AS environment_id,
               unnest($4::int[]) AS order_index,
               unnest($5::uuid[]) AS parent_stage_id,
               unnest($6::varchar[]) AS position,
               unnest($7::boolean[]) AS enabled
               "#,
            ids,
            feature_ids,
            environment_ids,
            order_indices,
            parent_stage_ids as &[Option<Uuid>],
            positions,
            enabled_values,
        )
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

            let result = sqlx::query!(
                r#"UPDATE features_pipeline_stages
                   SET environment_id = $1,
                       order_index = $2,
                       parent_stage_id = $3,
                       position = $4,
                       enabled = $5
                   WHERE id = $6"#,
                stage.environment_id,
                stage.order_index,
                parent_stage_id,
                &stage.position,
                stage.enabled,
                stage.id
            )
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
        let stages = &features
            .clone()
            .split_off(0)
            .into_iter()
            .filter_map(|r| {
                r.stage_id.map(|id| FeaturePipelineStage {
                    id,
                    feature_id: r.feature_id_stage.unwrap(),
                    environment_id: r.environment_id.unwrap(),
                    order_index: r.order_index.unwrap(),
                    parent_stage_id: r.parent_stage_id,
                    position: r.position.unwrap(),
                    enabled: r.enabled.unwrap(),
                })
            })
            .collect::<Vec<FeaturePipelineStage>>();

        let feature_type = match feature.feature_type.as_str() {
            "Simple" => FeatureType::Simple,
            "Contextual" => FeatureType::Contextual,
            _ => panic!("Unknown feature type, this should never happen"),
        };

        let context = features.clone().split_off(stages.len())
            .into_iter()
            .filter_map(|r| {
                r.context_id.map(|id| {
                    let entries = features
                        .iter()
                        .filter(|e| e.context_id == Some(id))
                        .filter_map(|e| {
                            e.entry_id.map(|entry_id| ContextualEntry {
                                id: entry_id,
                                value: e.entry_value.clone().unwrap_or_default(),
                            })
                        })
                        .collect::<Vec<ContextualEntry>>();

                    ContextualType {
                        id,
                        name: r.context_name.clone().unwrap_or_default(),
                        description: r.context_description.clone(),
                        entries,
                    }
                })
            })
            .collect::<Vec<ContextualType>>();

        Feature {
            id: feature.feature_id,
            name: feature.feature_name.clone(),
            description: feature.description.clone(),
            feature_type,
            team_id: feature.team_id,
            created_at: feature.created_at,
            stages: stages.clone(),
            dependencies: vec![], // Dependencies will be loaded separately
            contextual_types: Some(context)
        }
    }
}

#[async_trait::async_trait]
impl FeatureRepository for FeatureRepositoryImpl {
    async fn get_feature_by_id(&self, id: Uuid) -> Result<Feature, Error> {
        let result = sqlx::query_as::<_, FeatureWithStageRow>(
            r#"SELECT f.id as feature_id, f.name as feature_name, f.description, f.feature_type, f.team_id, f.created_at, 
            s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position, s.enabled,  c.id as context_id, c.name as context_name, c.description as context_description,
			e.id as entry_id, e.value as entry_value
			FROM features f LEFT JOIN features_pipeline_stages s ON f.id = s.feature_id
			LEFT JOIN contextual_type c ON f.id = c.feature_id
			LEFT JOIN contextual_entries e ON c.id = e.contextual_id WHERE f.id = $1"#,
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
        name: Option<String>,
        feature_type: Option<FeatureType>,
    ) -> Result<Vec<Feature>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT f.id as feature_id, f.name as feature_name, f.description, f.feature_type, f.team_id, f.created_at, 
            s.id as stage_id, s.feature_id as feature_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position, s.enabled,  c.id as context_id, c.name as context_name, c.description as context_description,
			e.id as entry_id, e.value as entry_value
			FROM features f LEFT JOIN features_pipeline_stages s ON f.id = s.feature_id
			LEFT JOIN contextual_type c ON f.id = c.feature_id
			LEFT JOIN contextual_entries e ON c.id = e.contextual_id"#,
        );
        query_builder.push(" WHERE f.team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND f.name ILIKE ");
            query_builder.push_bind(format!("%{name}%"));
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
        query_builder.push(" ORDER BY f.name");

        let result = query_builder
            .build_query_as::<FeatureWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let features_rows = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Vec<FeatureWithStageRow>> = HashMap::new();

        for row in features_rows {
            map.entry(row.feature_id).or_default().push(row);
            // if !map.contains_key(&row.feature_id) {
            //     let mut feature = Self::map_row_to_feature(row)?;
            // }
            //
            // let feature_entry = map.entry(row.feature_id).or_insert_with(|| {
            //     let feature_type = match row.feature_type.as_str() {
            //         "Simple" => FeatureType::Simple,
            //         "Contextual" => FeatureType::Contextual,
            //         _ => FeatureType::Simple, // Default to Simple if unknown
            //     };
            //     let mut feature = Self::map_row_to_feature(features)?;
            //     Feature {
            //         id: row.feature_id,
            //         name: row.feature_name.clone(),
            //         description: row.description.clone(),
            //         feature_type,
            //         team_id: row.team_id,
            //         created_at: row.created_at,
            //         stages: vec![],
            //         dependencies: vec![],
            //         contextual_types: None,
            //     }
            // });
            //
            // if let Some(stage_id) = row.stage_id {
            //     feature_entry.stages.push(FeaturePipelineStage {
            //         id: stage_id,
            //         feature_id: row.feature_id_stage.unwrap(),
            //         environment_id: row.environment_id.unwrap(),
            //         order_index: row.order_index.unwrap(),
            //         parent_stage_id: row.parent_stage_id,
            //         position: row.position.unwrap(),
            //         enabled: row.enabled.unwrap(),
            //     });
            // }
        }

        // Load dependencies for each feature
        let mut features: Vec<Feature> = map.into_values().map(Self::map_row_to_feature).collect();
        for feature in &mut features {
            let dependencies = self.get_feature_dependencies(&feature.id).await?;
            feature.dependencies = dependencies;
        }

        Ok(features)
    }

    async fn create_feature(&self, input: CreateFeature) -> Result<Uuid, Error> {
        let existing_feature = self
            .get_features(input.team_id, Some(input.name.clone()), None)
            .await;

        if let Ok(existing_feature) = existing_feature {
            if !existing_feature.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Feature with name '{}' already exists",
                    input.name
                )));
            }
        }

        let tx: Result<Transaction<'static, Postgres>, sqlx::Error> = self.pool.begin().await;
        if tx.is_err() {
            return Err(Error::DatabaseError(tx.err().unwrap()));
        }
        let mut tx: Transaction<'_, Postgres> = tx.unwrap();

        let id = Uuid::new_v4();
        let feature_type_str = match input.feature_type {
            FeatureType::Simple => "Simple",
            FeatureType::Contextual => "Contextual",
        };

        let result = sqlx::query!(
            r#"INSERT INTO features (id, name, description, feature_type, team_id) 
               VALUES ($1, $2, $3, $4, $5) RETURNING id"#,
            id,
            input.name,
            input.description,
            feature_type_str,
            input.team_id
        )
        .fetch_one(&mut *tx)
        .await;

        let handled_error = handle_error(None, result);
        match handled_error {
            Ok(saved_feature) => {
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
                Ok(saved_feature.id)
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

        let existing_feature = self.get_feature_by_id(input.id).await?;

        let feature_type_str = match input.feature_type.unwrap_or(existing_feature.feature_type) {
            FeatureType::Simple => "Simple",
            FeatureType::Contextual => "Contextual",
        };

        let result = sqlx::query!(
            r#"UPDATE features SET name = $1, description = $2, feature_type = $3 WHERE id = $4"#,
            input.name.unwrap_or(existing_feature.name),
            input.description.or(existing_feature.description),
            feature_type_str,
            input.id
        )
        .execute(&mut *tx)
        .await;

        if result.is_err() {
            let _ = tx.rollback().await;
            return Err(Error::DatabaseError(result.err().unwrap()));
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

        let result = handle_error(Some(input.id), result);
        match result {
            Ok(_) => {
                let _ = tx.commit().await;
                self.get_feature_by_id(input.id).await
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
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

    fn clone_box(&self) -> Box<dyn FeatureRepository> {
        Box::new(self.clone())
    }
}

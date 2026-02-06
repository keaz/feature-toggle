use crate::database::entity::{Pipeline, PipelineStage};
use crate::database::{Error, handle_error};
use mockall::automock;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreatePipeline {
    pub team_id: Uuid,
    pub name: String,
    pub stages: Vec<CreateStage>,
}

#[derive(Debug, Clone)]
pub struct CreateStage {
    pub id: Uuid, // For internal use, not part of the input
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage: Option<Box<CreateStage>>,
    pub position: String,
}

impl CreateStage {
    pub fn new(
        id: Uuid,
        environment_id: Uuid,
        order_index: i32,
        parent_stage: Option<Box<CreateStage>>,
        position: String,
    ) -> Self {
        Self {
            id,
            environment_id,
            order_index,
            parent_stage,
            position,
        }
    }
}

impl crate::logic::stage_builder::StageWithRelationship for CreateStage {
    fn order_index(&self) -> i32 {
        self.order_index
    }

    fn set_parent_stage(&mut self, parent: Box<Self>) {
        self.parent_stage = Some(parent);
    }
}

pub struct UpdatePipeline {
    pub id: Uuid,
    pub name: Option<String>,
    pub active: Option<bool>,
    pub stages: Vec<CreateStage>,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct PipelineWithStageRow {
    pipeline_id: Uuid,
    pipeline_name: String,
    active: bool,
    team_id: Uuid,

    stage_id: Option<Uuid>,
    pipeline_id_stage: Option<Uuid>,
    environment_id: Option<Uuid>,
    order_index: Option<i32>,
    parent_stage_id: Option<Uuid>,
    position: String,
}

#[automock]
#[async_trait::async_trait]
pub trait PipelineRepository: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error>;
    async fn get_pipelines_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Pipeline>, i64), Error>;
    async fn get_pipelines_with_offset(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Pipeline>, i64), Error>;
    async fn create_pipeline(&self, input: CreatePipeline) -> Result<Uuid, Error>;
    async fn update_pipeline(&self, input: UpdatePipeline) -> Result<Pipeline, Error>;
    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn PipelineRepository>;
}

impl Clone for Box<dyn PipelineRepository> {
    fn clone(&self) -> Box<dyn PipelineRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait PipelineRepositoryTx: PipelineRepository {
    async fn create_pipeline_tx(
        &self,
        conn: &mut PgConnection,
        input: CreatePipeline,
    ) -> Result<Uuid, Error>;
    async fn update_pipeline_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdatePipeline,
    ) -> Result<Pipeline, Error>;
    async fn delete_pipeline_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
}

pub fn pipeline_repository(pool: PgPool) -> Box<dyn PipelineRepository> {
    Box::new(PipelineRepositoryImpl::new(pool))
}

/// Returns a repository that also implements PipelineRepositoryTx for transaction support.
pub fn pipeline_repository_tx(pool: PgPool) -> PipelineRepositoryImpl {
    PipelineRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct PipelineRepositoryImpl {
    pool: PgPool,
}

impl PipelineRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn is_pipeline_exists_id(&self, id: Uuid) -> Result<Option<Uuid>, Error> {
        let result = sqlx::query_scalar!(r#"SELECT id FROM pipelines WHERE id = $1"#, id)
            .fetch_optional(&self.pool)
            .await;

        handle_error(Some(id), result)
    }

    async fn create_stage(
        &self,
        pipeline_id: &Uuid,
        stages: Vec<CreateStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        if stages.is_empty() {
            return Ok(PgQueryResult::default());
        }

        // stages.iter_mut().for_each(|stage| { stage.id = Some(Uuid::new_v4()); });
        let ids: &[Uuid] = &stages.iter().map(|stage| stage.id).collect::<Vec<Uuid>>();
        let pipeline_ids: &[Uuid] = &stages
            .iter()
            .map(|_| pipeline_id.to_owned())
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

        let result = sqlx::query!(
            r#"INSERT INTO pipeline_stages (id, pipeline_id, environment_id, order_index, parent_stage_id, position)
               SELECT unnest($1::uuid[]) AS id,
               unnest($2::uuid[]) AS pipeline_id,
               unnest($3::uuid[]) AS environment_id,
               unnest($4::int[]) AS order_index,
               unnest($5::uuid[]) AS parent_stage_id ,
               unnest($6::varchar[]) AS position
               "#,
            ids,
            pipeline_ids,
            environment_ids,
            order_indices,
            parent_stage_ids as &[Option<Uuid>],
            positions,
        )
        .execute(&mut *tx)
        .await;

        handle_error(None, result)
    }

    async fn update_pipeline_stage(
        &self,
        pipeline_id: &Uuid,
        input: Vec<CreateStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        let result = sqlx::query!(
            r#"DELETE FROM pipeline_stages WHERE pipeline_id = $1"#,
            *pipeline_id
        )
        .execute(&mut *tx)
        .await;
        handle_error(Some(*pipeline_id), result)?;

        self.create_stage(pipeline_id, input, tx).await?;

        Ok(PgQueryResult::default())
    }

    async fn delete_pipeline_stage(&self, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!(r#"DELETE FROM pipeline_stages WHERE pipeline_id = $1"#, id)
            .execute(&self.pool)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    fn map_row_to_pipeline(pipelines: Vec<PipelineWithStageRow>) -> Result<Pipeline, Error> {
        let pipeline = &pipelines[0];
        let stages = &pipelines
            .clone()
            .split_off(0)
            .into_iter()
            .filter_map(|r| {
                r.stage_id.map(|id| PipelineStage {
                    id,
                    pipeline_id: r.pipeline_id_stage.unwrap(),
                    environment_id: r.environment_id.unwrap(),
                    order_index: r.order_index.unwrap(),
                    parent_stage_id: r.parent_stage_id,
                    position: r.position,
                })
            })
            .collect::<Vec<PipelineStage>>();

        Ok(Pipeline {
            id: pipeline.pipeline_id,
            name: pipeline.pipeline_name.clone(),
            active: pipeline.active,
            team_id: pipeline.team_id,
            stages: stages.clone(),
        })
    }
}

#[async_trait::async_trait]
impl PipelineRepository for PipelineRepositoryImpl {
    async fn get_pipeline_by_id(&self, id: Uuid) -> Result<Pipeline, Error> {
        let result = sqlx::query_as::<_, PipelineWithStageRow>(
            r#"SELECT p.id as pipeline_id, p.name as pipeline_name, p.active, p.team_id, 
            s.id as stage_id, s.pipeline_id as pipeline_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position FROM pipelines p LEFT JOIN pipeline_stages s ON s.pipeline_id = p.id WHERE p.id = $1"#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await;

        let pipelines = handle_error(Some(id), result)?;
        if pipelines.is_empty() {
            return Err(Error::NotFound(id));
        }

        Self::map_row_to_pipeline(pipelines)
    }

    async fn get_pipelines(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT p.id as pipeline_id, p.name as pipeline_name, p.active, p.team_id, 
            s.id as stage_id, s.pipeline_id as pipeline_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position FROM pipelines p LEFT JOIN pipeline_stages s ON s.pipeline_id = p.id"#,
        );
        query_builder.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND p.name ILIKE ");
            query_builder.push_bind(format!("%{name}%"));
        }
        if let Some(active_value) = active {
            query_builder
                .push(" AND p.active = ")
                .push_bind(active_value);
        }
        query_builder.push(" ORDER BY p.name");

        let result = query_builder
            .build_query_as::<PipelineWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let pipelines = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Pipeline> = HashMap::new();

        for row in pipelines {
            let pipeline_entry = map.entry(row.pipeline_id).or_insert(Pipeline {
                id: row.pipeline_id,
                name: row.pipeline_name.clone(),
                active: row.active,
                team_id: row.team_id,
                stages: vec![],
            });

            if let Some(stage_id) = row.stage_id {
                pipeline_entry.stages.push(PipelineStage {
                    id: stage_id,
                    pipeline_id: row.pipeline_id_stage.unwrap(),
                    environment_id: row.environment_id.unwrap(),
                    order_index: row.order_index.unwrap(),
                    parent_stage_id: row.parent_stage_id,
                    position: row.position,
                });
            }
        }

        let mut pipelines = map.into_values().collect::<Vec<Pipeline>>();
        pipelines.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(pipelines)
    }

    async fn get_pipelines_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Pipeline>, i64), Error> {
        // First, get the total count
        let mut count_query =
            sqlx::QueryBuilder::new("SELECT COUNT(DISTINCT p.id) FROM pipelines p");
        count_query.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = &name {
            count_query.push(" AND p.name ILIKE ");
            count_query.push_bind(format!("%{name}%"));
        }
        if let Some(active_value) = active {
            count_query.push(" AND p.active = ").push_bind(active_value);
        }

        let total_count: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Now get the paginated results
        let offset = (page_number - 1) * page_size;
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT p.id as pipeline_id, p.name as pipeline_name, p.active, p.team_id, 
            s.id as stage_id, s.pipeline_id as pipeline_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position FROM pipelines p LEFT JOIN pipeline_stages s ON s.pipeline_id = p.id"#,
        );
        query_builder.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND p.name ILIKE ");
            query_builder.push_bind(format!("%{name}%"));
        }
        if let Some(active_value) = active {
            query_builder
                .push(" AND p.active = ")
                .push_bind(active_value);
        }
        query_builder.push(" ORDER BY p.name");
        query_builder.push(" LIMIT ").push_bind(page_size);
        query_builder.push(" OFFSET ").push_bind(offset);

        let result = query_builder
            .build_query_as::<PipelineWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let pipelines = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Pipeline> = HashMap::new();

        for row in pipelines {
            let pipeline_entry = map.entry(row.pipeline_id).or_insert(Pipeline {
                id: row.pipeline_id,
                name: row.pipeline_name.clone(),
                active: row.active,
                team_id: row.team_id,
                stages: vec![],
            });

            if let Some(stage_id) = row.stage_id {
                pipeline_entry.stages.push(PipelineStage {
                    id: stage_id,
                    pipeline_id: row.pipeline_id_stage.unwrap(),
                    environment_id: row.environment_id.unwrap(),
                    order_index: row.order_index.unwrap(),
                    parent_stage_id: row.parent_stage_id,
                    position: row.position,
                });
            }
        }

        let mut pipelines = map.into_values().collect::<Vec<Pipeline>>();
        pipelines.sort_by(|a, b| a.name.cmp(&b.name));
        Ok((pipelines, total_count))
    }

    async fn get_pipelines_with_offset(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Pipeline>, i64), Error> {
        let offset = offset.max(0);
        let limit = limit.max(1);

        let mut count_query =
            sqlx::QueryBuilder::new("SELECT COUNT(DISTINCT p.id) FROM pipelines p");
        count_query.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = &name {
            count_query.push(" AND p.name ILIKE ");
            count_query.push_bind(format!("%{name}%"));
        }
        if let Some(active_value) = active {
            count_query.push(" AND p.active = ").push_bind(active_value);
        }

        let total_count: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT p.id as pipeline_id, p.name as pipeline_name, p.active, p.team_id, 
            s.id as stage_id, s.pipeline_id as pipeline_id_stage, s.environment_id, s.order_index,
            s.parent_stage_id, s.position FROM pipelines p LEFT JOIN pipeline_stages s ON s.pipeline_id = p.id"#,
        );
        query_builder.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND p.name ILIKE ");
            query_builder.push_bind(format!("%{name}%"));
        }
        if let Some(active_value) = active {
            query_builder
                .push(" AND p.active = ")
                .push_bind(active_value);
        }
        query_builder.push(" ORDER BY p.name");
        query_builder.push(" LIMIT ").push_bind(limit);
        query_builder.push(" OFFSET ").push_bind(offset);

        let result = query_builder
            .build_query_as::<PipelineWithStageRow>()
            .fetch_all(&self.pool)
            .await;

        let pipelines = handle_error(None, result)?;
        let mut map: HashMap<Uuid, Pipeline> = HashMap::new();

        for row in pipelines {
            let pipeline_entry = map.entry(row.pipeline_id).or_insert(Pipeline {
                id: row.pipeline_id,
                name: row.pipeline_name.clone(),
                active: row.active,
                team_id: row.team_id,
                stages: vec![],
            });

            if let Some(stage_id) = row.stage_id {
                pipeline_entry.stages.push(PipelineStage {
                    id: stage_id,
                    pipeline_id: row.pipeline_id_stage.unwrap(),
                    environment_id: row.environment_id.unwrap(),
                    order_index: row.order_index.unwrap(),
                    parent_stage_id: row.parent_stage_id,
                    position: row.position,
                });
            }
        }

        let mut pipelines = map.into_values().collect::<Vec<Pipeline>>();
        pipelines.sort_by(|a, b| a.name.cmp(&b.name));
        Ok((pipelines, total_count))
    }

    async fn create_pipeline(&self, input: CreatePipeline) -> Result<Uuid, Error> {
        let existing_pipeline = self
            .get_pipelines(input.team_id, Some(input.name.clone()), None)
            .await;

        if let Ok(existing_pipeline) = existing_pipeline {
            if !existing_pipeline.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Pipeline with name '{}' already exists",
                    input.name
                )));
            }
        }

        let mut tx: Transaction<'_, Postgres> =
            self.pool.begin().await.map_err(Error::DatabaseError)?;

        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO pipelines (id, name, active, team_id) VALUES ($1, $2, true, $3) RETURNING id"#,
            id,
            input.name,
            input.team_id
        )
        .fetch_one(&mut *tx)
        .await;
        let saved_pipeline = handle_error(None, result)?;
        self.create_stage(&id, input.stages, &mut tx).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(saved_pipeline.id)
    }

    async fn update_pipeline(&self, input: UpdatePipeline) -> Result<Pipeline, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;

        let existing_env = self.get_pipeline_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipelines SET name = $1, active = $2 WHERE id = $3"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.id
        )
        .execute(&mut *tx)
        .await;

        handle_error(Some(input.id), result)?;

        if !input.stages.is_empty() {
            self.update_pipeline_stage(&input.id, input.stages, &mut tx)
                .await?;
        }

        tx.commit().await.map_err(Error::DatabaseError)?;
        self.get_pipeline_by_id(input.id).await
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        if self.is_pipeline_exists_id(id).await?.is_none() {
            return Err(Error::NotFound(id));
        }

        let result = sqlx::query!("DELETE FROM pipelines WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn PipelineRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl PipelineRepositoryTx for PipelineRepositoryImpl {
    async fn create_pipeline_tx(
        &self,
        conn: &mut PgConnection,
        input: CreatePipeline,
    ) -> Result<Uuid, Error> {
        // Check for existing pipeline
        let existing_pipeline = self
            .get_pipelines(input.team_id, Some(input.name.clone()), None)
            .await;

        if let Ok(existing_pipeline) = existing_pipeline {
            if !existing_pipeline.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Pipeline with name '{}' already exists",
                    input.name
                )));
            }
        }

        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO pipelines (id, name, active, team_id) VALUES ($1, $2, true, $3) RETURNING id"#,
            id, input.name, input.team_id
        )
        .fetch_one(&mut *conn)
        .await;

        let saved_pipeline = handle_error(None, result)?;
        self.create_stage(&id, input.stages, conn).await?;
        Ok(saved_pipeline.id)
    }

    async fn update_pipeline_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdatePipeline,
    ) -> Result<Pipeline, Error> {
        let existing_env = self.get_pipeline_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipelines SET name = $1, active = $2 WHERE id = $3"#,
            input.name.clone().unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.id
        )
        .execute(&mut *conn)
        .await;

        handle_error(Some(input.id), result)?;

        if !input.stages.is_empty() {
            self.update_pipeline_stage(&input.id, input.stages, conn)
                .await?;
        }

        self.get_pipeline_by_id(input.id).await
    }

    async fn delete_pipeline_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        if self.is_pipeline_exists_id(id).await?.is_none() {
            return Err(Error::NotFound(id));
        }

        let result = sqlx::query!("DELETE FROM pipelines WHERE id = $1", id)
            .execute(&mut *conn)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }
}

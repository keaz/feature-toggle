use std::collections::HashMap;

use crate::database::entity::{Pipeline, SENTINEL_UUID, Stage};
use crate::database::{Error, handle_error};
use mockall::automock;
use sqlx::postgres::PgQueryResult;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

pub struct CreatePipeline {
    pub team_id: Uuid,
    pub name: String,
    pub stages: Vec<CreateStage>,
}

pub struct CreateStage {
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
}

pub struct UpdatePipeline {
    pub id: Uuid,
    pub name: Option<String>,
    pub active: Option<bool>,
    pub stages: Vec<UpdateCreateStage>,
}

pub struct UpdateCreateStage {
    pub id: Uuid,
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
}

#[derive(Debug, sqlx::FromRow, Clone)]
struct PipelineWithStageRow {
    pipeline_id: Uuid,
    pipeline_name: String,
    active: bool,
    team_id: Uuid,

    stage_id: Option<Uuid>,
    pipeline_id_stage: Option<Uuid>, // alias in query if needed
    environment_id: Option<Uuid>,
    order_index: Option<i8>,
    parent_stage_id: Option<Uuid>,
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

pub fn pipeline_repository(pool: PgPool) -> Box<dyn PipelineRepository> {
    Box::new(PipelineRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct PipelineRepositoryImpl {
    pool: PgPool,
}

impl PipelineRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn get_stage_by_id(&self, id: Uuid) -> Result<Stage, Error> {
        let result = sqlx::query_as::<_, Stage>(
            r#"SELECT id, pipeline_id , environment_id, order_index, parent_stage_id FROM pipeline_stages WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_stage(
        &self,
        pipeline_id: Option<Uuid>,
        parent_stage_id: Option<Uuid>,
    ) -> Result<Vec<Stage>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT id, pipeline_id, environment_id, order_index, parent_stage_id FROM pipeline_stages"#,
        );

        let mut has_where = false;
        if pipeline_id.is_some() || parent_stage_id.is_some() {
            query_builder.push(" WHERE ");
        }

        if let Some(pipeline_id) = pipeline_id {
            query_builder.push(" pipeline_id ILIKE ");
            query_builder.push_bind(format!("%{}%", pipeline_id));
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
        query_builder.push(" ORDER BY name");

        let result = query_builder
            .build_query_as::<Stage>()
            .fetch_all(&self.pool)
            .await;

        handle_error(None, result)
    }

    async fn create_stage(
        &self,
        stages: Vec<CreateStage>,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        if stages.is_empty() {
            return Ok(PgQueryResult::default());
        }
        let ids: &[Uuid] = &stages.iter().map(|_| Uuid::new_v4()).collect::<Vec<Uuid>>();
        let pipeline_ids: &[Uuid] = &stages
            .iter()
            .map(|stage| stage.pipeline_id)
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
            .map(|stage| stage.parent_stage_id.unwrap_or(SENTINEL_UUID))
            .collect::<Vec<Uuid>>()[..];

        let result = sqlx::query!(
            r#"INSERT INTO pipeline_stages (id, pipeline_id, environment_id, order_index, parent_stage_id)
               SELECT unnest($1::uuid[]) AS id,
               unnest($2::uuid[]) AS pipeline_id,
               unnest($3::uuid[]) AS environment_id,
               unnest($4::int[]) AS order_index,
               unnest($5::uuid[]) AS parent_stage_id"#,
            ids,
            pipeline_ids,
            environment_ids,
            order_indices,
            parent_stage_ids
        )
        .execute(&mut *tx)
        .await;

        handle_error(None, result)
    }

    async fn update_pipeline_stage(
        &self,
        input: UpdateCreateStage,
        tx: &mut PgConnection,
    ) -> Result<PgQueryResult, Error> {
        // #FIXME: This function should be update to delete any removed stages and update existing ones.
        let existing_stage = self.get_stage_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipeline_stages SET environment_id = $1, order_index = $2, parent_stage_id = $3
               WHERE id = $4 "#,
            input.environment_id,
            input.order_index,
            input.parent_stage_id,
            input.id
        )
        .execute(&mut *tx)
        .await;

        handle_error(Some(input.id), result)
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
            .split_off(1)
            .into_iter()
            .filter_map(|r| {
                r.stage_id.map(|id| Stage {
                    id,
                    pipeline_id: r.pipeline_id_stage.unwrap(),
                    environment_id: r.environment_id.unwrap(),
                    order_index: r.order_index.unwrap(),
                    parent_stage_id: r.parent_stage_id,
                })
            })
            .collect::<Vec<Stage>>();

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
            s.id as stage_id, s.pipeline_id, s.environment_id, s.order_index, 
            s.parent_stage_id FROM pipelines p LEFT JOIN stages s ON s.pipeline_id = p.id WHERE id = $1"#,
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
            s.id as stage_id, s.pipeline_id, s.environment_id, s.order_index, 
            s.parent_stage_id FROM pipelines p LEFT JOIN stages s ON s.pipeline_id = p.id"#,
        );
        query_builder.push(" WHERE p.team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND p.name ILIKE ");
            query_builder.push_bind(format!("%{}%", name));
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
                pipeline_entry.stages.push(Stage {
                    id: stage_id,
                    pipeline_id: row.pipeline_id_stage.unwrap(),
                    environment_id: row.environment_id.unwrap(),
                    order_index: row.order_index.unwrap(),
                    parent_stage_id: row.parent_stage_id,
                });
            }
        }

        Ok(map.into_values().collect())
    }

    async fn create_pipeline(&self, input: CreatePipeline) -> Result<Uuid, Error> {
        let existing_pipeline = self
            .get_pipelines(input.team_id.clone(), Some(input.name.clone()), None)
            .await;

        if let Ok(existing_pipeline) = existing_pipeline {
            if !existing_pipeline.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Pipeline with name '{}' already exists",
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
        let result = sqlx::query!(
        r#"INSERT INTO pipelines (id, name, active, team_id) VALUES ($1, $2, true, $3) RETURNING id"#,
        id,input.name, input.team_id ).fetch_one(&mut *tx).await;

        let handled_error = handle_error(None, result);
        match handled_error {
            Ok(saved_pipeline) => {
                self.create_stage(input.stages, &mut tx).await;
                let _ = tx.commit().await;
                if let Err(e) = self.delete_pipeline(saved_pipeline.id).await {
                    return Err(e);
                }
                Ok(saved_pipeline.id)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    async fn update_pipeline(&self, input: UpdatePipeline) -> Result<Pipeline, Error> {
        let existing_env = self.get_pipeline_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipelines SET name = $1, active = $2 WHERE id = $3 RETURNING id, name, active, team_id"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.id
        ).fetch_one(&self.pool)
        .await;

        let pipeline = handle_error(Some(input.id), result)?;
        Ok(Pipeline {
            id: pipeline.id,
            name: pipeline.name,
            active: pipeline.active,
            team_id: pipeline.team_id,
        })
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        self.get_pipeline_by_id(id).await?;
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
